//! LightRemote application profiles and encrypted TMP connections.
pub mod crypto;
pub mod invite;
pub mod link;
pub mod records;
pub use invite::{HostTicket, Invite, Room};
pub use link::{Connection, IncomingStream, LinkHandle, MuxStream};

pub const PROFILES: &[&str] = &[
    "lightremote.control.v1",
    "lightremote.desktop.v1",
    "lightremote.input.v1",
    "lightremote.shell.v1",
    "lightremote.file.v1",
];
pub fn profile(route: &str) -> anyhow::Result<&'static str> {
    Ok(match route {
        "control" => PROFILES[0],
        "desktop" => PROFILES[1],
        "input" => PROFILES[2],
        "shell" => PROFILES[3],
        "file.get" | "file.put" => PROFILES[4],
        _ => anyhow::bail!("unsupported application route"),
    })
}
