//! `dump-scpi`: every command path a firmware image's SCPI parser
//! knows, read out of its own tables (docs/scpi/README.md).
//!
//! Pointers in the image are absolute and it is not relocated, so a
//! stored pointer is a file offset as it stands.  Nothing here knows an
//! address: the tables are found by their shape, and an image whose
//! tables are not found exactly once is refused rather than guessed at.

use std::path::Path;
use std::path::PathBuf;

use anyhow::Context as _;
use anyhow::Result;
use anyhow::bail;
use anyhow::ensure;

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

/// The longest half of a keyword pair in any image.
const KEYWORD_LIMIT: usize = 12;

/// A keyword the common-command list holds and the main tree does not,
/// which tells the two root lists apart.
const COMMON_MARKER: &str = "IDN";

/// A firmware image, read as the CPU32 reads it: big-endian.
struct Image<'a>(&'a [u8]);

impl Image<'_> {
    fn u16(&self, at: u32) -> Option<u16> {
        let at = usize::try_from(at).ok()?;
        Some(u16::from_be_bytes(self.0.get(at..at + 2)?.try_into().ok()?))
    }

    fn u32(&self, at: u32) -> Option<u32> {
        let at = usize::try_from(at).ok()?;
        Some(u32::from_be_bytes(self.0.get(at..at + 4)?.try_into().ok()?))
    }

    /// Whether `at` is inside the image.
    fn holds(&self, at: u32) -> bool {
        usize::try_from(at).is_ok_and(|at| at < self.0.len())
    }

    /// The keyword spelled by the pair at `at`: the short form, a NUL,
    /// the rest of the long form and a NUL, both upper case.  The rest
    /// comes back lower case, as the manuals write it.
    fn keyword(&self, at: u32) -> Option<String> {
        let half = |from: usize, least: usize| {
            let bytes = self.0.get(from..)?;
            let length = bytes
                .iter()
                .take(KEYWORD_LIMIT + 1)
                .position(|&byte| byte == 0)?;
            let text = &bytes[..length];
            (length >= least
                && text
                    .iter()
                    .all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit()))
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
    /// Whether the node has a query handler.
    pub(crate) query: bool,
    /// Whether the node has a setter.
    pub(crate) setter: bool,
}

impl Entry {
    /// The path with `?` where the node has a query handler and
    /// ` (set)` where it has a setter.
    fn line(&self) -> String {
        format!(
            "{}{}{}",
            self.path,
            if self.query { "?" } else { "" },
            if self.setter { " (set)" } else { "" }
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
            query: node.query != 0,
            setter: node.setter != 0,
        });
    }
    above.pop();
    Ok(())
}

/// Every path the image's parser knows, the common commands as `*`
/// and the rest as `:`, ordered as a case-blind sort of the paths.
fn entries(bytes: &[u8]) -> Result<Vec<Entry>> {
    let image = Image(bytes);
    let (common, main) = roots(&image)?;
    let mut entries = Vec::new();
    walk(&image, common, "*", &mut Vec::new(), &mut entries)?;
    walk(&image, main, ":", &mut Vec::new(), &mut entries)?;
    entries.sort_by_cached_key(|entry| entry.path.to_ascii_lowercase());
    entries.dedup_by(|later, earlier| later.path == earlier.path);
    Ok(entries)
}

/// An image read from `path`, with its tree.
fn read(path: &Path) -> Result<Vec<Entry>> {
    let bytes = std::fs::read(path).with_context(|| format!("reading {}", path.display()))?;
    entries(&bytes).with_context(|| format!("reading {}", path.display()))
}

/// Print one image's paths, one a line; with `models`, the trees of
/// all of them side by side as Markdown instead.
pub(crate) fn run(images: &[PathBuf], models: bool) -> Result<()> {
    if models {
        let trees = images
            .iter()
            .map(|path| Ok((name(path)?, read(path)?)))
            .collect::<Result<Vec<_>>>()?;
        print!("{}", models::markdown(&trees));
        return Ok(());
    }
    let [image] = images else {
        bail!("give one image, or --models and any number");
    };
    for entry in read(image)? {
        println!("{}", entry.line());
    }
    Ok(())
}

/// An image's name: its file name without the extension.
fn name(path: &Path) -> Result<String> {
    path.file_stem()
        .and_then(|stem| stem.to_str())
        .map(str::to_owned)
        .with_context(|| format!("{} has no usable name", path.display()))
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
    }

    #[test]
    fn an_image_without_a_tree_is_refused() {
        assert!(dump(&[0; 0x1000]).is_err());
    }
}
