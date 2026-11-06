use crate::BoxServiceIo;
use crate::records::{FRAME_LIMIT, INPUT_LIMIT, read_record, write_record};
use anyhow::{Context, Result, ensure};
use enigo::{Axis, Button, Coordinate, Direction, Enigo, Key, Keyboard, Mouse, Settings};
use image::codecs::jpeg::JpegEncoder;
use serde::Deserialize;
use serde_json::{Value, json};
use std::{collections::HashSet, io::Write, sync::Arc, time::Duration};
use tokio::{
    io::AsyncWriteExt,
    sync::OwnedSemaphorePermit,
    time::{Instant, sleep_until, timeout},
};
use xcap::Monitor;

const MAX_PIXELS: u64 = 16_777_216;
const MAX_HELD: usize = 128;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct DesktopOptions {
    #[serde(default)]
    display: usize,
    #[serde(default = "default_fps")]
    fps: u8,
    #[serde(default = "default_quality")]
    quality: u8,
    #[serde(default)]
    single: bool,
}
fn default_fps() -> u8 {
    10
}
fn default_quality() -> u8 {
    70
}

#[derive(Clone, Copy)]
struct Bounds {
    x: i32,
    y: i32,
    width: u32,
    height: u32,
}
fn monitor(display: usize) -> Result<(Monitor, Bounds)> {
    let monitor = Monitor::all()?
        .into_iter()
        .nth(display)
        .context("unknown display")?;
    let bounds = Bounds {
        x: monitor.x()?,
        y: monitor.y()?,
        width: monitor.width()?,
        height: monitor.height()?,
    };
    ensure!(
        bounds.width > 0 && bounds.height > 0 && bounds.width <= 16384 && bounds.height <= 16384,
        "display dimensions out of bounds"
    );
    ensure!(
        u64::from(bounds.width) * u64::from(bounds.height) <= MAX_PIXELS,
        "display pixel limit exceeded"
    );
    Ok((monitor, bounds))
}

pub(crate) fn displays() -> Result<Value> {
    let mut entries = Vec::new();
    for (index, monitor) in Monitor::all()?.into_iter().enumerate().take(64) {
        entries.push(json!({"id":index,"name":monitor.name()?,"width":monitor.width()?,
            "height":monitor.height()?,"x":monitor.x()?,"y":monitor.y()?,"primary":monitor.is_primary()?}));
    }
    Ok(json!({"displays":entries}))
}

pub(crate) fn probe_input() -> bool {
    Enigo::new(&settings()).is_ok()
}
pub(crate) fn probe_clipboard() -> bool {
    arboard::Clipboard::new().is_ok()
}
fn settings() -> Settings {
    Settings {
        release_keys_when_dropped: true,
        open_prompt_to_get_permissions: false,
        ..Settings::default()
    }
}

struct LimitedJpeg(Vec<u8>);
impl Write for LimitedJpeg {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if bytes.len() > FRAME_LIMIT.saturating_sub(self.0.len()) {
            return Err(std::io::Error::other("JPEG frame exceeds limit"));
        }
        self.0.extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

fn capture(display: usize, quality: u8) -> Result<Vec<u8>> {
    let (monitor, bounds) = monitor(display)?;
    #[cfg(target_os = "macos")]
    {
        let scale = f64::from(monitor.scale_factor()?);
        ensure!(
            scale.is_finite() && (0.5..=4.0).contains(&scale),
            "display scale out of bounds"
        );
        ensure!(
            f64::from(bounds.width) * f64::from(bounds.height) * scale * scale <= MAX_PIXELS as f64,
            "capture pixel limit exceeded"
        );
    }
    let image = monitor.capture_image()?;
    ensure!(
        u64::from(image.width()) * u64::from(image.height()) <= MAX_PIXELS,
        "capture pixel limit exceeded"
    );
    // Retina capture pixels may exceed the logical display geometry used by
    // Enigo. Normalize JPEGs to that geometry so remote coordinates match.
    let image = if image.width() != bounds.width || image.height() != bounds.height {
        image::imageops::resize(
            &image,
            bounds.width,
            bounds.height,
            image::imageops::FilterType::Triangle,
        )
    } else {
        image
    };
    let rgb = image::DynamicImage::ImageRgba8(image).into_rgb8();
    let mut output = LimitedJpeg(Vec::new());
    JpegEncoder::new_with_quality(&mut output, quality).encode_image(&rgb)?;
    Ok(output.0)
}

pub(crate) async fn serve(
    options: DesktopOptions,
    mut io: BoxServiceIo,
    idle: Duration,
    slot: Arc<OwnedSemaphorePermit>,
) -> Result<()> {
    ensure!((1..=30).contains(&options.fps), "fps out of bounds");
    ensure!(
        (1..=100).contains(&options.quality),
        "quality out of bounds"
    );
    let interval = Duration::from_secs_f64(1.0 / f64::from(options.fps));
    loop {
        let next = Instant::now() + interval;
        let display = options.display;
        let quality = options.quality;
        let capture_slot = slot.clone();
        let frame = tokio::task::spawn_blocking(move || {
            let _slot = capture_slot;
            capture(display, quality)
        })
        .await??;
        write_record(&mut io, &frame, FRAME_LIMIT, idle).await?;
        if options.single {
            timeout(idle, io.shutdown()).await??;
            return Ok(());
        }
        // One frame in flight. Slow clients naturally lower the capture rate.
        sleep_until(next).await;
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct InputOptions {
    #[serde(default)]
    display: usize,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase", deny_unknown_fields)]
enum Input {
    Move { x: i32, y: i32 },
    Button { button: MouseButton, down: bool },
    Scroll { dx: i32, dy: i32 },
    Key { key: String, down: bool },
    Text { text: String },
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Deserialize)]
#[serde(rename_all = "lowercase")]
enum MouseButton {
    Left,
    Right,
    Middle,
}
impl MouseButton {
    fn native(self) -> Button {
        match self {
            Self::Left => Button::Left,
            Self::Right => Button::Right,
            Self::Middle => Button::Middle,
        }
    }
}

fn key(value: &str) -> Result<Key> {
    let key = match value {
        "Enter" | "Return" => Key::Return,
        "Tab" => Key::Tab,
        "Escape" => Key::Escape,
        "Backspace" => Key::Backspace,
        "Delete" => Key::Delete,
        #[cfg(not(target_os = "macos"))]
        "Insert" => Key::Insert,
        "Home" => Key::Home,
        "End" => Key::End,
        "PageUp" => Key::PageUp,
        "PageDown" => Key::PageDown,
        "ArrowUp" => Key::UpArrow,
        "ArrowDown" => Key::DownArrow,
        "ArrowLeft" => Key::LeftArrow,
        "ArrowRight" => Key::RightArrow,
        "Shift" => Key::Shift,
        "Control" | "Ctrl" => Key::Control,
        "Alt" => Key::Alt,
        "Meta" | "Super" => Key::Meta,
        "F1" => Key::F1,
        "F2" => Key::F2,
        "F3" => Key::F3,
        "F4" => Key::F4,
        "F5" => Key::F5,
        "F6" => Key::F6,
        "F7" => Key::F7,
        "F8" => Key::F8,
        "F9" => Key::F9,
        "F10" => Key::F10,
        "F11" => Key::F11,
        "F12" => Key::F12,
        "Space" => Key::Unicode(' '),
        _ => {
            let mut chars = value.chars();
            let ch = chars.next().context("empty key")?;
            ensure!(
                chars.next().is_none() && !ch.is_control(),
                "unsupported key"
            );
            Key::Unicode(ch)
        }
    };
    Ok(key)
}

impl Input {
    fn validate(&self, bounds: Bounds) -> Result<()> {
        match self {
            Self::Move { x, y } => {
                ensure!(
                    *x >= 0 && *y >= 0 && (*x as u32) < bounds.width && (*y as u32) < bounds.height,
                    "coordinates outside selected display"
                );
                ensure!(
                    bounds.x.checked_add(*x).is_some() && bounds.y.checked_add(*y).is_some(),
                    "coordinate overflow"
                );
            }
            Self::Scroll { dx, dy } => ensure!(
                dx.unsigned_abs() <= 100 && dy.unsigned_abs() <= 100,
                "scroll out of bounds"
            ),
            Self::Key { key: value, .. } => {
                key(value)?;
            }
            Self::Text { text } => ensure!(
                text.len() <= 16 * 1024 && !text.contains('\0'),
                "text out of bounds"
            ),
            Self::Button { .. } => {}
        }
        Ok(())
    }
}

// Enigo records every Press in a Vec internally. Suppressing a duplicate here
// bounds that Vec as well as our own held-key set; a HashSet alone is not enough.
fn remember_key_press(keys: &mut HashSet<Key>, key: Key) -> Result<bool> {
    if keys.contains(&key) {
        return Ok(false);
    }
    ensure!(keys.len() < MAX_HELD, "too many held keys");
    keys.insert(key);
    Ok(true)
}

struct InputState {
    enigo: Enigo,
    bounds: Bounds,
    keys: HashSet<Key>,
    buttons: HashSet<MouseButton>,
    // Release backend state before allowing a replacement input stream.
    _slot: Arc<OwnedSemaphorePermit>,
    _input: OwnedSemaphorePermit,
}
impl InputState {
    fn new(
        display: usize,
        slot: Arc<OwnedSemaphorePermit>,
        input: OwnedSemaphorePermit,
    ) -> Result<Self> {
        let (_, bounds) = monitor(display)?;
        Ok(Self {
            _slot: slot,
            _input: input,
            enigo: Enigo::new(&settings())?,
            bounds,
            keys: HashSet::new(),
            buttons: HashSet::new(),
        })
    }
    fn apply(&mut self, input: Input) -> Result<()> {
        input.validate(self.bounds)?;
        let direction = |down| {
            if down {
                Direction::Press
            } else {
                Direction::Release
            }
        };
        match input {
            Input::Move { x, y } => {
                self.enigo
                    .move_mouse(self.bounds.x + x, self.bounds.y + y, Coordinate::Abs)?
            }
            Input::Scroll { dx, dy } => {
                self.enigo.scroll(dx, Axis::Horizontal)?;
                self.enigo.scroll(dy, Axis::Vertical)?;
            }
            Input::Button { button, down } => {
                if self.buttons.contains(&button) == down {
                    return Ok(());
                }
                // Remember before sending so even a partial OS error releases it.
                if down {
                    self.buttons.insert(button);
                }
                self.enigo.button(button.native(), direction(down))?;
                if !down {
                    self.buttons.remove(&button);
                }
            }
            Input::Key { key: value, down } => {
                let key = key(&value)?;
                if down {
                    if !remember_key_press(&mut self.keys, key)? {
                        return Ok(());
                    }
                } else if !self.keys.contains(&key) {
                    return Ok(());
                }
                self.enigo.key(key, direction(down))?;
                if !down {
                    self.keys.remove(&key);
                }
            }
            Input::Text { text } => self.enigo.text(&text)?,
        }
        Ok(())
    }
}
impl Drop for InputState {
    fn drop(&mut self) {
        for button in self.buttons.drain() {
            let _ = self.enigo.button(button.native(), Direction::Release);
        }
        for key in self.keys.drain() {
            let _ = self.enigo.key(key, Direction::Release);
        }
    }
}

pub(crate) async fn input(
    options: InputOptions,
    mut io: BoxServiceIo,
    idle: Duration,
    slot: Arc<OwnedSemaphorePermit>,
    input: OwnedSemaphorePermit,
) -> Result<()> {
    let mut state =
        tokio::task::spawn_blocking(move || InputState::new(options.display, slot, input))
            .await??;
    while let Some(record) = read_record(&mut io, INPUT_LIMIT, idle).await? {
        let input: Input = serde_json::from_slice(&record).context("invalid input record")?;
        state = tokio::task::spawn_blocking(move || {
            state.apply(input)?;
            Ok::<_, anyhow::Error>(state)
        })
        .await??;
    }
    Ok(())
}

// Keep the clipboard owner alive on Linux: dropping it after set_text would
// make the selection disappear when no clipboard manager is installed.
pub(crate) struct Clipboard(Option<arboard::Clipboard>);
impl Clipboard {
    pub fn new() -> Self {
        Self(None)
    }
    fn backend(&mut self) -> Result<&mut arboard::Clipboard> {
        if self.0.is_none() {
            self.0 = Some(arboard::Clipboard::new()?);
        }
        self.0.as_mut().context("clipboard unavailable")
    }
    pub fn get(&mut self) -> Result<Value> {
        let text = self.backend()?.get_text()?;
        ensure!(text.len() <= 256 * 1024, "clipboard text too large");
        Ok(json!({"text":text}))
    }
    pub fn set(&mut self, text: String) -> Result<Value> {
        ensure!(
            text.len() <= 256 * 1024 && !text.contains('\0'),
            "clipboard text out of bounds"
        );
        self.backend()?.set_text(text)?;
        Ok(json!({}))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn input_validation_rejects_unknown_types_fields_and_out_of_bounds() {
        let bounds = Bounds {
            x: i32::MAX - 10,
            y: 0,
            width: 20,
            height: 20,
        };
        for data in [
            r#"{"type":"exec","command":"foo"}"#,
            r#"{"type":"move","x":1,"y":2,"extra":true}"#,
            r#"{"type":"button","button":"fourth","down":true}"#,
            r#"{"type":"key","key":"a","down":1}"#,
        ] {
            assert!(serde_json::from_str::<Input>(data).is_err(), "{data}");
        }
        for input in [
            Input::Move { x: -1, y: 0 },
            Input::Move { x: 20, y: 0 },
            Input::Move { x: 15, y: 0 },
            Input::Scroll {
                dx: i32::MIN,
                dy: 0,
            },
            Input::Key {
                key: "UnknownKey".into(),
                down: true,
            },
            Input::Text {
                text: "a\0b".into(),
            },
        ] {
            assert!(input.validate(bounds).is_err());
        }
    }
    #[test]
    fn repeated_presses_and_unique_held_keys_are_bounded() {
        let mut keys = HashSet::new();
        assert!(remember_key_press(&mut keys, Key::Control).unwrap());
        for _ in 0..10_000 {
            assert!(!remember_key_press(&mut keys, Key::Control).unwrap());
        }
        assert_eq!(keys.len(), 1);
        for index in 0..MAX_HELD - 1 {
            assert!(
                remember_key_press(
                    &mut keys,
                    Key::Unicode(char::from_u32(0x400 + index as u32).unwrap())
                )
                .unwrap()
            );
        }
        assert!(remember_key_press(&mut keys, Key::Unicode('x')).is_err());
        assert_eq!(keys.len(), MAX_HELD);
    }

    #[test]
    fn jpeg_output_is_bounded() {
        let mut output = LimitedJpeg(vec![0; FRAME_LIMIT - 2]);
        assert!(output.write_all(&[0; 3]).is_err());
        assert_eq!(output.0.len(), FRAME_LIMIT - 2);
    }
}
