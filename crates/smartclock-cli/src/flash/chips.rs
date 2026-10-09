//! `join-chips` and `split-chips`: a flash image from, or into, four
//! chip dumps.
//!
//! The chips are in word-interleaved pairs, the M part holding the
//! even bytes and the L part the odd ones; pair 1 is the low half of
//! the image and pair 2 the high (docs/firmware/restart.md, "The
//! installer").  That is all either command does to the bytes, so it
//! does it whatever they hold, and then says what the result looks
//! like: whose reset vector it starts with, which boot checksum holds,
//! and which chips look swapped if neither does.  Nothing it finds is
//! a reason to refuse: these are files to look at, and `flash`, which
//! programs a receiver, makes its own checks.
//!
//! Pairs swapped fail the boot checksums, which cover different ranges
//! of each half.  Chips swapped within a pair keep the lane sums, since
//! the stored sums swap lanes with the bytes they cover; the reset
//! vector tells that case, read byte-swapped.

use std::path::Path;
use std::path::PathBuf;

use anyhow::Context as _;
use anyhow::Result;
use anyhow::ensure;
use sha2::Digest;
use sha2::Sha256;

use crate::flash::firmware::IMAGE_SIZE;
use crate::flash::firmware::Layout;
use crate::flash::firmware::PROFILES;

/// One AM29F010 holds a quarter of the image.
const CHIP_SIZE: usize = IMAGE_SIZE / 4;

/// The CPU32 reset vectors these receivers start with: the stack at
/// the top of the 64 KB of RAM at `0x100000`, and entry at `0x550` on a
/// 58503A, Z3801A or Z3805A, at `0x400` on a 58503B, Z3815A or Z3816A
/// (docs/firmware/restart.md, "The installer").
const RESET_VECTORS: [[u8; 8]; 2] = [
    [0x00, 0x10, 0xff, 0xfe, 0x00, 0x00, 0x05, 0x50],
    [0x00, 0x10, 0xff, 0xfe, 0x00, 0x00, 0x04, 0x00],
];

/// The boot checksums, each with the receivers whose boot code checks
/// it (docs/firmware/restart.md, "Boot check").
const CHECKSUMS: [(Layout, &str); 2] = [
    (
        Layout::AmdLanes,
        "the lane sums of a 58503A, Z3801A or Z3805A",
    ),
    (
        Layout::IntelWords,
        "the word sum of a 58503B, Z3815A or Z3816A",
    ),
];

/// Images known by their SHA-256 that are not audited for flashing:
/// named when joined or split, never programmed.  Model, revision and
/// hash, as `third_party/NOTICE` records them.
const KNOWN: [(&str, &str, &str); 2] = [
    (
        "58503B",
        "1.01.04",
        "2ea754e9d5a8f1990a586aa43f78391ab89cc8a88ae9152a91d66f5dac521dd4",
    ),
    (
        "Z3815A",
        "U-4010.0",
        "5aa9083d850128eefc2a1b9282e89f48c82c63c7abbe362862f8d88d70968e80",
    ),
];

/// The four chip files, by the position printed on their labels.
#[derive(Debug, clap::Args)]
pub(crate) struct ChipPaths {
    /// Chip 1L (U12 on a 58503A): odd bytes of the low half.
    #[arg(long = "1l", value_name = "FILE")]
    chip_1l: PathBuf,
    /// Chip 1M (U14): even bytes of the low half.
    #[arg(long = "1m", value_name = "FILE")]
    chip_1m: PathBuf,
    /// Chip 2L (U11): odd bytes of the high half.
    #[arg(long = "2l", value_name = "FILE")]
    chip_2l: PathBuf,
    /// Chip 2M (U13): even bytes of the high half.
    #[arg(long = "2m", value_name = "FILE")]
    chip_2m: PathBuf,
}

impl ChipPaths {
    /// The paths in the order `Chips` holds them.
    fn each(&self) -> [(&'static str, &Path); 4] {
        [
            ("1L", &self.chip_1l),
            ("1M", &self.chip_1m),
            ("2L", &self.chip_2l),
            ("2M", &self.chip_2m),
        ]
    }
}

/// `join-chips`'s arguments: the image to write and the dumps to read.
#[derive(Debug, clap::Args)]
pub(crate) struct JoinArgs {
    /// Where to write the 512 KiB image.
    image: PathBuf,
    #[command(flatten)]
    chips: ChipPaths,
}

/// `split-chips`'s arguments: the image to read and the dumps to write.
#[derive(Debug, clap::Args)]
pub(crate) struct SplitArgs {
    /// A full 512 KiB image, as `read-flash` writes or `join-chips`
    /// makes.
    image: PathBuf,
    #[command(flatten)]
    chips: ChipPaths,
}

/// The contents of the four chips, in label order: 1L, 1M, 2L, 2M.
pub(super) type Chips = [Vec<u8>; 4];

/// The image the four dumps make, with the M part of each pair on the
/// even bytes and pair 1 first.  Each must be a whole chip.
pub(super) fn join(chips: &Chips) -> Result<Vec<u8>> {
    for (name, chip) in ["1L", "1M", "2L", "2M"].into_iter().zip(chips) {
        ensure!(
            chip.len() == CHIP_SIZE,
            "chip {name} is {} bytes, not the {CHIP_SIZE} of an AM29F010",
            chip.len()
        );
    }
    let [chip_1l, chip_1m, chip_2l, chip_2m] = chips;
    let mut image = vec![0; IMAGE_SIZE];
    for (bank, (even, odd)) in [(chip_1m, chip_1l), (chip_2m, chip_2l)]
        .into_iter()
        .enumerate()
    {
        let half = &mut image[bank * 2 * CHIP_SIZE..][..2 * CHIP_SIZE];
        for (word, (&m, &l)) in half
            .as_chunks_mut::<2>()
            .0
            .iter_mut()
            .zip(even.iter().zip(odd))
        {
            *word = [m, l];
        }
    }
    Ok(image)
}

/// The four dumps an image splits into, the inverse of `join`.
pub(super) fn split(image: &[u8]) -> Result<Chips> {
    ensure!(
        image.len() == IMAGE_SIZE,
        "image is {} bytes, not the {IMAGE_SIZE} of a whole flash",
        image.len()
    );
    let lane = |half: &[u8], first: usize| half.iter().skip(first).step_by(2).copied().collect();
    let (low, high) = image.split_at(IMAGE_SIZE / 2);
    Ok([lane(low, 1), lane(low, 0), lane(high, 1), lane(high, 0)])
}

/// What an image's bytes say about how it was put together: a line
/// each, and whether all of them are as a receiver boots.
#[derive(Debug)]
pub(super) struct Findings {
    /// What was found, for the reader.
    pub(super) lines: Vec<String>,
    /// Whether it starts with a reset vector and sums as one of these
    /// receivers boots.
    pub(super) sound: bool,
}

/// Look at an image as these receivers' boot code would.
pub(super) fn examine(image: &[u8]) -> Findings {
    let mut lines = Vec::new();
    let vector = &image[..8];
    let swapped: Vec<u8> = vector
        .chunks(2)
        .flat_map(|word| [word[1], word[0]])
        .collect();
    let started = RESET_VECTORS.iter().any(|known| known == vector);
    lines.push(if started {
        "reset vector: as these receivers start".to_owned()
    } else if RESET_VECTORS.iter().any(|known| known.as_slice() == swapped) {
        format!("reset vector {vector:02x?} is byte-swapped: the L and M chips of pair 1 look swapped")
    } else {
        format!(
            "reset vector {vector:02x?} is none of these receivers': pair 1 may not be the low half, \
             or a dump is bad"
        )
    });
    let holds = |bytes: &[u8]| {
        CHECKSUMS
            .iter()
            .find(|(layout, _)| layout.verify(bytes).is_ok())
    };
    let summed = holds(image);
    lines.push(match summed {
        Some((_, which)) => format!("boot checksum: {which} holds"),
        None => {
            let (low, high) = image.split_at(IMAGE_SIZE / 2);
            if holds(&[high, low].concat()).is_some() {
                "no boot checksum holds as joined, but one does with the halves exchanged: \
                 pairs 1 and 2 look swapped"
                    .to_owned()
            } else {
                "no boot checksum holds: the chips may be out of order, or a dump is bad".to_owned()
            }
        }
    });
    Findings {
        lines,
        sound: started && summed.is_some(),
    }
}

/// Print what was found, a fault to standard error.
fn report(findings: &Findings) {
    for line in &findings.lines {
        if findings.sound {
            println!("{line}");
        } else {
            eprintln!("{line}");
        }
    }
}

/// Say which image this is, if it is known by its hash, and whether it
/// is audited for flashing.
fn identify(image: &[u8]) -> String {
    let hash: String = Sha256::digest(image)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    let audited = PROFILES
        .iter()
        .find(|profile| profile.sha256 == hash)
        .map(|profile| format!("{} {}", profile.model, profile.revision));
    let known = KNOWN
        .iter()
        .find(|(_, _, sha256)| *sha256 == hash)
        .map(|(model, revision, _)| format!("{model} {revision}, not audited for flashing"));
    let named = audited
        .or(known)
        .unwrap_or_else(|| "not a known image".to_owned());
    format!("Image: {named}; SHA-256 {hash}")
}

fn read(path: &Path) -> Result<Vec<u8>> {
    std::fs::read(path).with_context(|| format!("reading {}", path.display()))
}

fn write(path: &Path, bytes: &[u8]) -> Result<()> {
    std::fs::write(path, bytes).with_context(|| format!("writing {}", path.display()))
}

/// Read the four dumps, write the image, and say what it is and how it
/// looks.
pub(crate) fn run_join(args: &JoinArgs) -> Result<()> {
    let chips: Chips = args
        .chips
        .each()
        .map(|(_, path)| read(path))
        .into_iter()
        .collect::<Result<Vec<_>>>()?
        .try_into()
        .expect("four paths read four chips");
    let image = join(&chips)?;
    write(&args.image, &image)?;
    println!("{}; written to {}", identify(&image), args.image.display());
    report(&examine(&image));
    Ok(())
}

/// Read an image, write the four dumps, and say what the image is and
/// how it looks.
pub(crate) fn run_split(args: &SplitArgs) -> Result<()> {
    let image = read(&args.image)?;
    let chips = split(&image)?;
    println!("{}", identify(&image));
    report(&examine(&image));
    for ((name, path), chip) in args.chips.each().into_iter().zip(&chips) {
        write(path, chip)?;
        println!(
            "Chip {name}: {} bytes written to {}",
            chip.len(),
            path.display()
        );
    }
    Ok(())
}
