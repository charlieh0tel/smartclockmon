//! The local socket server.
//!
//! Authorization is socket permissions and nothing else: systemd's
//! `RuntimeDirectory` and its mode decide who can open the socket, and
//! anyone who can may issue whatever the configuration allows.  On a
//! single-operator machine that is the whole of it -- no peer
//! credentials, no tokens.  The cost is that the daemon cannot tell two
//! clients apart, so the audit trail records what was done but not by
//! whom.

use std::io::BufRead;
use std::io::BufReader;
use std::io::Read as _;
use std::io::Write;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering;
use std::thread;

use anyhow::Context as _;
use anyhow::Result;
use interprocess::local_socket::Listener;
use interprocess::local_socket::ListenerOptions;
use interprocess::local_socket::Name;
use interprocess::local_socket::Stream;
use interprocess::local_socket::prelude::*;
use smartclock::command::Argument;
use smartclock::command::Class;
use smartclock::command::Dialect;
use smartclock::command::Spec;
use smartclock::task::Cadence;
use smartclock::task::Handle;

use crate::audit::Audit;
use crate::proto::Message;
use crate::proto::Op;
use crate::proto::Request;
use crate::proto::VERSION;
use smartclock::wire::Reading;

/// Longest request line accepted.
///
/// A line is read until a newline, so without a cap a client that
/// never sends one grows a single string in the daemon for as long as
/// it keeps writing.  A status screen query is a few dozen bytes; this
/// is generous.
const MAX_REQUEST: u64 = 8192;

/// How many clients may be connected at once.
///
/// Each takes two threads.  Socket permissions decide who may connect
/// at all, but nothing stopped one mistaken loop from opening
/// connections until the daemon ran out of threads, and the failure
/// mode for that used to be a daemon that looked healthy and could
/// never be reached again.
const MAX_CLIENTS: usize = 16;

/// What a client is permitted to do.
///
/// Set once from the command line and never from the socket.  A daemon
/// started without a flag cannot be talked into the commands it gates,
/// which is the point: enabling one is a deliberate act outside the
/// client, and it survives any amount of fat-fingering at the socket.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct Policy {
    /// Allow commands the table marks `control`.
    pub(crate) control: bool,
    /// Allow commands the table marks `dangerous`.
    pub(crate) dangerous: bool,
    /// Allow raw SCPI that the table does not recognise at all.
    pub(crate) raw: bool,
}

impl Policy {
    /// Whether a class of command is permitted.
    fn allows(self, class: Class) -> bool {
        match class {
            Class::Query => true,
            Class::Control => self.control,
            Class::Dangerous => self.dangerous,
        }
    }

    /// The flag that would permit a class.
    fn flag(class: Class) -> &'static str {
        match class {
            Class::Query => "",
            Class::Control => "--allow-control",
            Class::Dangerous => "--allow-dangerous",
        }
    }
}

/// What the daemon tells a client about itself, shared so a reconnect
/// can replace it.
///
/// Frozen at the first connection, this went stale in the way that
/// matters: the task polls the newly opened device with its own
/// dialect while the socket kept classifying client commands against
/// the first one.  Repoint a ser2net endpoint, or swap a 58503A for a
/// Z3801A on the same by-id path, and the gate reads the wrong command
/// table.
pub(crate) type SharedInfo = Arc<Mutex<Info>>;

/// Take a lock, poisoned or not.
///
/// A poisoned `Info` means some thread panicked while holding it, not
/// that the contents are wrong -- it is a receiver's identity and a set
/// of flags, all written whole.  Refusing to serve because of it would
/// turn one panicked client thread into a daemon that answers nobody.
pub(crate) fn lock_or_poisoned<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// What the daemon tells a client about itself.
#[derive(Debug, Clone)]
pub(crate) struct Info {
    /// The receiver's identity string.
    pub identity: String,
    /// Which command tree is in use.
    pub dialect: Dialect,
    /// Where the snapshot log lives, so a client can open it read-only.
    pub database: String,
    /// What clients may do.
    pub policy: Policy,
    /// How often each tier is polled.  A client showing the age of a
    /// field needs these to know what counts as late; the defaults are
    /// only defaults.
    pub cadence: Cadence,
    /// Where to record commands that were not scheduled polls.
    pub audit: Audit,
}

/// Bind the socket.
///
/// Separate from [`serve`] so a bind failure reaches the caller.  When
/// the bind happened inside the serving thread, a daemon that could not
/// bind printed one line from a dying thread and then ran forever
/// claiming to listen.
pub(crate) fn bind(name: Name<'static>) -> Result<Listener> {
    ListenerOptions::new()
        .name(name)
        .create_sync()
        .context("binding the local socket")
}

/// One connected client, counted for as long as this is held.
///
/// The count is what `MAX_CLIENTS` is enforced against, so releasing it
/// has to survive the serving thread panicking.
struct Slot(Arc<AtomicUsize>);

impl Slot {
    fn take(clients: &Arc<AtomicUsize>) -> Self {
        clients.fetch_add(1, Ordering::Relaxed);
        Self(Arc::clone(clients))
    }
}

impl Drop for Slot {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::Relaxed);
    }
}

/// Listen for clients until the process ends.
pub(crate) fn serve(listener: Listener, handle: Handle, info: SharedInfo) -> Result<()> {
    let clients = Arc::new(AtomicUsize::new(0));
    for incoming in listener.incoming() {
        let stream = match incoming {
            Ok(stream) => stream,
            // One client failing to connect is not a reason to stop
            // serving the others.
            Err(e) => {
                eprintln!("smartclockd: rejected a connection: {e}");
                continue;
            }
        };
        if clients.load(Ordering::Relaxed) >= MAX_CLIENTS {
            eprintln!("smartclockd: refusing a client, {MAX_CLIENTS} already connected");
            // Say so rather than closing silently: a bare reset reads
            // as the daemon having crashed, which is the wrong thing
            // for an operator to go and investigate.
            let writer = Arc::new(Mutex::new(stream));
            let _ = write_line(
                &writer,
                &Message::err(
                    String::new(),
                    format!("{MAX_CLIENTS} clients are already connected"),
                ),
            );
            continue;
        }
        let handle = handle.clone();
        let info = Arc::clone(&info);
        let slot = Slot::take(&clients);
        // A spawn failure must not end the accept loop.  It used to
        // propagate, so one transient EAGAIN under thread pressure left
        // the daemon polling and logging, looking healthy, while no
        // client could ever connect again.
        let spawned = thread::Builder::new()
            .name("smartclockd-client".to_owned())
            .spawn(move || {
                // The slot is released when this closure's frame goes,
                // whether it returns or unwinds.  Decrementing on the
                // way out by hand leaked a slot permanently on a panic,
                // sixteen of which would have left the daemon accepting
                // nobody.
                let _slot = slot;
                if let Err(e) = talk(stream, &handle, &info) {
                    eprintln!("smartclockd: client ended: {e}");
                }
            });
        if let Err(e) = spawned {
            eprintln!("smartclockd: could not serve a client: {e}");
        }
    }
    Ok(())
}

/// Serve one client until it goes away.
///
/// Snapshots are pushed from their own thread, so a client slow to read
/// cannot hold up its own requests.  Both threads write to the same
/// half behind a mutex; the messages are single short lines, so holding
/// it is brief and it keeps them from interleaving mid-line.
fn talk(stream: Stream, handle: &Handle, info: &SharedInfo) -> Result<()> {
    let (recv, send_half) = stream.split();
    let writer = Arc::new(Mutex::new(send_half));

    // A client gets the current state immediately, so it can render
    // before the next poll rather than showing an empty screen.
    if let Some(snapshot) = handle.latest() {
        write_line(&writer, &Message::event(&snapshot))?;
    }

    let updates = handle.subscribe();
    let pusher = Arc::clone(&writer);
    thread::Builder::new()
        .name("smartclockd-push".to_owned())
        .spawn(move || {
            for snapshot in updates {
                if write_line(&pusher, &Message::event(&snapshot)).is_err() {
                    return;
                }
            }
        })
        .context("spawning the push thread")?;

    // Capped: an unterminated line would otherwise grow without limit.
    let mut reader = BufReader::new(recv);
    loop {
        let mut line = String::new();
        let read = (&mut reader).take(MAX_REQUEST + 1).read_line(&mut line)?;
        if read == 0 {
            return Ok(());
        }
        if read as u64 > MAX_REQUEST {
            write_line(
                &writer,
                &Message::err(
                    String::new(),
                    format!("a request may not exceed {MAX_REQUEST} bytes"),
                ),
            )?;
            return Ok(());
        }
        if line.trim().is_empty() {
            continue;
        }
        // Parse before locking, and hold the guard only across the
        // request itself: cloning the whole Info -- two Strings, the
        // policy and the audit handle -- per request line bought
        // nothing, since the lock is uncontended except at reconnect.
        // Read per request rather than per connection, so a reconnect
        // to a different receiver takes effect for the next command.
        let reply = match serde_json::from_str::<Request>(&line) {
            Ok(request) => handle_request(request, handle, &lock_or_poisoned(info)),
            Err(e) => Message::err(String::new(), format!("malformed request: {e}")),
        };
        write_line(&writer, &reply)?;
    }
}

fn handle_request(request: Request, handle: &Handle, info: &Info) -> Message {
    if request.v != VERSION {
        return Message::err(
            request.id,
            format!("this daemon speaks protocol {VERSION}, not {}", request.v),
        );
    }
    let id = request.id;
    match request.op {
        Op::Latest => match handle.latest() {
            Some(snapshot) => match serde_json::to_value(Reading::from(&snapshot)) {
                Ok(value) => Message::ok(id, value),
                Err(e) => Message::err(id, e),
            },
            None => Message::err(id, "nothing has been polled yet"),
        },
        Op::Info => Message::ok(
            id,
            serde_json::json!({
                "identity": info.identity,
                "dialect": format!("{:?}", info.dialect),
                "database": info.database,
                "allow_control": info.policy.control,
                "allow_dangerous": info.policy.dangerous,
                "allow_raw": info.policy.raw,
                "cadence_fast": info.cadence.fast.as_secs_f64(),
                "cadence_medium": info.cadence.medium.as_secs_f64(),
                "cadence_slow": info.cadence.slow.as_secs_f64(),
            }),
        ),
        Op::Query { scpi } => send(id, &scpi, handle, info),
    }
}

/// Run a client's command, if the policy permits it.
///
/// The gate is the command table, not a list kept here, so a command
/// reclassified there is reclassified for clients too.  A spelling the
/// table does not know is refused unless raw is allowed, since an
/// unknown command could be anything.
fn send(id: String, scpi: &str, handle: &Handle, info: &Info) -> Message {
    let scpi = scpi.trim();
    // Before anything else, because the class is read from the header
    // and everything after it is passed through untouched.
    if let Err(why) = well_formed(scpi) {
        return Message::err(id, why);
    }
    // A command the table knows brings its spec with it; one it does
    // not has no argument rules to check, only a guessed class.
    let (spec, class, to_send) = match classify(scpi, info.dialect) {
        Some((spec, to_send)) => (Some(spec), spec.class, to_send),
        None if info.policy.raw => (None, raw_class(scpi), scpi.to_owned()),
        None => {
            return Message::err(
                id,
                format!("{scpi} is not a command this dialect knows, and --allow-raw is off"),
            );
        }
    };
    let to_send = to_send.as_str();

    if let Some(spec) = spec {
        if let Err(why) = check_argument(scpi, spec) {
            return Message::err(id, why);
        }
    }
    if !info.policy.allows(class) {
        return Message::err(
            id,
            format!(
                "{scpi} is {class:?}; this daemon was started without {}",
                Policy::flag(class)
            ),
        );
    }

    let outcome = handle.request(to_send);
    let note = match &outcome {
        Ok(reply) if reply.lines.is_empty() => "ok".to_owned(),
        Ok(reply) => format!("ok: {}", reply.lines.join(" | ")),
        Err(e) => format!("failed: {e}"),
    };
    info.audit.record(to_send, class, &note);

    // A command that changed something should not wait up to a minute
    // to show in the snapshots.
    if class != Class::Query && outcome.is_ok() {
        handle.refresh();
    }

    match outcome {
        Ok(reply) => Message::ok(id, serde_json::json!({ "lines": reply.lines })),
        Err(e) => Message::err(id, e),
    }
}

/// Refuse anything that could carry a second command.
///
/// The class of a command with an argument comes from its header, and
/// the argument is concatenated in and written to the receiver
/// verbatim.  SCPI chains program message units with `;`, and the
/// transport appends only a terminator, so without this check a
/// permitted header smuggles anything after it:
/// `:SYSTem:STATus? ;:SYSTem:COMMunicate:SERial1:BAUD 1200` classified
/// as a query and ran on a daemon started with no flags at all,
/// stranding the link across power cycles.  An embedded newline does
/// the same on firmware that does not chain, and desynchronises the
/// session besides, since the reply to the second command is left in
/// the buffer for whatever asks next.
///
/// So arguments are whitelisted rather than filtered.  Everything this
/// receiver takes is a number, a bare word, a comma-separated list or a
/// quoted string; a colon or a semicolon in an argument is not a
/// legitimate value, it is a second command.
fn well_formed(scpi: &str) -> Result<(), String> {
    if scpi.is_empty() {
        return Err("an empty command".to_owned());
    }
    if !scpi.is_ascii() {
        return Err(format!("{scpi} is not ASCII"));
    }
    if let Some(bad) = scpi.chars().find(|c| c.is_control()) {
        return Err(format!(
            "{scpi:?} contains a control character ({:#04x}); \
             a command is one line",
            bad as u32
        ));
    }
    if scpi.contains(';') {
        return Err(format!(
            "{scpi} chains commands with ';', which would carry a second \
             command past the class check"
        ));
    }
    // The header may hold anything the table spells; only the argument
    // is restricted, and only to what a value can look like.
    if let Some((_, argument)) = scpi.split_once(char::is_whitespace) {
        const ALLOWED: [char; 7] = [' ', ',', '.', '+', '-', '"', '_'];
        if let Some(bad) = argument
            .chars()
            .find(|c| !c.is_ascii_alphanumeric() && !ALLOWED.contains(c))
        {
            return Err(format!(
                "{bad:?} is not allowed in an argument; values are numbers, \
                 words, lists and quoted strings"
            ));
        }
    }
    Ok(())
}

/// Find a command in the table and return its class and the string to
/// send.
///
/// Matching cannot be on the whole string.  Some entries carry their
/// argument, such as `:GPS:POSition:SURVey:STATe ONCE`, while others
/// take one the caller supplies, and an exact comparison made every
/// command with an argument unreachable: the table holds
/// `:GPS:SATellite:TRACking:EMANgle` while a client sends it followed
/// by a number.
///
/// So the whole string is tried first, then the part before the first
/// space as a header with the rest as its argument.  The table's
/// spelling is what gets sent, so a client may use any casing.
fn classify(scpi: &str, dialect: Dialect) -> Option<(&'static Spec, String)> {
    let specs = dialect.specs();
    if let Some(spec) = specs.iter().find(|s| s.scpi.eq_ignore_ascii_case(scpi)) {
        return Some((spec, spec.scpi.to_owned()));
    }
    let (header, argument) = scpi.split_once(char::is_whitespace)?;
    let argument = argument.trim();
    let spec = specs
        .iter()
        .find(|s| s.scpi.eq_ignore_ascii_case(header.trim()))?;
    Some((spec, format!("{} {argument}", spec.scpi)))
}

/// Check a command's argument against what the table permits.
///
/// The receiver would refuse an out-of-range value itself, so this is
/// defence in depth rather than a safety requirement.  What it buys is
/// a message naming the command and the bound, and an exchange that
/// never happened rather than one recorded in the audit trail as a
/// command that was sent and refused.
/// Takes the spec [`classify`] matched rather than looking one up
/// again: two searches with slightly different rules could disagree
/// about which entry a command is, and the gate must not be the place
/// that happens.
fn check_argument(scpi: &str, spec: &Spec) -> Result<(), String> {
    // An entry that carries its own argument, such as
    // ":GPS:POSition:SURVey:STATe ONCE", is already complete.
    if spec.scpi.eq_ignore_ascii_case(scpi) {
        return Ok(());
    }
    let argument = scpi
        .split_once(char::is_whitespace)
        .map_or("", |(_, argument)| argument.trim());

    match spec.argument {
        Argument::Free => Ok(()),
        Argument::None if argument.is_empty() => Ok(()),
        Argument::None => Err(format!("{} takes no argument", spec.scpi)),
        Argument::Integer { min, max } => match argument.parse::<i64>() {
            Ok(value) if (min..=max).contains(&value) => Ok(()),
            Ok(value) => Err(format!(
                "{value} is outside {min} to {max} for {}",
                spec.scpi
            )),
            Err(_) => Err(format!("{} takes a whole number", spec.scpi)),
        },
        Argument::Word(allowed) => {
            if allowed.iter().any(|w| w.eq_ignore_ascii_case(argument)) {
                Ok(())
            } else {
                Err(format!("{} takes one of {}", spec.scpi, allowed.join(", ")))
            }
        }
    }
}

/// Guess a class for a command the table does not know.
///
/// Raw passthrough is off by default and this only applies when it is
/// on, but even then an unrecognised command is treated as at least
/// control, and anything resembling the ones that can strand the link
/// as dangerous.  Guessing generously costs a client one more flag;
/// guessing kindly could erase the receiver.
fn raw_class(scpi: &str) -> Class {
    let upper = scpi.to_ascii_uppercase();
    // Short forms cannot evade these: SCPI's mandatory abbreviations are
    // COMMunicate, PRESet, ERASe and LANGuage, so every legal spelling
    // still contains its needle.  The rest are from the receiver's own
    // keyword table in docs/z3801-keywords.md: resets, anything that
    // writes non-volatile memory, and any other route to the UART,
    // since the gate's safety must not rest on COMMunicate being the
    // only one.  Turning the prompt off would strand the link as surely
    // as a baud change, because the session frames on it.
    const STRANDS: [&str; 12] = [
        "COMM", "PRES", "ERAS", "LANG", "RST", "EEPR", "WRIT", "SAVE", "BAUD", "UART", "PROM",
        "MEM",
    ];
    if STRANDS.iter().any(|needle| upper.contains(needle)) {
        return Class::Dangerous;
    }
    // A trailing '?' means a query only when there is nothing else in
    // the string: it is the last character of the whole command, so on
    // its own it would call anything ending in one a read.
    let is_bare_query = scpi.ends_with('?') && !scpi.contains(char::is_whitespace);
    if is_bare_query {
        Class::Query
    } else {
        Class::Control
    }
}

fn write_line<W: Write>(writer: &Arc<Mutex<W>>, message: &Message) -> Result<()> {
    let line = serde_json::to_string(message)?;
    let mut writer = writer
        .lock()
        .map_err(|_| anyhow::anyhow!("poisoned writer"))?;
    writer.write_all(line.as_bytes())?;
    writer.write_all(b"\n")?;
    writer.flush()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::Policy;
    use super::classify;
    use super::raw_class;
    use super::well_formed;
    use smartclock::command::Argument;
    use smartclock::command::Class;
    use smartclock::command::Dialect;

    const HP: Dialect = Dialect::Hp58503;

    /// What `send` does: classify, then check the argument against the
    /// spec classify matched, rather than looking one up a second time.
    fn check_argument(scpi: &str, dialect: Dialect) -> Result<(), String> {
        match classify(scpi, dialect) {
            Some((spec, _)) => super::check_argument(scpi, spec),
            None => Ok(()),
        }
    }

    #[test]
    fn a_command_with_no_argument_matches_whole() {
        let (spec, sent) = classify(":SYNChronization:TINTerval?", HP).expect("known");
        let class = spec.class;
        assert_eq!(class, Class::Query);
        assert_eq!(sent, ":SYNChronization:TINTerval?");
    }

    #[test]
    fn a_caller_supplied_argument_is_matched_by_header() {
        // The regression this exists for.  The table holds the header
        // alone, so comparing whole strings made every command taking an
        // argument unreachable, and refused it as though it were a typo.
        let (spec, sent) =
            classify(":GPS:SATellite:TRACking:EMANgle 10", HP).expect("known with argument");
        let class = spec.class;
        assert_eq!(class, Class::Control);
        assert_eq!(sent, ":GPS:SATellite:TRACking:EMANgle 10");
    }

    #[test]
    fn an_entry_carrying_its_own_argument_still_matches() {
        // :GPS:POSition:SURVey:STATe ONCE is one table entry, argument
        // and all, so the whole-string attempt has to come first.
        let (spec, sent) = classify(":GPS:POSition:SURVey:STATe ONCE", HP).expect("known");
        let class = spec.class;
        assert_eq!(class, Class::Control);
        assert_eq!(sent, ":GPS:POSition:SURVey:STATe ONCE");
    }

    #[test]
    fn an_argument_outside_its_range_is_refused_here_not_by_the_receiver() {
        // The receiver would answer -222, but refusing it here names the
        // command and the bound, and keeps a command that was never
        // going to work out of the audit trail.
        assert!(check_argument(":GPS:SATellite:TRACking:EMANgle 91", HP).is_err());
        assert!(check_argument(":GPS:SATellite:TRACking:EMANgle -1", HP).is_err());
        assert!(check_argument(":GPS:SATellite:TRACking:EMANgle abc", HP).is_err());
        assert!(check_argument(":GPS:SATellite:TRACking:EMANgle 90", HP).is_ok());
        assert!(check_argument(":GPS:SATellite:TRACking:EMANgle 0", HP).is_ok());
    }

    #[test]
    fn a_word_argument_must_be_one_of_the_documented_ones() {
        assert!(check_argument(":SYSTem:COMMunicate:SERial1:BAUD 38400", HP).is_err());
        assert!(check_argument(":SYSTem:COMMunicate:SERial1:BAUD 9600", HP).is_ok());
        // Case is the receiver's business, not the caller's.
        assert!(check_argument(":PTIMe:TCODe:FORMat f2", HP).is_ok());
        assert!(check_argument(":PTIMe:TCODe:FORMat F3", HP).is_err());
    }

    #[test]
    fn a_command_taking_no_argument_is_refused_one() {
        assert!(check_argument(":SYNChronization:HOLDover:INITiate now", HP).is_err());
        assert!(check_argument(":SYNChronization:HOLDover:INITiate", HP).is_ok());
        // An entry that carries its own argument is already complete.
        assert!(check_argument(":GPS:POSition:SURVey:STATe ONCE", HP).is_ok());
    }

    #[test]
    fn a_query_header_cannot_carry_a_payload() {
        // The gate reads the class from the header and passes the
        // argument through, so an argument nobody constrained was an
        // argument the receiver got to interpret.  Every entry used to
        // default to Argument::Free, which meant a query header --
        // permitted by the default policy -- could carry the payload of
        // the set form beside it.  Both of these reached the receiver.
        assert!(check_argument(":SYSTem:LANGuage? \"INSTALL\"", HP).is_err());
        assert!(check_argument(":SYSTem:COMMunicate:SERial1:BAUD? 1200", HP).is_err());
        // The bare queries are still fine.
        assert!(check_argument(":SYSTem:LANGuage?", HP).is_ok());
        assert!(check_argument(":SYSTem:COMMunicate:SERial1:BAUD?", HP).is_ok());
    }

    #[test]
    fn only_named_commands_take_an_unconstrained_argument() {
        // Argument::Free is now something an entry asks for.  If this
        // list grows, the entry that grew it should be one that really
        // does take an argument too various to describe.
        let free: Vec<&str> = HP
            .specs()
            .iter()
            .filter(|s| s.argument == Argument::Free)
            .map(|s| s.scpi)
            .collect();
        assert_eq!(
            free,
            vec![
                ":GPS:POSition",
                ":GPS:SATellite:TRACking:IGNore",
                ":GPS:SATellite:TRACking:INCLude",
                ":GPS:REFerence:ADELay",
                ":GPS:INITial:DATE",
                ":GPS:INITial:TIME",
                ":GPS:INITial:POSition",
                ":SYNChronization:HOLDover:RECovery:LIMit:IGNore",
                ":DIAGnostic:LOG:READ?",
                ":DIAGnostic:TEST?",
                ":PTIMe:TZONe",
                ":SYSTem:COMMunicate:SERial1:FDUPlex",
                ":SYSTem:LANGuage",
            ]
        );
    }

    #[test]
    fn an_unconstrained_command_still_accepts_its_value() {
        assert!(check_argument(":GPS:REFerence:ADELay +1.20000E-007", HP).is_ok());
        assert!(check_argument(":PTIMe:TZONe -8,0", HP).is_ok());
    }

    #[test]
    fn a_permitted_header_cannot_carry_a_second_command() {
        // The hole this check exists for.  The class comes from the
        // header and the argument is passed through verbatim, so
        // without well_formed a query header smuggled a baud change
        // onto a daemon started with no flags at all.
        for smuggled in [
            "*IDN? ;:SYSTem:PRESet",
            ":SYSTem:STATus? ;:SYSTem:COMMunicate:SERial1:BAUD 1200",
            ":GPS:SATellite:TRACking:EMANgle 10;:DIAGnostic:ERASe",
            "*IDN?\t;:SYSTem:PRESet",
        ] {
            assert!(
                well_formed(smuggled).is_err(),
                "{smuggled:?} was not rejected"
            );
        }
    }

    #[test]
    fn a_command_is_one_line() {
        // An embedded newline reaches the receiver as a second command
        // even on firmware that does not chain with ';', and leaves the
        // reply to it in the buffer for whatever asks next.
        for split in [
            "*IDN? x\n:SYSTem:PRESet",
            "*IDN? x\r:SYSTem:PRESet",
            ":PTIMe:TZONe 0,0\n",
        ] {
            assert!(well_formed(split).is_err(), "{split:?} was not rejected");
        }
    }

    #[test]
    fn real_arguments_are_still_accepted() {
        // The check must not refuse the values the receiver actually
        // takes: numbers, words, lists, quoted strings, exponents.
        for good in [
            ":GPS:SATellite:TRACking:EMANgle 10",
            ":GPS:POSition:SURVey:STATe ONCE",
            ":GPS:REFerence:ADELay +1.20000E-007",
            ":PTIMe:TZONe -8,0",
            ":SYSTem:LANGuage \"PRIMARY\"",
            ":GPS:POSition N,+37,+22,+30.2,W,+122,+5,+34.8,+43.5",
            "*IDN?",
            ":SYSTem:STATus?",
        ] {
            assert!(well_formed(good).is_ok(), "{good:?} was wrongly rejected");
        }
    }

    #[test]
    fn non_ascii_is_refused_rather_than_guessed_at() {
        // Cyrillic lookalikes would miss the substring checks in
        // raw_class; the receiver would reject them anyway, but the
        // gate should not be the thing relying on that.
        assert!(well_formed(":SYST\u{435}m:PRES\u{435}t").is_err());
    }

    #[test]
    fn a_trailing_question_mark_alone_does_not_make_a_command_a_query() {
        // raw_class saw the last character of the whole string, so
        // anything suffixed with a query read as one.
        assert_eq!(raw_class("*RST;*IDN?"), Class::Dangerous);
        assert_eq!(raw_class(":WHATEVER:THIS:IS 5;:OTHER?"), Class::Control);
        assert_eq!(raw_class(":WHATEVER:THIS:IS?"), Class::Query);
    }

    #[test]
    fn the_stranding_list_covers_the_receivers_own_vocabulary() {
        // Beyond the four documented families: resets, non-volatile
        // writes, and any other route to the UART or the prompt, since
        // the session frames on the prompt.
        for dangerous in [
            "*RST",
            ":DIAGnostic:EEPRom:WRITe 0,0",
            ":SYSTem:SAVE",
            ":DIAGnostic:UART:BAUD 1200",
            ":SYSTem:PROMpt OFF",
            ":DIAGnostic:MEMory:WRITe 0",
        ] {
            assert_eq!(raw_class(dangerous), Class::Dangerous, "{dangerous}");
        }
    }

    #[test]
    fn matching_by_header_does_not_let_a_dangerous_command_through() {
        // The header form must not become a way to smuggle one past the
        // gate by appending a parameter.
        let (spec, _) =
            classify(":SYSTem:COMMunicate:SERial1:BAUD 9600", HP).expect("known with argument");
        let class = spec.class;
        assert_eq!(class, Class::Dangerous);
        assert!(!Policy::default().allows(class));
    }

    #[test]
    fn the_table_spelling_is_what_gets_sent() {
        // A client may use any casing; the receiver gets the canonical
        // form either way.
        let (_, sent) = classify(":gps:satellite:tracking:emangle 15", HP).expect("known");
        assert_eq!(sent, ":GPS:SATellite:TRACking:EMANgle 15");
    }

    #[test]
    fn an_unknown_command_is_not_classified() {
        assert!(classify(":SOME:UNKNOWN:THING?", HP).is_none());
        assert!(classify(":SOME:UNKNOWN:THING 5", HP).is_none());
    }

    #[test]
    fn the_default_policy_permits_only_queries() {
        let policy = Policy::default();
        assert!(policy.allows(Class::Query));
        assert!(!policy.allows(Class::Control));
        assert!(!policy.allows(Class::Dangerous));
    }

    #[test]
    fn each_refusal_names_the_flag_that_would_permit_it() {
        assert_eq!(Policy::flag(Class::Control), "--allow-control");
        assert_eq!(Policy::flag(Class::Dangerous), "--allow-dangerous");
    }

    #[test]
    fn a_flag_opens_only_what_it_names() {
        let control = Policy {
            control: true,
            ..Policy::default()
        };
        assert!(control.allows(Class::Control));
        assert!(!control.allows(Class::Dangerous));
    }

    #[test]
    fn an_unrecognised_raw_command_is_guessed_generously() {
        // Guessing generously costs a client one more flag; guessing
        // kindly could erase the receiver.
        assert_eq!(raw_class(":WHATEVER:THIS:IS?"), Class::Query);
        assert_eq!(raw_class(":WHATEVER:THIS:IS 5"), Class::Control);
        for stranding in [
            ":SYSTem:COMMunicate:SERial9:BAUD 1200",
            ":SYSTem:PRESet",
            ":DIAGnostic:ERASe",
            ":SYSTem:LANGuage \"INSTALL\"",
        ] {
            assert_eq!(raw_class(stranding), Class::Dangerous, "{stranding}");
        }
    }
}
