//! Command framing over a transport.
//!
//! The receiver is not a VISA-style instrument.  It behaves as a
//! terminal: it echoes what it is sent, and it terminates every
//! exchange with a prompt, `scpi> ` normally or `E-nnn> ` when the
//! previous command raised an error.  Framing therefore reads to a
//! prompt rather than to a line terminator, which is also what makes
//! the multi-line status screen readable without knowing its length in
//! advance.
//!
//! One consequence shapes the whole design: a partially read reply
//! leaves the next read starting mid-prompt, and every later exchange
//! is then misaligned.  A `Session` owns its transport and is not
//! `Sync`, so only one caller can ever be issuing commands.

use std::time::Duration;
use std::time::Instant;

use crate::error::Error;
use crate::error::Result;
use crate::transport::Transport;
use crate::types::BaudRate;
use crate::types::ErrorEntry;

/// The prompt that ends an exchange.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Prompt {
    /// `scpi> `.  The previous command was accepted.
    Ready,
    /// `E-nnn> `.  The previous command raised an error; the queue holds
    /// the detail.
    Error(String),
}

/// A reply, with the prompt that closed it.
#[derive(Debug, Clone)]
pub struct Reply {
    /// Response lines, echo and prompt removed.  Empty for a command
    /// that returns nothing.
    pub lines: Vec<String>,
    /// How the receiver closed the exchange.
    pub prompt: Prompt,
}

impl Reply {
    /// The reply as a single line, which is what most queries return.
    pub fn one_line(&self, expected: &'static str) -> Result<&str> {
        match self.lines.as_slice() {
            [only] => Ok(only.as_str()),
            _ => Err(Error::Parse {
                reply: self.lines.join("\\n"),
                expected,
            }),
        }
    }
}

/// How to frame commands.
#[derive(Debug, Clone)]
pub struct Config {
    /// How long to wait for a prompt.  The status screen is about 1.8 KB
    /// and takes roughly a second at 19200, so this has to clear that by
    /// a wide margin.
    pub timeout: Duration,
    /// Whether the receiver echoes.  It does by default; the setting
    /// exists because full duplex can be turned off.
    pub echo: bool,
    /// How long the line must be quiet before [`Session::drain`] calls
    /// it drained.  One character at 19200 takes about half a
    /// millisecond, so this only has to clear the receiver's own gaps.
    pub idle: Duration,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            timeout: Duration::from_secs(5),
            echo: true,
            idle: Duration::from_millis(150),
        }
    }
}

impl Config {
    /// This configuration for a link at `baud`: the timeout raised, if
    /// it has to be, to twice the status screen's wire time, the longest
    /// reply.  At 9600 and above the default stands; at 4800 and below
    /// the screen alone takes most of the default or more, and would
    /// time out on a link that is working.
    pub fn at_rate(&self, baud: BaudRate) -> Self {
        Self {
            timeout: self.timeout.max(baud.wire_time(LONGEST_REPLY_BYTES) * 2),
            ..self.clone()
        }
    }
}

/// The status screen's size, the longest reply: 1574 bytes from a
/// 58503A, rounded up.
const LONGEST_REPLY_BYTES: u32 = 1800;

/// Bytes sent after every command.
const TERMINATOR: &str = "\r\n";

/// How long to wait before asking a quiet transport again.
///
/// Short next to any command's round trip, long enough that a transport
/// returning immediately cannot spin a core.
const IDLE_POLL: Duration = Duration::from_millis(2);

/// A framed command channel to one receiver.
#[derive(Debug)]
pub struct Session<T: Transport> {
    transport: T,
    config: Config,
    /// Bytes read but not yet consumed by a reply.
    buf: String,
    /// Errors drained while explaining a failure that were not that
    /// failure's own.
    ///
    /// The queue is first in, first out, so a command that fails while
    /// something unread is already in it is explained by the wrong
    /// entry unless the whole queue is taken.  Taking it recovers the
    /// attribution and leaves these, which belong to nobody in
    /// particular and would otherwise be silently consumed.  A caller
    /// that cares takes them with [`Session::take_stray_errors`].
    strays: Vec<ErrorEntry>,
    /// Whether the last prompt said the error queue holds anything.  The
    /// prompt follows the queue, not the last command
    /// (`docs/protocol.md`, "The error prompt"), so a queue left
    /// holding errors would make the next command's prompt say nothing
    /// about that command.
    errors_queued: bool,
}

impl<T: Transport> Session<T> {
    /// Wrap a transport.  Does not talk to the receiver; call
    /// [`Session::sync`] for that.
    pub fn new(transport: T, config: Config) -> Self {
        Self {
            transport,
            config,
            buf: String::new(),
            strays: Vec::new(),
            errors_queued: false,
        }
    }

    /// Give the transport back, for a caller that talks to something
    /// other than SCPI on the same port next, such as the debug
    /// console.  Anything still buffered is dropped with the session.
    pub fn into_transport(self) -> T {
        self.transport
    }

    /// What the transport is connected to.
    pub fn describe(&self) -> String {
        self.transport.describe()
    }

    /// Bring the session to a known state.
    ///
    /// Draining first is what makes this reliable.  A command abandoned
    /// mid-reply, such as a timed-out log dump, leaves kilobytes still
    /// arriving; provoking a prompt without draining would match the
    /// prompt that ends the *abandoned* reply, and every exchange after
    /// that reads one reply behind.  Observed on a 58503A, where a
    /// 222-entry log dump desynchronised the rest of a probe run.
    pub fn sync(&mut self) -> Result<Prompt> {
        self.drain()?;
        self.transport.write_all(TERMINATOR.as_bytes())?;
        self.transport.flush()?;
        let (_, prompt) = self.read_to_prompt()?;
        // Drain again: the prompt just matched may have been the tail
        // of what was already in flight rather than the answer to the
        // terminator, and anything left behind belongs to neither.
        self.drain()?;
        Ok(prompt)
    }

    /// Discard everything the receiver is still sending, until the line
    /// has been quiet for [`Config::idle`].
    ///
    /// Giving up is a failure, not a success.  Returning `Ok` at the
    /// deadline with bytes still arriving let [`Session::sync`] match
    /// the prompt belonging to the abandoned reply and report itself
    /// fine while one exchange behind -- which is the misattribution
    /// the whole prompt-framing design exists to prevent.
    pub fn drain(&mut self) -> Result<usize> {
        self.buf.clear();
        let deadline = Instant::now() + self.config.timeout;
        let mut chunk = [0u8; 512];
        let mut discarded = 0usize;
        let mut quiet_since = Instant::now();
        loop {
            let n = self.transport.read(&mut chunk)?;
            if n > 0 {
                discarded += n;
                quiet_since = Instant::now();
            } else if quiet_since.elapsed() >= self.config.idle {
                return Ok(discarded);
            } else {
                std::thread::sleep(IDLE_POLL);
            }
            if Instant::now() >= deadline {
                return Err(Error::Timeout {
                    waited: self.config.timeout,
                    seen: format!("{discarded} bytes still arriving"),
                });
            }
        }
    }

    /// Send a command and read its reply, without interpreting an error
    /// prompt.  Use [`Session::query`] unless you need the raw prompt.
    pub fn send_raw(&mut self, command: &str) -> Result<Reply> {
        self.transport.write_all(command.as_bytes())?;
        self.transport.write_all(TERMINATOR.as_bytes())?;
        self.transport.flush()?;

        let (body, prompt) = self.read_to_prompt()?;
        let body = if self.config.echo {
            strip_echo(&body, command)
        } else {
            body.as_str()
        };
        let lines = body
            .lines()
            .map(str::trim_end)
            .filter(|l| !l.is_empty())
            .map(str::to_owned)
            .collect();
        Ok(Reply { lines, prompt })
    }

    /// Send a command, turning an error prompt into an [`Error::Device`]
    /// by reading the receiver's error queue.
    ///
    /// The prompt shows the queue, not the command (`docs/protocol.md`,
    /// "The error prompt"): it carries the newest queued error, and a
    /// command that succeeds while one is queued answers under it.  So
    /// a queue the last prompt showed holding errors is read off first,
    /// into [`Session::strays`], and only then is the command sent --
    /// except a command that reads or clears the queue itself.
    /// After it, a query that answered succeeded, since a failing one
    /// answers with the prompt alone (097-59551-02, A-6), and whatever
    /// the prompt shows is kept as a stray.  Otherwise the error is the
    /// command's: the whole queue is read, its newest entry explains the
    /// command, and anything older is kept as a stray.  That is right
    /// unless the receiver raised something on its own during the
    /// exchange.
    pub fn query(&mut self, command: &str) -> Result<Reply> {
        // Emptied first, so that an error prompt after this command can
        // only mean something happened since it was sent -- unless the
        // command is how the queue is read or cleared, which emptying it
        // first would leave with nothing to read.
        if self.errors_queued && !reads_queue(command) {
            self.keep_queued_as_strays()?;
        }
        let reply = self.send_raw(command)?;
        let Prompt::Error(prompt) = &reply.prompt else {
            return Ok(reply);
        };
        // A query that fails answers with the prompt alone
        // (097-59551-02, A-6), so one that answered succeeded; the error
        // arrived unasked.
        if is_query(command) && !reply.lines.is_empty() {
            self.keep_queued_as_strays()?;
            return Ok(reply);
        }
        let prompt = prompt.clone();

        let mut own = None;
        for _ in 0..MAX_QUEUE_DRAIN {
            // An answer that did not parse also ends the drain: the
            // receiver and its queue have drifted out of step, which the
            // caller learns from the unexplained error below if nothing
            // was read.
            let Some(entry) = self.next_error()? else {
                break;
            };
            // Whatever was held is older than what just arrived, so it
            // cannot be this command's.
            if let Some(older) = own.replace(entry) {
                self.remember_stray(older);
            }
        }
        match own {
            // The command's own error was discarded by a full queue;
            // the marker describes the queue, and is kept with the
            // other errors the command did not cause.
            Some(entry) if entry.is_overflow() => {
                self.remember_stray(entry);
                Err(Error::ErrorLost { prompt })
            }
            Some(entry) => Err(Error::Device {
                code: entry.code,
                message: entry.message,
            }),
            None => Err(Error::UnexplainedError { prompt }),
        }
    }

    /// Read the error queue until it is empty, and say how many entries
    /// it held.
    ///
    /// For after a probe at the wrong line settings: the receiver
    /// records the garbled bytes as errors that explain nothing and
    /// belong to nobody.  Only the queue is read.  `*CLS` would empty
    /// it too, and also clear the event registers, which are the
    /// front-panel alarm's and not this tool's to clear.
    pub fn discard_errors(&mut self) -> Result<usize> {
        let mut discarded = 0;
        while discarded < MAX_QUEUE_DRAIN && self.next_error()?.is_some() {
            discarded += 1;
        }
        Ok(discarded)
    }

    /// Read the error queue until it is empty, keeping what it held as
    /// strays: errors no command of ours is known to have caused.
    fn keep_queued_as_strays(&mut self) -> Result<()> {
        for _ in 0..MAX_QUEUE_DRAIN {
            let Some(entry) = self.next_error()? else {
                break;
            };
            self.remember_stray(entry);
        }
        Ok(())
    }

    /// Read one entry off the error queue: `None` when it is empty, or
    /// when the answer does not parse.
    fn next_error(&mut self) -> Result<Option<ErrorEntry>> {
        let detail = self.send_raw(":SYSTem:ERRor?")?;
        Ok(detail
            .lines
            .first()
            .and_then(|l| parse_error(l))
            .filter(|(code, _)| *code != 0)
            .map(|(code, message)| ErrorEntry { code, message }))
    }

    /// Take the errors drained while explaining other failures.
    ///
    /// Empty almost always.  Non-empty means the receiver had raised
    /// something nobody had read when a command happened to fail, and
    /// these are those: real errors, correctly detached from the
    /// command that merely uncovered them.
    pub fn take_stray_errors(&mut self) -> Vec<ErrorEntry> {
        std::mem::take(&mut self.strays)
    }

    /// Keep a stray, bounded.
    ///
    /// Nothing obliges a caller to collect them, and a receiver
    /// refusing everything could otherwise grow this without limit in
    /// a daemon meant to run for months.  The oldest go first: the
    /// earliest error is usually the one that explains the rest, but
    /// not at the cost of the memory of a process nobody is watching.
    fn remember_stray(&mut self, entry: ErrorEntry) {
        if self.strays.len() >= MAX_STRAY_ERRORS {
            self.strays.remove(0);
        }
        self.strays.push(entry);
    }

    /// Read until a prompt closes the reply, returning the body before
    /// it.
    fn read_to_prompt(&mut self) -> Result<(String, Prompt)> {
        let deadline = Instant::now() + self.config.timeout;
        let mut chunk = [0u8; 512];
        loop {
            if let Some((body, prompt)) = split_prompt(&self.buf) {
                let body = body.to_owned();
                self.buf.clear();
                self.errors_queued = matches!(prompt, Prompt::Error(_));
                return Ok((body, prompt));
            }
            if Instant::now() >= deadline {
                return Err(Error::Timeout {
                    waited: self.config.timeout,
                    seen: std::mem::take(&mut self.buf),
                });
            }
            let n = self.transport.read(&mut chunk)?;
            if n > 0 {
                self.buf.push_str(&String::from_utf8_lossy(&chunk[..n]));
            } else {
                // A transport whose read returns at once when idle would
                // otherwise turn this into a busy loop.  The serial port
                // blocks for its own timeout, but nothing in the
                // contract requires that.
                std::thread::sleep(IDLE_POLL);
            }
        }
    }
}

/// Remove the receiver's echo of the command it was sent.
///
/// Leading whitespace is trimmed first because the prompt's trailing
/// space often arrives after the prompt has already been recognised,
/// and so turns up at the head of the next reply.
fn strip_echo<'a>(body: &'a str, command: &str) -> &'a str {
    let trimmed = body.trim_start();
    match trimmed.strip_prefix(command) {
        // Keep leading spaces after the echo: status screen columns
        // depend on them.
        Some(rest) => rest.trim_start_matches(['\r', '\n']),
        None => trimmed,
    }
}

/// Split a buffer into its body and the prompt that closes it, if a
/// complete prompt has arrived.
fn split_prompt(buf: &str) -> Option<(&str, Prompt)> {
    // A prompt is the last thing on the wire and is not newline
    // terminated, so it is whatever follows the final newline.
    let (body, tail) = match buf.rfind('\n') {
        Some(i) => buf.split_at(i + 1),
        None => ("", buf),
    };
    // The manuals render the prompt "scpi>", but the wire carries
    // "scpi > ", with a space on both sides of the angle bracket.  The
    // previous prompt's trailing space can also land at the front of
    // this one: the Z3801A, which does not echo, sends a command with
    // no reply as nothing but " E-230> " -- no newline to split on, so
    // the stray space is part of the tail.
    let tail = tail.trim();
    let token = tail.strip_suffix('>')?.trim_end();
    let prompt = if token.eq_ignore_ascii_case("scpi") {
        Prompt::Ready
    } else if token.starts_with('E') || token.starts_with('e') {
        Prompt::Error(tail.to_owned())
    } else {
        return None;
    };
    Some((body, prompt))
}

/// Whether a command reads or clears the error queue: `:SYSTem:ERRor?`
/// in any of its spellings, or `*CLS`.
fn reads_queue(command: &str) -> bool {
    let header = command
        .split_whitespace()
        .next()
        .unwrap_or_default()
        .trim_start_matches(':')
        .to_ascii_uppercase();
    header == "*CLS"
        || matches!(
            header.as_str(),
            "SYST:ERR?" | "SYST:ERROR?" | "SYSTEM:ERR?" | "SYSTEM:ERROR?"
        )
}

/// Whether a command asks for an answer: its header ends in `?`.
fn is_query(command: &str) -> bool {
    command
        .split_whitespace()
        .next()
        .is_some_and(|header| header.ends_with('?'))
}

/// How many entries one explanation will take from the queue.
///
/// The receiver's queue holds thirty, 097-59551-02 5-31, so this ends
/// a drain that is not converging rather than bounding a real queue.
const MAX_QUEUE_DRAIN: usize = 32;

/// How many unattributed errors to keep for a caller that may never
/// ask.
pub(crate) const MAX_STRAY_ERRORS: usize = 32;

/// Parse `:SYSTem:ERRor?`, documented as `<code>,"<description>"`.
fn parse_error(line: &str) -> Option<(i32, String)> {
    let (code, message) = line.split_once(',')?;
    let code = code.trim().parse().ok()?;
    let message = message
        .trim()
        .trim_matches(|c| c == '"' || c == '\u{201c}' || c == '\u{201d}')
        .to_owned();
    Some((code, message))
}

#[cfg(test)]
mod tests {
    use super::Prompt;
    use super::parse_error;
    use super::split_prompt;
    use super::strip_echo;

    #[test]
    fn a_ready_prompt_closes_a_reply() {
        let (body, prompt) = split_prompt(":GPS:POS?\r\n+37,19,32.4\r\nscpi> ").expect("prompt");
        assert_eq!(prompt, Prompt::Ready);
        assert!(body.ends_with("+37,19,32.4\r\n"));
    }

    #[test]
    fn an_error_prompt_is_recognised_and_kept() {
        let (_, prompt) = split_prompt("bogus\r\nE-113 > ").expect("prompt");
        assert_eq!(prompt, Prompt::Error("E-113 >".to_owned()));
    }

    #[test]
    fn a_partial_prompt_is_not_a_prompt() {
        // Still arriving; committing here would misframe the next read.
        assert!(split_prompt(":GPS:POS?\r\n+37,19,32.4\r\nscp").is_none());
        assert!(split_prompt("").is_none());
    }

    #[test]
    fn the_prompt_is_recognised_however_it_is_spaced() {
        // The wire carries "scpi > "; the manuals print "scpi>".  Accept
        // both, and the form with no trailing space too.
        for form in ["scpi > ", "scpi >", "scpi> ", "scpi>"] {
            assert_eq!(
                split_prompt(form).map(|(_, p)| p),
                Some(Prompt::Ready),
                "did not recognise {form:?}"
            );
        }
    }

    #[test]
    fn a_prompt_behind_the_last_ones_stray_space_is_still_a_prompt() {
        // Seen on a Z3801A: a command with no reply, from a receiver
        // that does not echo, arrives as the previous prompt's trailing
        // space and then the prompt, with no newline between.
        assert_eq!(
            split_prompt(" E-230> "),
            Some(("", Prompt::Error("E-230>".to_owned())))
        );
        assert_eq!(split_prompt(" scpi> "), Some(("", Prompt::Ready)));
        assert!(split_prompt(" scp").is_none());
    }

    #[test]
    fn echo_is_removed_only_when_it_matches() {
        assert_eq!(
            strip_echo("\r\n:GPS:POS?\r\n+37\r\n", ":GPS:POS?"),
            "+37\r\n"
        );
        // Echo off, or a receiver that did not echo: leave the body be.
        assert_eq!(strip_echo("+37\r\n", ":GPS:POS?"), "+37\r\n");
    }

    #[test]
    fn the_prompts_orphaned_trailing_space_does_not_defeat_echo_removal() {
        // Observed on a 58503A: the prompt is "scpi > ", but the final
        // space lands after the prompt has been recognised, so it heads
        // the next reply.
        assert_eq!(
            strip_echo(" *IDN?\r\nHEWLETT-PACKARD\r\n", "*IDN?"),
            "HEWLETT-PACKARD\r\n"
        );
    }

    #[test]
    fn leading_spaces_inside_a_reply_are_kept() {
        // Status screen columns are significant.
        assert_eq!(
            strip_echo(" :SYST:STAT?\r\n   PRN El Az\r\n", ":SYST:STAT?"),
            "   PRN El Az\r\n"
        );
    }

    #[test]
    fn errors_parse_with_either_quote_style() {
        // The manuals print curly quotes; the receiver sends straight
        // ones.  Accept both so fixtures typed from the PDF still work.
        assert_eq!(
            parse_error("-113,\"Undefined header\""),
            Some((-113, "Undefined header".to_owned()))
        );
        assert_eq!(
            parse_error("-113,\u{201c}Undefined header\u{201d}"),
            Some((-113, "Undefined header".to_owned()))
        );
        assert_eq!(
            parse_error("0,\"No error\""),
            Some((0, "No error".to_owned()))
        );
    }

    #[test]
    fn the_timeout_covers_the_status_screen_at_slow_rates() {
        use super::Config;
        use crate::types::BaudRate;
        let default = Config::default();
        assert_eq!(default.at_rate(BaudRate::B19200).timeout, default.timeout);
        assert_eq!(default.at_rate(BaudRate::B9600).timeout, default.timeout);
        assert!(default.at_rate(BaudRate::B4800).timeout > default.timeout);
        assert!(default.at_rate(BaudRate::B2400).timeout >= std::time::Duration::from_secs(15));
        assert!(default.at_rate(BaudRate::B1200).timeout >= std::time::Duration::from_secs(30));
    }

    #[test]
    fn reading_the_queue_is_recognized_in_any_spelling() {
        use super::reads_queue;
        for command in [
            ":SYSTem:ERRor?",
            "SYST:ERR?",
            ":system:error?",
            "*CLS",
            "*cls",
        ] {
            assert!(reads_queue(command), "{command}");
        }
        for command in ["*IDN?", ":SYSTem:STATus?", ":SYST:ERR"] {
            assert!(!reads_queue(command), "{command}");
        }
    }
}
