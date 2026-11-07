//! Stable GitHub release updates with bounded verification and session exclusion.
//!
//! Downloads rely on GitHub HTTPS and SHA-256 integrity, not publisher signatures.
//! No candidate executable is run. See `docs/updating.md` for the admission protocol.

mod archive;
mod binary;
mod error;
mod install;
mod release;
mod session;

pub use error::{Error, Result};
pub use install::{InstalledUpdate, PendingUpdate};
pub use release::{Product, Release, Updater};
pub use session::{SessionGate, SessionLease};
