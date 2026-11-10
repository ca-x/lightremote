pub(crate) fn desktop_guidance(os: &str, compiled: bool) -> &'static str {
    if !compiled {
        return "This host is a headless build. Desktop, input and clipboard require a desktop-enabled host build.";
    }
    match os {
        "linux" => {
            "Run the host in the intended graphical user session. Wayland capture depends on compositor and portal support; approve any system sharing prompt. Remote input is unavailable in Wayland sessions: this build has only an X11 injector. Clipboard requires that same desktop session."
        }
        "macos" => {
            "Enable Screen Recording for capture and Accessibility for input in macOS System Settings, then restart the host if requested. Host policy is separate from macOS consent. No permissions are requested automatically."
        }
        "windows" => {
            "Run the host in the intended interactive Windows session. Elevated applications and the secure desktop may reject input from a non-elevated host. This app never elevates privileges or bypasses UAC automatically."
        }
        _ => {
            "Desktop availability depends on the host operating system and explicit local permission."
        }
    }
}

#[cfg(feature = "desktop")]
pub(crate) fn input_supported() -> bool {
    #[cfg(target_os = "linux")]
    {
        x11_input_supported(
            std::env::var("XDG_SESSION_TYPE").ok().as_deref(),
            std::env::var("WAYLAND_DISPLAY").ok().as_deref(),
        )
    }
    #[cfg(not(target_os = "linux"))]
    {
        true
    }
}

#[cfg(any(all(feature = "desktop", target_os = "linux"), test))]
fn x11_input_supported(session_type: Option<&str>, wayland_display: Option<&str>) -> bool {
    !session_type.is_some_and(|value| value.trim().eq_ignore_ascii_case("wayland"))
        && !wayland_display.is_some_and(|value| !value.trim().is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn headless_support_never_suggests_granting_os_permissions() {
        for os in ["linux", "macos", "windows", "android"] {
            assert!(desktop_guidance(os, false).contains("headless build"));
        }
    }

    #[test]
    fn guidance_distinguishes_platform_boundaries() {
        assert!(desktop_guidance("linux", true).contains("Wayland"));
        assert!(desktop_guidance("macos", true).contains("Screen Recording"));
        assert!(desktop_guidance("windows", true).contains("UAC"));
        assert!(desktop_guidance("other", true).contains("explicit local permission"));
    }

    #[test]
    fn xwayland_never_implies_native_wayland_input() {
        for (session, display) in [
            (Some("wayland"), None),
            (Some("Wayland"), Some("")),
            (Some("x11"), Some("wayland-0")),
            (None, Some("wayland-1")),
        ] {
            assert!(!x11_input_supported(session, display));
        }
        assert!(x11_input_supported(Some("x11"), None));
        assert!(x11_input_supported(None, Some("")));
    }
}
