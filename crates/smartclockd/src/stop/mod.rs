//! Stopping on a signal: the daemon's one difference between Unix and
//! Windows, laid out as `smartclock::platform` is.

use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use anyhow::Result;
use smartclock::task::Handle;

#[cfg_attr(unix, path = "unix.rs")]
#[cfg_attr(windows, path = "windows.rs")]
mod os;

/// Stop on SIGTERM or SIGINT (Ctrl-C), and at once on a second one.
///
/// The first sets the returned flag and wakes the task so the
/// supervisor returns; the log thread then writes what it holds.  A
/// second signal is someone who will not wait for that.
pub(crate) fn watch_for_stop(handle: Handle) -> Result<Arc<AtomicBool>> {
    os::watch_for_stop(handle)
}
