//! `dump-scpi --models`: the trees of several images side by side,
//! against the manuals and the command table (docs/scpi/models.md).

use std::collections::BTreeMap;
use std::collections::BTreeSet;
use std::fmt::Write as _;

use smartclock::command::Dialect;
use smartclock::command::Evidence;

use super::Entry;

/// A manual's command paths, as `docs/scpi/manual-*.txt` lists them.
struct Manual {
    /// Its document number.
    name: &'static str,
    /// The letter the path table names it by, to stay narrow.
    letter: char,
    /// The models it is the manual of, as an image's name starts.
    models: &'static [&'static str],
    /// Models with no manual of their own, counted against this one
    /// where an image's tree is that of one of `models`.
    borrowers: &'static [&'static str],
    /// One `LANGUAGE PATH` a line; `#` lines are comments.
    paths: &'static str,
}

/// Every manual whose paths are listed.
const MANUALS: [Manual; 3] = [
    Manual {
        name: "097-59551-02",
        letter: 'A',
        models: &["58503a"],
        borrowers: &[],
        paths: include_str!("../../../../docs/scpi/manual-097-59551-02.txt"),
    },
    Manual {
        name: "097-58503-13",
        letter: 'B',
        models: &["58503b"],
        borrowers: &[],
        paths: include_str!("../../../../docs/scpi/manual-097-58503-13.txt"),
    },
    Manual {
        name: "097-z3801-01",
        letter: 'Z',
        models: &["z3801a"],
        borrowers: &["z3805a"],
        paths: include_str!("../../../../docs/scpi/manual-097-z3801-01.txt"),
    },
];

/// The language a manual lists a command under that only the
/// installer, not the primary firmware, speaks.
const INSTALL: &str = "INSTALL";

/// The section the path table puts the common commands in.
const COMMON_SECTION: &str = "Common commands";
/// The section it puts top-level keywords with no children in, which
/// would otherwise each have a table of one row.
const LEAVES_SECTION: &str = "Top-level keywords without children";

/// How many documented paths the alias table names for each alias.
const ALIASES_SHOWN: usize = 2;

/// The optional header a path may leave out.
const OPTIONAL_HEADER: &str = "SOURce";

/// How a manual lists a path, if it does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Listed {
    /// Among the primary firmware's commands.
    Primary,
    /// Only among the installer's.
    Install,
}

impl Manual {
    /// The commands it lists, with the language each is under.
    fn commands(&self) -> impl Iterator<Item = (Listed, &'static str)> {
        self.paths
            .lines()
            .filter(|line| !line.is_empty() && !line.starts_with('#'))
            .filter_map(|line| {
                let (language, path) = line.split_once(' ')?;
                Some((
                    if language == INSTALL {
                        Listed::Install
                    } else {
                        Listed::Primary
                    },
                    path,
                ))
            })
    }

    /// How it lists `path`, if it does: under the primary firmware's
    /// commands wherever it is listed there.
    fn lists(&self, path: &str) -> Option<Listed> {
        let found: Vec<Listed> = self
            .commands()
            .filter(|(_, written)| spells(written, path))
            .map(|(listed, _)| listed)
            .collect();
        if found.contains(&Listed::Primary) {
            Some(Listed::Primary)
        } else {
            found.first().copied()
        }
    }

    /// Whether it is the manual of the image named `image`.
    fn covers(&self, image: &str) -> bool {
        is_model(image, self.models)
    }

    /// Whether the image named `image` is of a model it is borrowed by.
    fn lends(&self, image: &str) -> bool {
        is_model(image, self.borrowers)
    }
}

/// Whether the image named `image` is of one of `models`, by the part
/// of its name before the first `-`.
fn is_model(image: &str, models: &[&str]) -> bool {
    image
        .split('-')
        .next()
        .is_some_and(|first| models.iter().any(|model| first.eq_ignore_ascii_case(model)))
}

/// Whether two trees hold the same paths with the same handlers,
/// wherever their nodes are.
fn same_tree(one: &[Entry], other: &[Entry]) -> bool {
    one.len() == other.len()
        && one.iter().zip(other).all(|(one, other)| {
            one.path == other.path
                && one.query.is_some() == other.query.is_some()
                && one.setter.is_some() == other.setter.is_some()
        })
}

/// Whether `written`, a command as a manual or the command table
/// writes it, names the tree's `path`.  Each keyword may be written
/// short, long, or anywhere between, in any case; a leading `:SOURce`
/// of the path may be left out; arguments and a trailing `?` are
/// ignored.
pub(super) fn spells(written: &str, path: &str) -> bool {
    let written = written
        .split_whitespace()
        .next()
        .unwrap_or_default()
        .trim_end_matches('?');
    if written.starts_with('*') || path.starts_with('*') {
        return written.eq_ignore_ascii_case(path);
    }
    let tokens: Vec<&str> = written.trim_start_matches(':').split(':').collect();
    let keywords: Vec<&str> = path.trim_start_matches(':').split(':').collect();
    let matches = |keywords: &[&str]| {
        keywords.len() == tokens.len()
            && tokens.iter().zip(keywords).all(|(token, keyword)| {
                let token = token.to_ascii_uppercase();
                let short: String = keyword
                    .chars()
                    .filter(|letter| !letter.is_ascii_lowercase())
                    .collect();
                keyword.to_ascii_uppercase().starts_with(&token) && token.starts_with(&short)
            })
    };
    matches(&keywords) || (keywords.first() == Some(&OPTIONAL_HEADER) && matches(&keywords[1..]))
}

/// The receiver each dialect's `hardware` evidence came from: the
/// 58503A is the only one of its dialect on the bench, and every
/// `z3801` entry with that evidence cites the Z3805A.
const CONFIRMED_ON: [(Dialect, &str); 2] =
    [(Dialect::Hp58503, "58503A"), (Dialect::Z3801, "Z3805A")];

/// The receivers that have answered the command table's entry for
/// `path`.
fn confirmed(path: &str) -> Vec<&'static str> {
    CONFIRMED_ON
        .into_iter()
        .filter(|(dialect, _)| {
            dialect
                .specs()
                .iter()
                .any(|spec| spec.evidence == Evidence::Hardware && spells(spec.scpi, path))
        })
        .map(|(_, model)| model)
        .collect()
}

/// An image's name as a heading: model and revision upper case, as
/// the receivers print them, `z3805a-3543b` as `Z3805A 3543B`.
fn title(name: &str) -> String {
    name.to_ascii_uppercase().replacen('-', " ", 1)
}

/// A tree's cell for one path: `q` for a query handler, `s` for a
/// setter, `-` for neither, blank where the image has no such path.
fn cell(entry: Option<&Entry>) -> &'static str {
    match entry {
        None => "",
        Some(entry) => match (entry.query.is_some(), entry.setter.is_some()) {
            (true, true) => "qs",
            (true, false) => "q",
            (false, true) => "s",
            (false, false) => "-",
        },
    }
}

/// How many of `entries` its own manual lists, and how many it does
/// not, counted by path and by node with a handler.
#[derive(Debug, Default)]
struct Counts {
    /// Paths it lists.
    paths_listed: usize,
    /// Paths it does not.
    paths_unlisted: usize,
    /// Nodes with a handler it lists by at least one of their paths.
    handlers_listed: usize,
    /// Nodes with a handler it lists by none.
    handlers_unlisted: usize,
}

fn count(manual: &Manual, entries: &[Entry]) -> Counts {
    let primary = |entry: &Entry| manual.lists(&entry.path) == Some(Listed::Primary);
    let paths_listed = entries.iter().filter(|entry| primary(entry)).count();
    let handlers: BTreeSet<u32> = entries
        .iter()
        .filter(|entry| entry.query.is_some() || entry.setter.is_some())
        .map(|entry| entry.node)
        .collect();
    let listed: BTreeSet<u32> = entries
        .iter()
        .filter(|entry| primary(entry))
        .map(|entry| entry.node)
        .collect();
    let handlers_listed = handlers.intersection(&listed).count();
    Counts {
        paths_listed,
        paths_unlisted: entries.len() - paths_listed,
        handlers_listed,
        handlers_unlisted: handlers.len() - handlers_listed,
    }
}

/// The trees of `images`, each named as its file is, side by side as
/// Markdown.
pub(super) fn markdown(images: &[(String, Vec<Entry>)]) -> String {
    let mut out = String::from(
        "# SCPI trees by model\n\n\
         Generated by `smartclock-cli dump-scpi --models` from the images in\n\
         `third_party/firmware/`, the manual path lists beside this file and\n\
         `crates/smartclock/commands.toml`.  Run `make docs` to regenerate; a\n\
         test fails if this file and they disagree.  `README.md` here says\n\
         how the trees are read.\n\n",
    );
    summary(&mut out, images);
    aliases(&mut out, images);
    stubs(&mut out, images);
    table(&mut out, images);
    out
}

/// Whether any manual lists `path`, in either language.
fn in_a_manual(path: &str) -> bool {
    MANUALS.iter().any(|manual| manual.lists(path).is_some())
}

/// Whether any manual lists `path` among the primary firmware's
/// commands.
fn in_a_primary_list(path: &str) -> bool {
    MANUALS
        .iter()
        .any(|manual| manual.lists(path) == Some(Listed::Primary))
}

/// The images, by heading, in which `holds` is true of an entry for
/// `path`.
fn holding(
    images: &[(String, Vec<Entry>)],
    path: &str,
    holds: impl Fn(&Entry) -> bool,
) -> Vec<String> {
    images
        .iter()
        .filter(|(_, entries)| {
            entries
                .iter()
                .any(|entry| entry.path == path && holds(entry))
        })
        .map(|(name, _)| title(name))
        .collect()
}

/// Paths in no manual whose node has the very handlers of a path a
/// manual lists.
fn aliases(out: &mut String, images: &[(String, Vec<Entry>)]) {
    out.push_str(
        "## Same handlers as a documented path\n\n\
         Each path here is in no manual, and its node has the same query\n\
         handler and setter, at the same addresses, as the node of a path\n\
         a manual lists among the primary firmware's commands -- up to two\n\
         of which are named.  A shared child list reaching one node by\n\
         several paths is not counted.  A handler is passed its node, so\n\
         this makes the path another name for the documented one only\n\
         where the handler does not tell them apart; the\n\
         `:SYSTem:COMMunicate` ports, for one, share a query that does.\n\n\
         | Path | Same handlers as | Images |\n\
         | ---- | ---------------- | ------ |\n",
    );
    let mut found: Vec<(String, BTreeSet<String>)> = Vec::new();
    for (_, entries) in images {
        for entry in entries {
            if entry.refuses
                || entry.query.is_none() && entry.setter.is_none()
                || in_a_manual(&entry.path)
            {
                continue;
            }
            let documented: BTreeSet<String> = entries
                .iter()
                .filter(|other| {
                    other.node != entry.node
                        && other.query == entry.query
                        && other.setter == entry.setter
                        && in_a_primary_list(&other.path)
                })
                .map(|other| other.path.clone())
                .collect();
            if documented.is_empty() {
                continue;
            }
            match found.iter_mut().find(|(path, _)| *path == entry.path) {
                Some((_, all)) => all.extend(documented),
                None => found.push((entry.path.clone(), documented)),
            }
        }
    }
    found.sort_by_key(|(path, _)| path.to_ascii_lowercase());
    for (path, documented) in found {
        let shown: Vec<String> = documented
            .iter()
            .take(ALIASES_SHOWN)
            .map(|other| format!("`{other}`"))
            .collect();
        let more = documented.len().saturating_sub(ALIASES_SHOWN);
        let _ = writeln!(
            out,
            "| `{path}` | {}{} | {} |",
            shown.join(", "),
            if more == 0 {
                String::new()
            } else {
                format!(" and {more} more")
            },
            holding(images, &path, |entry| entry.query.is_some()
                || entry.setter.is_some())
            .join(", ")
        );
    }
    out.push('\n');
}

/// Paths whose every handler only refuses.
fn stubs(out: &mut String, images: &[(String, Vec<Entry>)]) {
    out.push_str(
        "## Stubs\n\n\
         Each path here has handlers, and each of them does nothing but\n\
         call the routine every such handler calls to refuse: the parser\n\
         knows the path and the firmware does not implement it.  Images\n\
         names those where it is a stub.\n\n\
         | Path | Manuals | Images |\n\
         | ---- | ------- | ------ |\n",
    );
    let paths: BTreeSet<(String, &str)> = images
        .iter()
        .flat_map(|(_, entries)| entries)
        .filter(|entry| entry.refuses)
        .map(|entry| (entry.path.to_ascii_lowercase(), entry.path.as_str()))
        .collect();
    for (_, path) in paths {
        let _ = writeln!(
            out,
            "| `{path}` | {} | {} |",
            letters(path).join(", "),
            holding(images, path, |entry| entry.refuses).join(", ")
        );
    }
    out.push('\n');
}

/// The manuals listing `path`, each by its letter, `(INSTALL)` where
/// only among the installer's commands.
fn letters(path: &str) -> Vec<String> {
    MANUALS
        .iter()
        .filter_map(|manual| {
            manual.lists(path).map(|listed| match listed {
                Listed::Primary => manual.letter.to_string(),
                Listed::Install => format!("{} (INSTALL)", manual.letter),
            })
        })
        .collect()
}

fn summary(out: &mut String, images: &[(String, Vec<Entry>)]) {
    out.push_str(
        "## Against each model's manual\n\n\
         A path counts as in the manual where the manual lists it among the\n\
         primary firmware's commands.  By path, a node under a list several\n\
         parents share counts once for each path that reaches it; by\n\
         handler, each node with a query handler or setter counts once, as\n\
         listed if any of its paths is.\n\n\
         | Image | Manual | Paths | Listed | Not | Handlers | Listed | Not |\n\
         | ----- | ------ | ----- | ------ | --- | -------- | ------ | --- |\n",
    );
    let mut notes = Vec::new();
    for (name, entries) in images {
        let handlers: BTreeSet<u32> = entries
            .iter()
            .filter(|entry| entry.query.is_some() || entry.setter.is_some())
            .map(|entry| entry.node)
            .collect();
        let heading = title(name);
        let own = MANUALS.iter().find(|manual| manual.covers(name));
        let borrowed = || {
            MANUALS.iter().find_map(|manual| {
                let lender = images.iter().find(|(other, tree)| {
                    manual.lends(name) && manual.covers(other) && same_tree(tree, entries)
                })?;
                Some((manual, &lender.0))
            })
        };
        let found = own.map(|manual| (manual, String::new())).or_else(|| {
            borrowed().map(|(manual, lender)| {
                let note = notes.len() + 1;
                notes.push(format!(
                    "[^{note}]: The {heading} has no manual of its own.  Its tree is \
                     the {}'s, path for path and handler for handler, so it is \
                     counted against {}.",
                    title(lender),
                    manual.name
                ));
                (manual, format!("[^{note}]"))
            })
        });
        match found {
            Some((manual, note)) => {
                let counts = count(manual, entries);
                let _ = writeln!(
                    out,
                    "| {heading} | {}{note} | {} | {} | {} | {} | {} | {} |",
                    manual.name,
                    entries.len(),
                    counts.paths_listed,
                    counts.paths_unlisted,
                    handlers.len(),
                    counts.handlers_listed,
                    counts.handlers_unlisted,
                );
            }
            None => {
                let _ = writeln!(
                    out,
                    "| {heading} | none | {} | | | {} | | |",
                    entries.len(),
                    handlers.len(),
                );
            }
        }
    }
    out.push('\n');
    for note in notes {
        let _ = writeln!(out, "{note}\n");
    }
}

fn table(out: &mut String, images: &[(String, Vec<Entry>)]) {
    out.push_str(
        "## Every path\n\n\
         Manuals names each manual listing the path by a letter, with\n\
         `(INSTALL)` where it lists it only among the installer's commands;\n\
         blank, the path is in no manual.  Confirmed names the bench\n\
         receiver that has answered the path's `commands.toml` entry\n\
         (evidence `hardware`); blank, no receiver has, or the path has no\n\
         entry.  An image's cell is `q` where its node has a\n\
         query handler, `s` a setter, `qs` both, `-` neither, and blank\n\
         where the image has no such path.  Paths that reach the same node\n\
         in every image that holds them share a row, the others named after\n\
         \"also\" by the keyword that differs; a path a manual lists leads,\n\
         else the shortest.  A table for each top-level keyword keeps each\n\
         one short.\n\n",
    );
    for manual in &MANUALS {
        let _ = writeln!(
            out,
            "- {}: {}, the {}'s manual",
            manual.letter,
            manual.name,
            manual.models.join(", ").to_ascii_uppercase()
        );
    }
    out.push('\n');
    let header = format!(
        "| Path | Manuals | Confirmed | {} |\n|{}\n",
        images
            .iter()
            .map(|(name, _)| title(name).replacen(' ', "<br>", 1))
            .collect::<Vec<_>>()
            .join(" | "),
        " --- |".repeat(images.len() + 3)
    );
    for (name, rows) in sections(images) {
        let _ = write!(out, "### {name}\n\n{header}");
        for row in rows {
            let also = if row.also.is_empty() {
                String::new()
            } else {
                format!(
                    " (also {})",
                    row.also
                        .iter()
                        .map(|other| format!("`{other}`"))
                        .collect::<Vec<_>>()
                        .join(", ")
                )
            };
            let _ = writeln!(
                out,
                "| `{}`{also} | {} | {} | {} |",
                row.lead,
                row.manuals.join(", "),
                row.confirmed.join(", "),
                row.cells.join(" | ")
            );
        }
        out.push('\n');
    }
}

/// The page `html` fills: everything but the data, which replaces
/// `DATA_MARK`.
const PAGE: &str = include_str!("models.html");

/// Where in `PAGE` the data goes.
const DATA_MARK: &str = "__DATA__";

/// The trees of `images` side by side as a page that filters them: the
/// path table's rows as JSON in `PAGE`.
pub(super) fn html(images: &[(String, Vec<Entry>)]) -> String {
    let data = serde_json::json!({
        "images": images.iter().map(|(name, _)| title(name)).collect::<Vec<_>>(),
        "manuals": MANUALS.iter().map(|manual| serde_json::json!({
            "letter": manual.letter.to_string(),
            "name": manual.name,
            "model": manual.models.join(", ").to_ascii_uppercase(),
        })).collect::<Vec<_>>(),
        "sections": sections(images).into_iter().map(|(name, rows)| serde_json::json!({
            "name": name,
            "rows": rows.into_iter().map(|row| serde_json::json!({
                "lead": row.lead,
                "also": row.also,
                "cells": row.cells,
                "manuals": row.manuals,
                "confirmed": row.confirmed,
            })).collect::<Vec<_>>(),
        })).collect::<Vec<_>>(),
    });
    PAGE.replacen(DATA_MARK, &data.to_string(), 1)
}

/// One row of the path table: the paths that reach the same node in
/// every image that holds any of them.
#[derive(Debug)]
struct Row {
    /// The path shown: one a manual lists, else the shortest.
    lead: String,
    /// The other paths, each as it differs from the lead.
    also: Vec<String>,
    /// Each image's cell, in the images' order (`cell`).
    cells: Vec<&'static str>,
    /// The manuals listing any of the paths (`letters`).
    manuals: Vec<String>,
    /// The receivers that have answered any of them (`confirmed`).
    confirmed: Vec<String>,
}

/// The path table's rows by section, in the order they are shown.
fn sections(images: &[(String, Vec<Entry>)]) -> Vec<(String, Vec<Row>)> {
    let nodes: Vec<BTreeMap<&str, &Entry>> = images
        .iter()
        .map(|(_, entries)| {
            entries
                .iter()
                .map(|entry| (entry.path.as_str(), entry))
                .collect()
        })
        .collect();
    let rows = rows(&nodes);
    let branches: BTreeSet<&str> = rows
        .iter()
        .filter_map(|row| {
            let path = row[0];
            let (root, rest) = path.trim_start_matches(':').split_once(':')?;
            (!path.starts_with('*') && !rest.is_empty()).then_some(root)
        })
        .collect();
    let section = |path: &str| -> String {
        let root = path
            .trim_start_matches(':')
            .split(':')
            .next()
            .unwrap_or_default();
        if path.starts_with('*') {
            COMMON_SECTION.to_owned()
        } else if branches.contains(root) {
            format!(":{root}")
        } else {
            LEAVES_SECTION.to_owned()
        }
    };
    let mut sections: Vec<(String, Vec<Row>)> = Vec::new();
    for paths in &rows {
        let lead = paths[0];
        let row = Row {
            lead: lead.to_owned(),
            also: paths[1..]
                .iter()
                .map(|other| difference(lead, other).to_owned())
                .collect(),
            cells: nodes
                .iter()
                .map(|image| cell(image.get(lead).copied()))
                .collect(),
            manuals: merged(paths.iter().flat_map(|path| letters(path))),
            confirmed: merged(
                paths
                    .iter()
                    .flat_map(|path| confirmed(path).into_iter().map(str::to_owned)),
            ),
        };
        let name = section(lead);
        match sections.iter_mut().find(|(each, _)| *each == name) {
            Some((_, members)) => members.push(row),
            None => sections.push((name, vec![row])),
        }
    }
    sections.sort_by_key(|(name, _)| {
        (
            name != COMMON_SECTION,
            name == LEAVES_SECTION,
            name.to_ascii_lowercase(),
        )
    });
    sections
}

/// The table's rows: each a set of paths that reach the same node in
/// every image that holds any of them, the one to show first leading.
/// A path a manual lists leads; otherwise the shortest.
fn rows<'a>(nodes: &[BTreeMap<&'a str, &'a Entry>]) -> Vec<Vec<&'a str>> {
    let mut groups: BTreeMap<Vec<Option<u32>>, Vec<&str>> = BTreeMap::new();
    let paths: BTreeSet<&str> = nodes
        .iter()
        .flat_map(|image| image.keys().copied())
        .collect();
    for path in paths {
        let key = nodes
            .iter()
            .map(|image| image.get(path).map(|entry| entry.node))
            .collect();
        groups.entry(key).or_default().push(path);
    }
    let mut rows: Vec<Vec<&str>> = groups
        .into_values()
        .map(|mut group| {
            group.sort_by_key(|path| (!in_a_manual(path), path.len(), path.to_ascii_lowercase()));
            group
        })
        .collect();
    rows.sort_by_key(|row| row[0].to_ascii_lowercase());
    rows
}

/// How `other` differs from `lead`: the one keyword that differs, when
/// only one does; otherwise the whole path.
fn difference<'a>(lead: &str, other: &'a str) -> &'a str {
    let ours: Vec<&str> = lead.split(':').collect();
    let theirs: Vec<&str> = other.split(':').collect();
    if ours.len() == theirs.len() {
        let differing: Vec<usize> = (0..ours.len()).filter(|&i| ours[i] != theirs[i]).collect();
        if let [only] = differing.as_slice() {
            return theirs[*only];
        }
    }
    other
}

/// `items` in order, each once.
fn merged(items: impl Iterator<Item = String>) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for item in items {
        if !out.contains(&item) {
            out.push(item);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::spells;

    #[test]
    fn a_keyword_is_spelled_short_long_or_between_and_source_is_optional() {
        assert!(spells(":SYST:ERR?", ":SYSTem:ERRor"));
        assert!(spells(":DIAG:LOG:COUN?", ":DIAGnostic:LOG:COUNt"));
        assert!(spells(
            ":ROSC:HOLD:DUR?",
            ":SOURce:ROSCillator:HOLDover:DURation"
        ));
        assert!(spells("*idn?", "*IDN"));
        assert!(!spells(":SY:ERR?", ":SYSTem:ERRor"));
        assert!(!spells(":SYSTem:ERRors?", ":SYSTem:ERRor"));
        assert!(!spells(":SYSTem", ":SYSTem:ERRor"));
    }
}
