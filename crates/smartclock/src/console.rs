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
//! value is one this project never sends, and only this value is.
//!
//! [`back_to_scpi`] leaves the console with its own word `halt`, which
//! recreates the SCPI task and deletes the console's (docs/firmware.md,
//! "Leaving it").  If that does not bring back the primary's SCPI, it
//! falls back to the primary's own exit into the installer, from which
//! `:SYSTem:LANGuage "PRIMARY"` restarts the primary (docs/firmware.md,
//! "Forced installer entry with an unusable primary").  Nothing is
//! erased or programmed on either way.

use std::fmt;
use std::io::Read;
use std::io::Write;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering;
use std::time::Duration;
use std::time::Instant;

use crate::error::Error;
use crate::session::Config;
use crate::session::Session;
use crate::transport::Transport;

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

/// How long the line must be silent before a resync calls it drained:
/// longer than the gaps inside one reply, short next to a retry.
const QUIET: Duration = Duration::from_millis(300);

/// How many times a request is tried before the read gives up.
const ATTEMPTS: usize = 4;

/// How often a read reports progress, in bytes.
const PROGRESS_INTERVAL: u32 = 0x10000;

/// How long a language change is given before the port is synced: the
/// old interpreter can acknowledge before it exits.
pub const LANGUAGE_SETTLE: Duration = Duration::from_secs(2);

/// The console's word that recreates the SCPI task and ends the console.
const HALT: &str = "halt";

/// The `trap #11` instruction, the primary's only way into the installer.
const TRAP_11: u32 = 0x4e4b;

/// The prompt, `p4th D > ` in decimal and `p4th X > ` in hex: nine
/// bytes, the base letter in the middle.
const PROMPT_LEN: usize = 9;

/// Anything that can go wrong at the console or on the way back.
#[derive(Debug, thiserror::Error)]
pub enum ConsoleError {
    /// A read or write on the port failed.
    #[error("io: {0}")]
    Io(#[from] std::io::Error),

    /// The SCPI session on either side of the console failed.
    #[error(transparent)]
    Session(#[from] Error),

    /// No console prompt arrived in time.
    #[error("no pForth prompt within {waited:?} after {line:?}; received {seen:?}")]
    NoPrompt {
        /// The line that was sent.
        line: String,
        /// How long the read waited.
        waited: Duration,
        /// The tail of what did arrive.
        seen: String,
    },

    /// A reply to a read held something other than hex words.
    #[error("{0:?} is not a hex word")]
    NotHex(String),

    /// A reply held the wrong number of words.
    #[error("{request:?} answered {got} words, not {wanted}")]
    WordCount {
        /// The request.
        request: String,
        /// Words received.
        got: usize,
        /// Words asked for.
        wanted: usize,
    },

    /// Every attempt at one chunk failed.
    #[error("gave up at {address:#x}: {last}")]
    GaveUp {
        /// The chunk's address.
        address: u32,
        /// Why the last attempt failed.
        last: Box<ConsoleError>,
    },

    /// Stopped on request, between chunks.
    #[error("stopped on request at {0:#x}")]
    Stopped(u32),

    /// The range is not whole long words, or runs off the address space.
    #[error("{0}")]
    Range(&'static str),

    /// Not exactly one known image's exit matched the unit's memory.
    #[error(
        "{0} known installer exits match this unit's memory, not one; \
         staying in the console, which a power cycle leaves"
    )]
    NoExit(usize),

    /// The exit did not land in the installer.
    #[error("expected the installer after the {exit} exit, found {language:?}")]
    NotInstaller {
        /// The exit taken.
        exit: &'static Exit,
        /// What `:SYSTem:LANGuage?` answered.
        language: String,
    },

    /// The read failed, but the port is back at SCPI.
    #[error("{read}; the port is back at SCPI: {identity}")]
    ReadFailed {
        /// Why the read failed.
        read: Box<ConsoleError>,
        /// The primary's `*IDN?` answer.
        identity: String,
    },

    /// The read failed, and so did the way back.
    #[error("{read}; and returning to SCPI failed: {back}")]
    BothFailed {
        /// Why the read failed.
        read: Box<ConsoleError>,
        /// Why the way back failed.
        back: Box<ConsoleError>,
    },

    /// `halt` did not leave the port at the primary's SCPI parser.
    #[error("after halt the port is at {0:?}, not the primary's SCPI")]
    NotPrimary(String),

    /// Neither way out of the console worked.
    #[error("halt failed ({halt}), and so did the installer exit ({installer})")]
    NoWayOut {
        /// Why `halt` failed.
        halt: Box<ConsoleError>,
        /// Why the installer exit failed.
        installer: Box<ConsoleError>,
    },

    /// `:SYSTem:LANGuage "PRIMARY"` left the unit in the installer.
    #[error(
        "the unit stayed in the installer ({0:?}): its primary's checksums fail; \
         leave the daemon stopped"
    )]
    StayedInInstaller(String),
}

/// A result at the console.
pub type Result<T> = std::result::Result<T, ConsoleError>;

/// One image's exit into the installer, as the console can reach it.
/// `execute` calls the address held in the cell it is given; `cell` is
/// the operand of the SCPI task's call to the routine that ends in
/// `trap #11` (docs/firmware.md, "Forced installer entry with an
/// unusable primary").
#[derive(Debug)]
pub struct Exit {
    /// The model, as `*IDN?` names it.
    pub model: &'static str,
    /// The primary's revision, as `*IDN?` names it.
    pub revision: &'static str,
    /// The long word `execute` is given.
    cell: u32,
    /// What `cell` holds in this image.
    routine: u32,
    /// Where this image's `trap #11` is.
    trap: u32,
}

impl fmt::Display for Exit {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} {}", self.model, self.revision)
    }
}

/// Each is checked against the running unit's memory before use, so an
/// image not listed here is refused rather than guessed at.
const EXITS: &[Exit] = &[
    Exit {
        model: "Z3801A",
        revision: "3543",
        cell: 0x28ffc,
        routine: 0x12f2c,
        trap: 0x12f2c,
    },
    Exit {
        model: "Z3805A",
        revision: "3543B",
        cell: 0x2a138,
        routine: 0x12f2c,
        trap: 0x12f2c,
    },
    Exit {
        model: "58503A",
        revision: "3633",
        cell: 0x2940e,
        routine: 0x13028,
        trap: 0x1304e,
    },
    Exit {
        model: "58503A",
        revision: "3704",
        cell: 0x294b6,
        routine: 0x130da,
        trap: 0x13100,
    },
];

/// The exit known for a model and primary revision, if any.
pub fn exit_for(model: &str, revision: &str) -> Option<&'static Exit> {
    EXITS
        .iter()
        .find(|exit| exit.model == model && exit.revision == revision)
}

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
        Err(ConsoleError::NoPrompt {
            line: line.to_owned(),
            waited: timeout,
            seen: String::from_utf8_lossy(&reply[reply.len().saturating_sub(80)..]).into_owned(),
        })
    }

    /// Read and discard until the line has been quiet for [`QUIET`],
    /// giving up after [`REQUEST_TIMEOUT`].
    fn drain(&mut self) -> Result<()> {
        let deadline = Instant::now() + REQUEST_TIMEOUT;
        let mut last_byte = Instant::now();
        let mut chunk = [0u8; 1024];
        while last_byte.elapsed() < QUIET {
            if Instant::now() > deadline {
                return Err(ConsoleError::NoPrompt {
                    line: String::new(),
                    waited: REQUEST_TIMEOUT,
                    seen: "the receiver kept sending".to_owned(),
                });
            }
            if self.port.read(&mut chunk)? > 0 {
                last_byte = Instant::now();
            }
        }
        Ok(())
    }

    /// Bring the console back to a prompt that answers the next line:
    /// drain what is still arriving, ask for a fresh prompt, and drain
    /// again in case the prompt matched was an older one.
    fn resync(&mut self) -> Result<()> {
        self.drain()?;
        self.send_within("", PROBE_TIMEOUT)?;
        self.drain()
    }

    /// The long word at `address`, read in `hex`.
    fn long(&mut self, address: u32) -> Result<u32> {
        let request = format!("{address:X} @ u.");
        match words(&self.send(&request)?, &request)?.as_slice() {
            [word] => Ok(*word),
            other => Err(ConsoleError::WordCount {
                request,
                got: other.len(),
                wanted: 1,
            }),
        }
    }
}

/// The 32-bit words in a reply to `rd`, which prints each unsigned in
/// the current base without leading zeros.  Any token that is not hex
/// -- an echo of the request, a stray error -- makes the reply unusable.
fn words(reply: &str, request: &str) -> Result<Vec<u32>> {
    let body = reply.trim_start().strip_prefix(request).unwrap_or(reply);
    body.split_whitespace()
        .map(|token| {
            u32::from_str_radix(token, 16).map_err(|_| ConsoleError::NotHex(token.to_owned()))
        })
        .collect()
}

/// What a read reports as it goes.
#[derive(Debug)]
pub enum Progress {
    /// The port was not at the console, so `:SYSTem:LANGuage "PFORTH"`
    /// is being sent.
    Entering,
    /// Another [`PROGRESS_INTERVAL`] bytes were read.
    Read {
        /// Bytes read so far.
        done: u32,
        /// Since the first request.
        elapsed: Duration,
        /// Chunks so far that differed from the comparison image.
        differing: usize,
    },
}

/// What a read found.
#[derive(Debug)]
pub struct Summary {
    /// Chunks that differed from the comparison image, by address.
    pub differing: Vec<u32>,
    /// How long the read took.
    pub took: Duration,
}

/// Read `length` bytes from `from` into `out`, checking each chunk
/// against `compare` when given, which holds the bytes expected at
/// `from` onwards.  Setting `stop` ends the read before its next chunk.
/// The port is left at the console.
pub fn read_memory<T: Read + Write>(
    port: T,
    from: u32,
    length: u32,
    out: &mut impl Write,
    compare: Option<&[u8]>,
    mut progress: impl FnMut(Progress),
    stop: &AtomicBool,
) -> Result<Summary> {
    if !(from.is_multiple_of(4) && length.is_multiple_of(4)) {
        return Err(ConsoleError::Range(
            "the address and length must be multiples of four, since memory is read a long word at a time",
        ));
    }
    let end = from.checked_add(length).ok_or(ConsoleError::Range(
        "the range runs past the end of the address space",
    ))?;
    let mut console = Console { port };
    if console.send_within("", PROBE_TIMEOUT).is_err() {
        progress(Progress::Entering);
        console.send(ENTER)?;
    }
    console.send(READ_WORD)?;
    console.send("hex")?;
    let started = Instant::now();
    let mut differing = Vec::new();
    let mut address = from;
    while address < end {
        if stop.load(Ordering::Relaxed) {
            return Err(ConsoleError::Stopped(address));
        }
        let size = CHUNK.min(end - address);
        let wanted = (size / 4) as usize;
        let request = format!("{address:X} {size:X} rd");
        let mut last = None;
        let mut got = None;
        for _ in 0..ATTEMPTS {
            match console
                .send(&request)
                .and_then(|reply| words(&reply, &request))
            {
                Ok(words) if words.len() == wanted => {
                    got = Some(words);
                    break;
                }
                Ok(words) => {
                    last = Some(ConsoleError::WordCount {
                        request: request.clone(),
                        got: words.len(),
                        wanted,
                    });
                }
                Err(e) => last = Some(e),
            }
            // A failed attempt can leave a late reply and its prompt on
            // the way, to be read as the next attempt's words.
            if let Err(e) = console.resync() {
                last = Some(e);
                break;
            }
        }
        let Some(words) = got else {
            return Err(ConsoleError::GaveUp {
                address,
                last: Box::new(last.unwrap_or(ConsoleError::Range("no attempts made"))),
            });
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
        if (address - from).is_multiple_of(PROGRESS_INTERVAL) {
            progress(Progress::Read {
                done: address - from,
                elapsed: started.elapsed(),
                differing: differing.len(),
            });
        }
    }
    out.flush()?;
    console.send("decimal")?;
    Ok(Summary {
        differing,
        took: started.elapsed(),
    })
}

/// Leave the console through the running image's exit into the
/// installer, and return which exit that was.  The exit is used only
/// when exactly one known image's cell and `trap #11` are where that
/// image has them.  No prompt follows: the installer's SCPI parser has
/// the port afterwards.
fn enter_installer<T: Read + Write>(port: T) -> Result<&'static Exit> {
    let mut console = Console { port };
    console.send_within("", PROBE_TIMEOUT)?;
    console.send("hex")?;
    let mut found = Vec::new();
    for exit in EXITS {
        let aligned = exit.trap & !3;
        let long_at_trap = console.long(aligned)?;
        let half = if exit.trap == aligned {
            long_at_trap >> 16
        } else {
            long_at_trap & 0xffff
        };
        if console.long(exit.cell)? == exit.routine && half == TRAP_11 {
            found.push(exit);
        }
    }
    let [exit] = found.as_slice() else {
        return Err(ConsoleError::NoExit(found.len()));
    };
    console
        .port
        .write_all(format!("{:X} execute\r", exit.cell).as_bytes())?;
    console.port.flush()?;
    Ok(exit)
}

/// Leave the console with `halt`, and return the primary's `*IDN?`
/// answer once `:SYSTem:LANGuage?` says `"PRIMARY"`.  The port comes
/// back either way, for the fallback.
fn by_halt<T: Transport>(mut port: T, config: &Config) -> (Result<String>, T) {
    let halted = Console { port: &mut port }
        .send_within("", PROBE_TIMEOUT)
        .and_then(|_| {
            port.write_all(format!("{HALT}\r").as_bytes())?;
            port.flush()?;
            Ok(())
        });
    if let Err(e) = halted {
        return (Err(e), port);
    }
    std::thread::sleep(LANGUAGE_SETTLE);
    let mut session = Session::new(port, config.clone());
    let primary = primary(&mut session);
    (primary, session.into_transport())
}

/// The primary's `*IDN?` answer, if `:SYSTem:LANGuage?` says the port
/// is at its SCPI parser.
fn primary<T: Transport>(session: &mut Session<T>) -> Result<String> {
    session.sync()?;
    let language = session.query(":SYSTem:LANGuage?")?;
    let language = language.one_line("language")?;
    if language != "\"PRIMARY\"" {
        return Err(ConsoleError::NotPrimary(language.to_owned()));
    }
    Ok(session.query("*IDN?")?.one_line("identity")?.to_owned())
}

/// Leave the console through the image's exit into the installer, then
/// `:SYSTem:LANGuage "PRIMARY"`, which reruns the reset code's
/// checksums and starts the primary.
fn by_installer<T: Transport>(mut port: T, config: Config) -> Result<String> {
    let exit = enter_installer(&mut port)?;
    std::thread::sleep(LANGUAGE_SETTLE);
    let mut session = Session::new(port, config);
    session.sync()?;
    let language = session.query(":SYSTem:LANGuage?")?;
    let language = language.one_line("language")?;
    if language != "\"INSTALL\"" {
        return Err(ConsoleError::NotInstaller {
            exit,
            language: language.to_owned(),
        });
    }
    // The installer restarts the primary without a prompt of its own,
    // so waiting for one times out; the checks after the sync are what
    // decide whether the primary came back.
    match session.send_raw(":SYSTem:LANGuage \"PRIMARY\"") {
        Ok(_) | Err(Error::Timeout { .. }) => {}
        Err(e) => return Err(e.into()),
    }
    std::thread::sleep(LANGUAGE_SETTLE);
    primary(&mut session).map_err(|e| match e {
        ConsoleError::NotPrimary(language) => ConsoleError::StayedInInstaller(language),
        e => e,
    })
}

/// Return a port at the console to the primary's SCPI parser, with
/// `halt` and, failing that, through the installer.  Each step is
/// checked, and the primary's `*IDN?` answer is returned.
pub fn back_to_scpi<T: Transport>(port: T, config: Config) -> Result<String> {
    let (halted, port) = by_halt(port, &config);
    let Err(halt) = halted else {
        return halted;
    };
    by_installer(port, config).map_err(|installer| ConsoleError::NoWayOut {
        halt: Box::new(halt),
        installer: Box::new(installer),
    })
}

/// [`read_memory`], then [`back_to_scpi`] whether or not the read
/// succeeded, since a read that fails part way leaves the port at the
/// console too.  Returns the summary and the primary's `*IDN?` answer.
#[expect(
    clippy::too_many_arguments,
    reason = "read_memory's arguments and the way back's config, passed straight through"
)]
pub fn read_and_return<T: Transport>(
    mut port: T,
    from: u32,
    length: u32,
    out: &mut impl Write,
    compare: Option<&[u8]>,
    progress: impl FnMut(Progress),
    stop: &AtomicBool,
    config: Config,
) -> Result<(Summary, String)> {
    let read = read_memory(&mut port, from, length, out, compare, progress, stop);
    match (read, back_to_scpi(port, config)) {
        (Ok(summary), Ok(identity)) => Ok((summary, identity)),
        (Ok(_), Err(back)) => Err(back),
        (Err(read), Ok(identity)) => Err(ConsoleError::ReadFailed {
            read: Box::new(read),
            identity,
        }),
        (Err(read), Err(back)) => Err(ConsoleError::BothFailed {
            read: Box::new(read),
            back: Box::new(back),
        }),
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::io::Read;
    use std::io::Write;
    use std::sync::atomic::AtomicBool;
    use std::time::Duration;

    use super::ConsoleError;
    use super::EXITS;
    use super::at_prompt;
    use super::back_to_scpi;
    use super::enter_installer;
    use super::exit_for;
    use super::read_memory;
    use super::words;
    use crate::session::Config;
    use crate::transport::Transport;

    /// A console that answers `<hex> @ u.` from `memory`, and anything
    /// else with a bare prompt, recording every line it was sent.  With
    /// `halts`, `halt` hands the port to an SCPI parser that echoes and
    /// answers `*IDN?` and `:SYSTem:LANGuage?` as a primary does.
    struct Fake {
        memory: HashMap<u32, u32>,
        halts: bool,
        at_scpi: bool,
        pending: Vec<u8>,
        reply: Vec<u8>,
        sent: Vec<String>,
    }

    impl Fake {
        fn new(memory: &[(u32, u32)]) -> Self {
            Self {
                memory: memory.iter().copied().collect(),
                halts: false,
                at_scpi: false,
                pending: Vec::new(),
                reply: Vec::new(),
                sent: Vec::new(),
            }
        }

        fn scpi(&mut self, line: &str) {
            let answer = match line {
                "*IDN?" => "HEWLETT-PACKARD,Z3805A,3625A01487,3543B-A\r\n",
                ":SYSTem:LANGuage?" => "\"PRIMARY\"\r\n",
                _ => "",
            };
            self.reply
                .extend_from_slice(format!("{line}\r\n{answer}scpi > ").as_bytes());
        }
    }

    // Borrowed, so a test can read what was sent once the session that
    // took the port is gone.
    impl Transport for &mut Fake {
        fn describe(&self) -> String {
            "fake console".to_owned()
        }
    }

    impl Write for Fake {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.pending.extend_from_slice(bytes);
            while let Some(end) = self.pending.iter().position(|&b| b == b'\r') {
                let line = String::from_utf8_lossy(&self.pending[..end])
                    .trim_start_matches('\n')
                    .to_owned();
                self.pending.drain(..=end);
                if self.at_scpi {
                    self.scpi(&line);
                    self.sent.push(line);
                    continue;
                }
                if line == "halt" && self.halts {
                    self.at_scpi = true;
                    self.reply.extend_from_slice(b"\r\nscpi > ");
                    self.sent.push(line);
                    continue;
                }
                let word = line
                    .strip_suffix(" @ u.")
                    .and_then(|address| u32::from_str_radix(address, 16).ok())
                    .map(|address| self.memory.get(&address).copied().unwrap_or(0));
                if let Some(word) = word {
                    self.reply
                        .extend_from_slice(format!(" {word:X}").as_bytes());
                }
                self.reply.extend_from_slice(b"p4th X > ");
                self.sent.push(line);
            }
            Ok(bytes.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    impl Read for Fake {
        fn read(&mut self, out: &mut [u8]) -> std::io::Result<usize> {
            let n = out.len().min(self.reply.len());
            out[..n].copy_from_slice(&self.reply[..n]);
            self.reply.drain(..n);
            Ok(n)
        }
    }

    #[test]
    fn the_prompt_is_recognized_in_either_base() {
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

    #[test]
    fn each_exit_matches_its_image() {
        for (exit, name) in EXITS.iter().zip([
            "z3801a-3543.bin",
            "z3805a-3543b.bin",
            "58503a-3633.bin",
            "58503a-3704.bin",
        ]) {
            let path = format!("{}/../../third_party/{name}", env!("CARGO_MANIFEST_DIR"));
            let image = std::fs::read(&path).expect(&path);
            let at = |address: u32, n: usize| &image[address as usize..address as usize + n];
            assert_eq!(at(exit.cell, 4), exit.routine.to_be_bytes(), "{exit}");
            assert_eq!(at(exit.trap, 2), [0x4e, 0x4b], "{exit}");
        }
        assert!(exit_for("Z3801A", "3543").is_some());
        assert!(exit_for("Z3816A", "4001").is_none());
    }

    #[test]
    fn the_installer_exit_is_taken_only_where_memory_matches_one_image() {
        let mut z3801a = Fake::new(&[(0x28ffc, 0x12f2c), (0x12f2c, 0x4e4b_4e75)]);
        assert_eq!(
            enter_installer(&mut z3801a).expect("exit").to_string(),
            "Z3801A 3543"
        );
        assert_eq!(
            z3801a.sent.last().map(String::as_str),
            Some("28FFC execute")
        );

        let mut a58503 = Fake::new(&[(0x294b6, 0x130da), (0x13100, 0x4e4b_4e75)]);
        assert_eq!(
            enter_installer(&mut a58503).expect("exit").to_string(),
            "58503A 3704"
        );

        // The cell alone is not enough: without the trap there, nothing
        // is executed.
        let mut unknown = Fake::new(&[(0x28ffc, 0x12f2c)]);
        assert!(enter_installer(&mut unknown).is_err());
        assert!(!unknown.sent.iter().any(|line| line.ends_with("execute")));
    }

    fn quick() -> Config {
        Config {
            timeout: Duration::from_millis(200),
            idle: Duration::from_millis(1),
            ..Config::default()
        }
    }

    #[test]
    fn halt_is_tried_first_and_checked() {
        let mut console = Fake::new(&[]);
        console.halts = true;
        let identity = back_to_scpi(&mut console, quick()).expect("back at SCPI");
        assert!(identity.ends_with("3543B-A"), "{identity}");
        assert!(!console.sent.iter().any(|line| line.ends_with("execute")));
    }

    #[test]
    fn the_installer_exit_is_the_fallback_and_both_failures_are_reported() {
        let mut console = Fake::new(&[]);
        let error = back_to_scpi(&mut console, quick()).expect_err("no way out");
        assert!(matches!(error, ConsoleError::NoWayOut { .. }), "{error}");
        assert!(console.sent.iter().any(|line| line == "halt"));
    }

    #[test]
    fn a_read_stops_on_request_before_its_next_chunk() {
        let mut console = Fake::new(&[]);
        let stop = AtomicBool::new(true);
        let error = read_memory(&mut console, 0, 8, &mut Vec::new(), None, |_| {}, &stop)
            .expect_err("stopped");
        assert!(matches!(error, ConsoleError::Stopped(0)), "{error}");
        assert!(!console.sent.iter().any(|line| line.ends_with("rd")
            && line != ": rd ( addr n -- ) over + swap do i @ u. 4 +loop ;"));
    }

    #[test]
    fn a_failed_chunk_is_retried_only_after_a_fresh_prompt() {
        // The fake answers `rd` with no words, so every attempt fails.
        let mut console = Fake::new(&[]);
        let stop = AtomicBool::new(false);
        let error = read_memory(&mut console, 0, 8, &mut Vec::new(), None, |_| {}, &stop)
            .expect_err("no words");
        assert!(matches!(error, ConsoleError::GaveUp { .. }), "{error}");
        let after_first: Vec<&str> = console
            .sent
            .iter()
            .map(String::as_str)
            .skip_while(|line| *line != "0 8 rd")
            .collect();
        assert_eq!(after_first.get(1), Some(&""), "{after_first:?}");
        assert_eq!(after_first.get(2), Some(&"0 8 rd"), "{after_first:?}");
    }
}
