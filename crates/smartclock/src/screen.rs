//! Scraper for the `:SYSTem:STATus?` screen.
//!
//! This is not a convenience.  Per-satellite elevation, azimuth and
//! signal strength appear nowhere else: `:GPS:SATellite:TRACking?`
//! returns bare PRNs.  The health monitor line and the survey detail
//! are likewise screen-only.
//!
//! The layout is not stable between firmware revisions, so the scraper
//! works from labels rather than fixed columns.  Firmware 3704-C heads
//! the signal column `SS` and pads headings with underscores; the
//! manuals head it `C/N` and pad with spaces, over a narrower screen.

use serde::Deserialize;
use serde::Serialize;

use crate::types::Degrees;
use crate::types::Prn;
use crate::types::SatelliteInfo;
use crate::types::SignalStrength;

/// What the scraper could recover from a screen.
///
/// Every field is optional: screens differ between firmware revisions
/// and between receiver states, and a missing field should degrade the
/// display rather than fail the parse.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Screen {
    /// The bracketed text on the SYNCHRONIZATION line.
    pub synchronization: Option<String>,
    /// The mode marked with `>>`.
    pub mode: Option<String>,
    /// Time figure of merit.
    pub tfom: Option<u8>,
    /// Frequency figure of merit.
    pub ffom: Option<u8>,
    /// The bracketed text on the ACQUISITION line.
    pub acquisition: Option<String>,
    /// Satellites being tracked.
    pub tracking: Option<u32>,
    /// Satellites seen but not tracked.
    pub not_tracking: Option<u32>,
    /// Every satellite in the table, tracked and not.
    pub satellites: Vec<SatelliteInfo>,
    /// Set when the parsed table disagrees with the counts printed
    /// above it.
    ///
    /// The screen states how many satellites are tracked and how many
    /// are not, so the table can be checked against itself.  When they
    /// disagree the rows are still reported -- a partial sky beats
    /// none -- but nothing should present them as certain.  Every way
    /// the scraper has been wrong so far, inventing a satellite from a
    /// blank cell or reading an untracked one as tracked, broke this
    /// invariant and would have been caught by it.
    pub satellites_suspect: bool,
    /// The bracketed text on the HEALTH MONITOR line.
    pub health: Option<String>,
    /// Each `Label: value` pair from the health line, in order.
    pub health_items: Vec<(String, String)>,
    /// Elevation mask in degrees.
    pub elevation_mask: Option<i16>,
    /// Antenna delay in nanoseconds.
    pub antenna_delay_ns: Option<i64>,
    /// The position MODE field, such as `Hold`, `Navigation` or
    /// `Survey: 71.1% complete`.
    pub position_mode: Option<String>,
    /// Survey completion, when surveying.
    pub survey_percent: Option<f64>,
    /// Why a survey is suspended, such as `track <4 sats`.
    pub survey_suspended: Option<String>,
    /// `1PPS TI` as printed, or absent when the screen shows `--`.
    pub time_interval: Option<String>,
    /// `HOLD THR` as printed, or absent when the screen shows `Off`.
    pub hold_threshold: Option<String>,
    /// The predicted 24 hour holdover uncertainty as printed.
    pub holdover_predict: Option<String>,
    /// Time of day as printed.
    pub time: Option<String>,
    /// Date as printed.
    pub date: Option<String>,
    /// Whether the screen marked the time suspect with `[?]`.
    pub time_suspect: bool,
    /// Which scale the displayed time is on: `UTC`, `GPS` or `LOCL`.
    pub time_scale: Option<String>,
    /// The 1 PPS synchronization line, such as `Synchronized to UTC` or
    /// `Invalid: not tracking`.
    pub sync_status: Option<String>,
    /// Whether position is labelled `AVG`, `INIT` or plain, which says
    /// whether the receiver is surveying, seeded, or in hold.
    pub position_label: Option<String>,
}

/// Parse a status screen.
pub fn parse(screen: &str) -> Screen {
    // Underscores are decorative fill on firmware 3704-C.  Flattening
    // them first lets one set of label searches serve both layouts.
    let flattened: Vec<String> = screen
        .lines()
        .map(|l| l.replace('_', " ").trim_end().to_owned())
        .collect();
    let lines: Vec<&str> = flattened.iter().map(String::as_str).collect();

    let boundary = panel_column(&lines);
    let mut out = Screen {
        synchronization: bracketed(&lines, "SYNCHRONIZATION"),
        acquisition: bracketed(&lines, "ACQUISITION"),
        health: bracketed(&lines, "HEALTH MONITOR"),
        ..Screen::default()
    };
    for line in &lines {
        if let Some(rest) = clip(line, boundary).trim_start().strip_prefix(">>") {
            out.mode = Some(collapse(rest));
        }
        out.tfom = out
            .tfom
            .or_else(|| after_label(line, "TFOM").and_then(|v| v.parse().ok()));
        out.ffom = out
            .ffom
            .or_else(|| after_label(line, "FFOM").and_then(|v| v.parse().ok()));
        out.elevation_mask = out
            .elevation_mask
            .or_else(|| after_label(line, "ELEV MASK").and_then(|v| v.parse().ok()));
        out.antenna_delay_ns = out
            .antenna_delay_ns
            .or_else(|| after_label(line, "ANT DLY").and_then(|v| v.parse().ok()));
        out.position_mode = out.position_mode.clone().or_else(|| {
            after_label(line, "MODE").map(|_| {
                let rest = line.split_once("MODE").map(|(_, r)| r).unwrap_or_default();
                collapse(rest)
            })
        });
        out.not_tracking = out
            .not_tracking
            .or_else(|| after_label(line, "Not Tracking:").and_then(|v| v.parse().ok()));
        out.tracking = out.tracking.or_else(|| tracked_count(line));
    }

    if let Some(mode) = &out.position_mode {
        out.survey_percent = mode
            .split_once("Survey:")
            .and_then(|(_, r)| r.split('%').next())
            .map(str::trim)
            .and_then(|v| v.trim_start_matches(['<', '>']).parse().ok());
    }
    out.survey_suspended = lines
        .iter()
        .find_map(|l| l.split_once("Suspended:"))
        .map(|(_, r)| collapse(r));

    out.satellites = satellites(&lines);
    out.satellites_suspect = !agrees_with_counts(&out);
    out.health_items = health_items(&lines);
    panel_fields(&lines, boundary, &mut out);
    out
}

/// Values printed in the right-hand panel.
///
/// The firmware's format strings, recovered in
/// `docs/screen-format-strings.md`, enumerate what each of these can
/// hold.  Several print a placeholder instead of a value: `1PPS TI --`,
/// `HOLD THR  Off`, `Predict --`, `--:--:--` and `-- --- ----`.  Those
/// become absent rather than being carried around as strings that look
/// like data.
fn panel_fields(lines: &[&str], boundary: usize, out: &mut Screen) {
    /// Placeholders the firmware prints when a value is unavailable.
    const ABSENT: [&str; 5] = ["--", "---", "Off", "--:--:--", "-- --- ----"];
    let absent = |v: &str| v.is_empty() || ABSENT.contains(&v);

    for line in lines {
        let panel = panel_of(line, boundary);
        if panel.is_empty() {
            continue;
        }
        if let Some(rest) = panel.trim_start().strip_prefix("1PPS TI") {
            // The suffix varies with width: "relative to GPS", "rel to
            // GPS" or "rel GPS".  Only the value before it is wanted.
            let value = collapse(rest.split(" rel").next().unwrap_or(rest));
            out.time_interval = (!absent(&value)).then_some(value);
        }
        if let Some(rest) = panel.trim_start().strip_prefix("HOLD THR") {
            let value = collapse(rest);
            out.hold_threshold = (!absent(&value)).then_some(value);
        }
        if let Some(rest) = panel.trim_start().strip_prefix("Predict") {
            let value = collapse(rest);
            out.holdover_predict = (!absent(&value)).then_some(value);
        }
        parse_time_line(panel, &mut *out, absent);
        parse_position_line(panel, out);
        if panel.contains("Synchronized to") || panel.starts_with("GPS 1PPS") {
            let text = collapse(panel.trim_start().trim_start_matches("GPS 1PPS"));
            if !text.is_empty() {
                out.sync_status = Some(text);
            }
        }
    }
}

/// The `UTC hh:mm:ss dd Mon yyyy` line, whose scale label varies.
fn parse_time_line(panel: &str, out: &mut Screen, absent: impl Fn(&str) -> bool) {
    let trimmed = panel.trim_start();
    let Some(scale) = ["UTC", "GPS", "LOCL", "LOCAL"]
        .into_iter()
        .find(|s| trimmed.starts_with(s))
    else {
        return;
    };
    // "GPS 1PPS ..." also starts with GPS but is the status line.
    let rest = trimmed[scale.len()..].trim_start();
    if rest.starts_with("1PPS") {
        return;
    }
    let mut fields = rest.split_whitespace();
    let Some(clock) = fields.next() else { return };
    // A questionable time is marked "[?]", sometimes joined to the
    // clock and sometimes standing alone.
    let suspect = rest.contains("[?]");
    let clock = clock.trim_end_matches("[?]");
    out.time_scale = Some(scale.to_owned());
    out.time_suspect = suspect;
    out.time = (!absent(clock)).then(|| clock.to_owned());
    let date = collapse(&rest[rest.find(clock).map_or(0, |i| i + clock.len())..])
        .replace("[?]", "")
        .trim()
        .to_owned();
    out.date = (!absent(&date)).then_some(date);
}

/// The `LAT` line, whose label says what kind of position it is.
fn parse_position_line(panel: &str, out: &mut Screen) {
    let trimmed = panel.trim_start();
    // Anchored on a following space so a line beginning "LATER" or
    // "LAT" as part of a longer word is not read as a position.
    for label in ["AVG LAT ", "INIT LAT ", "LAT "] {
        if trimmed.starts_with(label) {
            out.position_label = Some(
                label
                    .trim_end()
                    .strip_suffix("LAT")
                    .map(str::trim)
                    .filter(|l| !l.is_empty())
                    .unwrap_or("HOLD")
                    .to_owned(),
            );
            return;
        }
    }
}

/// The part of a line right of the satellite table.
fn panel_of(line: &str, boundary: usize) -> &str {
    match line.char_indices().nth(boundary) {
        Some((at, _)) => &line[at..],
        None => "",
    }
}

/// Whether the parsed table matches the counts printed above it.
///
/// True when a count is missing: an unstated count cannot disagree.
fn agrees_with_counts(screen: &Screen) -> bool {
    let tracked = screen.satellites.iter().filter(|s| s.tracked).count();
    let untracked = screen.satellites.len() - tracked;
    let matches = |stated: Option<u32>, parsed: usize| stated.is_none_or(|n| n as usize == parsed);
    matches(screen.tracking, tracked) && matches(screen.not_tracking, untracked)
}

/// The text inside `[ ... ]` on the line carrying `label`.
///
/// The *last* bracket on the line, not the first.  These summaries sit
/// at the right-hand end after a run of dots, so anything else
/// bracketed on the same row -- a `[?]` beside a clock, a `[TI ...]` --
/// comes earlier and would otherwise win.  The line cannot simply be
/// clipped to the left panel: it spans the full width, which is what
/// distinguishes a section heading from a field.
fn bracketed(lines: &[&str], label: &str) -> Option<String> {
    let line = lines.iter().find(|l| l.contains(label))?;
    let start = line.rfind('[')?;
    let end = line[start..].find(']')? + start;
    Some(collapse(&line[start + 1..end]))
}

/// The count after `Tracking:`, which is not the one after
/// `Not Tracking:`.
///
/// Every plain search for `Tracking:` finds the substring inside
/// `Not Tracking:` first.  An earlier attempt at this re-read the line
/// with `split_once("Tracking:")`, which has the same flaw; it passed
/// only because every fixture happens to print the tracked count first.
/// With the counts in the other order, or on separate lines, the
/// tracked count silently became the untracked one.
fn tracked_count(line: &str) -> Option<u32> {
    let mut from = 0;
    while let Some(at) = line[from..].find("Tracking:") {
        let at = from + at;
        let preceded_by_not = line[..at].trim_end().ends_with("Not");
        if !preceded_by_not {
            return line[at + "Tracking:".len()..]
                .split_whitespace()
                .next()
                .and_then(|v| v.parse().ok());
        }
        from = at + "Tracking:".len();
    }
    None
}

/// The whitespace-delimited token following `label` on this line.
fn after_label<'a>(line: &'a str, label: &str) -> Option<&'a str> {
    let (_, rest) = line.split_once(label)?;
    rest.split_whitespace().next()
}

/// Squeeze runs of whitespace and trim.
fn collapse(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Where the right-hand panel begins.
///
/// The satellite table and the time and position panel share every
/// line, so the table has to be cut out before it can be read.  The
/// panel has a straight left edge, so its column is the leftmost at
/// which any of its headings appears.
///
/// Anchoring on a single heading is not enough: `Time` sits on the
/// tracking-count line on firmware 3704-C but on the table header line
/// in the manuals, and missing the boundary lets panel text leak into
/// both the mode line and the table header, where `Time` was read as a
/// fourth satellite column.
fn panel_column(lines: &[&str]) -> usize {
    /// Headings that begin at the panel's left edge.
    ///
    /// Each must be text that appears only there.  "GPS 1PPS" looks
    /// like a panel heading but also occurs in the mode line, as in
    /// ">> Holdover: GPS 1PPS invalid", where anchoring on it would cut
    /// the mode text in half.
    const PANEL_HEADINGS: [&str; 9] = [
        "Reference Outputs",
        "Holdover Uncertainty",
        "HOLD THR",
        "TFOM",
        "Time",
        "UTC",
        "ANT DLY",
        "Position",
        "MODE",
    ];
    // A character index, not a byte offset.  `find` returns bytes while
    // `clip` counts characters, and serial noise decoded lossily brings
    // three-byte replacement characters, so the two disagreed exactly
    // when the line was already damaged.
    lines
        .iter()
        .flat_map(|line| {
            PANEL_HEADINGS
                .iter()
                .filter_map(move |h| line.find(h).map(|at| line[..at].chars().count()))
        })
        .min()
        .unwrap_or(usize::MAX)
}

/// One column group of the satellite table, as the header declares it.
struct Group {
    /// How many value fields follow the PRN, such as El, Az and SS.
    fields: usize,
    /// Whether this group lists satellites the receiver is using.
    tracked: bool,
    /// Where the group's `PRN` heading starts.
    ///
    /// Not used to slice values -- the manuals' own ASCII does not line
    /// data up with its header -- only to tell an empty cell from a
    /// missing one, which token order cannot express.
    column: usize,
}

/// Read the header row into column groups.
///
/// Position cannot be used for this.  The manuals' own ASCII does not
/// line its data up with its header: on one screen the `C/N` heading
/// sits at column 10 while its values sit at column 14.  Only the token
/// sequence is reliable, so each group is read as a field count and the
/// data rows are consumed token by token.
///
/// A group is the tracked one when it carries a signal column, `SS` on
/// firmware 3704-C or `C/N` in the manuals.  That is what distinguishes
/// the two groups when both are present, and what identifies a lone
/// group when only one is.
fn groups(header: &str) -> Vec<Group> {
    let mut found: Vec<Group> = Vec::new();
    for (column, token) in word_offsets(header) {
        if token.eq_ignore_ascii_case("PRN") {
            found.push(Group {
                fields: 0,
                tracked: false,
                column,
            });
        } else if let Some(group) = found.last_mut() {
            group.fields += 1;
            if !token.eq_ignore_ascii_case("El") && !token.eq_ignore_ascii_case("Az") {
                group.tracked = true;
            }
        }
    }
    found
}

/// Read the satellite table.
fn satellites(lines: &[&str]) -> Vec<SatelliteInfo> {
    let boundary = panel_column(lines);
    let Some(header_at) = lines.iter().position(|l| {
        let left = clip(l, boundary);
        left.contains("PRN") && left.contains("El")
    }) else {
        return Vec::new();
    };
    let groups = groups(clip(lines[header_at], boundary));
    if groups.is_empty() {
        return Vec::new();
    }

    let mut found = Vec::new();
    for line in lines.iter().skip(header_at + 1) {
        let left = clip(line, boundary);
        if left.contains("ELEV MASK") || left.contains("HEALTH") || left.contains("ACQUISITION") {
            break;
        }
        if left.trim().is_empty() {
            // The two groups can be different lengths, so a gap on one
            // side is normal; keep reading until a section heading.
            continue;
        }
        // Only the first word's offset is wanted, so the row is not
        // collected into a Vec to read one number from it.
        let first_column = word_offsets(left).next().map_or(usize::MAX, |(at, _)| at);
        let mut tokens = tokenize(left).into_iter().peekable();
        for (n, group) in groups.iter().enumerate() {
            if tokens.peek().is_none() {
                break;
            }
            // A row whose first token starts at or beyond the next
            // group's heading has nothing in this one.  Token order
            // alone cannot see that, and sliding the next group's
            // values into the gap is what invented satellites: a blank
            // signal column produced a PRN 24 at an elevation of 204
            // degrees, and a short tracked column reported untracked
            // satellites as tracked.
            let next_column = groups.get(n + 1).map_or(usize::MAX, |g| g.column);
            if first_column + COLUMN_SLACK >= next_column {
                continue;
            }
            found.push(satellite(&mut tokens, group));
        }
    }
    found.into_iter().flatten().collect()
}

/// How far a row's data may sit from its heading and still belong to
/// it.  The manuals' own ASCII is off by as much as four columns.
const COLUMN_SLACK: usize = 4;

/// Each whitespace-delimited word with the byte offset it starts at.
///
/// Byte offsets, not character columns.  Everything compared against
/// these -- `Group::column`, `first_column`, `COLUMN_SLACK` -- is a
/// byte offset too, so they agree; but `panel_column` counts characters,
/// and the two must not be mixed.  On a line that is pure ASCII, which
/// every status screen the receiver emits is, they are the same number.
fn word_offsets(line: &str) -> impl Iterator<Item = (usize, &str)> {
    line.char_indices().filter_map(move |(at, c)| {
        let starts_word = !c.is_whitespace()
            && (at == 0
                || line[..at]
                    .chars()
                    .next_back()
                    .is_some_and(char::is_whitespace));
        starts_word.then(|| {
            let end = line[at..]
                .find(char::is_whitespace)
                .map_or(line.len(), |len| at + len);
            (at, &line[at..end])
        })
    })
}

/// Split a row into tokens, reattaching a detached acquiring marker.
///
/// Firmware 3704-C writes `* 1`, the manuals write `*1`.  Joining them
/// here keeps one satellite to one token run.
fn tokenize(line: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for token in line.split_whitespace() {
        match out.last_mut() {
            Some(last) if last == "*" => last.push_str(token),
            _ => out.push(token.to_owned()),
        }
    }
    out
}

/// Take one satellite's worth of tokens.
fn satellite(
    tokens: &mut std::iter::Peekable<std::vec::IntoIter<String>>,
    group: &Group,
) -> Option<SatelliteInfo> {
    let first = tokens.next()?;
    let (acquiring, digits) = match first.strip_prefix('*') {
        Some(rest) => (true, rest.to_owned()),
        None => (false, first),
    };
    let prn = digits.parse::<u8>().ok().and_then(Prn::new)?;

    // "--", "---" and "Acq .." all mean the value is unavailable.  Each
    // still occupies a field, so the slot is consumed either way.
    let mut values: [Option<i16>; 3] = [None; 3];
    for slot in values.iter_mut().take(group.fields.min(3)) {
        // An asterisk only ever begins a satellite cell, so it ends
        // this one.  Without that, a blank column let the next
        // satellite's marker be eaten as this one's signal reading and
        // the satellite itself disappeared.
        if tokens.peek().is_some_and(|t| t.starts_with('*')) {
            break;
        }
        let Some(token) = tokens.next() else { break };
        *slot = token.parse().ok();
    }
    Some(SatelliteInfo {
        prn,
        elevation: values[0].map(Degrees::new),
        azimuth: values[1].map(Degrees::new),
        signal: values[2].map(SignalStrength::new),
        tracked: group.tracked,
        acquiring,
    })
}

/// Pairs from the health monitor line, such as `OCXO: OK`.
fn health_items(lines: &[&str]) -> Vec<(String, String)> {
    let Some(line) = lines.iter().find(|l| l.contains("Self Test:")) else {
        return Vec::new();
    };
    let mut items = Vec::new();
    let parts: Vec<&str> = line.split(':').collect();
    for n in 0..parts.len().saturating_sub(1) {
        // The label is the tail of the part before the colon, the value
        // the first token of the part after it.
        let Some(label) = tail_words(parts[n]) else {
            continue;
        };
        let Some(value) = parts[n + 1].split_whitespace().next() else {
            continue;
        };
        // A health verdict is a word.  Splitting the whole line on ':'
        // turned a clock sharing the row into pairs like ("12", "34"),
        // on the line an operator reads to decide the unit is well.
        // The line spans the full width, so it cannot be clipped to the
        // left panel instead.
        if !value.chars().all(|c| c.is_ascii_alphabetic()) {
            continue;
        }
        items.push((label, value.to_owned()));
    }
    items
}

/// The label immediately before a colon: everything after the previous
/// value, which is the last one or two words.
fn tail_words(text: &str) -> Option<String> {
    let words: Vec<&str> = text.split_whitespace().collect();
    let take = match words.as_slice() {
        [.., a, _b]
            if a.eq_ignore_ascii_case("Self")
                || a.eq_ignore_ascii_case("Int")
                || a.eq_ignore_ascii_case("Oven")
                || a.eq_ignore_ascii_case("GPS") =>
        {
            2
        }
        [] => return None,
        _ => 1,
    };
    let start = words.len().checked_sub(take)?;
    Some(words[start..].join(" "))
}

/// The part of a line left of the right-hand panel.
fn clip(line: &str, boundary: usize) -> &str {
    match line.char_indices().nth(boundary) {
        Some((at, _)) => &line[..at],
        None => line,
    }
}
