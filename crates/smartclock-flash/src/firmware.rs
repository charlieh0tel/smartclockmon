//! Audited image identities and the flash layouts they target.

use std::fmt::Write as _;

use sha2::Digest;
use sha2::Sha256;
use thiserror::Error;

/// File validation failures are distinct from receiver and transport errors.
#[derive(Debug, Error)]
pub(super) enum ImageError {
    #[error("expected a full 512 KiB image, got {0} bytes")]
    Size(usize),
    #[error(
        "unknown or modified firmware (SHA-256 {0}); an audited compatibility profile is required"
    )]
    Unknown(String),
    #[error("bad flash checksum at {address:#x}, lane {lane}")]
    Checksum { address: usize, lane: usize },
}

pub(super) const IMAGE_SIZE: usize = 0x80000;
pub(super) const RECORD_SIZE: usize = 64;
const AMD_BANK_SIZE: usize = IMAGE_SIZE / 2;

/// The bootloader's protected range and checksum algorithm.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Layout {
    AmdLanes,
    IntelWords,
}

impl Layout {
    pub(super) const fn primary_start(self) -> usize {
        match self {
            Self::AmdLanes => 0x10000,
            Self::IntelWords => 0x20000,
        }
    }

    fn verify(self, bytes: &[u8]) -> Result<(), ImageError> {
        match self {
            Self::AmdLanes => {
                for (start, end) in [
                    (self.primary_start(), AMD_BANK_SIZE),
                    (AMD_BANK_SIZE, IMAGE_SIZE),
                ] {
                    for lane in 0..2 {
                        let sum = bytes[start + lane..end - 4]
                            .iter()
                            .step_by(2)
                            .fold(0u16, |sum, &b| sum.wrapping_add(u16::from(b)));
                        let stored =
                            u16::from_be_bytes([bytes[end - 4 + lane], bytes[end - 2 + lane]]);
                        if sum != stored {
                            return Err(ImageError::Checksum {
                                address: start,
                                lane,
                            });
                        }
                    }
                }
            }
            Self::IntelWords => {
                // Z3816A reset code 0x526..0x53a, not the AMD lane sums.
                let sum = bytes[self.primary_start()..IMAGE_SIZE - 2]
                    .as_chunks::<2>()
                    .0
                    .iter()
                    .fold(0u16, |sum, word| {
                        sum.wrapping_add(u16::from_be_bytes([word[0], word[1]]))
                    });
                let stored = u16::from_be_bytes([bytes[IMAGE_SIZE - 2], bytes[IMAGE_SIZE - 1]]);
                if sum != stored {
                    return Err(ImageError::Checksum {
                        address: self.primary_start(),
                        lane: 0,
                    });
                }
            }
        }
        Ok(())
    }
}

/// Model provenance comes from the audited dump, never its filename or
/// an arbitrary model-looking string inside the binary.
#[derive(Debug)]
pub(super) struct Profile {
    pub(super) model: &'static str,
    pub(super) revision: &'static str,
    pub(super) installer: &'static str,
    pub(super) sha256: &'static str,
    pub(super) layout: Layout,
}

pub(super) const PROFILES: &[Profile] = &[
    Profile {
        model: "Z3801A",
        revision: "3543",
        installer: "Peru",
        layout: Layout::AmdLanes,
        sha256: "29e33b6d85b7371cef16cbf68b071f8a4ca047a8ab391d3c1e8087e553199bed",
    },
    Profile {
        model: "Z3805A",
        revision: "3543B",
        installer: "Peru",
        layout: Layout::AmdLanes,
        sha256: "216daf929b293be02bfd92ed61cca8c7e70d577696f2567a12f01905f6998792",
    },
    Profile {
        model: "58503A",
        revision: "3633",
        installer: "Oman",
        layout: Layout::AmdLanes,
        sha256: "a8676aae6ce89ee20ad572965cf5b1721f7f0ddb66f19f590cfd4e909e807709",
    },
    Profile {
        model: "58503A",
        revision: "3704",
        installer: "USA",
        layout: Layout::AmdLanes,
        sha256: "d13b9ff1e4a0a59517aac4d066c60e22b290cf4ff6810c5c2bf01f1bc9491ca3",
    },
    Profile {
        model: "Z3816A",
        revision: "4001",
        installer: "USA",
        layout: Layout::IntelWords,
        sha256: "5a2e34cdbb2c709340d9578ddf162023653d0c280f42e18caa9a4cb9fd1e0963",
    },
];

/// A recognized, checksummed image held in memory before opening a port.
#[derive(Debug)]
pub(super) struct Firmware {
    bytes: Vec<u8>,
    pub(super) profile: &'static Profile,
}

impl Firmware {
    pub(super) fn validate(bytes: Vec<u8>) -> Result<Self, ImageError> {
        if bytes.len() != IMAGE_SIZE {
            return Err(ImageError::Size(bytes.len()));
        }
        let hash: String = Sha256::digest(&bytes)
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect();
        let profile = PROFILES
            .iter()
            .find(|p| p.sha256 == hash)
            .ok_or(ImageError::Unknown(hash))?;
        profile.layout.verify(&bytes)?;
        Ok(Self { bytes, profile })
    }

    pub(super) fn records(&self) -> impl Iterator<Item = (usize, String)> + '_ {
        let start = self.profile.layout.primary_start();
        self.bytes[start..]
            .chunks(RECORD_SIZE)
            .enumerate()
            .map(move |(n, bytes)| {
                let address = start + n * RECORD_SIZE;
                (address, srecord(self.profile.layout, address, bytes))
            })
    }
}

/// S2 has a 24-bit address. The installers program whole 16-bit words.
pub(super) fn srecord(layout: Layout, address: usize, bytes: &[u8]) -> String {
    assert!(address >= layout.primary_start() && address + bytes.len() <= IMAGE_SIZE);
    assert!(address.is_multiple_of(2) && bytes.len().is_multiple_of(2));
    assert!(!bytes.is_empty() && bytes.len() <= RECORD_SIZE);
    let count = (bytes.len() + 4) as u8;
    let mut sum = count;
    let mut record = format!("S2{count:02X}");
    for byte in [(address >> 16) as u8, (address >> 8) as u8, address as u8]
        .into_iter()
        .chain(bytes.iter().copied())
    {
        sum = sum.wrapping_add(byte);
        write!(record, "{byte:02X}").expect("writing to String");
    }
    write!(record, "{:02X}", !sum).expect("writing to String");
    record
}
