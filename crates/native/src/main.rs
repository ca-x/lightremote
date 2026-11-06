#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]

mod client;
mod desktop;
mod input;
mod surface;
mod ui;

#[cfg(target_os = "linux")]
use nsh_platform_linux as platform;
#[cfg(target_os = "macos")]
use nsh_platform_macos as platform;
#[cfg(target_os = "windows")]
use nsh_platform_windows as platform;

#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
fn main() -> impl std::process::Termination {
    platform::run_app(
        platform::AppSource::Rust(platform::Root::of::<ui::App>()),
        platform::AppOptions {
            title: "LightRemote".into(),
            components: surface::components(),
            #[cfg(any(target_os = "linux", target_os = "windows"))]
            app_id: "org.libx.lightremote".into(),
            size: (1100, 900),
            ..Default::default()
        },
    )
}

#[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
compile_error!("The native controller supports Linux, Windows and macOS.");
