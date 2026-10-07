//! The daemon's log: one SQLite file per receiver.
//!
//! The daemon writes it and two programs read it, the web view and the
//! monitor.  Neither reader links the daemon, so what all three must
//! agree on lives here: the tables, the schema version, how a
//! timestamp is stored, where the cadence is recorded, and every query
//! a reader makes.  How a log is opened for writing and its version
//! checked is here too, since every writer does it alike; the writes
//! and the migrations stay with the daemon, which is the only thing
//! that performs them.
//!
//! The readers' tests build their logs from [`schema::TABLES`], the
//! same definition the daemon creates, so a change to a table that
//! forgets a reader fails here rather than on a bench.

pub mod error;
pub mod reader;
pub mod schema;
pub mod sensors;
pub mod writer;
