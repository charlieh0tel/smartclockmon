//! Windows: the console says why it wants the daemon to stop, on a
//! thread of the system's own.
//!
//! Ctrl-C and Ctrl-Break leave the daemon running until it has stopped
//! by itself.  Closing the console, logging off and shutting down end
//! it as soon as the handler returns, so for those the handler holds on
//! for [`GRACE`], within which the daemon writes what it holds and
//! exits on its own.

use std::sync::Arc;
use std::sync::OnceLock;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering;
use std::thread;
use std::time::Duration;

use anyhow::Result;
use smartclock::task::Handle;
use windows_sys::Win32::Foundation::FALSE;
use windows_sys::Win32::Foundation::TRUE;
use windows_sys::Win32::System::Console::CTRL_BREAK_EVENT;
use windows_sys::Win32::System::Console::CTRL_C_EVENT;
use windows_sys::Win32::System::Console::CTRL_CLOSE_EVENT;
use windows_sys::Win32::System::Console::CTRL_LOGOFF_EVENT;
use windows_sys::Win32::System::Console::CTRL_SHUTDOWN_EVENT;
use windows_sys::Win32::System::Console::SetConsoleCtrlHandler;
use windows_sys::core::BOOL;

use super::AGAIN;

/// How long the daemon is given once the console is closing, logging
/// off or shutting down.  Windows ends the process itself five seconds
/// in, so this is the most that is safe to ask for.
const GRACE: Duration = Duration::from_secs(4);

/// What the handler acts on: the task to wake, and the flag the caller
/// watches.  Set once, before the handler can run.
static STOPPING: OnceLock<(Handle, Arc<AtomicBool>)> = OnceLock::new();

pub(super) fn watch_for_stop(handle: Handle) -> Result<Arc<AtomicBool>> {
    let stopping = Arc::new(AtomicBool::new(false));
    if STOPPING.set((handle, Arc::clone(&stopping))).is_err() {
        anyhow::bail!("already watching for a stop");
    }
    #[expect(
        unsafe_code,
        reason = "windows-sys has no safe call for SetConsoleCtrlHandler"
    )]
    // SAFETY: `on_console` is a plain function with the signature the
    // console expects, living as long as the process, and touches only
    // `STOPPING`, which is set above and never changes again.
    let added = unsafe { SetConsoleCtrlHandler(Some(on_console), TRUE) };
    if added == FALSE {
        return Err(std::io::Error::last_os_error().into());
    }
    Ok(stopping)
}

/// Called by the console, on a thread of its own, with what happened.
extern "system" fn on_console(event: u32) -> BOOL {
    let Some((handle, stopping)) = STOPPING.get() else {
        return FALSE;
    };
    let interrupt = matches!(event, CTRL_C_EVENT | CTRL_BREAK_EVENT);
    if stopping.swap(true, Ordering::SeqCst) {
        if interrupt {
            eprintln!("smartclockd: interrupted again, exiting now");
            std::process::exit(AGAIN);
        }
    } else {
        eprintln!("smartclockd: {}, stopping", describe(event));
        handle.stop();
    }
    if !interrupt {
        thread::sleep(GRACE);
    }
    TRUE
}

/// What a console event is, for the journal.
fn describe(event: u32) -> String {
    match event {
        CTRL_C_EVENT => "Ctrl-C".to_owned(),
        CTRL_BREAK_EVENT => "Ctrl-Break".to_owned(),
        CTRL_CLOSE_EVENT => "the console closing".to_owned(),
        CTRL_LOGOFF_EVENT => "logging off".to_owned(),
        CTRL_SHUTDOWN_EVENT => "shutting down".to_owned(),
        other => format!("console event {other}"),
    }
}

#[cfg(test)]
mod tests {
    use super::on_console;
    use super::watch_for_stop;
    use smartclock::task::Handle;
    use smartclock::task::Shared;
    use std::sync::atomic::Ordering;
    use std::sync::mpsc::channel;
    use windows_sys::Win32::Foundation::TRUE;
    use windows_sys::Win32::System::Console::CTRL_C_EVENT;

    #[test]
    fn ctrl_c_asks_the_daemon_to_stop_and_lets_it() {
        let (requests, _queue) = channel();
        let stopping = watch_for_stop(Handle::new(requests, Shared::new())).expect("watch");
        assert!(!stopping.load(Ordering::SeqCst));
        assert_eq!(on_console(CTRL_C_EVENT), TRUE);
        assert!(stopping.load(Ordering::SeqCst));
    }
}
