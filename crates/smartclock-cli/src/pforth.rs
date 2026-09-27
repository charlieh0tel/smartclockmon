//! Reading a receiver's memory through its pForth debug console.
//!
//! `:SYSTem:LANGuage "PFORTH"` turns the SCPI port into a Forth console
//! (`docs/firmware.md`, "The debug console").  Its `@` reads the running
//! unit's own memory, so the ROM and the EEPROM can be read without
//! opening the case.  Loops are compile-only there, so one word is
//! defined for the purpose, in the console's RAM dictionary, and nothing
//! else is written:
//!
//! ```text
//! : rd ( addr n -- ) over + swap do i @ u. 4 +loop ;
//! ```
//!
//! When the port is not already at the console prompt, the console is
//! entered with `:SYSTem:LANGuage "PFORTH"`.  That command's `"INSTALL"`
//! value is one this project never sends, and only this value is.  A
//! power cycle is the only way back to SCPI.

use std::io::Read;
use std::io::Write;
use std::time::Duration;
use std::time::Instant;

use anyhow::Context as _;
use anyhow::Result;

/// The word defined for reading, as sent.
const READ_WORD: &str = ": rd ( addr n -- ) over + swap do i @ u. 4 +loop ;";

/// Bytes asked for in one request: 256 words, a couple of kilobytes of
/// hex, a second or so at 19200 baud.
const CHUNK: u32 = 0x400;

/// How long one request may take before it is abandoned and retried.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

/// How long an empty line waits for a console prompt before the port
/// is taken to be at SCPI.
const PROBE_TIMEOUT: Duration = Duration::from_secs(3);

/// The command that turns the SCPI port into the console.
const ENTER: &str = ":SYSTem:LANGuage \"PFORTH\"";

/// How many times a request is tried before the read gives up.
const ATTEMPTS: usize = 4;

/// The prompt, `p4th D > ` in decimal and `p4th X > ` in hex: nine
/// bytes, the base letter in the middle.
const PROMPT_LEN: usize = 9;

/// Whether `buffer` ends at a console prompt.
fn at_prompt(buffer: &[u8]) -> bool {
    buffer.len() >= PROMPT_LEN && {
        let tail = &buffer[buffer.len() - PROMPT_LEN..];
        tail.starts_with(b"p4th ") && tail.ends_with(b" > ")
    }
}

/// A console on a serial port.
struct Console<T> {
    port: T,
}

impl<T: Read + Write> Console<T> {
    /// Send one line and return everything up to the prompt after it.
    fn send(&mut self, line: &str) -> Result<String> {
        self.send_within(line, REQUEST_TIMEOUT)
    }

    /// `send`, giving up after `timeout`.
    fn send_within(&mut self, line: &str, timeout: Duration) -> Result<String> {
        self.port.write_all(line.as_bytes())?;
        self.port.write_all(b"\r")?;
        self.port.flush()?;
        let deadline = Instant::now() + timeout;
        let mut reply = Vec::new();
        let mut chunk = [0u8; 1024];
        while Instant::now() < deadline {
            let n = self.port.read(&mut chunk)?;
            reply.extend_from_slice(&chunk[..n]);
            if at_prompt(&reply) {
                reply.truncate(reply.len() - PROMPT_LEN);
                return Ok(String::from_utf8_lossy(&reply).into_owned());
            }
        }
        anyhow::bail!(
            "no pForth prompt within {timeout:?} after {line:?}; received {:?}",
            String::from_utf8_lossy(&reply[reply.len().saturating_sub(80)..])
        )
    }
}

/// The 32-bit words in a reply to `rd`, which prints each unsigned in
/// the current base without leading zeros.  Any token that is not hex
/// -- an echo of the request, a stray error -- makes the reply unusable.
fn words(reply: &str, request: &str) -> Result<Vec<u32>> {
    let body = reply.trim_start().strip_prefix(request).unwrap_or(reply);
    body.split_whitespace()
        .map(|token| {
            u32::from_str_radix(token, 16).with_context(|| format!("{token:?} is not a hex word"))
        })
        .collect()
}

/// What a read found, for the summary line.
pub(crate) struct Summary {
    /// Chunks that differed from the comparison image, by address.
    pub(crate) differing: Vec<u32>,
    /// How long the read took.
    pub(crate) took: Duration,
}

/// Read `length` bytes from `from` into `out`, checking each chunk
/// against `compare` when given, which holds the bytes expected at
/// `from` onwards.
pub(crate) fn read_memory<T: Read + Write>(
    port: T,
    from: u32,
    length: u32,
    out: &mut impl Write,
    compare: Option<&[u8]>,
) -> Result<Summary> {
    anyhow::ensure!(
        from.is_multiple_of(4) && length.is_multiple_of(4),
        "the address and length must be multiples of four, since memory is read a long word at a time"
    );
    let mut console = Console { port };
    if console.send_within("", PROBE_TIMEOUT).is_err() {
        eprintln!("not at the pForth prompt; sending {ENTER}");
        console
            .send(ENTER)
            .context("the receiver did not enter the pForth console")?;
    }
    console.send(READ_WORD)?;
    console.send("hex")?;
    let started = Instant::now();
    let mut differing = Vec::new();
    let mut address = from;
    let end = from
        .checked_add(length)
        .context("the range runs past the end of the address space")?;
    while address < end {
        let size = CHUNK.min(end - address);
        let request = format!("{address:X} {size:X} rd");
        let mut last = None;
        let mut got = None;
        for _ in 0..ATTEMPTS {
            match console
                .send(&request)
                .and_then(|reply| words(&reply, &request))
            {
                Ok(words) if words.len() == (size / 4) as usize => {
                    got = Some(words);
                    break;
                }
                Ok(words) => {
                    last = Some(anyhow::anyhow!("{} words, not {}", words.len(), size / 4))
                }
                Err(e) => last = Some(e),
            }
        }
        let Some(words) = got else {
            let why = last.map_or_else(String::new, |e| format!("{e:#}"));
            anyhow::bail!("gave up at {address:#x}: {why}");
        };
        let bytes: Vec<u8> = words.iter().flat_map(|w| w.to_be_bytes()).collect();
        out.write_all(&bytes)?;
        let offset = (address - from) as usize;
        if let Some(expected) = compare
            && expected.get(offset..offset + bytes.len()) != Some(bytes.as_slice())
        {
            differing.push(address);
        }
        address += size;
        if (address - from).is_multiple_of(0x10000) {
            eprintln!(
                "{:#x} of {length:#x} read, {} s, {} differing chunks",
                address - from,
                started.elapsed().as_secs(),
                differing.len()
            );
        }
    }
    out.flush()?;
    console.send("decimal")?;
    Ok(Summary {
        differing,
        took: started.elapsed(),
    })
}

#[cfg(test)]
mod tests {
    use super::at_prompt;
    use super::words;

    #[test]
    fn the_prompt_is_recognised_in_either_base() {
        assert!(at_prompt(b" 10FFFE 550p4th X > "));
        assert!(at_prompt(b"p4th D > "));
        assert!(!at_prompt(b"p4th D >"));
        assert!(!at_prompt(b"scpi > "));
    }

    #[test]
    fn a_reply_to_rd_reads_as_words_with_or_without_the_echo() {
        assert_eq!(
            words(" 10FFFE 550 1005B4", "0 C rd").expect("words"),
            vec![0x0010_fffe, 0x550, 0x0010_05b4]
        );
        assert_eq!(
            words("0 C rd 10FFFE 550", "0 C rd").expect("words"),
            vec![0x0010_fffe, 0x550]
        );
        assert!(words(" 10FFFE ?Stack empty", "0 8 rd").is_err());
    }
}
