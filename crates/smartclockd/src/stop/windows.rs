//! Windows: a signal only sets a flag, so a thread watches it.

use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering;
use std::thread;
use std::time::Duration;

use anyhow::Context as _;
use anyhow::Result;
use signal_hook::consts::SIGINT;
use signal_hook::consts::SIGTERM;
use smartclock::task::Handle;

/// How often the stop flag is looked at.
const STOP_CHECK: Duration = Duration::from_millis(200);

pub(super) fn watch_for_stop(handle: Handle) -> Result<Arc<AtomicBool>> {
    let stopping = Arc::new(AtomicBool::new(false));
    for signal in [SIGTERM, SIGINT] {
        // Registered first, so it sees the flag as the previous signal
        // left it: set means this is the second.
        signal_hook::flag::register_conditional_shutdown(signal, 1, Arc::clone(&stopping))
            .context("watching for signals")?;
        signal_hook::flag::register(signal, Arc::clone(&stopping))
            .context("watching for signals")?;
    }
    let flag = Arc::clone(&stopping);
    thread::Builder::new()
        .name("smartclockd-signals".to_owned())
        .spawn(move || {
            while !flag.load(Ordering::SeqCst) {
                thread::sleep(STOP_CHECK);
            }
            eprintln!("smartclockd: interrupted, stopping");
            handle.stop();
        })
        .context("spawning the signal thread")?;
    Ok(stopping)
}
