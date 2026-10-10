//! `dump-pforth`: every word a firmware image's pForth console knows,
//! read out of its two tables (docs/firmware/pforth/README.md).
//!
//! The kernel's dictionary is a linked list; the diagnostic words are
//! a table of fixed records.  Each is found from a word every image
//! has, and an image where either is not found is refused.

use std::collections::BTreeSet;
use std::fmt::Write as _;
use std::path::Path;
use std::path::PathBuf;

use anyhow::Context as _;
use anyhow::Result;
use anyhow::bail;
use anyhow::ensure;

use crate::image::Image;

/// A kernel word every image's dictionary holds, to find it by.
const KERNEL_ANCHOR: &str = "halt";
/// A diagnostic word every image's table holds, to find it by.
const DIAGNOSTIC_ANCHOR: &str = "loop_time";

/// Where a dictionary entry keeps the link to the entry before it,
/// zero for the first.
const LINK_SLOT: u32 = 0;
/// Where it keeps its code's address.
const CODE_SLOT: u32 = 4;
/// Where its name starts, NUL-terminated.
const NAME_SLOT: u32 = 12;
/// How far past an entry the next one may start.
const ENTRY_REACH: u32 = 64;

/// The size of a diagnostic record: the code's address, a name field of
/// `RECORD_NAME` bytes and six bytes more.
const RECORD_SIZE: u32 = 38;
/// The width of a diagnostic record's name field.
const RECORD_NAME: u32 = 28;

/// The longest name either table holds.
const NAME_LIMIT: usize = 31;

/// The table a word comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Table {
    /// The interpreter's dictionary.
    Kernel,
    /// The firmware's own words.
    Diagnostic,
}

impl Table {
    fn name(self) -> &'static str {
        match self {
            Self::Kernel => "kernel",
            Self::Diagnostic => "diagnostic",
        }
    }
}

/// One word of the console.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Word {
    /// Its table.
    table: Table,
    /// Its name as typed.
    name: String,
    /// Where its code is.
    code: u32,
}

impl Image<'_> {
    /// The NUL-terminated name at `at`, if it is printable ASCII.
    fn name(&self, at: u32) -> Option<String> {
        let start = usize::try_from(at).ok()?;
        let bytes = self.0.get(start..)?;
        let length = bytes
            .iter()
            .take(NAME_LIMIT + 1)
            .position(|&byte| byte == 0)?;
        let text = &bytes[..length];
        (length > 0 && text.iter().all(u8::is_ascii_graphic))
            .then(|| String::from_utf8_lossy(text).into_owned())
    }

    /// Whether `at` is even and inside the image: a place code can be.
    fn code_at(&self, at: u32) -> bool {
        at.is_multiple_of(2) && self.holds(at) && at != 0
    }

    /// The dictionary entry at `at`: its link, code and name.
    fn entry(&self, at: u32) -> Option<(u32, u32, String)> {
        let link = self.u32(at + LINK_SLOT)?;
        let code = self.u32(at + CODE_SLOT)?;
        let name = self.name(at + NAME_SLOT)?;
        (self.code_at(code) && (link == 0 || self.holds(link))).then_some((link, code, name))
    }

    /// The diagnostic record at `at`: its code and name, the name field
    /// padded with NULs.
    fn record(&self, at: u32) -> Option<(u32, String)> {
        let code = self.u32(at)?;
        let name = self.name(at + 4)?;
        let start = usize::try_from(at + 4).ok()?;
        let field = self
            .0
            .get(start..start + usize::try_from(RECORD_NAME).ok()?)?;
        let padded = field[name.len()..].iter().all(|&byte| byte == 0);
        (self.code_at(code) && padded && name.len() < field.len()).then_some((code, name))
    }
}

/// Where the first occurrence of `name` followed by a NUL is.
fn find(image: &Image, name: &str) -> Vec<u32> {
    let needle: Vec<u8> = name.bytes().chain([0]).collect();
    image
        .0
        .windows(needle.len())
        .enumerate()
        .filter(|(_, window)| *window == needle.as_slice())
        .filter_map(|(at, _)| u32::try_from(at).ok())
        .collect()
}

/// The kernel's dictionary, from its first entry to its last.
fn kernel(image: &Image) -> Result<Vec<Word>> {
    let anchors: Vec<u32> = find(image, KERNEL_ANCHOR)
        .into_iter()
        .filter_map(|at| at.checked_sub(NAME_SLOT))
        .filter(|&at| {
            image
                .entry(at)
                .is_some_and(|(_, _, name)| name == KERNEL_ANCHOR)
        })
        .collect();
    let [anchor] = anchors.as_slice() else {
        bail!(
            "{} dictionary entries named {KERNEL_ANCHOR} found, not one",
            anchors.len()
        );
    };
    let mut chain = vec![*anchor];
    let mut at = *anchor;
    while let Some((link, _, _)) = image.entry(at) {
        if link == 0 {
            break;
        }
        ensure!(!chain.contains(&link), "the dictionary loops at {link:#x}");
        ensure!(
            image.entry(link).is_some(),
            "the dictionary's link to {link:#x} is not an entry"
        );
        chain.push(link);
        at = link;
    }
    chain.reverse();
    let mut at = *anchor;
    loop {
        let (_, _, name) = image.entry(at).context("a dictionary entry")?;
        let after = at + NAME_SLOT + u32::try_from(name.len()).context("a name")?;
        let next: Vec<u32> = (after..after + ENTRY_REACH)
            .filter(|&candidate| {
                image
                    .entry(candidate)
                    .is_some_and(|(link, _, _)| link == at)
            })
            .collect();
        match next.as_slice() {
            [] => break,
            [one] => {
                chain.push(*one);
                at = *one;
            }
            many => bail!("{} entries follow {at:#x}; refusing to guess", many.len()),
        }
    }
    chain
        .into_iter()
        .map(|at| {
            let (_, code, name) = image.entry(at).context("a dictionary entry")?;
            Ok(Word {
                table: Table::Kernel,
                name,
                code,
            })
        })
        .collect()
}

/// The diagnostic words' table, record by record.
fn diagnostic(image: &Image) -> Result<Vec<Word>> {
    let anchors: Vec<u32> = find(image, DIAGNOSTIC_ANCHOR)
        .into_iter()
        .filter_map(|at| at.checked_sub(4))
        .filter(|&at| {
            image
                .record(at)
                .is_some_and(|(_, name)| name == DIAGNOSTIC_ANCHOR)
        })
        .collect();
    let [anchor] = anchors.as_slice() else {
        bail!(
            "{} diagnostic records named {DIAGNOSTIC_ANCHOR} found, not one",
            anchors.len()
        );
    };
    let mut first = *anchor;
    while let Some(before) = first.checked_sub(RECORD_SIZE) {
        if image.record(before).is_none() {
            break;
        }
        first = before;
    }
    let mut words = Vec::new();
    let mut at = first;
    while let Some((code, name)) = image.record(at) {
        words.push(Word {
            table: Table::Diagnostic,
            name,
            code,
        });
        at += RECORD_SIZE;
    }
    Ok(words)
}

/// Every word of the image's console, by table and then by name.
fn words(bytes: &[u8]) -> Result<Vec<Word>> {
    let image = Image(bytes);
    let mut words = kernel(&image)?;
    words.extend(diagnostic(&image)?);
    words.sort_by(|a, b| (a.table, &a.name).cmp(&(b.table, &b.name)));
    words.dedup_by(|later, earlier| later.table == earlier.table && later.name == earlier.name);
    Ok(words)
}

/// An image's words, one a line: its table, its name and its code.
fn lines(words: &[Word]) -> Vec<String> {
    words
        .iter()
        .map(|word| format!("{} {} {:#07x}", word.table.name(), word.name, word.code))
        .collect()
}

/// The words of `images`, each named as its file is, side by side as
/// Markdown: a row per word, a tick per image that has it.
fn markdown(images: &[(String, Vec<Word>)]) -> String {
    let mut out = String::from(
        "# pForth words by model\n\n\
         Generated by `smartclock-cli dump-pforth --models` from the images\n\
         in `third_party/firmware/`.  Run `make docs` to regenerate; a test\n\
         fails if this file and the images disagree.  `README.md` here says\n\
         how the tables are read; `../console.md` is the console itself.\n\n",
    );
    let heading = |name: &str| name.to_ascii_uppercase().replacen('-', "<br>", 1);
    let header = format!(
        "| Word | {} |\n|{}\n",
        images
            .iter()
            .map(|(name, _)| heading(name))
            .collect::<Vec<_>>()
            .join(" | "),
        " --- |".repeat(images.len() + 1)
    );
    out.push_str("| Table | ");
    out.push_str(
        &images
            .iter()
            .map(|(name, _)| heading(name))
            .collect::<Vec<_>>()
            .join(" | "),
    );
    let _ = writeln!(out, " |\n|{}", " --- |".repeat(images.len() + 1));
    for table in [Table::Kernel, Table::Diagnostic] {
        let counts: Vec<String> = images
            .iter()
            .map(|(_, words)| {
                words
                    .iter()
                    .filter(|word| word.table == table)
                    .count()
                    .to_string()
            })
            .collect();
        let _ = writeln!(out, "| {} words | {} |", table.name(), counts.join(" | "));
    }
    out.push('\n');
    for table in [Table::Kernel, Table::Diagnostic] {
        let names: BTreeSet<&str> = images
            .iter()
            .flat_map(|(_, words)| words)
            .filter(|word| word.table == table)
            .map(|word| word.name.as_str())
            .collect();
        let has = |name: &str, words: &[Word]| {
            words
                .iter()
                .any(|word| word.table == table && word.name == name)
        };
        let (everywhere, some): (Vec<&str>, Vec<&str>) = names
            .into_iter()
            .partition(|name| images.iter().all(|(_, words)| has(name, words)));
        let _ = writeln!(out, "## The {} words\n", table.name());
        let _ = writeln!(
            out,
            "In every image, {}:\n\n{}\n",
            everywhere.len(),
            everywhere
                .iter()
                .map(|name| format!("`{name}`"))
                .collect::<Vec<_>>()
                .join(" ")
        );
        if some.is_empty() {
            continue;
        }
        let _ = write!(out, "In some images, {}:\n\n{header}", some.len());
        for name in some {
            let marks: Vec<&str> = images
                .iter()
                .map(|(_, words)| if has(name, words) { "✓" } else { "" })
                .collect();
            let _ = writeln!(out, "| `{name}` | {} |", marks.join(" | "));
        }
        out.push('\n');
    }
    out
}

/// An image read from `path`, with its words.
fn read(path: &Path) -> Result<Vec<Word>> {
    let bytes = std::fs::read(path).with_context(|| format!("reading {}", path.display()))?;
    words(&bytes).with_context(|| format!("reading {}", path.display()))
}

/// Print one image's words, one a line; with `models`, the words of
/// all of them side by side as Markdown instead.
pub(crate) fn run(images: &[PathBuf], models: bool) -> Result<()> {
    if models {
        let all = images
            .iter()
            .map(|path| {
                let name = path
                    .file_stem()
                    .and_then(|stem| stem.to_str())
                    .with_context(|| format!("{} has no usable name", path.display()))?;
                Ok((name.to_owned(), read(path)?))
            })
            .collect::<Result<Vec<_>>>()?;
        print!("{}", markdown(&all));
        return Ok(());
    }
    let [image] = images else {
        bail!("give one image, or --models and any number");
    };
    for line in lines(&read(image)?) {
        println!("{line}");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::Word;
    use super::lines;
    use super::markdown;
    use super::words;

    /// Each image in `third_party/firmware/`, by the name its list has
    /// in `docs/firmware/pforth/`.
    const IMAGES: [&str; 7] = [
        "58503a-3633",
        "58503a-3704",
        "58503b-1.01.04",
        "z3801a-3543",
        "z3805a-3543b",
        "z3815a-4010",
        "z3816a-4001",
    ];

    fn root() -> String {
        format!("{}/../..", env!("CARGO_MANIFEST_DIR"))
    }

    fn image(name: &str) -> Vec<Word> {
        let bytes = std::fs::read(format!("{}/third_party/firmware/{name}.bin", root()))
            .expect("the image is in the repository");
        words(&bytes).expect("the image has a console")
    }

    #[test]
    fn every_image_gives_the_words_its_list_holds() {
        for name in IMAGES {
            let held =
                std::fs::read_to_string(format!("{}/docs/firmware/pforth/{name}.txt", root()))
                    .expect("the list is in the repository");
            assert_eq!(
                lines(&image(name)),
                held.lines().collect::<Vec<_>>(),
                "{name}"
            );
        }
    }

    #[test]
    fn the_models_document_is_what_the_images_give() {
        let all: Vec<(String, Vec<Word>)> = IMAGES
            .iter()
            .map(|name| ((*name).to_owned(), image(name)))
            .collect();
        let held = std::fs::read_to_string(format!("{}/docs/firmware/pforth/models.md", root()))
            .expect("the document is in the repository");
        assert_eq!(
            markdown(&all).lines().collect::<Vec<_>>(),
            held.lines().collect::<Vec<_>>(),
            "docs/firmware/pforth/models.md is stale; run make docs"
        );
    }

    #[test]
    fn an_image_without_a_console_is_refused() {
        assert!(words(&[0; 0x1000]).is_err());
    }
}
