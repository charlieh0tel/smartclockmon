//! Our timestamps as the logs store them, and the SQL that reads them.
//!
//! Every column holding a time we recorded is written and read through
//! here, so how one is stored is decided in one place.  The receiver's
//! own times -- its log entries' stamps, its date and time of day --
//! are what it said, and are kept as text elsewhere.
//!
//! Until schema 12 a timestamp was stored as text; [`from_text`] is the
//! conversion the migration makes.

use std::fmt;

use jiff::Timestamp;
use rusqlite::types::FromSql;
use rusqlite::types::FromSqlError;
use rusqlite::types::FromSqlResult;
use rusqlite::types::ToSql;
use rusqlite::types::ToSqlOutput;
use rusqlite::types::ValueRef;

/// Nanoseconds in a second.
const NANOSECONDS: i64 = 1_000_000_000;

/// One of our timestamps, as a log column holds it: nanoseconds since
/// the Unix epoch, UTC, as an integer.
///
/// An integer rather than text or a float, so that a value reads back
/// exactly as it was written, compares exactly, and orders as time
/// does.  Sixty-four bits of nanoseconds run to the year 2262.
/// Displayed as RFC 3339 with nine fractional digits, which is how the
/// web view reports it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Stored(pub Timestamp);

impl fmt::Display for Stored {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:.9}", self.0)
    }
}

impl ToSql for Stored {
    fn to_sql(&self) -> rusqlite::Result<ToSqlOutput<'_>> {
        let nanoseconds = i64::try_from(self.0.as_nanosecond())
            .map_err(|e| rusqlite::Error::ToSqlConversionFailure(Box::new(e)))?;
        Ok(ToSqlOutput::from(nanoseconds))
    }
}

impl FromSql for Stored {
    fn column_result(value: ValueRef<'_>) -> FromSqlResult<Self> {
        Timestamp::from_nanosecond(i128::from(value.as_i64()?))
            .map(Stored)
            .map_err(|e| FromSqlError::Other(Box::new(e)))
    }
}

/// `column`, an SQL expression for one of our timestamps, in unix
/// seconds with their fraction.
pub fn seconds(column: &str) -> String {
    format!("({column} / 1e9)")
}

/// `column` in whole unix seconds, the fraction dropped.
pub fn whole_seconds(column: &str) -> String {
    format!("({column} / {NANOSECONDS})")
}

/// A bound of a range, from `seconds`, an SQL expression in whole unix
/// seconds, in a form compared directly with a timestamp column, so
/// the column's index can be used.  A bound at a whole second falls
/// before every reading within that second, which is what an inclusive
/// lower and an exclusive upper bound want.
pub fn bound(seconds: &str) -> String {
    format!("(({seconds}) * {NANOSECONDS})")
}

/// `column`, a timestamp stored as text in the form schema 11 and
/// earlier wrote -- RFC 3339 in UTC with nine fractional digits -- as
/// the integer [`Stored`] now writes.  Exact: the whole seconds from
/// SQLite's date functions, the nanoseconds from the digits, since a
/// conversion through a float would round them.
pub fn from_text(column: &str) -> String {
    format!("(unixepoch({column}) * {NANOSECONDS} + CAST(substr({column}, 21, 9) AS INTEGER))")
}

/// An SQL condition true where `column` holds a timestamp text
/// [`from_text`] can convert exactly.
pub fn convertible(column: &str) -> String {
    format!(
        "(length({column}) = 30 AND {column} GLOB \
         '[0-9][0-9][0-9][0-9]-[0-9][0-9]-[0-9][0-9]T[0-9][0-9]:[0-9][0-9]:[0-9][0-9].\
         [0-9][0-9][0-9][0-9][0-9][0-9][0-9][0-9][0-9]Z')"
    )
}

#[cfg(test)]
mod tests {
    use super::Stored;
    use super::bound;
    use super::convertible;
    use super::from_text;
    use super::seconds;
    use super::whole_seconds;
    use jiff::Timestamp;
    use rusqlite::Connection;

    #[test]
    fn a_timestamp_comes_back_exactly_as_it_went_in() {
        let conn = Connection::open_in_memory().expect("open");
        let at: Timestamp = "2026-10-08T12:34:56.000000001Z".parse().expect("a time");
        let back: Stored = conn
            .query_row("SELECT ?1", [Stored(at)], |row| row.get(0))
            .expect("read back");
        assert_eq!(back, Stored(at));
        assert_eq!(Stored(at).to_string(), "2026-10-08T12:34:56.000000001Z");
    }

    #[test]
    fn a_bound_falls_before_every_reading_in_its_second() {
        let conn = Connection::open_in_memory().expect("open");
        let at: Timestamp = "2026-10-08T12:34:56.5Z".parse().expect("a time");
        let whole = at.as_second();
        let (inside, back, truncated): (bool, f64, i64) = conn
            .query_row(
                &format!(
                    "SELECT ?1 >= {} AND ?1 < {}, {}, {}",
                    bound("?2"),
                    bound("?2 + 1"),
                    seconds("?1"),
                    whole_seconds("?1"),
                ),
                (Stored(at), whole),
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .expect("compare");
        assert!(inside);
        assert!((back - (whole as f64 + 0.5)).abs() < 1e-6);
        assert_eq!(truncated, whole);
    }

    #[test]
    fn text_as_schema_11_wrote_it_converts_to_the_same_nanosecond() {
        let conn = Connection::open_in_memory().expect("open");
        let text = "2026-10-08T12:34:56.123456789Z";
        let (ok, converted): (bool, Stored) = conn
            .query_row(
                &format!("SELECT {}, {}", convertible("?1"), from_text("?1")),
                [text],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .expect("convert");
        assert!(ok);
        assert_eq!(converted, Stored(text.parse().expect("a time")));
    }

    #[test]
    fn text_in_any_other_form_is_not_convertible() {
        let conn = Connection::open_in_memory().expect("open");
        for text in [
            "2026-10-08",
            "2026-10-08T12:34:56Z",
            "2026-10-08T12:34:56.1Z",
            "",
        ] {
            let ok: bool = conn
                .query_row(&format!("SELECT {}", convertible("?1")), [text], |row| {
                    row.get(0)
                })
                .expect("check");
            assert!(!ok, "{text:?}");
        }
    }
}
