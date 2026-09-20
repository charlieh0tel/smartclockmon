//! The scraper against every recorded status screen.
//!
//! The fixtures span two firmware layouts and several receiver states,
//! which is the point: a scraper tuned to one screen silently misreads
//! the others.

use std::fs;
use std::path::Path;
use std::path::PathBuf;

use smartclock::screen;

fn fixture(name: &str) -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/status_screen")
        .join(name);
    fs::read_to_string(&path).unwrap_or_else(|e| panic!("reading {}: {e}", path.display()))
}

fn all_fixtures() -> Vec<(String, String)> {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/status_screen");
    let mut found: Vec<PathBuf> = fs::read_dir(&dir)
        .expect("fixture directory")
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x == "txt"))
        .collect();
    found.sort();
    found
        .into_iter()
        .map(|p| {
            let name = p.file_name().expect("name").to_string_lossy().into_owned();
            (name, fs::read_to_string(&p).expect("read"))
        })
        .collect()
}

#[test]
fn the_live_screen_reads_correctly() {
    let s = screen::parse(&fixture("58503a-live-01.txt"));
    assert_eq!(
        s.synchronization.as_deref(),
        Some("Outputs Valid/Reduced Accuracy")
    );
    assert_eq!(
        s.mode.as_deref(),
        Some("Locked to GPS: stabilizing frequency")
    );
    assert_eq!(s.tfom, Some(3));
    assert_eq!(s.ffom, Some(1));
    assert_eq!(s.acquisition.as_deref(), Some("GPS 1PPS Valid"));
    assert_eq!(s.tracking, Some(6));
    assert_eq!(s.not_tracking, Some(3));
    assert_eq!(s.elevation_mask, Some(10));
    assert_eq!(s.antenna_delay_ns, Some(0));
    assert_eq!(s.health.as_deref(), Some("OK"));
    assert_eq!(s.position_mode.as_deref(), Some("Hold"));
}

#[test]
fn the_live_satellite_table_matches_the_separate_query() {
    // :GPS:SATellite:TRACking? returned +3,+4,+16,+26,+28,+31 in the
    // same probe run, so the table and the query must agree.
    let s = screen::parse(&fixture("58503a-live-01.txt"));
    let tracked: Vec<u8> = s
        .satellites
        .iter()
        .filter(|sat| sat.tracked)
        .map(|sat| sat.prn.get())
        .collect();
    assert_eq!(tracked, vec![3, 4, 16, 26, 28, 31]);

    let untracked: Vec<u8> = s
        .satellites
        .iter()
        .filter(|sat| !sat.tracked)
        .map(|sat| sat.prn.get())
        .collect();
    assert_eq!(untracked, vec![1, 6, 9]);

    // The asterisk means "attempting to acquire", which is narrower
    // than "not tracked": PRN 6 is neither used nor being chased.
    let acquiring: Vec<u8> = s
        .satellites
        .iter()
        .filter(|sat| sat.acquiring)
        .map(|sat| sat.prn.get())
        .collect();
    assert_eq!(acquiring, vec![1, 9]);
    assert_eq!(s.satellites.len(), 9);
}

#[test]
fn signal_strength_is_read_from_the_ss_column() {
    let s = screen::parse(&fixture("58503a-live-01.txt"));
    let prn3 = s
        .satellites
        .iter()
        .find(|x| x.prn.get() == 3)
        .expect("PRN 3");
    assert_eq!(prn3.elevation.expect("elevation").get(), 88);
    assert_eq!(prn3.azimuth.expect("azimuth").get(), 281);
    assert_eq!(prn3.signal.expect("signal").raw(), 111);

    // Not-tracked satellites have no signal column at all.
    let prn1 = s
        .satellites
        .iter()
        .find(|x| x.prn.get() == 1)
        .expect("PRN 1");
    assert_eq!(prn1.elevation.expect("elevation").get(), 24);
    assert_eq!(prn1.azimuth.expect("azimuth").get(), 204);
    assert_eq!(prn1.signal, None);
}

#[test]
fn an_acquiring_satellite_with_no_fix_yet_has_no_angles() {
    // "* 9  Acq .." on the live screen: known to exist, position unknown.
    let s = screen::parse(&fixture("58503a-live-01.txt"));
    let prn9 = s
        .satellites
        .iter()
        .find(|x| x.prn.get() == 9)
        .expect("PRN 9");
    assert!(prn9.acquiring);
    assert_eq!(prn9.elevation, None);
    assert_eq!(prn9.azimuth, None);
}

#[test]
fn the_manual_layout_reads_too() {
    // 58503a-03 is a holdover screen from 097-59551-02, with a C/N
    // column and space padding rather than SS and underscores.
    let s = screen::parse(&fixture("58503a-03.txt"));
    assert_eq!(s.mode.as_deref(), Some("Holdover: GPS 1PPS invalid"));
    assert_eq!(s.tfom, Some(3));
    assert_eq!(s.ffom, Some(2));
    assert_eq!(s.acquisition.as_deref(), Some("GPS 1PPS Invalid"));
    assert_eq!(s.tracking, Some(0));
    assert_eq!(s.not_tracking, Some(7));
    assert_eq!(s.satellites.len(), 7);
    // Tracking: 0, so neither column group is the tracked one.
    assert!(s.satellites.iter().all(|x| !x.tracked));
    assert!(s.satellites.iter().filter(|x| x.acquiring).count() >= 5);
}

#[test]
fn a_power_up_screen_with_no_angles_at_all_reads() {
    // 58503b-01: every satellite is "*nn -- ---".
    let s = screen::parse(&fixture("58503b-01.txt"));
    assert_eq!(s.mode.as_deref(), Some("Power-up:GPS acquisition"));
    assert_eq!(s.tracking, Some(0));
    assert_eq!(s.not_tracking, Some(6));
    assert_eq!(s.satellites.len(), 6);
    assert!(s.satellites.iter().all(|x| !x.tracked));
    assert!(s.satellites.iter().all(|x| x.acquiring));
    assert!(s.satellites.iter().all(|x| x.elevation.is_none()));
}

#[test]
fn the_health_line_splits_into_named_checks() {
    let s = screen::parse(&fixture("58503a-live-01.txt"));
    let items: Vec<(&str, &str)> = s
        .health_items
        .iter()
        .map(|(k, v)| (k.as_str(), v.as_str()))
        .collect();
    assert_eq!(
        items,
        vec![
            ("Self Test", "OK"),
            ("Int Pwr", "OK"),
            ("Oven Pwr", "OK"),
            ("OCXO", "OK"),
            ("EFC", "OK"),
            ("GPS Rcv", "OK"),
        ]
    );
}

#[test]
fn every_fixture_yields_the_core_fields() {
    // A regression net: whatever else changes, no screen should parse
    // to nothing.
    for (name, text) in all_fixtures() {
        let s = screen::parse(&text);
        assert!(
            s.synchronization.is_some(),
            "{name}: no synchronization line"
        );
        assert!(s.acquisition.is_some(), "{name}: no acquisition line");
        assert!(s.health.is_some(), "{name}: no health line");
        assert!(s.mode.is_some(), "{name}: no mode marked with >>");
        assert!(s.tfom.is_some(), "{name}: no TFOM");
        assert!(s.ffom.is_some(), "{name}: no FFOM");
        assert!(!s.satellites.is_empty(), "{name}: no satellites");
        // The table and the counts above it must agree, and the
        // tracked flag must agree with the tracked count.
        assert_eq!(
            s.satellites.len(),
            s.tracking.unwrap_or(0) as usize + s.not_tracking.unwrap_or(0) as usize,
            "{name}: table rows disagree with the counts"
        );
        assert_eq!(
            s.satellites.iter().filter(|x| x.tracked).count(),
            s.tracking.unwrap_or(0) as usize,
            "{name}: tracked flags disagree with the tracking count"
        );
    }
}

#[test]
fn the_panel_fields_read_from_the_live_screen() {
    let s = screen::parse(&fixture("58503a-live-01.txt"));
    assert_eq!(s.time_interval.as_deref(), Some("-4.8 ns"));
    assert_eq!(s.hold_threshold.as_deref(), Some("1.000 us"));
    assert_eq!(
        s.holdover_predict.as_deref(),
        Some("432.0 us/initial 24 hrs")
    );
    assert_eq!(s.time_scale.as_deref(), Some("UTC"));
    assert_eq!(s.time.as_deref(), Some("20:04:20"));
    assert_eq!(s.date.as_deref(), Some("04 Feb 2007"));
    assert!(!s.time_suspect);
    assert_eq!(s.sync_status.as_deref(), Some("Synchronized to UTC"));
    // In hold, the position carries no AVG or INIT prefix.
    assert_eq!(s.position_label.as_deref(), Some("HOLD"));
    assert_eq!(s.survey_percent, None);
}

#[test]
fn a_placeholder_reads_as_absent_rather_than_as_text() {
    // 58503a-03 prints "1PPS TI --" and "Predict 432.0 us/initial..."
    // while in holdover.  A "--" must not become the string "--".
    let s = screen::parse(&fixture("58503a-03.txt"));
    assert_eq!(s.time_interval, None);
    assert_eq!(s.hold_threshold.as_deref(), Some("1.000 us"));
    assert_eq!(s.sync_status.as_deref(), Some("Inaccurate: not tracking"));
}

#[test]
fn a_suspect_time_is_flagged_and_not_folded_into_the_clock() {
    // 58503b-01 prints "UTC 12:00:00[?] 01 Jan 1996" at power-up.
    let s = screen::parse(&fixture("58503b-01.txt"));
    assert!(s.time_suspect, "the [?] marker was missed");
    assert_eq!(s.time.as_deref(), Some("12:00:00"));
    assert_eq!(s.date.as_deref(), Some("01 Jan 1996"));
    // Seeded position, not yet surveyed.
    assert_eq!(s.position_label.as_deref(), Some("INIT"));
    assert_eq!(s.survey_percent, Some(0.0));
    assert_eq!(s.survey_suspended.as_deref(), Some("track <4 sats"));
}

#[test]
fn a_survey_in_progress_reports_its_percentage() {
    // 58503a-01 prints "MODE  Survey: 1.2%    complete" with AVG
    // position labels.
    let s = screen::parse(&fixture("58503a-01.txt"));
    assert_eq!(s.survey_percent, Some(1.2));
    assert_eq!(s.position_label.as_deref(), Some("AVG"));
}

#[test]
fn a_blank_cell_does_not_invent_a_satellite() {
    // A missing signal reading used to let the next satellite's marker
    // be eaten as this one's value, so that satellite vanished and one
    // was assembled from the leftovers at an elevation of 204 degrees.
    let screen = screen::parse(&two_column(
        "PRN El Az C/N  PRN El Az",
        "Tracking: 1       Not Tracking: 1",
        &["  3  88 281        * 1  24 204"],
    ));
    let seen: Vec<(u8, bool)> = screen
        .satellites
        .iter()
        .map(|s| (s.prn.get(), s.tracked))
        .collect();
    assert_eq!(seen, vec![(3, true), (1, false)]);
    let acquiring = screen.satellites.iter().find(|s| s.prn.get() == 1);
    assert_eq!(
        acquiring.expect("PRN 1").elevation.map(|d| d.get()),
        Some(24)
    );
    assert!(!screen.satellites_suspect);
}

#[test]
fn a_short_tracked_column_does_not_promote_untracked_satellites() {
    // Rows past the end of the tracked column have nothing in the left
    // group; the first group used to claim their tokens anyway.
    let screen = screen::parse(&two_column(
        "PRN El Az C/N  PRN El Az",
        "Tracking: 1       Not Tracking: 3",
        &[
            "  2  70 301   40    16 13 258",
            "                    19 40 102",
            "                    22 71  60",
        ],
    ));
    let tracked: Vec<u8> = screen
        .satellites
        .iter()
        .filter(|s| s.tracked)
        .map(|s| s.prn.get())
        .collect();
    assert_eq!(tracked, vec![2]);
    assert_eq!(screen.satellites.len(), 4);
    assert!(!screen.satellites_suspect);
}

#[test]
fn the_tracked_count_is_not_the_untracked_one() {
    // Every plain search for "Tracking:" finds it inside "Not
    // Tracking:" first.  Fixtures all print the tracked count first, so
    // this only shows up with the order reversed or on two lines.
    for counts in [
        "Not Tracking: 3   Tracking: 6",
        "Tracking: 6       Not Tracking: 3",
    ] {
        let screen = screen::parse(&two_column(
            "PRN  El  Az   SS",
            counts,
            &["  3  88 281  111"],
        ));
        assert_eq!(screen.tracking, Some(6), "with counts as {counts:?}");
        assert_eq!(screen.not_tracking, Some(3), "with counts as {counts:?}");
    }
}

#[test]
fn a_table_that_disagrees_with_its_counts_is_marked_suspect() {
    // The screen states how many satellites it is tracking, so the
    // table can be checked against itself.  Reporting a partial sky is
    // fine; presenting it as certain is not.
    let screen = screen::parse(&two_column(
        "PRN  El  Az   SS",
        "Tracking: 6       Not Tracking: 3",
        &["  3  88 281  111"],
    ));
    assert_eq!(screen.satellites.len(), 1);
    assert!(screen.satellites_suspect);
}

#[test]
fn every_recorded_screen_agrees_with_its_own_counts() {
    for (name, text) in all_fixtures() {
        let screen = screen::parse(&text);
        assert!(
            !screen.satellites_suspect,
            "{name}: the table disagrees with the counts printed above it"
        );
    }
}

/// A screen with the given table header, counts line and rows.
///
/// The right-hand panel starts at column 46, as it does on the
/// receiver; anything closer would be clipped by the scraper.
fn two_column(header: &str, counts: &str, rows: &[&str]) -> String {
    let pad = |left: &str, right: &str| format!("{left:<46}{right}\n");
    let mut out = String::from("---- Receiver Status ----\n");
    out.push_str("SYNCHRONIZATION ..................... [ Outputs Valid ]\n");
    out.push_str(&pad("SmartClock Mode", "Reference Outputs"));
    out.push_str(&pad(">> Locked to GPS", "TFOM     3            FFOM     1"));
    out.push_str("ACQUISITION ......................... [ GPS 1PPS Valid ]\n");
    out.push_str(&pad(counts, "Time"));
    out.push_str(&pad(header, "UTC      20:04:20     04 Feb 2007"));
    for row in rows {
        out.push_str(&pad(row, ""));
    }
    out.push_str("HEALTH MONITOR ...................... [ OK ]\n");
    out.push_str("Self Test: OK   GPS Rcv: OK\n");
    out
}

#[test]
fn a_bracket_earlier_on_the_line_does_not_become_the_summary() {
    // These summaries sit at the right-hand end after a run of dots, so
    // anything else bracketed on the row comes earlier.  The line spans
    // the full width, so clipping it to the left panel is not an option.
    let mut screen = two_column(
        "PRN  El  Az   SS",
        "Tracking: 1       Not Tracking: 0",
        &["  3  88 281  111"],
    );
    screen = screen.replace(
        "SYNCHRONIZATION ..................... [ Outputs Valid ]",
        "SYNCHRONIZATION [?] ................. [ Outputs Valid ]",
    );
    let parsed = screen::parse(&screen);
    assert_eq!(parsed.synchronization.as_deref(), Some("Outputs Valid"));
}

#[test]
fn a_clock_on_the_health_row_does_not_become_a_health_check() {
    // Splitting the row on ':' produced pairs like ("12", "34") beside
    // the real ones, on the line that says whether the unit is well.
    let mut screen = two_column(
        "PRN  El  Az   SS",
        "Tracking: 1       Not Tracking: 0",
        &["  3  88 281  111"],
    );
    screen = screen.replace(
        "Self Test: OK   GPS Rcv: OK",
        "Self Test: OK   GPS Rcv: OK          12:34:56",
    );
    let parsed = screen::parse(&screen);
    let labels: Vec<&str> = parsed
        .health_items
        .iter()
        .map(|(label, _)| label.as_str())
        .collect();
    assert_eq!(labels, vec!["Self Test", "GPS Rcv"]);
    assert!(
        parsed.health_items.iter().all(|(_, v)| v == "OK"),
        "a clock was read as a health verdict: {:?}",
        parsed.health_items
    );
}
