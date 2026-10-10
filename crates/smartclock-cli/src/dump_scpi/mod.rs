//! `dump-scpi`: every command path a firmware image's SCPI parser
//! knows, read out of its own tables (docs/scpi/README.md).
//!
//! Pointers in the image are absolute and it is not relocated, so a
//! stored pointer is a file offset as it stands.  Nothing here knows an
//! address: the tables are found by their shape, and an image whose
//! tables are not found exactly once is refused rather than guessed at.

use std::collections::BTreeSet;
use std::path::PathBuf;

use anyhow::Context as _;
use anyhow::Result;
use anyhow::bail;
use anyhow::ensure;

use crate::image;
use crate::image::Image;

mod models;

/// Where a node keeps the pointer to its keyword pair.
const KEYWORD_SLOT: u32 = 0;
/// Where a node keeps the pointer to its child list, zero for a leaf.
const CHILDREN_SLOT: u32 = 4;
/// Where a node keeps its setter, zero if it has none.
const SETTER_SLOT: u32 = 8;
/// Where a node keeps its query handler, zero if it has none.
const QUERY_SLOT: u32 = 18;

/// Where a child list keeps how many children follow.
const COUNT_SLOT: u32 = 2;
/// Where a child list's pointers to its children start.
const CHILDREN_START: u32 = 6;
/// No child list in any image holds this many; a larger count is not
/// a child list.
const CHILDREN_LIMIT: u16 = 80;

/// The one character besides letters and digits a keyword may hold:
/// the HP 53131A-family counters, whose firmware shares this parser,
/// list a common command `_TRG` beside `*TRG`.
const KEYWORD_PUNCTUATION: u8 = b'_';

/// The longest half of a keyword pair in any image.
const KEYWORD_LIMIT: usize = 12;

/// A handler that does nothing but refuse, as these images compile one:
/// `move.l (12,sp),-(sp)`, `move.b #n,-(sp)`, `jsr` to the routine
/// every such handler calls, `addq.l #6,sp`, `rts`.  `None` stands for
/// the byte `n` and the four of the routine's address, which vary.
const REFUSAL: [Option<u8>; 18] = [
    Some(0x2f),
    Some(0x2f),
    Some(0x00),
    Some(0x0c),
    Some(0x1f),
    Some(0x3c),
    Some(0x00),
    None,
    Some(0x4e),
    Some(0xb9),
    None,
    None,
    None,
    None,
    Some(0x5c),
    Some(0x8f),
    Some(0x4e),
    Some(0x75),
];

/// Where in a `REFUSAL` stub the routine's address is.
const REFUSAL_TARGET: u32 = 10;
/// `rts`.
const RTS: u16 = 0x4e75;
/// `jsr` to an absolute long address.
const JSR_ABSOLUTE: u16 = 0x4eb9;
/// `jsr` relative to the program counter.
const JSR_RELATIVE: u16 = 0x4eba;
/// How far `calls` looks for the end of a routine.
const CALLS_LIMIT: u32 = 64;

/// A keyword the common-command list holds and the main tree does not,
/// which tells the two root lists apart.
const COMMON_MARKER: &str = "IDN";

/// What the SCPI walk reads from an image beyond words and longs.
impl Image<'_> {
    /// The routine the handler at `at` passes its refusal to, if the
    /// handler is the `REFUSAL` stub.
    fn refusal_target(&self, at: u32) -> Option<u32> {
        let start = usize::try_from(at).ok()?;
        let code = self.0.get(start..start + REFUSAL.len())?;
        code.iter()
            .zip(REFUSAL)
            .all(|(byte, wanted)| wanted.is_none_or(|wanted| *byte == wanted))
            .then(|| self.u32(at + REFUSAL_TARGET))
            .flatten()
    }

    /// The routines the code at `at` calls, in order, up to its first
    /// `rts`, and how many bytes that is; `None` if no `rts` comes
    /// within `CALLS_LIMIT` bytes.  Calls are `jsr` to an absolute long
    /// address or relative to the program counter.
    fn calls(&self, at: u32) -> Option<(Vec<u32>, u32)> {
        let mut calls = Vec::new();
        let mut offset = 0;
        while offset < CALLS_LIMIT {
            match self.u16(at + offset)? {
                RTS => return Some((calls, offset + 2)),
                JSR_ABSOLUTE => {
                    calls.push(self.u32(at + offset + 2)?);
                    offset += 6;
                }
                JSR_RELATIVE => {
                    let base = at + offset + 2;
                    let displacement = i16::from_be_bytes(self.u16(base)?.to_be_bytes());
                    calls.push(base.checked_add_signed(i32::from(displacement))?);
                    offset += 4;
                }
                _ => offset += 2,
            }
        }
        None
    }

    /// The keyword spelled by the pair at `at`: the short form, a NUL,
    /// the rest of the long form and a NUL, both upper case, digits or
    /// `KEYWORD_PUNCTUATION`.  The rest comes back lower case, as the
    /// manuals write it.
    fn keyword(&self, at: u32) -> Option<String> {
        let half = |from: usize, least: usize| {
            let bytes = self.0.get(from..)?;
            let length = bytes
                .iter()
                .take(KEYWORD_LIMIT + 1)
                .position(|&byte| byte == 0)?;
            let text = &bytes[..length];
            (length >= least
                && text.iter().all(|&byte| {
                    byte.is_ascii_uppercase()
                        || byte.is_ascii_digit()
                        || byte == KEYWORD_PUNCTUATION
                }))
            .then(|| String::from_utf8_lossy(text).into_owned())
        };
        let at = usize::try_from(at).ok()?;
        let short = half(at, 1)?;
        let rest = half(at + short.len() + 1, 0)?;
        Some(short + &rest.to_ascii_lowercase())
    }

    /// The node at `at`, if its keyword slot points at a keyword pair.
    fn node(&self, at: u32) -> Option<Node> {
        let keyword = self.keyword(self.u32(at + KEYWORD_SLOT)?)?;
        Some(Node {
            keyword,
            children: self.u32(at + CHILDREN_SLOT)?,
            setter: self.u32(at + SETTER_SLOT)?,
            query: self.u32(at + QUERY_SLOT)?,
        })
    }

    /// The nodes the child list at `at` points to, if it is one: a
    /// count in range and that many pointers, each to a node.
    fn children(&self, at: u32) -> Option<Vec<(u32, Node)>> {
        let count = self.u16(at.checked_add(COUNT_SLOT)?)?;
        if count == 0 || count >= CHILDREN_LIMIT {
            return None;
        }
        (0..u32::from(count))
            .map(|index| {
                let pointer = self.u32(at + CHILDREN_START + 4 * index)?;
                Some((pointer, self.node(pointer)?))
            })
            .collect()
    }
}

/// One node of the tree, as its record holds it.
struct Node {
    /// Its keyword, long form, with the short form upper case.
    keyword: String,
    /// Its child list, zero if it is a leaf.
    children: u32,
    /// Its setter, zero if it has none.
    setter: u32,
    /// Its query handler, zero if it has none.
    query: u32,
}

/// The two root lists: the common commands, `*IDN?` and its kind, and
/// the main tree.  The parser keeps a pointer to each, side by side.
fn roots(image: &Image) -> Result<(u32, u32)> {
    let common = |list: u32| {
        image.children(list).is_some_and(|children| {
            children
                .iter()
                .any(|(_, node)| node.keyword == COMMON_MARKER)
        })
    };
    let found: Vec<(u32, u32)> = (0..)
        .step_by(2)
        .map_while(|at| Some((at, image.u32(at)?, image.u32(at + 4)?)))
        .filter(|&(_, first, second)| {
            common(first) && !common(second) && image.children(second).is_some()
        })
        .map(|(_, first, second)| (first, second))
        .collect();
    match found.as_slice() {
        [one] => Ok(*one),
        [] => bail!("no pair of root lists found: not a SmartClock image, or not one this reads"),
        many => bail!(
            "{} candidate pairs of root lists found; refusing to guess",
            many.len()
        ),
    }
}

/// One path of the tree, and the node it reaches.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Entry {
    /// The path, `*` or `:` first, keywords long form.
    pub(crate) path: String,
    /// Where the node is in the image; a node under a list several
    /// parents share is reached by several paths.
    pub(crate) node: u32,
    /// The node's query handler, if it has one.
    pub(crate) query: Option<u32>,
    /// The node's setter, if it has one.
    pub(crate) setter: Option<u32>,
    /// Whether it has a handler and every one it has only refuses: a
    /// path the parser knows and the firmware does not implement.
    pub(crate) refuses: bool,
}

impl Entry {
    /// The path with `?` where the node has a query handler and
    /// ` (set)` where it has a setter.
    fn line(&self) -> String {
        format!(
            "{}{}{}",
            self.path,
            if self.query.is_some() { "?" } else { "" },
            if self.setter.is_some() { " (set)" } else { "" }
        )
    }
}

/// Every path the tree under `list` holds, prefixed by `prefix`.
/// `above` is the lists the walk is inside, to refuse a cycle.
fn walk(
    image: &Image,
    list: u32,
    prefix: &str,
    above: &mut Vec<u32>,
    entries: &mut Vec<Entry>,
) -> Result<()> {
    ensure!(
        !above.contains(&list),
        "the tree loops back to the child list at {list:#x}"
    );
    let children = image
        .children(list)
        .with_context(|| format!("no child list at {list:#x}"))?;
    above.push(list);
    for (at, node) in children {
        let path = format!("{prefix}{}", node.keyword);
        for (slot, handler) in [("setter", node.setter), ("query", node.query)] {
            ensure!(
                handler == 0 || (handler % 2 == 0 && image.holds(handler)),
                "{path} (node at {at:#x}) has {slot} {handler:#x}, neither zero nor code"
            );
        }
        if node.children != 0 {
            walk(image, node.children, &format!("{path}:"), above, entries)?;
        }
        entries.push(Entry {
            path,
            node: at,
            query: (node.query != 0).then_some(node.query),
            setter: (node.setter != 0).then_some(node.setter),
            refuses: false,
        });
    }
    above.pop();
    Ok(())
}

/// Mark each entry whose every handler only refuses.
///
/// The image's refusal routine is the one its `REFUSAL` stubs call, if
/// they all call one.  A handler refuses if it is such a stub, or if it
/// is no longer than the routine and makes exactly the routine's calls
/// in the same order: the routine written out in place, as some
/// setters are.
fn mark_refusals(image: &Image, entries: &mut [Entry]) {
    let handlers = || {
        entries
            .iter()
            .flat_map(|entry| [entry.query, entry.setter])
            .flatten()
    };
    let targets: BTreeSet<u32> = handlers()
        .filter_map(|handler| image.refusal_target(handler))
        .collect();
    let routine = match targets.into_iter().collect::<Vec<_>>().as_slice() {
        [one] => image.calls(*one),
        _ => None,
    };
    let refuses = |handler: u32| {
        image.refusal_target(handler).is_some()
            || routine.as_ref().is_some_and(|(calls, length)| {
                image
                    .calls(handler)
                    .is_some_and(|(made, made_length)| made == *calls && made_length <= *length)
            })
    };
    for entry in entries {
        entry.refuses = (entry.query.is_some() || entry.setter.is_some())
            && [entry.query, entry.setter]
                .into_iter()
                .flatten()
                .all(refuses);
    }
}

/// Every path the image's parser knows, the common commands as `*`
/// and the rest as `:`, ordered as a case-blind sort of the paths.
fn entries(bytes: &[u8]) -> Result<Vec<Entry>> {
    let image = Image(bytes);
    let (common, main) = roots(&image)?;
    let mut entries = Vec::new();
    walk(&image, common, "*", &mut Vec::new(), &mut entries)?;
    walk(&image, main, ":", &mut Vec::new(), &mut entries)?;
    mark_refusals(&image, &mut entries);
    entries.sort_by_cached_key(|entry| entry.path.to_ascii_lowercase());
    entries.dedup_by(|later, earlier| later.path == earlier.path);
    Ok(entries)
}

/// Print one image's paths, one a line; with `models`, the trees of
/// all of them side by side, as Markdown or with `html` as a page.
pub(crate) fn run(images: &[PathBuf], models: bool, html: bool) -> Result<()> {
    if models {
        let trees = image::load_all(images, entries)?;
        if html {
            print!("{}", models::html(&trees));
        } else {
            print!("{}", models::markdown(&trees));
        }
        return Ok(());
    }
    let [image] = images else {
        bail!("give one image, or --models and any number");
    };
    for entry in image::load(image, entries)? {
        println!("{}", entry.line());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use anyhow::Result;

    use super::Entry;
    use super::entries;

    /// The tree as `dump-scpi` prints it, a line a path.
    fn dump(bytes: &[u8]) -> Result<Vec<String>> {
        Ok(entries(bytes)?.iter().map(Entry::line).collect())
    }

    /// Each image in `third_party/firmware/`, by the name its tree has
    /// in `docs/scpi/`.
    const IMAGES: [&str; 7] = [
        "58503a-3633",
        "58503a-3704",
        "58503b-1.01.04",
        "z3801a-3543",
        "z3805a-3543b",
        "z3815a-4010",
        "z3816a-4001",
    ];

    #[test]
    fn every_image_gives_the_tree_its_document_holds() {
        let root = format!("{}/../..", env!("CARGO_MANIFEST_DIR"));
        for name in IMAGES {
            let image = std::fs::read(format!("{root}/third_party/firmware/{name}.bin"))
                .expect("the image is in the repository");
            let held = std::fs::read_to_string(format!("{root}/docs/scpi/{name}.txt"))
                .expect("the tree is in the repository");
            let read = dump(&image).expect("the image has a tree");
            assert_eq!(read, held.lines().collect::<Vec<_>>(), "{name}");
        }
    }

    #[test]
    fn the_models_document_is_what_the_images_give() {
        let root = format!("{}/../..", env!("CARGO_MANIFEST_DIR"));
        let trees: Vec<(String, Vec<Entry>)> = IMAGES
            .iter()
            .map(|name| {
                let image = std::fs::read(format!("{root}/third_party/firmware/{name}.bin"))
                    .expect("the image is in the repository");
                (
                    (*name).to_owned(),
                    entries(&image).expect("the image has a tree"),
                )
            })
            .collect();
        let held = std::fs::read_to_string(format!("{root}/docs/scpi/models.md"))
            .expect("the document is in the repository");
        assert_eq!(
            super::models::markdown(&trees).lines().collect::<Vec<_>>(),
            held.lines().collect::<Vec<_>>(),
            "docs/scpi/models.md is stale; run make docs"
        );
        let page = std::fs::read_to_string(format!("{root}/docs/scpi/models.html"))
            .expect("the page is in the repository");
        assert_eq!(
            super::models::html(&trees).lines().collect::<Vec<_>>(),
            page.lines().collect::<Vec<_>>(),
            "docs/scpi/models.html is stale; run make docs"
        );
    }

    #[test]
    fn every_path_the_undocumented_commands_name_is_in_a_tree() {
        let root = format!("{}/../..", env!("CARGO_MANIFEST_DIR"));
        let trees: Vec<Vec<Entry>> = IMAGES
            .iter()
            .map(|name| {
                let image = std::fs::read(format!("{root}/third_party/firmware/{name}.bin"))
                    .expect("the image is in the repository");
                entries(&image).expect("the image has a tree")
            })
            .collect();
        let held = std::fs::read_to_string(format!("{root}/docs/scpi/undocumented.md"))
            .expect("the document is in the repository");
        let headings: Vec<&str> = held
            .lines()
            .filter_map(|line| line.strip_prefix("### `"))
            .filter_map(|rest| rest.split('`').next())
            .collect();
        assert!(!headings.is_empty(), "no path headings found");
        for heading in headings {
            assert!(
                trees
                    .iter()
                    .flatten()
                    .any(|entry| super::models::spells(heading, &entry.path)),
                "{heading} is in no image's tree"
            );
        }
    }

    /// A minimal image: a root pair at 0 naming a common list of `IDN`
    /// and `_TRG` and a main list of `SYSTem`, each node with a query
    /// handler.
    fn tiny_image() -> Vec<u8> {
        const KEYWORDS: u32 = 0x100;
        const NODES: u32 = 0x200;
        const NODE_SIZE: u32 = 0x20;
        const COMMON: u32 = 0x300;
        const MAIN: u32 = 0x320;
        const HANDLER: u32 = 0x400;
        let mut image = vec![0u8; 0x1000];
        let mut put = |at: u32, bytes: &[u8]| {
            let at = usize::try_from(at).expect("in range");
            image[at..at + bytes.len()].copy_from_slice(bytes);
        };
        put(0, &COMMON.to_be_bytes());
        put(4, &MAIN.to_be_bytes());
        let pairs: [&[u8]; 3] = [b"IDN\0\0", b"_TRG\0\0", b"SYST\0EM\0"];
        let mut keyword = KEYWORDS;
        for (index, pair) in (0..).zip(pairs) {
            let node = NODES + index * NODE_SIZE;
            put(keyword, pair);
            put(node, &keyword.to_be_bytes());
            put(node + 18, &HANDLER.to_be_bytes());
            keyword += 0x10;
        }
        for (list, nodes) in [(COMMON, &[0, 1][..]), (MAIN, &[2][..])] {
            put(
                list + 2,
                &u16::try_from(nodes.len()).expect("small").to_be_bytes(),
            );
            put(list + 4, &[0xff, 0xff]);
            for (slot, node) in (0..).zip(nodes) {
                put(
                    list + 6 + 4 * slot,
                    &(NODES + node * NODE_SIZE).to_be_bytes(),
                );
            }
        }
        image
    }

    #[test]
    fn a_keyword_may_hold_an_underscore() {
        assert_eq!(
            dump(&tiny_image()).expect("the image has a tree"),
            ["*_TRG?", "*IDN?", ":SYSTem?"]
        );
    }

    #[test]
    fn an_image_without_a_tree_is_refused() {
        assert!(dump(&[0; 0x1000]).is_err());
    }
}
