//! Stopping when asked to: by a signal on Unix, by the console on
//! Windows.  Where things are, which also differs, is in
//! `smartclock::defaults`.

use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use anyhow::Result;
use smartclock::task::Handle;

#[cfg_attr(unix, path = "unix.rs")]
#[cfg_attr(windows, path = "windows.rs")]
mod os;

/// The exit status of a daemon told to stop twice, which goes without
/// writing what it holds.
const AGAIN: i32 = 1;

/// Stop when asked: on Unix SIGTERM or SIGINT, on Windows Ctrl-C,
/// Ctrl-Break or the console closing, logging off or shutting down.
/// Asked again to interrupt, exit at once with [`AGAIN`].
///
/// The first sets the returned flag and wakes the task so the
/// supervisor returns; the log thread then writes what it holds.  A
/// second interrupt is someone who will not wait for that.
pub(crate) fn watch_for_stop(handle: Handle) -> Result<Arc<AtomicBool>> {
    os::watch_for_stop(handle)
}
