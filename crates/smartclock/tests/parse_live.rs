//! Parsers checked against replies recorded from a 58503A running
//! firmware 3704-C, during phase 1 probing.

use jiff::civil::date;
use smartclock::parse;
use smartclock::types::Datum;
use smartclock::types::LeapPending;

#[test]
fn integers_keep_their_leading_plus() {
    assert_eq!(parse::int("+10").expect("int"), 10);
    assert_eq!(parse::int("+222").expect("int"), 222);
    assert_eq!(parse::int("+19200").expect("int"), 19200);
    assert_eq!(parse::int("+36302").expect("int"), 36302);
    assert_eq!(parse::int("-113").expect("int"), -113);
    assert_eq!(parse::int("+0").expect("int"), 0);
    assert!(parse::int("LOCK").is_err());
}

#[test]
fn the_learned_tempco_is_a_plain_real() {
    // Read from the receiver in September 2026 and identical to the
    // reading the phase 7 sweep took, which is the first evidence that
    // this may be a stored calibration rather than a value the receiver
    // keeps revising.
    assert!((parse::real("-3.36500E+001").expect("real") + 33.65).abs() < 1e-9);
}

#[test]
fn reals_arrive_in_scientific_notation() {
    assert!((parse::real("+3.60971E+001").expect("real") - 36.0971).abs() < 1e-9);
    assert!((parse::real("-4.1E-009").expect("real") + 4.1e-9).abs() < 1e-18);
    assert_eq!(parse::real("+0.00000E+000").expect("real"), 0.0);
}

#[test]
fn the_efc_reading_reports_how_much_range_is_used() {
    let efc = parse::efc("+3.60971E+001").expect("efc");
    assert!((efc.percent() - 36.0971).abs() < 1e-9);
    // A third of the way to the rail, which is the number that matters
    // for an ageing oscillator.
    assert!((efc.range_used() - 0.360971).abs() < 1e-6);
    // Outside the documented range is a parse failure, not a clamp.
    assert!(parse::efc("+1.5E+002").is_err());
}

#[test]
fn a_lone_zero_means_an_empty_list() {
    // How the receiver says "no satellites ignored".
    assert_eq!(parse::int_list("+0").expect("list"), Vec::<i64>::new());
    assert_eq!(
        parse::int_list("+3,+4,+16,+26,+28,+31").expect("list"),
        vec![3, 4, 16, 26, 28, 31]
    );
}

#[test]
fn tracked_satellites_parse_as_prns() {
    let prns = parse::prn_list("+3,+4,+16,+26,+28,+31").expect("prns");
    assert_eq!(prns.len(), 6);
    assert_eq!(prns[0].get(), 3);
    assert!(parse::prn_list("+0").expect("none").is_empty());
    // 33 is outside the GPS constellation.
    assert!(parse::prn_list("+33").is_err());
}

#[test]
fn the_full_include_list_is_all_thirty_two() {
    let reply = (1..=32)
        .map(|n| format!("+{n}"))
        .collect::<Vec<_>>()
        .join(",");
    assert_eq!(parse::prn_list(&reply).expect("prns").len(), 32);
}

#[test]
fn holdover_duration_carries_a_flag() {
    let idle = parse::holdover_duration("+0.00000E+000,0").expect("pair");
    assert_eq!(idle.elapsed.as_secs(), 0.0);
    assert!(!idle.active);
    let predicted = parse::holdover_duration("+4.320E-004,0").expect("pair");
    assert!((predicted.elapsed.as_micros() - 432.0).abs() < 1e-6);
}

#[test]
fn a_time_quantity_prints_in_a_readable_scale() {
    // The same field is natural in three scales depending on what it
    // is, which is why callers should not hand-multiply.
    assert_eq!(
        parse::seconds("-4.1E-009").expect("s").to_string(),
        "-4.1 ns"
    );
    assert_eq!(
        parse::seconds("+4.320E-004").expect("s").to_string(),
        "432.0 us"
    );
    assert_eq!(
        parse::seconds("+8.64E+004").expect("s").to_string(),
        "86400.0 s"
    );
}

#[test]
fn the_error_queue_entry_splits_into_code_and_text() {
    let none = parse::error_entry("+0,\"No error\"").expect("entry");
    assert!(none.is_empty());
    assert_eq!(none.message, "No error");

    // -221 means the header parsed and the receiver declined on state,
    // which is an answer rather than a bad command.
    let conflict = parse::error_entry("-221,\"Settings conflict\"").expect("entry");
    assert_eq!(conflict.code, -221);
    assert!(conflict.is_state_refusal());
    assert!(!conflict.is_empty());
}

#[test]
fn the_identity_string_splits_into_four_fields() {
    let id = parse::identity("HEWLETT-PACKARD,58503A,0000A00000,3704-C").expect("identity");
    assert_eq!(id.manufacturer, "HEWLETT-PACKARD");
    assert_eq!(id.model, "58503A");
    assert_eq!(id.serial, "0000A00000");
    assert_eq!(id.firmware, "3704-C");
}

#[test]
fn the_reported_date_and_time_parse() {
    assert_eq!(parse::ymd("+2007,+2,+4").expect("date"), date(2007, 2, 4));
    assert_eq!(
        parse::hms("+16,+9,+55").expect("time").to_string(),
        "16:09:55"
    );
    assert_eq!(
        parse::hms("+20,+4,+31").expect("time").to_string(),
        "20:04:31"
    );
    assert!(parse::tzone("+0,+0").expect("offset").is_utc());
}

#[test]
fn a_leap_second_is_an_allowed_sixtieth_second() {
    let leap = parse::hms("+23,+59,+60").expect("time");
    assert!(leap.is_leap_second());
    assert_eq!(leap.to_string(), "23:59:60");
    assert!(parse::hms("+24,+0,+0").is_err());
}

#[test]
fn the_position_reply_becomes_signed_degrees() {
    let reply = "N,+37,+22,+3.02770E+001,W,+122,+5,+3.48160E+001,+4.35100E+001";
    let p = parse::position(reply, Datum::MeanSeaLevel).expect("position");
    // 37 deg 22 min 30.277 s north.
    assert!(
        (p.latitude - 37.3750769).abs() < 1e-6,
        "lat was {}",
        p.latitude
    );
    // West is negative.
    assert!(
        (p.longitude + 122.0930044).abs() < 1e-6,
        "lon was {}",
        p.longitude
    );
    assert!((p.height - 43.51).abs() < 1e-9);
    assert_eq!(p.datum, Datum::MeanSeaLevel);
}

#[test]
fn the_live_timecode_parses_and_its_checksum_verifies() {
    // Recorded from the unit; cross-checks against :SYNC:TFOMerit? = +3
    // and :SYNC:FFOMerit? = +1 taken moments earlier.
    let code = parse::timecode("T2200702042004323100034").expect("timecode");
    assert_eq!(code.date, Some(date(2007, 2, 4)));
    assert_eq!(code.time.expect("time").to_string(), "20:04:32");
    assert_eq!(code.tfom.get(), 3);
    assert_eq!(code.ffom.get(), 1);
    assert_eq!(code.leap, LeapPending::None);
    assert!(!code.service_requested);
    assert!(code.valid);
}

#[test]
fn a_corrupted_timecode_is_rejected_rather_than_believed() {
    // One digit changed, so the checksum no longer holds.  This message
    // is emitted against a deadline just before the on-time edge, so a
    // corrupted one is exactly the kind that must not be trusted.
    assert!(parse::timecode("T2200702042004323100035").is_err());
    assert!(parse::timecode("T2200702042004423100034").is_err());
}

#[test]
fn the_t1_format_carries_gps_seconds_instead_of_a_date() {
    // Built to the documented shape: T1 #H<8 hex> t f l r v cc.
    let body = "T1#H0F4B3A2031000";
    let cc = body.bytes().fold(0u8, |a, b| a.wrapping_add(b));
    let message = format!("{body}{cc:02X}");
    let code = parse::timecode(&message).expect("timecode");
    assert_eq!(code.gps_seconds, Some(0x0F4B_3A20));
    assert_eq!(code.date, None);
}

#[test]
fn words_and_quoted_strings_are_distinguished() {
    assert_eq!(parse::word("LOCK").expect("word"), "LOCK");
    assert_eq!(parse::word("NONE").expect("word"), "NONE");
    assert_eq!(parse::string("\"PRIMARY\"").expect("string"), "PRIMARY");
    assert_eq!(parse::string("\"20:04:31\"").expect("string"), "20:04:31");
}

#[test]
fn booleans_reject_anything_else() {
    assert!(!parse::bool01("0").expect("bool"));
    assert!(parse::bool01("1").expect("bool"));
    assert!(parse::bool01("2").is_err());
    assert!(parse::bool01("").is_err());
}
