//! Library for HP / Symmetricom SmartClock GPS time and frequency
//! receivers, spoken to over RS-232.
//!
//! See `PLAN.md` at the repository root for architecture and decisions.

/// Allan deviation from the receiver's phase readings.
pub mod adev;
pub mod client;
pub mod control;
pub mod device;
pub mod error;
pub mod matrix;
/// What this build is, as `<version>-<commits>+g<commit>`, with
/// `+dirty` when it was built from a tree with uncommitted changes.
///
/// Every binary reports this, the daemon logs it and serves it, and the
/// Debian package takes its version from it, so one string identifies a
/// build wherever it turns up.
pub const VERSION: &str = env!("SMARTCLOCK_VERSION");

pub mod parse;
pub mod protocol;
pub mod rollover;
pub mod screen;
pub mod session;
pub mod snapshot;
pub mod task;
pub mod transport;
pub mod types;
pub mod wire;

pub mod command {
    //! The command table, generated from `commands.toml` by `build.rs`.

    include!(concat!(env!("OUT_DIR"), "/commands.rs"));
}
