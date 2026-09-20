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

use crate::types::Prn;
use crate::types::SatelliteInfo;

/// What the scraper could recover from a screen.
///
/// Every field is optional: screens differ between firmware revisions
/// and between receiver states, and a missing field should degrade the
/// display rather than fail the parse.
#[derive(Debug, Clone, Default, PartialEq)]
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
    /// The bracketed text on the HEALTH MONITOR line.
    pub health: Option<String>,
    /// Each `Label: value` pair from the health line, in order.
    pub health_items: Vec<(String, String)>,
    /// Elevation mask in degrees.
    pub elevation_mask: Option<i16>,
    /// Antenna delay in nanoseconds.
    pub antenna_delay_ns: Option<i64>,
    /// The position MODE field, such as `Hold` or `Survey: 71.1%`.
    pub position_mode: Option<String>,
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

    let mut out = Screen {
        synchronization: bracketed(&lines, "SYNCHRONIZATION"),
        acquisition: bracketed(&lines, "ACQUISITION"),
        health: bracketed(&lines, "HEALTH MONITOR"),
        ..Screen::default()
    };

    let boundary = panel_column(&lines);
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
        out.tracking = out
            .tracking
            .or_else(|| after_label(line, "Tracking:").and_then(|v| v.parse().ok()));
        out.not_tracking = out
            .not_tracking
            .or_else(|| after_label(line, "Not Tracking:").and_then(|v| v.parse().ok()));
    }

    // "Tracking:" also matches inside "Not Tracking:", so a line
    // carrying both would set the wrong field.  Re-read it explicitly.
    if let Some(line) = lines.iter().find(|l| l.contains("Not Tracking:")) {
        out.tracking = line
            .split_once("Tracking:")
            .and_then(|(_, r)| r.split_whitespace().next())
            .and_then(|v| v.parse().ok());
    }

    out.satellites = satellites(&lines);
    out.health_items = health_items(&lines);
    out
}

/// The text inside `[ ... ]` on the line carrying `label`.
fn bracketed(lines: &[&str], label: &str) -> Option<String> {
    let line = lines.iter().find(|l| l.contains(label))?;
    let start = line.find('[')?;
    let end = line[start..].find(']')? + start;
    Some(collapse(&line[start + 1..end]))
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
    lines
        .iter()
        .flat_map(|line| PANEL_HEADINGS.iter().filter_map(move |h| line.find(h)))
        .min()
        .unwrap_or(usize::MAX)
}

/// One column group of the satellite table, as the header declares it.
struct Group {
    /// How many value fields follow the PRN, such as El, Az and SS.
    fields: usize,
    /// Whether this group lists satellites the receiver is using.
    tracked: bool,
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
    for token in header.split_whitespace() {
        if token.eq_ignore_ascii_case("PRN") {
            found.push(Group {
                fields: 0,
                tracked: false,
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
        let mut tokens = tokenize(left).into_iter().peekable();
        for group in &groups {
            if tokens.peek().is_none() {
                break;
            }
            found.push(satellite(&mut tokens, group));
        }
        found.retain(|s: &Option<SatelliteInfo>| s.is_some());
    }
    found.into_iter().flatten().collect()
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
    let mut values = [None; 3];
    for slot in values.iter_mut().take(group.fields.min(3)) {
        let Some(token) = tokens.next() else { break };
        *slot = token.parse().ok();
    }
    Some(SatelliteInfo {
        prn,
        elevation: values[0],
        azimuth: values[1],
        signal: values[2],
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
