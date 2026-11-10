//! NSH registered native surfaces: no web content or global OS input hooks.
#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "windows")]
mod windows;

#[cfg(target_os = "linux")]
pub use linux::components;
#[cfg(target_os = "macos")]
pub use macos::components;
#[cfg(target_os = "windows")]
pub use windows::components;

#[cfg(not(target_os = "android"))]
pub const NAME: &str = "lightremote/desktop";

#[cfg(not(target_os = "android"))]
pub fn key_name(name: &str) -> Option<String> {
    let name = match name {
        "Shift_L" | "Shift_R" | "Shift" => "Shift",
        "Control_L" | "Control_R" | "Control" => "Control",
        "Alt_L" | "Alt_R" | "Alt" | "ISO_Level3_Shift" => "Alt",
        "Super_L" | "Super_R" | "Meta_L" | "Meta_R" | "Meta" => "Meta",
        "Return" | "KP_Enter" => "Enter",
        "ISO_Left_Tab" => "Tab",
        "space" | " " => "Space",
        "Left" => "ArrowLeft",
        "Right" => "ArrowRight",
        "Up" => "ArrowUp",
        "Down" => "ArrowDown",
        "Page_Up" => "PageUp",
        "Page_Down" => "PageDown",
        "BackSpace" => "Backspace",
        other => other,
    };
    if matches!(
        name,
        "Shift"
            | "Control"
            | "Alt"
            | "Meta"
            | "Enter"
            | "Escape"
            | "Tab"
            | "Backspace"
            | "Delete"
            | "Insert"
            | "Home"
            | "End"
            | "PageUp"
            | "PageDown"
            | "ArrowLeft"
            | "ArrowRight"
            | "ArrowUp"
            | "ArrowDown"
            | "Space"
            | "F1"
            | "F2"
            | "F3"
            | "F4"
            | "F5"
            | "F6"
            | "F7"
            | "F8"
            | "F9"
            | "F10"
            | "F11"
            | "F12"
    ) {
        Some(name.into())
    } else if name.chars().count() == 1 && !name.chars().next()?.is_control() {
        Some(name.to_ascii_lowercase())
    } else {
        None
    }
}
