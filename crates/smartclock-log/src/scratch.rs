//! A log file for a test, under the test runner's temp dir.

use std::path::Path;
use std::path::PathBuf;

use rusqlite::Connection;

/// A log path that deletes itself, and the `-wal` and `-shm` SQLite
/// writes beside it, on drop.  Drop also runs on a panicking test,
/// where a line at the end of the test body would not.
pub(crate) struct Scratch(PathBuf);

impl Scratch {
    /// A path named for the test, emptied of anything a previous run
    /// left behind.
    pub(crate) fn new(name: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "smartclock-log-{name}-{}.sqlite",
            std::process::id()
        ));
        let guard = Self(path);
        guard.wipe();
        guard
    }

    pub(crate) fn path(&self) -> &Path {
        &self.0
    }

    /// A write connection, as a service would hold.
    pub(crate) fn connect(&self) -> Connection {
        Connection::open(&self.0).expect("open the log for writing")
    }

    fn wipe(&self) {
        for suffix in ["", "-wal", "-shm"] {
            let mut name = self.0.clone().into_os_string();
            name.push(suffix);
            let _ = std::fs::remove_file(PathBuf::from(name));
        }
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        self.wipe();
    }
}
