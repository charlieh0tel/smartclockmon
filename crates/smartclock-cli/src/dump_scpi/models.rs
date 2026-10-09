//! `dump-scpi --models`: the trees of several images side by side,
//! against the manuals and the command table (docs/scpi/models.md).

use std::collections::BTreeSet;
use std::fmt::Write as _;

use smartclock::command::Dialect;
use smartclock::command::Evidence;

use super::Entry;

/// A manual's command paths, as `docs/scpi/manual-*.txt` lists them.
struct Manual {
    /// Its document number.
    name: &'static str,
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
        models: &["58503a"],
        borrowers: &[],
        paths: include_str!("../../../../docs/scpi/manual-097-59551-02.txt"),
    },
    Manual {
        name: "097-58503-13",
        models: &["58503b"],
        borrowers: &[],
        paths: include_str!("../../../../docs/scpi/manual-097-58503-13.txt"),
    },
    Manual {
        name: "097-z3801-01",
        models: &["z3801a"],
        borrowers: &["z3805a"],
        paths: include_str!("../../../../docs/scpi/manual-097-z3801-01.txt"),
    },
];

/// The language a manual lists a command under that only the
/// installer, not the primary firmware, speaks.
const INSTALL: &str = "INSTALL";

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
            one.path == other.path && one.query == other.query && one.setter == other.setter
        })
}

/// Whether `written`, a command as a manual or the command table
/// writes it, names the tree's `path`.  Each keyword may be written
/// short, long, or anywhere between, in any case; a leading `:SOURce`
/// of the path may be left out; arguments and a trailing `?` are
/// ignored.
fn spells(written: &str, path: &str) -> bool {
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

/// The dialects whose command table entry for `path` a receiver has
/// answered.
fn answered(path: &str) -> Vec<&'static str> {
    [Dialect::Hp58503, Dialect::Z3801]
        .into_iter()
        .filter(|dialect| {
            dialect
                .specs()
                .iter()
                .any(|spec| spec.evidence == Evidence::Hardware && spells(spec.scpi, path))
        })
        .map(Dialect::name)
        .collect()
}

/// A tree's cell for one path: `q` for a query handler, `s` for a
/// setter, `-` for neither, blank where the image has no such path.
fn cell(entry: Option<&Entry>) -> &'static str {
    match entry {
        None => "",
        Some(entry) => match (entry.query, entry.setter) {
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
        .filter(|entry| entry.query || entry.setter)
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
    table(&mut out, images);
    out
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
            .filter(|entry| entry.query || entry.setter)
            .map(|entry| entry.node)
            .collect();
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
                    "[^{note}]: {name} has no manual of its own.  Its tree is \
                         {lender}'s, path for path and handler for handler, so it is \
                         counted against {}.",
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
                    "| {name} | {}{note} | {} | {} | {} | {} | {} | {} |",
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
                    "| {name} | none | {} | | | {} | | |",
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
         A cell is `q` where that image's node has a query handler, `s` a\n\
         setter, `qs` both, `-` neither, and blank where the image has no\n\
         such path.  Manuals names each manual listing the path, with\n\
         `(INSTALL)` where it lists it only among the installer's commands.\n\
         Answered names each dialect whose `commands.toml` entry for the\n\
         path a receiver has answered (evidence `hardware`).\n\n",
    );
    let _ = writeln!(
        out,
        "| Path | {} | Manuals | Answered |",
        images
            .iter()
            .map(|(name, _)| name.as_str())
            .collect::<Vec<_>>()
            .join(" | ")
    );
    let _ = writeln!(out, "|{}", " --- |".repeat(images.len() + 3));
    let paths: BTreeSet<(String, &str)> = images
        .iter()
        .flat_map(|(_, entries)| entries)
        .map(|entry| (entry.path.to_ascii_lowercase(), entry.path.as_str()))
        .collect();
    for (_, path) in paths {
        let cells: Vec<&str> = images
            .iter()
            .map(|(_, entries)| cell(entries.iter().find(|entry| entry.path == path)))
            .collect();
        let manuals: Vec<String> = MANUALS
            .iter()
            .filter_map(|manual| {
                manual.lists(path).map(|listed| match listed {
                    Listed::Primary => manual.name.to_owned(),
                    Listed::Install => format!("{} (INSTALL)", manual.name),
                })
            })
            .collect();
        let _ = writeln!(
            out,
            "| `{path}` | {} | {} | {} |",
            cells.join(" | "),
            manuals.join(", "),
            answered(path).join(", ")
        );
    }
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
