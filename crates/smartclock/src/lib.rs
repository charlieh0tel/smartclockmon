//! Library for HP / Symmetricom SmartClock GPS time and frequency
//! receivers, spoken to over RS-232.
//!
//! See `PLAN.md` at the repository root for architecture and decisions.

pub mod control;
pub mod device;
pub mod error;
pub mod parse;
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
