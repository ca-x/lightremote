mod client;
mod desktop;
mod input;
mod surface;
mod ui;

use nsh_core::prelude::*;

pub use ui::{App, root};

#[cfg(not(target_os = "android"))]
pub use surface::components;

#[native_module("roc-remote/native")]
fn native(registry: &mut Registry) {
    registry.register::<App>();
}
