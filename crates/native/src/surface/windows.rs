use nsh_platform_windows::{
    ComponentContext, Components, NativeComponent,
    windows::{self, core::w},
};
use serde_json::{Value, json};
use std::{
    cell::{Cell, RefCell},
    collections::BTreeMap,
    rc::Rc,
    sync::Once,
};
use windows::Win32::{
    Foundation::{HINSTANCE, HWND, LPARAM, LRESULT, RECT, WPARAM},
    Graphics::Gdi::*,
    System::LibraryLoader::GetModuleHandleW,
    UI::{Input::KeyboardAndMouse::*, WindowsAndMessaging::*},
};

pub fn components() -> Components {
    let components = Components::default();
    components.register(
        super::NAME,
        Rc::new(|context| Box::new(Surface::new(context))),
    );
    components
}
struct Data {
    context: ComponentContext,
    frame: RefCell<Option<(u32, u32, Vec<u8>)>>,
    keys: RefCell<BTreeMap<usize, String>>,
    buttons: Cell<u8>,
}
struct Surface {
    hwnd: Option<HWND>,
}
impl Surface {
    fn new(context: ComponentContext) -> Self {
        static REGISTER: Once = Once::new();
        // SAFETY: Registration and child-window creation are confined to NSH's UI thread.
        let hwnd = unsafe {
            let instance = GetModuleHandleW(None)
                .ok()
                .map(|module| HINSTANCE(module.0));
            REGISTER.call_once(|| {
                let class = WNDCLASSW {
                    lpfnWndProc: Some(window_proc),
                    hInstance: instance.unwrap_or_default(),
                    lpszClassName: w!("LightRemoteDesktopInput"),
                    hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(),
                    ..Default::default()
                };
                RegisterClassW(&class);
            });
            CreateWindowExW(
                WINDOW_EX_STYLE(0),
                w!("LightRemoteDesktopInput"),
                w!(""),
                WS_POPUP | WS_CLIPSIBLINGS,
                0,
                0,
                1,
                1,
                None,
                None,
                instance,
                None,
            )
            .ok()
        };
        if let Some(hwnd) = hwnd {
            let data = Box::new(Data {
                context,
                frame: RefCell::new(None),
                keys: RefCell::default(),
                buttons: Cell::new(0),
            });
            // SAFETY: The window owns this pointer until WM_NCDESTROY takes it back.
            unsafe {
                SetWindowLongPtrW(hwnd, GWLP_USERDATA, Box::into_raw(data) as isize);
            }
        } else {
            context.emit(
                "input",
                json!({"type":"error","message":"Cannot create native desktop input surface"}),
            );
        }
        Self { hwnd }
    }
}
impl NativeComponent for Surface {
    fn window(&self) -> Option<HWND> {
        self.hwnd
    }
    fn set_prop(&mut self, name: &str, value: Option<&Value>) {
        if name != "frame" {
            return;
        }
        let Some(hwnd) = self.hwnd else {
            return;
        };
        // SAFETY: This component owns the live HWND and its Data, on the UI thread.
        let data = unsafe { &*(GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *const Data) };
        let uri = value.and_then(Value::as_str).unwrap_or_default();
        if uri.is_empty() {
            *data.frame.borrow_mut() = None;
        } else {
            let decoded = nsh_core::image::decode_data_uri(uri)
                .map_err(|e| e.to_string())
                .and_then(|(_, bytes)| {
                    crate::desktop::admit_jpeg(&bytes).map_err(|error| error.to_string())?;
                    let mut reader = image::ImageReader::with_format(
                        std::io::Cursor::new(bytes),
                        image::ImageFormat::Jpeg,
                    );
                    let mut limits = image::Limits::default();
                    limits.max_image_width = Some(16384);
                    limits.max_image_height = Some(16384);
                    limits.max_alloc = Some(256 * 1024 * 1024);
                    reader.limits(limits);
                    reader.decode().map_err(|e| e.to_string())
                });
            match decoded {
                Ok(image) => {
                    let (w, h) = (image.width(), image.height());
                    let mut pixels = image.to_rgba8().into_raw();
                    for pixel in pixels.as_chunks_mut::<4>().0 {
                        pixel.swap(0, 2);
                    }
                    *data.frame.borrow_mut() = Some((w, h, pixels));
                }
                Err(error) => data.context.emit(
                    "input",
                    json!({"type":"error","message":format!("Cannot display JPEG: {error}")}),
                ),
            }
        }
        // SAFETY: Invalidate only the component's own window.
        unsafe {
            let _ = InvalidateRect(Some(hwnd), None, false);
        }
    }
}
impl Drop for Surface {
    fn drop(&mut self) {
        if let Some(hwnd) = self.hwnd.take() {
            unsafe {
                let _ = DestroyWindow(hwnd);
            }
        }
    }
}
fn point(hwnd: HWND, kind: &str, lparam: LPARAM) -> Value {
    let (x, y) = (
        (lparam.0 as u16 as i16) as i32,
        ((lparam.0 >> 16) as u16 as i16) as i32,
    );
    let mut rect = RECT::default();
    unsafe {
        let _ = GetClientRect(hwnd, &mut rect);
    }
    // SAFETY: Snapshot key state on the same window thread as this mouse event.
    let modifiers: Vec<_> = unsafe {
        [
            (GetKeyState(VK_SHIFT.0 as i32) < 0, "Shift"),
            (GetKeyState(VK_CONTROL.0 as i32) < 0, "Control"),
            (GetKeyState(VK_MENU.0 as i32) < 0, "Alt"),
            (
                GetKeyState(VK_LWIN.0 as i32) < 0 || GetKeyState(VK_RWIN.0 as i32) < 0,
                "Meta",
            ),
        ]
        .into_iter()
        .filter_map(|(down, name)| down.then_some(name))
        .collect()
    };
    json!({"type":kind,"x":x,"y":y,"width":rect.right,"height":rect.bottom,"modifiers":modifiers})
}
fn release(data: &Data) {
    data.keys.borrow_mut().clear();
    data.buttons.set(0);
    data.context.emit("input", json!({"type":"release"}));
}
fn key(code: usize) -> Option<String> {
    let name = match code as u32 {
        0x08 => "Backspace",
        0x09 => "Tab",
        0x0d => "Enter",
        0x10 | 0xa0 | 0xa1 => "Shift",
        0x11 | 0xa2 | 0xa3 => "Control",
        0x12 | 0xa4 | 0xa5 => "Alt",
        0x1b => "Escape",
        0x20 => "Space",
        0x21 => "PageUp",
        0x22 => "PageDown",
        0x23 => "End",
        0x24 => "Home",
        0x25 => "ArrowLeft",
        0x26 => "ArrowUp",
        0x27 => "ArrowRight",
        0x28 => "ArrowDown",
        0x2d => "Insert",
        0x2e => "Delete",
        0x5b | 0x5c => "Meta",
        0x70..=0x7b => return Some(format!("F{}", code - 0x6f)),
        _ => {
            let mapped = unsafe { MapVirtualKeyW(code as u32, MAPVK_VK_TO_CHAR) } & 0xffff;
            return char::from_u32(mapped).and_then(|ch| super::key_name(&ch.to_string()));
        }
    };
    Some(name.into())
}
unsafe extern "system" fn window_proc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    // SAFETY: Data is installed after creation, used only by this window thread,
    // and reclaimed exactly once by WM_NCDESTROY. No callback retains a reference.
    unsafe {
        let pointer = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut Data;
        if pointer.is_null() {
            return DefWindowProcW(hwnd, message, wparam, lparam);
        }
        let data = &*pointer;
        match message {
            WM_SETFOCUS => data.context.emit("input", json!({"type":"focus"})),
            WM_KILLFOCUS | WM_CANCELMODE => release(data),
            WM_CAPTURECHANGED if data.buttons.get() != 0 => release(data),
            WM_MOUSEMOVE => data.context.emit("input", point(hwnd, "move", lparam)),
            WM_LBUTTONDOWN | WM_MBUTTONDOWN | WM_RBUTTONDOWN | WM_LBUTTONUP | WM_MBUTTONUP
            | WM_RBUTTONUP => {
                let (button, bit, down) = match message {
                    WM_LBUTTONDOWN => ("left", 1, true),
                    WM_LBUTTONUP => ("left", 1, false),
                    WM_MBUTTONDOWN => ("middle", 2, true),
                    WM_MBUTTONUP => ("middle", 2, false),
                    WM_RBUTTONDOWN => ("right", 4, true),
                    _ => ("right", 4, false),
                };
                if down {
                    let _ = SetFocus(Some(hwnd));
                    SetCapture(hwnd);
                    data.buttons.set(data.buttons.get() | bit);
                } else {
                    data.buttons.set(data.buttons.get() & !bit);
                }
                let mut event = point(hwnd, "button", lparam);
                event["button"] = button.into();
                event["down"] = down.into();
                data.context.emit("input", event);
                if data.buttons.get() == 0 {
                    let _ = ReleaseCapture();
                }
            }
            WM_MOUSEWHEEL | WM_MOUSEHWHEEL => {
                let mut p = windows::Win32::Foundation::POINT {
                    x: (lparam.0 as u16 as i16) as i32,
                    y: ((lparam.0 >> 16) as u16 as i16) as i32,
                };
                let _ = ScreenToClient(hwnd, &mut p);
                let mut rect = RECT::default();
                let _ = GetClientRect(hwnd, &mut rect);
                if p.x >= 0 && p.y >= 0 && p.x < rect.right && p.y < rect.bottom {
                    let delta = f64::from((wparam.0 >> 16) as u16 as i16) / 120.0;
                    data.context.emit("input",json!({"type":"scroll","dx":if message==WM_MOUSEHWHEEL{delta}else{0.0},"dy":if message==WM_MOUSEWHEEL{-delta}else{0.0}}));
                }
            }
            WM_KEYDOWN | WM_SYSKEYDOWN => {
                if !data.keys.borrow().contains_key(&wparam.0)
                    && let Some(key) = key(wparam.0)
                {
                    data.keys.borrow_mut().insert(wparam.0, key.clone());
                    data.context
                        .emit("input", json!({"type":"key","key":key,"down":true}));
                }
            }
            WM_KEYUP | WM_SYSKEYUP => {
                if let Some(key) = data.keys.borrow_mut().remove(&wparam.0) {
                    data.context
                        .emit("input", json!({"type":"key","key":key,"down":false}));
                }
            }
            WM_GETDLGCODE => {
                return LRESULT((DLGC_WANTALLKEYS | DLGC_WANTARROWS | DLGC_WANTTAB) as isize);
            }
            WM_ERASEBKGND => return LRESULT(1),
            WM_PAINT => {
                let mut paint = PAINTSTRUCT::default();
                let dc = BeginPaint(hwnd, &mut paint);
                let mut rect = RECT::default();
                let _ = GetClientRect(hwnd, &mut rect);
                FillRect(dc, &rect, HBRUSH(GetStockObject(BLACK_BRUSH).0));
                if let Some((width, height, pixels)) = &*data.frame.borrow() {
                    let scale = (f64::from(rect.right) / f64::from(*width))
                        .min(f64::from(rect.bottom) / f64::from(*height));
                    let (w, h) = (
                        (f64::from(*width) * scale).round() as i32,
                        (f64::from(*height) * scale).round() as i32,
                    );
                    let info = BITMAPINFO {
                        bmiHeader: BITMAPINFOHEADER {
                            biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                            biWidth: *width as i32,
                            biHeight: -(*height as i32),
                            biPlanes: 1,
                            biBitCount: 32,
                            biCompression: BI_RGB.0,
                            ..Default::default()
                        },
                        ..Default::default()
                    };
                    SetStretchBltMode(dc, HALFTONE);
                    StretchDIBits(
                        dc,
                        (rect.right - w) / 2,
                        (rect.bottom - h) / 2,
                        w,
                        h,
                        0,
                        0,
                        *width as i32,
                        *height as i32,
                        Some(pixels.as_ptr().cast()),
                        &info,
                        DIB_RGB_COLORS,
                        SRCCOPY,
                    );
                }
                let _ = EndPaint(hwnd, &paint);
            }
            WM_NCDESTROY => {
                SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
                drop(Box::from_raw(pointer));
                return DefWindowProcW(hwnd, message, wparam, lparam);
            }
            _ => return DefWindowProcW(hwnd, message, wparam, lparam),
        }
        LRESULT(0)
    }
}
