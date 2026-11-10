#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]

#[cfg(target_os = "linux")]
use nsh_platform_linux as platform;
#[cfg(target_os = "macos")]
use nsh_platform_macos as platform;
#[cfg(target_os = "windows")]
use nsh_platform_windows as platform;

#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
fn main() -> impl std::process::Termination {
    platform::run_app(
        platform::AppSource::Rust(roc_remote_native::root()),
        platform::AppOptions {
            title: "roc-remote".into(),
            components: roc_remote_native::components(),
            app_id: "io.github.cax.roc_remote".into(),
            #[cfg(target_os = "windows")]
            icon: std::env::current_exe().ok().and_then(|path| {
                path.parent()
                    .map(|directory| directory.join("branding/icon.ico"))
            }),
            size: (1120, 860),
            ..Default::default()
        },
    )
}

#[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
compile_error!("The native controller supports Linux, Windows and macOS.");
