//! Our timestamps as the logs store them, and the SQL that reads them.
//!
//! Every column holding a time we recorded is written and read through
//! here, so how one is stored is decided in one place.  The receiver's
//! own times -- its log entries' stamps, its date and time of day --
//! are what it said, and are kept as text elsewhere.

use std::fmt;

use jiff::Timestamp;
use rusqlite::types::FromSql;
use rusqlite::types::FromSqlError;
use rusqlite::types::FromSqlResult;
use rusqlite::types::ToSql;
use rusqlite::types::ToSqlOutput;
use rusqlite::types::ValueRef;

/// One of our timestamps, as a log column holds it.
///
/// RFC 3339 in UTC, always with nine fractional digits, so that text
/// order is time order: the default form trims trailing zeros, and
/// `00Z`, `00.1Z` and `00.11Z` sort the wrong way round as text.
/// Displayed the same way, which is how the web view reports it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Stored(pub Timestamp);

impl fmt::Display for Stored {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:.9}", self.0)
    }
}

impl ToSql for Stored {
    fn to_sql(&self) -> rusqlite::Result<ToSqlOutput<'_>> {
        Ok(ToSqlOutput::from(self.to_string()))
    }
}

impl FromSql for Stored {
    fn column_result(value: ValueRef<'_>) -> FromSqlResult<Self> {
        value
            .as_str()?
            .parse::<Timestamp>()
            .map(Stored)
            .map_err(|e| FromSqlError::Other(Box::new(e)))
    }
}

/// `column`, an SQL expression for one of our timestamps, in unix
/// seconds with their fraction.
pub fn seconds(column: &str) -> String {
    format!("unixepoch({column}, 'subsec')")
}

/// `column` in whole unix seconds, the fraction dropped.
pub fn whole_seconds(column: &str) -> String {
    format!("unixepoch({column})")
}

/// A bound of a range, from `seconds`, an SQL expression in whole unix
/// seconds, in a form compared directly with a timestamp column.
///
/// Compared against the column itself, so its index can be used;
/// `unixepoch(at) >= ?` reads every row in the table.  A bound at a
/// whole second sorts before every row within that second, which is
/// what an inclusive lower and an exclusive upper bound want.
pub fn bound(seconds: &str) -> String {
    format!("strftime('%Y-%m-%dT%H:%M:%S', {seconds}, 'unixepoch')")
}

#[cfg(test)]
mod tests {
    use super::Stored;
    use super::bound;
    use super::seconds;
    use jiff::Timestamp;
    use rusqlite::Connection;

    #[test]
    fn a_timestamp_comes_back_as_it_went_in() {
        let conn = Connection::open_in_memory().expect("open");
        let at: Timestamp = "2026-10-08T12:34:56.000000001Z".parse().expect("a time");
        let back: Stored = conn
            .query_row("SELECT ?1", [Stored(at)], |row| row.get(0))
            .expect("read back");
        assert_eq!(back, Stored(at));
    }

    #[test]
    fn stored_order_is_time_order() {
        let times = [
            "2026-10-08T00:00:00Z",
            "2026-10-08T00:00:00.1Z",
            "2026-10-08T00:00:00.11Z",
        ]
        .map(|t| Stored(t.parse().expect("a time")));
        let mut texts: Vec<String> = times.iter().map(Stored::to_string).collect();
        texts.sort();
        assert_eq!(texts, times.map(|t| t.to_string()));
        assert_eq!(texts[1], "2026-10-08T00:00:00.100000000Z");
    }

    #[test]
    fn a_bound_falls_before_every_reading_in_its_second() {
        let conn = Connection::open_in_memory().expect("open");
        let at: Timestamp = "2026-10-08T12:34:56.5Z".parse().expect("a time");
        let whole = at.as_second();
        let (inside, seconds_back): (bool, f64) = conn
            .query_row(
                &format!(
                    "SELECT ?1 >= {} AND ?1 < {}, {}",
                    bound("?2"),
                    bound("?2 + 1"),
                    seconds("?1")
                ),
                (Stored(at), whole),
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .expect("compare");
        assert!(inside);
        assert!((seconds_back - (whole as f64 + 0.5)).abs() < 1e-3);
    }
}
