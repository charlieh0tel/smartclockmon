//! SmartClock installer model; see docs/firmware.md for the protocol.

use crate::receiver::Answer;
use crate::receiver::eq;
use crate::receiver::split;
use thiserror::Error;

const FLASH_SIZE: usize = 0x80000;

/// Flash wiring and boot checksum format, independently modeled here.
#[derive(Debug, Clone, Copy)]
pub enum FlashLayout {
    AmdLanes,
    IntelWords,
}

impl FlashLayout {
    fn primary_start(self) -> usize {
        match self {
            Self::AmdLanes => 0x10000,
            Self::IntelWords => 0x20000,
        }
    }
}

/// Invalid input or injected failure in the simulated installer.
#[derive(Debug, Error)]
pub enum InstallerError {
    #[error("invalid flash image or revision offset")]
    Image,
    #[error("invalid language")]
    Language,
    #[error("undefined installer header")]
    Header,
    #[error("invalid S2 record encoding, count or checksum")]
    Record,
    #[error("invalid programming range or alignment")]
    Range,
    #[error("simulated programming failure")]
    Program,
    #[error("flash must be erased before programming")]
    NotErased,
}

impl InstallerError {
    pub(crate) fn scpi_code(&self) -> i32 {
        match self {
            Self::Header => -113, // Undefined header, 097-59551-02 appendix A.
            _ => -222,
        }
    }
}

/// Flash state and fault injection for installer tests. Timing and wear
/// are not modeled. Only the word-aligned S2 records used by the flasher
/// are supported, not every S-record type accepted by real firmware.
#[derive(Debug)]
pub struct Installer {
    layout: FlashLayout,
    revision_offset: usize,
    /// Bootloader revision, such as Peru, Oman or USA; remains after upgrades.
    pub revision: String,
    /// Full address-zero image, including protected boot flash.
    pub flash: Vec<u8>,
    /// INSTALL is active when true, PRIMARY otherwise.
    pub active: bool,
    /// Refuse any download covering this address.
    pub fail_at: Option<usize>,
    /// Leave flash unchanged when asked to erase.
    pub erase_fails: bool,
}

impl Installer {
    /// Start with a full 512 KiB image; bad boot checksums enter INSTALL.
    pub fn new(
        flash: Vec<u8>,
        layout: FlashLayout,
        revision: String,
        revision_offset: usize,
    ) -> Result<Self, InstallerError> {
        if flash.len() != FLASH_SIZE || revision_offset >= FLASH_SIZE {
            return Err(InstallerError::Image);
        }
        let mut installer = Self {
            flash,
            layout,
            revision,
            revision_offset,
            active: false,
            fail_at: None,
            erase_fails: false,
        };
        installer.active = !installer.bootable();
        Ok(installer)
    }

    pub(crate) fn primary_revision(&self) -> String {
        self.flash[self.revision_offset..]
            .iter()
            .copied()
            .take_while(|byte| byte.is_ascii_alphanumeric())
            .map(char::from)
            .collect()
    }

    pub(crate) fn respond(&mut self, command: &str) -> Result<Option<Answer>, InstallerError> {
        let (header, argument) = split(command);
        if eq(header, ":SYSTem:LANGuage?") {
            return Ok(Some(Answer::line(if self.active {
                "\"INSTALL\""
            } else {
                "\"PRIMARY\""
            })));
        }
        if eq(header, ":SYSTem:LANGuage") {
            match argument.trim_matches('"').to_ascii_uppercase().as_str() {
                "INSTALL" => self.active = true,
                "PRIMARY" => self.active = !self.bootable(),
                _ => return Err(InstallerError::Language),
            }
            return Ok(Some(Answer::silent()));
        }
        if !self.active {
            return Ok(None);
        }
        if eq(header, ":DIAGnostic:ERASe?") {
            let blank = self.flash[self.layout.primary_start()..]
                .iter()
                .all(|&byte| byte == 0xff);
            return Ok(Some(Answer::line(if blank { "+1" } else { "+0" })));
        }
        if eq(header, ":DIAGnostic:ERASe") {
            if !self.erase_fails {
                let start = self.layout.primary_start();
                self.flash[start..].fill(0xff);
            }
        } else if eq(header, ":DIAGnostic:DOWNload") {
            self.download(argument)?;
        } else {
            return Err(InstallerError::Header);
        }
        Ok(Some(Answer::silent()))
    }

    fn download(&mut self, record: &str) -> Result<(), InstallerError> {
        // SCPI string data must be quoted. An unquoted S-record is
        // parsed as a mnemonic and the real installer rejects its length.
        let record = record
            .strip_prefix('"')
            .and_then(|record| record.strip_suffix('"'))
            .ok_or(InstallerError::Record)?;
        let hex = record.strip_prefix("S2").ok_or(InstallerError::Record)?;
        if !hex.is_ascii() || !hex.len().is_multiple_of(2) {
            return Err(InstallerError::Record);
        }
        let bytes: Vec<u8> = (0..hex.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&hex[i..i + 2], 16))
            .collect::<Result<_, _>>()
            .map_err(|_| InstallerError::Record)?;
        if bytes.len() < 7
            || usize::from(bytes[0]) + 1 != bytes.len()
            || bytes.iter().map(|&b| u32::from(b)).sum::<u32>() % 256 != 255
        {
            return Err(InstallerError::Record);
        }
        let address =
            usize::from(bytes[1]) * 65536 + usize::from(bytes[2]) * 256 + usize::from(bytes[3]);
        let data = &bytes[4..bytes.len() - 1];
        let end = address + data.len();
        if address < self.layout.primary_start()
            || end > FLASH_SIZE
            || !address.is_multiple_of(2)
            || !data.len().is_multiple_of(2)
        {
            return Err(InstallerError::Range);
        }
        if self
            .fail_at
            .is_some_and(|fault| (address..end).contains(&fault))
        {
            return Err(InstallerError::Program);
        }
        for (stored, &new) in self.flash[address..end].iter_mut().zip(data) {
            if *stored & new != new {
                return Err(InstallerError::NotErased);
            }
            *stored = new;
        }
        Ok(())
    }

    fn bootable(&self) -> bool {
        if matches!(self.layout, FlashLayout::IntelWords) {
            let sum: u64 = self.flash[0x20000..FLASH_SIZE - 2]
                .as_chunks::<2>()
                .0
                .iter()
                .map(|word| u64::from(word[0]) * 256 + u64::from(word[1]))
                .sum();
            let expected =
                u64::from(self.flash[FLASH_SIZE - 2]) * 256 + u64::from(self.flash[FLASH_SIZE - 1]);
            return sum % 65536 == expected;
        }
        for (start, end) in [(0x10000, 0x40000), (0x40000, FLASH_SIZE)] {
            let mut sums = [0u32; 2];
            for address in start..end - 4 {
                sums[address % 2] += u32::from(self.flash[address]);
            }
            for (lane, sum) in sums.into_iter().enumerate() {
                let expected = u32::from(self.flash[end - 4 + lane]) * 256
                    + u32::from(self.flash[end - 2 + lane]);
                if sum % 65536 != expected {
                    return false;
                }
            }
        }
        true
    }
}
