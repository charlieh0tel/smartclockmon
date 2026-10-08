//! `join-chips` and `split-chips`: a flash image from, or into, the
//! four AM29F010 chip dumps of a Z3801A, Z3805A or 58503A.
//!
//! The chips are in word-interleaved pairs, the M part holding the
//! even bytes and the L part the odd ones; pair 1 is the low half of
//! the image and pair 2 the high (docs/firmware/restart.md, "The
//! installer").  Pairs swapped fail the boot code's lane checksums,
//! which cover different ranges of each half.  Chips swapped within a
//! pair do not, since the stored sums swap lanes with the bytes they
//! cover; the reset vector tells that case, read byte-swapped.

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

/// The CPU32 reset vector every AMD-flash image starts with: the
/// stack at the top of the 64 KB of RAM at `0x100000`, and entry at
/// `0x550` (docs/firmware/restart.md, "The installer").
const RESET_VECTOR: [u8; 8] = [0x00, 0x10, 0xff, 0xfe, 0x00, 0x00, 0x05, 0x50];

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

/// Whether the image starts and sums as one of these receivers boots.
pub(super) fn check(image: &[u8]) -> Result<()> {
    let vector = &image[..RESET_VECTOR.len()];
    if vector != RESET_VECTOR {
        let swapped: Vec<u8> = vector
            .chunks(2)
            .flat_map(|word| [word[1], word[0]])
            .collect();
        let why = if swapped == RESET_VECTOR {
            "the L and M chips of pair 1 are swapped"
        } else {
            "not the image of a receiver with these chips (the Z3816A's is one part); \
             from dumps, pair 1 may not be the low half, or a dump is bad"
        };
        anyhow::bail!("reset vector is {vector:02x?}, not {RESET_VECTOR:02x?}: {why}");
    }
    Layout::AmdLanes
        .verify(image)
        .context("boot checksums fail: the pairs may be swapped, or a dump is bad")?;
    Ok(())
}

/// Say which audited image this is, if any.
fn identify(image: &[u8]) -> String {
    let hash: String = Sha256::digest(image)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    let known = PROFILES
        .iter()
        .find(|profile| profile.sha256 == hash)
        .map_or_else(
            || "not an audited image".to_string(),
            |profile| format!("{} {}", profile.model, profile.revision),
        );
    format!("Image: {known}; SHA-256 {hash}")
}

fn read(path: &Path) -> Result<Vec<u8>> {
    std::fs::read(path).with_context(|| format!("reading {}", path.display()))
}

fn write(path: &Path, bytes: &[u8]) -> Result<()> {
    std::fs::write(path, bytes).with_context(|| format!("writing {}", path.display()))
}

/// Read the four dumps, write the image, and say what it is: which
/// audited image, if any, and whether it starts and sums as it
/// should.  A failed check is reported after the image is written,
/// since the bytes are still worth looking at.
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
    check(&image)?;
    println!("Reset vector and boot checksums valid.");
    Ok(())
}

/// Read an image, require it to be one these chips hold, and write
/// the four dumps.
pub(crate) fn run_split(args: &SplitArgs) -> Result<()> {
    let image = read(&args.image)?;
    let chips = split(&image)?;
    println!("{}", identify(&image));
    check(&image)?;
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
