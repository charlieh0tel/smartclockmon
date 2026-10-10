//! `dump-oncore`: the Motorola Oncore messages a firmware image knows,
//! and the scripts and polling lists that send them, read out of its
//! tables (docs/firmware/oncore/README.md).
//!
//! The message table is found from the strings of two IDs every table
//! holds; the scripts from the two jump tables the GPS task uses to
//! dispatch a request past the messages; the polling lists by shape.
//! An image where the table or the scripts are not found is refused.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::PathBuf;

use anyhow::Context as _;
use anyhow::Result;
use anyhow::bail;
use anyhow::ensure;

use crate::image;
use crate::image::Image;

/// The IDs, each NUL-terminated with a NUL before it, of two
/// consecutive messages every table holds, to find the table and its
/// entry size by.
const ANCHORS: [&[u8]; 2] = [b"\0Ab\0", b"\0Ac\0"];
/// The entry sizes tables have: without and with a send filter.
const ENTRY_SIZES: [u32; 2] = [56, 60];

/// Where an entry keeps the pointer to its ID, zero for an empty slot.
const ID_SLOT: u32 = 0;
/// Where it keeps its kind (`Kind`).
const KIND_SLOT: u32 = 4;
/// Where it keeps its argument encoders, zero after the last.
const ENCODER_SLOT: u32 = 6;
/// How many encoders an entry has room for.
const ENCODERS: u32 = 4;
/// Where it keeps the pointer to its query form, zero if it has none.
const QUERY_SLOT: u32 = 0x16;
/// Where it keeps the length of the message the engine sends.
const LENGTH_SLOT: u32 = 0x1c;
/// Where its decoders start: pairs of a decoder and the record it
/// fills.
const DECODER_SLOT: u32 = 0x20;
/// How many decoder pairs an entry has room for.
const DECODERS: u32 = 3;
/// Where an entry of `ENTRY_SIZES[1]` bytes keeps its send filter.
const FILTER_SLOT: u32 = 0x38;

/// The dispatch every jump table here starts with: `suba.l #first,a0`,
/// `cmpa.l #count-1,a0`, a `bhi.w`, then `movea.w (d8,pc,a0.w),a0`.
/// `None` stands for the byte that varies.
const DISPATCH: [Option<u8>; 18] = [
    Some(0x91),
    Some(0xfc),
    Some(0),
    Some(0),
    Some(0),
    None,
    Some(0xb1),
    Some(0xfc),
    Some(0),
    Some(0),
    Some(0),
    None,
    Some(0x62),
    Some(0),
    None,
    None,
    Some(0x30),
    Some(0x7b),
];
/// Where in `DISPATCH` the first index is.
const DISPATCH_FIRST: usize = 5;
/// Where the index count, less one, is.
const DISPATCH_LAST: usize = 11;
/// Where the `movea.w`'s extension word starts, past the pattern.
const DISPATCH_EXTENSION: u32 = 18;
/// `movea.l #script,a1`, which each script's jump target holds.
const SCRIPT_LOAD: [u8; 2] = [0x22, 0x7c];
/// How far past its jump target a script's load may be.
const SCRIPT_REACH: u32 = 16;

/// Where a script or list record keeps its index.  A record is the
/// index, a mode, a byte and the arguments.
const RECORD_INDEX: u32 = 0;
/// Where a record keeps its mode (`Mode`).
const RECORD_MODE: u32 = 2;
/// Where a record's arguments start, a long for each of its message's
/// encoders: encoder *k* sends argument *k*.
const RECORD_ARGUMENTS: u32 = 4;
/// The mode that sets a message from the record's own arguments.
const MODE_OWN: Mode = 0;
/// The largest mode a record holds.
const MODE_LIMIT: u8 = 2;

/// The opcodes that load a long address a polling list is found by:
/// `pea` absolute, `movea.l` or `move.l` immediate, `lea` absolute.
const LIST_LOADS: [(u8, u8, u8); 3] = [(0x48, 0x48, 0x79), (0x20, 0x2f, 0x7c), (0x41, 0x4f, 0xf9)];
/// The fewest records a polling list holds.
const LIST_LEAST: usize = 2;

/// An entry's kind.  The sender composes only kinds with bit 0 set
/// (58503B 1.01.04 `FUN_000584c0`); what bit 1 means was not traced.
type Kind = u8;

/// What a record asks of the GPS task: 0 set, from the record's own
/// arguments; 1 query; 2 set, from the request's arguments.
type Mode = u8;

/// One entry of the message table.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Message {
    /// Its index, as requests name it.
    index: u16,
    /// Its two-letter ID, after `@@`.
    id: String,
    /// Its kind.
    kind: Kind,
    /// How many argument encoders it has.
    encoders: u32,
    /// Whether it has a query form.
    query: bool,
    /// The length of the message the engine sends, `@@` to line end.
    length: u16,
    /// Its decoders, each with the record it fills.
    decoders: Vec<(u32, u32)>,
    /// Its send filter, if it has one.
    filter: Option<u32>,
}

/// A summary row of the models table: its label and how an image's
/// value is shown.
type Summary = (&'static str, fn(&Oncore) -> String);

/// One record of a script or polling list.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Record {
    /// A message's index, or a step's.
    index: u16,
    /// What it asks.
    mode: Mode,
    /// The arguments it sets its message from, in mode 0; empty
    /// otherwise, and for a step, whose arguments are not read here.
    arguments: Vec<i32>,
}

/// Everything read out of one image.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Oncore {
    /// Where the message table is.
    table: u32,
    /// Its entry size.
    entry_size: u32,
    /// Its entries that hold a message, by index.
    messages: Vec<Message>,
    /// The first step index: one past the table's last entry.
    first_step: u16,
    /// The first script index: one past the last step.
    first_script: u16,
    /// Each script: its index, where it is, its records.
    scripts: Vec<(u16, u32, Vec<Record>)>,
    /// Each polling list: where it is, its records.
    lists: Vec<(u32, Vec<Record>)>,
}

impl Oncore {
    /// What a record's index names: a message's ID, or a step.
    fn name(&self, index: u16) -> String {
        self.messages
            .iter()
            .find(|message| message.index == index)
            .map_or_else(
                || format!("step-{index:#04x}"),
                |message| message.id.clone(),
            )
    }

    /// Records as a line shows them: name, a slash and the mode, and
    /// any arguments in parentheses.
    fn records(&self, records: &[Record]) -> String {
        records
            .iter()
            .map(|record| {
                let mut out = format!("{}/{}", self.name(record.index), record.mode);
                if !record.arguments.is_empty() {
                    let arguments: Vec<String> =
                        record.arguments.iter().map(i32::to_string).collect();
                    let _ = write!(out, "({})", arguments.join(","));
                }
                out
            })
            .collect::<Vec<_>>()
            .join(" ")
    }
}

/// The message table: where it starts and its entry size.
fn table(image: &Image) -> Result<(u32, u32)> {
    let pointers = |anchor: &[u8]| -> Vec<u32> {
        image
            .find(anchor)
            .into_iter()
            .flat_map(|at| image.pointers_to(at + 1))
            .collect()
    };
    let [first, second] = ANCHORS.map(pointers);
    let found: Vec<(u32, u32)> = first
        .iter()
        .flat_map(|&b| second.iter().map(move |&c| (b, c)))
        .filter_map(|(b, c)| {
            let size = c.checked_sub(b)?;
            ENTRY_SIZES
                .contains(&size)
                .then(|| Some((b.checked_sub(size)?, size)))?
        })
        .collect();
    match found.as_slice() {
        [one] => Ok(*one),
        [] => bail!("no Oncore message table found: not an image for an Oncore engine"),
        many => bail!(
            "{} candidate message tables found; refusing to guess",
            many.len()
        ),
    }
}

/// The entries of the table at `start`, `count` of them, that hold a
/// message.
fn messages(image: &Image, start: u32, size: u32, count: u16) -> Result<Vec<Message>> {
    let mut out = Vec::new();
    for index in 0..count {
        let at = start + size * u32::from(index);
        let id_at = image.u32(at + ID_SLOT).context("an entry")?;
        if id_at == 0 {
            continue;
        }
        let id = (0..2)
            .map(|offset| image.u8(id_at + offset))
            .collect::<Option<Vec<u8>>>()
            .filter(|bytes| bytes.iter().all(u8::is_ascii_alphabetic))
            .map(|bytes| String::from_utf8_lossy(&bytes).into_owned());
        let Some(id) = id else {
            continue;
        };
        let decoders = (0..DECODERS)
            .filter_map(|pair| {
                let decoder = image.u32(at + DECODER_SLOT + 8 * pair)?;
                let record = image.u32(at + DECODER_SLOT + 8 * pair + 4)?;
                (decoder != 0).then_some((decoder, record))
            })
            .collect();
        let encoders = (0..ENCODERS)
            .take_while(|&slot| {
                image
                    .u32(at + ENCODER_SLOT + 4 * slot)
                    .is_some_and(|e| e != 0)
            })
            .count();
        out.push(Message {
            index,
            id,
            encoders: u32::try_from(encoders)?,
            kind: image.u8(at + KIND_SLOT).context("an entry")?,
            query: image.u32(at + QUERY_SLOT).context("an entry")? != 0,
            length: image.u16(at + LENGTH_SLOT).context("an entry")?,
            decoders,
            filter: if size > FILTER_SLOT {
                Some(image.u32(at + FILTER_SLOT).context("an entry")?).filter(|&f| f != 0)
            } else {
                None
            },
        });
    }
    Ok(out)
}

/// Each dispatch jump table: its first index and its targets.
fn dispatches(image: &Image) -> Vec<(u16, Vec<u32>)> {
    let bytes = image.0;
    (0..bytes.len().saturating_sub(DISPATCH.len()))
        .filter(|at| {
            DISPATCH
                .iter()
                .zip(&bytes[*at..])
                .all(|(want, &byte)| want.is_none_or(|want| want == byte))
        })
        .filter_map(|at| {
            let first = u16::from(bytes[at + DISPATCH_FIRST]);
            let count = u32::from(bytes[at + DISPATCH_LAST]) + 1;
            let at = u32::try_from(at).ok()?;
            let extension = at + DISPATCH_EXTENSION;
            let displacement = i8::from_ne_bytes([image.u8(extension + 1)?]);
            let base = extension.checked_add_signed(i32::from(displacement))?;
            let targets = (0..count)
                .map(|entry| {
                    let offset = i16::from_ne_bytes(image.u16(base + 2 * entry)?.to_ne_bytes());
                    base.checked_add_signed(i32::from(offset))
                })
                .collect::<Option<Vec<u32>>>()?;
            Some((first, targets))
        })
        .collect()
}

/// The script a jump target loads, if it loads one.
fn script_at(image: &Image, target: u32) -> Option<u32> {
    (target..target + SCRIPT_REACH)
        .step_by(2)
        .find(|&at| image.u16(at) == Some(u16::from_be_bytes(SCRIPT_LOAD)))
        .and_then(|at| image.u32(at + 2))
}

/// The record at `at`, if it is one: an index below `limit` and a
/// mode in range.  `encoders` gives each message's encoder count, and
/// so how many arguments a mode-0 record of it holds.
fn record(image: &Image, at: u32, limit: u16, encoders: &BTreeMap<u16, u32>) -> Option<Record> {
    let index = image.u16(at + RECORD_INDEX)?;
    let mode = image.u8(at + RECORD_MODE)?;
    if !at.is_multiple_of(2) || index >= limit || mode > MODE_LIMIT {
        return None;
    }
    let count = if mode == MODE_OWN {
        encoders.get(&index).copied().unwrap_or(0)
    } else {
        0
    };
    let arguments = (0..count)
        .map(|k| {
            let long = image.u32(at + RECORD_ARGUMENTS + 4 * k)?;
            Some(i32::from_ne_bytes(long.to_ne_bytes()))
        })
        .collect::<Option<Vec<i32>>>()?;
    Some(Record {
        index,
        mode,
        arguments,
    })
}

/// The zero-terminated list of record pointers at `at`, if every
/// pointer is to a record.
fn records(
    image: &Image,
    at: u32,
    limit: u16,
    encoders: &BTreeMap<u16, u32>,
) -> Option<Vec<Record>> {
    let mut out = Vec::new();
    let mut slot = at;
    loop {
        let pointer = image.u32(slot)?;
        if pointer == 0 {
            return Some(out);
        }
        out.push(record(image, pointer, limit, encoders)?);
        slot += 4;
    }
}

/// An image read: the message table, the scripts and the polling lists.
fn oncore(bytes: &[u8]) -> Result<Oncore> {
    let image = Image(bytes);
    let (table, entry_size) = table(&image)?;
    let mut scripts = Vec::new();
    let mut steps = Vec::new();
    for (first, targets) in dispatches(&image) {
        let loaded: Option<Vec<u32>> = targets.iter().map(|&t| script_at(&image, t)).collect();
        match loaded {
            Some(loaded) => scripts.push((first, loaded)),
            None => steps.push((first, targets.len())),
        }
    }
    let [(first_script, script_starts)] = scripts.as_slice() else {
        bail!("{} script dispatches found, not one", scripts.len());
    };
    let [(first_step, step_count)] = steps.as_slice() else {
        bail!("{} step dispatches found, not one", steps.len());
    };
    ensure!(
        usize::from(*first_step) + step_count <= usize::from(*first_script),
        "the steps, {first_step:#x} on, run past where the scripts start, {first_script:#x}"
    );
    let limit = first_script
        .checked_add(u16::try_from(script_starts.len())?)
        .context("a script index")?;
    let messages = messages(&image, table, entry_size, *first_step)?;
    let encoders: BTreeMap<u16, u32> = messages
        .iter()
        .map(|message| (message.index, message.encoders))
        .collect();
    let scripts = script_starts
        .iter()
        .zip(*first_script..)
        .map(|(&at, index)| {
            let records = records(&image, at, limit, &encoders).with_context(|| {
                format!("script {index:#x} at {at:#x} is not a list of records")
            })?;
            Ok((index, at, records))
        })
        .collect::<Result<Vec<_>>>()?;
    let mut lists: BTreeMap<u32, Vec<Record>> = BTreeMap::new();
    for at in 0..u32::try_from(bytes.len())?.saturating_sub(6) {
        let (Some(op), Some(mode)) = (image.u8(at), image.u8(at + 1)) else {
            continue;
        };
        if !LIST_LOADS
            .iter()
            .any(|&(low, high, second)| (low..=high).contains(&op) && mode == second)
        {
            continue;
        }
        let Some(list) = image.u32(at + 2) else {
            continue;
        };
        if !list.is_multiple_of(2) || scripts.iter().any(|(_, start, _)| *start == list) {
            continue;
        }
        if let Some(found) =
            records(&image, list, limit, &encoders).filter(|r| r.len() >= LIST_LEAST)
        {
            lists.insert(list, found);
        }
    }
    Ok(Oncore {
        table,
        entry_size,
        messages,
        first_step: *first_step,
        first_script: *first_script,
        scripts,
        lists: lists.into_iter().collect(),
    })
}

/// An image's tables, one item a line.
fn lines(oncore: &Oncore) -> Vec<String> {
    let mut out = vec![format!(
        "table {:#x} entries {} of {} bytes",
        oncore.table, oncore.first_step, oncore.entry_size
    )];
    for message in &oncore.messages {
        let mut line = format!(
            "message {:#04x} {} kind {} length {}",
            message.index, message.id, message.kind, message.length
        );
        if message.query {
            line.push_str(" query");
        }
        for (decoder, record) in &message.decoders {
            let _ = write!(line, " decoder {decoder:#x} {record:#x}");
        }
        if let Some(filter) = message.filter {
            let _ = write!(line, " filter {filter:#x}");
        }
        out.push(line);
    }
    out.push(format!(
        "steps {:#04x} to {:#04x}",
        oncore.first_step,
        oncore.first_script - 1
    ));
    for (index, at, records) in &oncore.scripts {
        out.push(format!(
            "script {index:#04x} {at:#x} {}",
            oncore.records(records)
        ));
    }
    for (at, records) in &oncore.lists {
        out.push(format!("list {at:#x} {}", oncore.records(records)));
    }
    out
}

/// What an image does with a message, as a cell of the models table:
/// `s` a script or list sets it, `q` one queries it, `d` it has a
/// decoder, `-` none of these; blank where its table has no such ID.
fn cell(oncore: &Oncore, id: &str) -> String {
    let indices: Vec<u16> = oncore
        .messages
        .iter()
        .filter(|message| message.id == id)
        .map(|message| message.index)
        .collect();
    if indices.is_empty() {
        return String::new();
    }
    let modes: Vec<Mode> = oncore
        .scripts
        .iter()
        .map(|(_, _, records)| records)
        .chain(oncore.lists.iter().map(|(_, records)| records))
        .flatten()
        .filter(|record| indices.contains(&record.index))
        .map(|record| record.mode)
        .collect();
    let decoded = oncore
        .messages
        .iter()
        .any(|message| message.id == id && !message.decoders.is_empty());
    let mut out = String::new();
    if modes.iter().any(|&mode| mode != 1) {
        out.push('s');
    }
    if modes.contains(&1) {
        out.push('q');
    }
    if decoded {
        out.push('d');
    }
    if out.is_empty() {
        out.push('-');
    }
    out
}

/// The messages of `images`, each named as its file is, side by side
/// as Markdown: a row per ID, a cell per image.
fn markdown(images: &[(String, Oncore)]) -> String {
    let mut out = String::from(
        "# Oncore messages by model\n\n\
         Generated by `smartclock-cli dump-oncore --models` from the images\n\
         in `third_party/firmware/` that talk to an Oncore.  Run `make docs`\n\
         to regenerate; a test fails if this file and the images disagree.\n\
         `README.md` here says how the tables are read; `../gps.md` is the\n\
         link itself.\n\n\
         A cell is `s` where a script or polling list sets the message, `q`\n\
         where one queries it, `d` where the table has a decoder for it, `-`\n\
         where the table holds it and none of these, blank where it does not\n\
         hold it.  Code can also request a message by its index, which this\n\
         does not see; `../gps.md` names the requests found.\n\n",
    );
    let heading = |name: &str| name.to_ascii_uppercase().replacen('-', "<br>", 1);
    let names: Vec<String> = images.iter().map(|(name, _)| heading(name)).collect();
    let _ = writeln!(
        out,
        "| Image | {} |\n|{}",
        names.join(" | "),
        " --- |".repeat(images.len() + 1)
    );
    let rows: [Summary; 4] = [
        ("Table entries", |o| o.first_step.to_string()),
        ("Entry size", |o| o.entry_size.to_string()),
        ("Scripts", |o| o.scripts.len().to_string()),
        ("Polling lists", |o| o.lists.len().to_string()),
    ];
    for (label, value) in rows {
        let values: Vec<String> = images.iter().map(|(_, oncore)| value(oncore)).collect();
        let _ = writeln!(out, "| {label} | {} |", values.join(" | "));
    }
    let mut ids: Vec<(u16, String)> = Vec::new();
    for (_, oncore) in images {
        for message in &oncore.messages {
            if !ids.iter().any(|(_, id)| *id == message.id) {
                ids.push((message.index, message.id.clone()));
            }
        }
    }
    ids.sort();
    let _ = writeln!(
        out,
        "\n| Message | {} |\n|{}",
        names.join(" | "),
        " --- |".repeat(images.len() + 1)
    );
    for (_, id) in ids {
        let cells: Vec<String> = images.iter().map(|(_, oncore)| cell(oncore, &id)).collect();
        let _ = writeln!(out, "| `@@{id}` | {} |", cells.join(" | "));
    }
    out
}

/// Print one image's tables, one item a line; with `models`, the
/// messages of all of them side by side as Markdown instead.
pub(crate) fn run(images: &[PathBuf], models: bool) -> Result<()> {
    if models {
        print!("{}", markdown(&image::load_all(images, oncore)?));
        return Ok(());
    }
    let [image] = images else {
        bail!("give one image, or --models and any number");
    };
    for line in lines(&image::load(image, oncore)?) {
        println!("{line}");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::Oncore;
    use super::lines;
    use super::markdown;
    use super::oncore;

    /// Each image in `third_party/firmware/` for an Oncore engine, by
    /// the name its list has in `docs/firmware/oncore/`.
    const IMAGES: [&str; 6] = [
        "58503a-3633",
        "58503a-3704",
        "58503b-1.01.04",
        "z3801a-3543",
        "z3805a-3543b",
        "z3816a-4001",
    ];

    fn root() -> String {
        format!("{}/../..", env!("CARGO_MANIFEST_DIR"))
    }

    fn bytes(name: &str) -> Vec<u8> {
        std::fs::read(format!("{}/third_party/firmware/{name}.bin", root()))
            .expect("the image is in the repository")
    }

    fn image(name: &str) -> Oncore {
        oncore(&bytes(name)).expect("the image talks to an Oncore")
    }

    #[test]
    fn every_image_gives_the_tables_its_list_holds() {
        for name in IMAGES {
            let held =
                std::fs::read_to_string(format!("{}/docs/firmware/oncore/{name}.txt", root()))
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
        let all: Vec<(String, Oncore)> = IMAGES
            .iter()
            .map(|name| ((*name).to_owned(), image(name)))
            .collect();
        let held = std::fs::read_to_string(format!("{}/docs/firmware/oncore/models.md", root()))
            .expect("the document is in the repository");
        assert_eq!(
            markdown(&all).lines().collect::<Vec<_>>(),
            held.lines().collect::<Vec<_>>(),
            "docs/firmware/oncore/models.md is stale; run make docs"
        );
    }

    #[test]
    fn the_furuno_image_and_an_empty_one_are_refused() {
        assert!(oncore(&bytes("z3815a-4010")).is_err());
        assert!(oncore(&[0; 0x1000]).is_err());
    }
}
