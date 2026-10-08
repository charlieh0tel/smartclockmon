//! The readers against logs built from the daemon's own table
//! definitions, so a change to a table that forgets a reader fails
//! here.

use rusqlite::Connection;

use super::Log;
use super::MAX_PHASE_ROWS;
use super::Modes;
use super::Series;
use crate::schema::META;
use crate::schema::TABLES;
use crate::scratch::Scratch;
use crate::timestamp::Stored;

/// A new log holding the empty tables.
fn fresh(name: &str) -> Scratch {
    let scratch = Scratch::new(name);
    let conn = scratch.connect();
    conn.execute_batch(META).expect("meta");
    conn.execute_batch(TABLES).expect("the tables");
    scratch
}

/// A timestamp `second` unix seconds in, as the daemon stores it.
fn at(second: i64) -> Stored {
    Stored(jiff::Timestamp::from_second(second).expect("a timestamp"))
}

/// A run of phase readings on disk, shaped the way the daemon writes
/// them: a row per publish, so the steps of the slower tiers repeat the
/// fast tier's last reading under a later `at`.
fn phase_log(name: &str, rows: &[(i64, f64, &str, i64)]) -> Scratch {
    let scratch = fresh(name);
    let conn = scratch.connect();
    conn.execute_batch(
        "INSERT INTO receiver (id, serial, model, first_seen, last_seen)
         VALUES (1, 'AAA', '58503A', 1788220800000000000, 1788220800000000000);",
    )
    .expect("a receiver");
    for &(second, interval, mode, holdover) in rows {
        insert_phase(
            &conn,
            &at(second),
            &at(second),
            Some(interval),
            mode,
            holdover,
        );
    }
    scratch
}

/// One snapshot row carrying a phase reading.
fn insert_phase(
    conn: &Connection,
    at: &Stored,
    fast_at: &Stored,
    interval: Option<f64>,
    mode: &str,
    holdover: i64,
) {
    conn.execute(
        "INSERT INTO snapshot
         (at, freshness, fast_at, time_interval_s, mode, holdover_active, receiver_id)
         VALUES (?1, 'live', ?2, ?3, ?4, ?5, 1)",
        rusqlite::params![at, fast_at, interval, mode, holdover],
    )
    .expect("a row");
}

#[test]
fn a_steady_ramp_has_no_deviation_and_the_repeats_are_not_counted() {
    // The receiver's phase walking at a constant rate is a constant
    // frequency offset, not instability, so the curve must sit on the
    // floor.  The slower tiers publish rows of their own half a second
    // after a reading, carrying the fast tier's earlier `fast_at`:
    // those are not readings.  Each is given an interval far off the
    // ramp, so counting even one would lift the curve.
    let rows: Vec<_> = (0..600i64)
        .map(|i| (1_700_000_000 + i, 2e-9 * i as f64, "Locked", 0))
        .collect();
    let scratch = phase_log("adev-ramp", &rows);
    let conn = scratch.connect();
    for i in (9..600i64).step_by(10) {
        let read = jiff::Timestamp::from_second(1_700_000_000 + i).expect("a timestamp");
        let later = read + jiff::SignedDuration::from_millis(500);
        insert_phase(
            &conn,
            &Stored(later),
            &Stored(read),
            Some(1e-6),
            "Locked",
            0,
        );
    }
    drop(conn);
    let log = Log::open(scratch.path()).expect("open");
    let (deviation, _) = log
        .phase(1, 0, 2_000_000_000, MAX_PHASE_ROWS)
        .expect("a deviation");

    assert_eq!(deviation.tau0, 1.0);
    assert_eq!(deviation.segments, 1);
    assert_eq!(deviation.present, 600, "repeats were counted as readings");
    assert_eq!(deviation.holes, 0);
    assert!(!deviation.points.is_empty());
    for point in &deviation.points {
        assert!(point.deviation < 1e-15, "{point:?}");
    }
}

#[test]
fn a_relock_does_not_become_instability() {
    // Holdover, then a relock that steps the phase by a microsecond.
    // Joined, that step is a deviation of about 1e-6 at tau = 1; split,
    // it is not a measurement at all.
    let mut rows = Vec::new();
    for i in 0..300i64 {
        rows.push((1_700_000_000 + i, 1e-9 * i as f64, "Holdover", 1));
    }
    for i in 300..600i64 {
        rows.push((1_700_000_000 + i, 1e-6 + 1e-9 * i as f64, "Locked", 0));
    }
    let scratch = phase_log("adev-relock", &rows);
    let log = Log::open(scratch.path()).expect("open");
    let (deviation, _) = log
        .phase(1, 0, 2_000_000_000, MAX_PHASE_ROWS)
        .expect("a deviation");

    assert_eq!(deviation.segments, 2);
    for point in &deviation.points {
        assert!(point.deviation < 1e-15, "{point:?}");
    }
}

#[test]
fn a_holdover_inside_a_held_reading_still_breaks_the_run() {
    // One-second rows with the interval updated every ten, as the
    // receiver does.  Three rows of holdover sit inside one held value,
    // and the phase steps by a microsecond at the next update.  The
    // query thins held rows out, and must keep the state changes or the
    // step is read as instability.
    let rows: Vec<_> = (0..600i64)
        .map(|i| {
            let update = i / 10;
            let step = if i >= 310 { 1e-6 } else { 0.0 };
            let mode = if (302..305).contains(&i) {
                "Holdover"
            } else {
                "Locked"
            };
            (1_700_000_000 + i, step + 1e-9 * update as f64, mode, 0)
        })
        .collect();
    let scratch = phase_log("adev-held-holdover", &rows);
    let log = Log::open(scratch.path()).expect("open");
    let (deviation, truncated) = log
        .phase(1, 0, 2_000_000_000, MAX_PHASE_ROWS)
        .expect("a deviation");
    assert!(!truncated);
    assert!(deviation.segments >= 2, "{} segments", deviation.segments);
    for point in &deviation.points {
        assert!(point.deviation < 1e-12, "{point:?}");
    }
}

#[test]
fn a_holdover_with_no_interval_breaks_the_run() {
    // Locked, five rows of holdover in which the interval was refused
    // and so is NULL, then locked with the phase stepped.
    let mut rows: Vec<_> = (0..200i64)
        .map(|i| (1_700_000_000 + i, 1e-9 * i as f64, "Locked", 0))
        .collect();
    rows.extend((205..400i64).map(|i| (1_700_000_000 + i, 5e-6 + 1e-9 * i as f64, "Locked", 0)));
    let scratch = phase_log("adev-null-holdover", &rows);
    let conn = scratch.connect();
    for i in 200..205i64 {
        let when = at(1_700_000_000 + i);
        insert_phase(&conn, &when, &when, None, "Holdover", 1);
    }
    drop(conn);
    let log = Log::open(scratch.path()).expect("open");
    let (deviation, _) = log
        .phase(1, 0, 2_000_000_000, MAX_PHASE_ROWS)
        .expect("a deviation");
    assert!(deviation.segments >= 2, "{} segments", deviation.segments);
    for point in &deviation.points {
        assert!(point.deviation < 1e-12, "{point:?}");
    }
}

#[test]
fn a_range_past_the_limit_is_measured_over_its_newest_readings() {
    // The oldest sixty wander; the newest forty are a clean ramp.  Only
    // the newest forty give a curve on the floor.
    let rows: Vec<_> = (0..100i64)
        .map(|i| {
            let wander = if i < 60 {
                0.37e-9 * ((i * 7919) % 13) as f64
            } else {
                0.0
            };
            (1_700_000_000 + i, 1e-9 * i as f64 + wander, "Locked", 0)
        })
        .collect();
    let scratch = phase_log("adev-limit", &rows);
    let log = Log::open(scratch.path()).expect("open");

    let (all, truncated) = log.phase(1, 0, 2_000_000_000, 100).expect("a deviation");
    assert!(!truncated);
    assert_eq!(all.present, 100);

    let (newest, truncated) = log.phase(1, 0, 2_000_000_000, 40).expect("a deviation");
    assert!(truncated);
    assert_eq!(newest.present, 40);
    assert!(!newest.points.is_empty());
    for point in &newest.points {
        assert!(point.deviation < 1e-15, "the oldest were kept: {point:?}");
    }
}

#[test]
fn a_range_with_too_little_in_it_yields_no_curve() {
    // Nine readings cannot support even tau = 1, and the honest answer
    // is an empty curve rather than a point drawn from two differences.
    let rows: Vec<_> = (0..9i64)
        .map(|i| (1_700_000_000 + i, 1e-9 * i as f64, "Locked", 0))
        .collect();
    let scratch = phase_log("adev-short", &rows);
    let log = Log::open(scratch.path()).expect("open");
    let (deviation, _) = log
        .phase(1, 0, 2_000_000_000, MAX_PHASE_ROWS)
        .expect("a deviation");
    assert!(deviation.points.is_empty());
    assert_eq!(deviation.present, 9);
}

/// A log holding two receivers, each with its own snapshots, log
/// entries, events and errors.
fn two_units(name: &str) -> Scratch {
    let scratch = fresh(name);
    scratch
        .connect()
        .execute_batch(
            r#"
            INSERT INTO receiver (id, serial, manufacturer, model, firmware, first_seen, last_seen)
            VALUES
                (1,'AAA','HEWLETT-PACKARD','58503A','3704-C',
                 1788220800000000000,1788228000000000000),
                (2,'BBB','HEWLETT-PACKARD','Z3805A','3611-A',
                 1788307200000000000,1788314400000000000);

            INSERT INTO snapshot (at, freshness, fast_at, efc_percent, receiver_id) VALUES
                (1788220800000000000,'live',1788220800000000000, 10.0, 1),
                (1788220801000000000,'live',1788220801000000000, 11.0, 1),
                (1788307200000000000,'live',1788307200000000000, 90.0, 2),
                (1788307201000000000,'live',1788307201000000000, 91.0, 2);

            -- Out of entry order on purpose, and with a power-on whose
            -- stamp is midnight on a stale date: ordering by stamp puts
            -- entry 2 last, which is the bug this ordering replaced.
            INSERT INTO receiver_log (at, entry, stamp, message, receiver_id, generation) VALUES
                (1788220800000000000,1,'20050528.00:01:00','one A',        1,0),
                (1788220801000000000,2,'20050528.00:00:00','Power on',     1,0),
                (1788220802000000000,1,'20050530.00:00:00','after clear A',1,1),
                (1788307200000000000,1,'20050601.00:00:00','one B',        2,0);

            INSERT INTO receiver_event (at, register, bits, decoded, receiver_id) VALUES
                (1788220800000000000,'alarm',0,'clear A',1),
                (1788307200000000000,'alarm',8,'holdover B',2);

            INSERT INTO receiver_error (at, code, message, receiver_id) VALUES
                (1788220800000000000,-113,'undefined header A',1),
                (1788307200000000000,-230,'data corrupt or stale B',2);
            "#,
        )
        .expect("fill it");
    scratch
}

#[test]
fn a_plot_shows_one_receiver_and_not_the_other() {
    // The bug this exists for: every query read the whole table, so two
    // units' readings were drawn as one trace and a swap looked like an
    // oscillator stepping.
    let scratch = two_units("plot");
    let log = Log::open(scratch.path()).expect("open");
    let columns = vec!["efc_percent".to_owned()];
    // The window is every row either unit has, so anything left out was
    // left out by the filter and not by the range.
    let (first, last) = {
        let (fa, la) = log.extent(1).expect("A's extent");
        let (fb, lb) = log.extent(2).expect("B's extent");
        (fa.min(fb) as i64, la.max(lb) as i64)
    };
    let a = log
        .series(1, &columns, first, last, 100, Modes::Every)
        .expect("unit A");
    let b = log
        .series(2, &columns, first, last, 100, Modes::Every)
        .expect("unit B");
    // The window spans both units' days, so each one's two rows fall in
    // a single bucket: the mean of that bucket is the test.
    // Contamination could not hide in it -- mixing A's 10 and 11 with
    // B's 90 and 91 gives 50.5, not 10.5.
    let span = |s: &Series| {
        let plot = &s.plots[0];
        (
            plot.mean.iter().flatten().copied().collect::<Vec<_>>(),
            plot.min.iter().flatten().copied().collect::<Vec<_>>(),
            plot.max.iter().flatten().copied().collect::<Vec<_>>(),
        )
    };
    assert_eq!(span(&a), (vec![10.5], vec![10.0], vec![11.0]), "A's only");
    assert_eq!(span(&b), (vec![90.5], vec![90.0], vec![91.0]), "B's only");
}

#[test]
fn a_column_not_plottable_is_refused() {
    let scratch = two_units("refused");
    let log = Log::open(scratch.path()).expect("open");
    let asked = vec!["efc_percent); DROP TABLE snapshot; --".to_owned()];
    assert!(
        log.series(1, &asked, 0, 2_000_000_000, 100, Modes::Every)
            .is_err()
    );
}

#[test]
fn a_slower_tier_that_stops_reading_stops_being_plotted() {
    // Fast rows every second for two minutes, each carrying the
    // temperature the medium tier last read.  The medium tier reads at
    // 0 s and 10 s and then fails.  Its value is current for three of
    // its intervals after the last read, to 40 s, and is not a
    // measurement after that.
    let scratch = fresh("stale-tier");
    let conn = scratch.connect();
    conn.execute_batch(
        "INSERT INTO meta VALUES ('cadence_medium', '10');
         INSERT INTO receiver (id, serial, first_seen, last_seen) VALUES (1, 'AAA', 1788220800000000000, 1788220800000000000);",
    )
    .expect("cadence and a receiver");
    let start = 1_700_000_000;
    for second in 0..120 {
        conn.execute(
            "INSERT INTO snapshot (at, freshness, fast_at, medium_at, temperature_c, receiver_id)
             VALUES (?1, 'stale', ?1, ?2, 35.0, 1)",
            rusqlite::params![at(start + second), at(start + second.min(10) / 10 * 10)],
        )
        .expect("a row");
    }
    drop(conn);

    let log = Log::open(scratch.path()).expect("open");
    let series = log
        .series(
            1,
            &["temperature_c".to_owned()],
            start,
            start + 119,
            120,
            Modes::Every,
        )
        .expect("series");
    let plotted: Vec<f64> = series
        .at
        .iter()
        .zip(&series.plots[0].mean)
        .filter(|(_, mean)| mean.is_some())
        .map(|(at, _)| at - start as f64)
        .collect();
    assert_eq!(plotted.first(), Some(&0.0));
    assert_eq!(plotted.last(), Some(&40.0));
}

#[test]
fn a_gap_in_the_record_breaks_the_line() {
    // Asked for a window covering two clusters of readings at a
    // resolution finer than the hole between them, the series must
    // carry a null there: without one the chart joins the points either
    // side and draws a receiver sitting perfectly steady through hours
    // it was unplugged.
    let scratch = two_units("gap");
    scratch
        .connect()
        .execute_batch(
            "INSERT INTO snapshot (at, freshness, fast_at, efc_percent, receiver_id) VALUES
                (1788566400000000000,'live',1788566400000000000, 70.0, 2),
                (1788566401000000000,'live',1788566401000000000, 71.0, 2);",
        )
        .expect("extend");
    let log = Log::open(scratch.path()).expect("open");
    let (first, last) = log.extent(2).expect("B's extent");
    let s = log
        .series(
            2,
            &["efc_percent".to_owned()],
            first as i64,
            last as i64,
            200,
            Modes::Every,
        )
        .expect("unit B over the whole span");
    let mean = &s.plots[0].mean;
    assert!(
        mean.iter().any(Option::is_none),
        "a window spanning the empty days must carry a null: {mean:?}"
    );
    assert_eq!(mean.iter().filter(|v| v.is_some()).count(), 2);
    assert_eq!(s.at.len(), mean.len(), "every point needs an x");
}

#[test]
fn a_locked_series_leaves_out_what_was_read_while_not_locked() {
    // In holdover the EFC is frozen and in recovery it is slewed, so a
    // series asked for while locked must not average those in.
    let scratch = two_units("locked");
    scratch
        .connect()
        .execute_batch(
            "INSERT INTO snapshot (at, freshness, fast_at, mode, efc_percent, receiver_id) VALUES
                (1788566400000000000,'live',1788566400000000000,'Locked', 10.0, 2),
                (1788566401000000000,'live',1788566401000000000,'Recovery', 90.0, 2);",
        )
        .expect("extend");
    let log = Log::open(scratch.path()).expect("open");
    let window = 1_788_566_400;
    let mean = |modes| {
        log.series(
            2,
            &["efc_percent".to_owned()],
            window,
            window + 1,
            16,
            modes,
        )
        .expect("series")
        .plots[0]
            .mean
            .iter()
            .flatten()
            .copied()
            .collect::<Vec<f64>>()
    };
    assert_eq!(mean(Modes::Locked), [10.0]);
    assert_eq!(mean(Modes::Every), [10.0, 90.0]);
}

#[test]
fn the_extent_is_the_chosen_receivers_own() {
    // A shared extent would open a view on a window in which the
    // selected unit has nothing, which reads as a dead receiver.
    let scratch = two_units("extent");
    let log = Log::open(scratch.path()).expect("open");
    let (first_a, last_a) = log.extent(1).expect("A");
    let (first_b, _) = log.extent(2).expect("B");
    assert!(last_a < first_b, "A's history ends before B's begins");
    assert!(first_a < last_a);
}

#[test]
fn the_journal_is_one_receivers_and_in_the_receivers_own_order() {
    let scratch = two_units("journal");
    let log = Log::open(scratch.path()).expect("open");
    let a = log.journal(1, 50).expect("A's journal");
    assert_eq!(
        a.entries
            .iter()
            .map(|e| e.message.as_str())
            .collect::<Vec<_>>(),
        vec!["after clear A", "Power on", "one A"],
        "newest generation first, then by entry number, not by stamp"
    );
    assert_eq!(a.events.len(), 1);
    assert_eq!(a.events[0].decoded, "clear A");
    assert_eq!(a.errors.len(), 1);
    assert_eq!(a.errors[0].code, -113);

    let b = log.journal(2, 50).expect("B's journal");
    assert_eq!(
        b.entries
            .iter()
            .map(|e| e.message.as_str())
            .collect::<Vec<_>>(),
        vec!["one B"]
    );
    assert_eq!(b.errors[0].code, -230);
}

#[test]
fn notes_come_newest_first_and_a_fact_shows_its_latest_value() {
    let scratch = two_units("notes");
    scratch
        .connect()
        .execute_batch(
            r#"
            INSERT INTO note (at, text, receiver_id) VALUES
                (1788220800000000000,'first A',1),
                (1788220805000000000,'later A',1),
                (1788307200000000000,'only B',2);
            INSERT INTO fact (since, key, value, receiver_id) VALUES
                (1788220800000000000,'ocxo.serial','old',1),
                (1788393600000000000,'ocxo.serial','new',1),
                (1788220800000000000,'antenna.feed','LNA',1),
                (1788307200000000000,'ocxo.serial','B',2);
            "#,
        )
        .expect("fill it");
    let log = Log::open(scratch.path()).expect("open");
    let notes = log.journal(1, 50).expect("A's journal").notes;
    assert_eq!(
        notes.iter().map(|n| n.text.as_str()).collect::<Vec<_>>(),
        vec!["later A", "first A"]
    );
    let facts = log.facts(1).expect("A's facts");
    assert_eq!(
        facts
            .iter()
            .map(|f| (f.key.as_str(), f.value.as_str()))
            .collect::<Vec<_>>(),
        vec![("antenna.feed", "LNA"), ("ocxo.serial", "new")],
        "by key, each at its latest value, and none of B's"
    );
}

#[test]
fn notes_are_read_by_range_however_old() {
    let scratch = two_units("ranged-notes");
    let conn = scratch.connect();
    // More newer notes than the journal shows, after the one asked for.
    conn.execute(
        "INSERT INTO note (at, text, receiver_id) VALUES (1767225600000000000, 'old', 1)",
        [],
    )
    .expect("the old note");
    for n in 0..600 {
        conn.execute(
            "INSERT INTO note (at, text, receiver_id) VALUES (?1, 'newer', 1)",
            [at(1_790_000_000 + n)],
        )
        .expect("a newer note");
    }
    let log = Log::open(scratch.path()).expect("open");
    let jan = 1_767_225_600; // 2026-01-01T00:00:00Z
    let old = log.notes(1, jan - 60, jan + 60).expect("the notes");
    assert_eq!(
        old.iter().map(|n| n.text.as_str()).collect::<Vec<_>>(),
        vec!["old"]
    );
    assert!(
        log.notes(2, jan - 60, jan + 60)
            .expect("B's notes")
            .is_empty()
    );
}

#[test]
fn a_log_older_than_notes_has_none() {
    let scratch = two_units("old-notes");
    scratch
        .connect()
        .execute_batch("DROP TABLE note; DROP TABLE fact;")
        .expect("make it old");
    let log = Log::open(scratch.path()).expect("open");
    assert!(log.journal(1, 50).expect("journal").notes.is_empty());
    assert!(log.facts(1).expect("facts").is_empty());
    assert!(
        log.notes(1, 0, i64::MAX / 2)
            .expect("ranged notes")
            .is_empty()
    );
}

#[test]
fn many_events_do_not_crowd_the_diagnostic_log_out_of_the_journal() {
    let scratch = two_units("crowded");
    let conn = scratch.connect();
    for n in 0..20 {
        conn.execute(
            "INSERT INTO receiver_event (at, register, bits, decoded, receiver_id)
             VALUES (?1, 'alarm', 0, 'clear', 1)",
            [at(1_800_000_000 + n)],
        )
        .expect("an event");
    }
    drop(conn);
    let log = Log::open(scratch.path()).expect("open");
    let journal = log.journal(1, 5).expect("A's journal");
    assert_eq!(journal.events.len(), 5);
    assert_eq!(journal.entries.len(), 3, "the log was crowded out");
}

#[test]
fn the_default_receiver_is_the_one_seen_most_recently() {
    let scratch = two_units("newest");
    let log = Log::open(scratch.path()).expect("open");
    let found = log.receivers().expect("list");
    assert_eq!(
        found.iter().map(super::Receiver::label).collect::<Vec<_>>(),
        vec!["Z3805A BBB".to_owned(), "58503A AAA".to_owned()]
    );
}

#[test]
fn a_log_from_before_the_journal_tables_reads_as_empty() {
    let path = std::env::temp_dir().join(format!(
        "smartclock-log-ancient-{}.sqlite",
        std::process::id()
    ));
    let _ = std::fs::remove_file(&path);
    Connection::open(&path)
        .expect("make a log")
        .execute_batch("CREATE TABLE snapshot (id INTEGER PRIMARY KEY, at TEXT NOT NULL);")
        .expect("an old table");
    let log = Log::open(&path).expect("open");
    let receivers = log.receivers().expect("no receiver table is no receivers");
    let journal = log.journal(1, 50).expect("no journal tables is no journal");
    let _ = std::fs::remove_file(&path);
    assert!(receivers.is_empty());
    assert!(journal.entries.is_empty() && journal.events.is_empty() && journal.errors.is_empty());
}

#[test]
#[cfg(unix)]
fn a_closed_wal_log_in_a_directory_the_reader_cannot_write_is_still_read() {
    use std::os::unix::fs::PermissionsExt as _;
    // A directory of its own, so it can be made read-only.
    let dir = std::env::temp_dir().join(format!("smartclock-log-sealed-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir(&dir).expect("a directory");
    let path = dir.join("log.sqlite");
    {
        let conn = Connection::open(&path).expect("create the log");
        conn.pragma_update(None, "journal_mode", "WAL")
            .expect("WAL");
        conn.execute_batch(META).expect("meta");
        conn.execute_batch(TABLES).expect("the tables");
        conn.execute_batch(
            "INSERT INTO receiver (id, serial, manufacturer, model, firmware, first_seen, last_seen)
             VALUES (1,'AAA','HEWLETT-PACKARD','58503A','3704-C',
                     1788220800000000000,1788220800000000000);",
        )
        .expect("a receiver");
    }
    // The last connection to close took -shm with it.
    let mut shm = path.clone().into_os_string();
    shm.push("-shm");
    assert!(!std::path::Path::new(&shm).exists());
    let mode = |m| std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(m));
    mode(0o555).expect("seal the directory");
    let read = Log::open(&path).and_then(|log| log.receivers());
    mode(0o755).expect("unseal the directory");
    let _ = std::fs::remove_dir_all(&dir);
    assert_eq!(
        read.expect("read the receivers")
            .iter()
            .map(|r| r.serial.as_str())
            .collect::<Vec<_>>(),
        vec!["AAA"]
    );
}
