//! Rendering the command table as documentation.
//!
//! Generated rather than written, so it cannot drift from the table it
//! describes.  A test compares the checked-in file against what this
//! produces, so a command added without regenerating fails CI.

use std::fmt::Write as _;

use crate::command::Class;
use crate::command::CommandId;
use crate::command::Dialect;
use crate::command::Evidence;

/// Every dialect, in the order the columns appear.
const DIALECTS: [(Dialect, &str); 2] = [
    (Dialect::Hp58503, "58503A/B, 59551A"),
    (Dialect::Z3801, "Z3801A, Z3816A"),
];

/// How far an entry is confirmed, as a one-letter marker.
fn mark(evidence: Evidence) -> char {
    match evidence {
        Evidence::Hardware => 'H',
        Evidence::Firmware => 'F',
        Evidence::Manual => 'M',
    }
}

/// Render the whole matrix as Markdown.
pub fn markdown() -> String {
    let mut out = String::new();
    out.push_str(
        "# Command matrix\n\n\
         Generated from `crates/smartclock/commands.toml`.  Run `make docs`\n\
         to regenerate; a test fails if this file and the table disagree.\n\n",
    );
    summary(&mut out);
    undocumented(&mut out);
    table(&mut out);
    out
}

fn summary(out: &mut String) {
    out.push_str("## What is in the table\n\n");
    out.push_str("| Tree | Commands | Hardware | Firmware | Manual |\n");
    out.push_str("| ---- | -------- | -------- | -------- | ------ |\n");
    for (dialect, models) in DIALECTS {
        let specs = dialect.specs();
        let count = |e: Evidence| specs.iter().filter(|s| s.evidence == e).count();
        let _ = writeln!(
            out,
            "| {models} | {} | {} | {} | {} |",
            specs.len(),
            count(Evidence::Hardware),
            count(Evidence::Firmware),
            count(Evidence::Manual),
        );
    }

    out.push_str(
        "\n**H** means the receiver answered it.  **F** means every keyword\n\
         appears in the firmware's own keyword table, so the spelling is\n\
         right though no such receiver has been on the line.  **M** means\n\
         it was transcribed from a manual and nothing more.\n\n",
    );

    out.push_str("| Class | Commands | Gate |\n| ----- | -------- | ---- |\n");
    for (class, gate) in [
        (Class::Query, "none"),
        (Class::Control, "`--allow-control`"),
        (Class::Dangerous, "`--allow-dangerous`"),
    ] {
        let count = Dialect::Hp58503
            .specs()
            .iter()
            .filter(|s| s.class == class)
            .count();
        let _ = writeln!(out, "| {class:?} | {count} | {gate} |");
    }
    out.push('\n');
}

/// Commands that appear in no manual, found by sweeping a receiver.
fn undocumented(out: &mut String) {
    let found: Vec<_> = Dialect::Hp58503
        .specs()
        .iter()
        .filter(|s| s.cite.starts_with("discovered on "))
        .collect();
    if found.is_empty() {
        return;
    }
    out.push_str("## Undocumented\n\n");
    let _ = writeln!(
        out,
        "{} commands appear in none of the manuals here.  They were found\n\
         by building candidate paths from the firmware's keyword table and\n\
         sending them to a receiver: an unknown header returns -113 and\n\
         changes nothing, so a sweep is safe and settles the question.\n",
        found.len()
    );
    out.push_str("| Command | Operation | Found on |\n| ------- | --------- | -------- |\n");
    for spec in found {
        let _ = writeln!(
            out,
            "| `{}` | {} | {} |",
            spec.scpi,
            name(spec.id),
            spec.cite.trim_start_matches("discovered on ")
        );
    }
    out.push('\n');
}

fn table(out: &mut String) {
    out.push_str("## Every command\n\n");
    out.push_str(
        "One row per logical operation.  A blank cell means that tree has\n\
         no spelling for it, and the library returns `Unsupported` without\n\
         anything reaching the receiver.\n\n",
    );
    out.push_str("| Operation | Class | 58503A/B, 59551A | Z3801A, Z3816A |\n");
    out.push_str("| --------- | ----- | ---------------- | -------------- |\n");

    // Ordered by the primary tree, since that is the one with an entry
    // for everything.
    for spec in Dialect::Hp58503.specs() {
        let other = Dialect::Z3801.spec(spec.id).map_or_else(String::new, |s| {
            format!("`{}` {}", s.scpi, mark(s.evidence))
        });
        let models = if spec.models.len() == 1 {
            format!(" ({})", spec.models[0])
        } else {
            String::new()
        };
        let _ = writeln!(
            out,
            "| {} | {:?} | `{}` {}{} | {} |",
            name(spec.id),
            spec.class,
            spec.scpi,
            mark(spec.evidence),
            models,
            other
        );
    }
}

/// The operation's name as a reader would say it.
fn name(id: CommandId) -> String {
    let debug = format!("{id:?}");
    let mut out = String::with_capacity(debug.len() + 4);
    for (n, c) in debug.char_indices() {
        if c.is_uppercase() && n > 0 {
            out.push(' ');
        }
        out.push(c.to_ascii_lowercase());
    }
    out
}
