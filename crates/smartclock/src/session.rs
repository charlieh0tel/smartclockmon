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
}

impl Default for Config {
    fn default() -> Self {
        Self {
            timeout: Duration::from_secs(5),
            echo: true,
        }
    }
}

/// Bytes sent after every command.
const TERMINATOR: &str = "\r\n";

/// A framed command channel to one receiver.
#[derive(Debug)]
pub struct Session<T: Transport> {
    transport: T,
    config: Config,
    /// Bytes read but not yet consumed by a reply.
    buf: String,
}

impl<T: Transport> Session<T> {
    /// Wrap a transport.  Does not talk to the receiver; call
    /// [`Session::sync`] for that.
    pub fn new(transport: T, config: Config) -> Self {
        Self {
            transport,
            config,
            buf: String::new(),
        }
    }

    /// What the transport is connected to.
    pub fn describe(&self) -> String {
        self.transport.describe()
    }

    /// Bring the session to a known state by provoking a prompt and
    /// discarding whatever precedes it.  Needed on connect, because the
    /// receiver may be mid-reply from a previous user.
    pub fn sync(&mut self) -> Result<Prompt> {
        self.buf.clear();
        self.transport.write_all(TERMINATOR.as_bytes())?;
        self.transport.flush()?;
        let (_, prompt) = self.read_to_prompt()?;
        Ok(prompt)
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
    pub fn query(&mut self, command: &str) -> Result<Reply> {
        let reply = self.send_raw(command)?;
        let Prompt::Error(prompt) = &reply.prompt else {
            return Ok(reply);
        };
        let prompt = prompt.clone();

        // The error prompt says only that something failed.  The queue
        // says what, and reading it also clears it, which keeps the next
        // command from inheriting a stale error prompt.
        let detail = self.send_raw(":SYSTem:ERRor?")?;
        match detail.lines.first().map(|l| parse_error(l)) {
            Some(Some((0, _))) | None => Err(Error::UnexplainedError { prompt }),
            Some(Some((code, message))) => Err(Error::Device { code, message }),
            Some(None) => Err(Error::UnexplainedError { prompt }),
        }
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
            }
        }
    }
}

/// Remove the receiver's echo of the command it was sent.
fn strip_echo<'a>(body: &'a str, command: &str) -> &'a str {
    let trimmed = body.trim_start_matches(['\r', '\n']);
    match trimmed.strip_prefix(command) {
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
    let token = tail.strip_suffix(' ').unwrap_or(tail).strip_suffix('>')?;
    let prompt = if token.eq_ignore_ascii_case("scpi") {
        Prompt::Ready
    } else if token.starts_with('E') || token.starts_with('e') {
        Prompt::Error(tail.trim_end().to_owned())
    } else {
        return None;
    };
    Some((body, prompt))
}

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
        let (_, prompt) = split_prompt("bogus\r\nE-113> ").expect("prompt");
        assert_eq!(prompt, Prompt::Error("E-113>".to_owned()));
    }

    #[test]
    fn a_partial_prompt_is_not_a_prompt() {
        // Still arriving; committing here would misframe the next read.
        assert!(split_prompt(":GPS:POS?\r\n+37,19,32.4\r\nscp").is_none());
        assert!(split_prompt("").is_none());
    }

    #[test]
    fn a_prompt_without_its_trailing_space_still_counts() {
        let (_, prompt) = split_prompt("scpi>").expect("prompt");
        assert_eq!(prompt, Prompt::Ready);
    }

    #[test]
    fn echo_is_removed_only_when_it_matches() {
        assert_eq!(strip_echo("\r\n:GPS:POS?\r\n+37\r\n", ":GPS:POS?"), "+37\r\n");
        // Echo off, or a receiver that did not echo: leave the body be.
        assert_eq!(strip_echo("+37\r\n", ":GPS:POS?"), "+37\r\n");
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
        assert_eq!(parse_error("0,\"No error\""), Some((0, "No error".to_owned())));
    }
}
