//! Unix: signals arrive in order, on a thread of their own.

use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering;
use std::thread;

use anyhow::Context as _;
use anyhow::Result;
use signal_hook::consts::SIGINT;
use signal_hook::consts::SIGTERM;
use signal_hook::iterator::Signals;
use smartclock::task::Handle;

pub(super) fn watch_for_stop(handle: Handle) -> Result<Arc<AtomicBool>> {
    let stopping = Arc::new(AtomicBool::new(false));
    let mut signals = Signals::new([SIGTERM, SIGINT]).context("watching for signals")?;
    let flag = Arc::clone(&stopping);
    thread::Builder::new()
        .name("smartclockd-signals".to_owned())
        .spawn(move || {
            for signal in signals.forever() {
                if flag.swap(true, Ordering::SeqCst) {
                    eprintln!("smartclockd: signal {signal} again, exiting now");
                    std::process::exit(1);
                }
                eprintln!("smartclockd: signal {signal}, stopping");
                handle.stop();
            }
        })
        .context("spawning the signal thread")?;
    Ok(stopping)
}
