//! What the daemon answers on its socket, and what it permits.  The
//! serving itself -- clients, request lines, the socket's mode -- is
//! `smartclock::server`'s.
//!
//! Authorization is socket permissions and nothing else: systemd's
//! `RuntimeDirectory` and its mode decide who can open the socket, and
//! anyone who can may issue whatever the configuration allows.  On a
//! single-operator machine that is the whole of it -- no peer
//! credentials, no tokens.  The cost is that the daemon cannot tell two
//! clients apart, so the audit trail records what was done but not by
//! whom.

use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering;
use std::sync::mpsc::RecvTimeoutError;
use std::thread;
use std::time::Duration;

use jiff::Timestamp;
use smartclock::command::Argument;
use smartclock::command::Class;
use smartclock::command::Dialect;
use smartclock::command::Spec;
use smartclock::control::names;
use smartclock::protocol::Message;
use smartclock::protocol::Op;
use smartclock::server::Held;
use smartclock::server::Service;
use smartclock::server::Writer;
use smartclock::server::write_line;
use smartclock::task::Cadence;
use smartclock::task::Handle;
use smartclock::wire::Reading;

use crate::inbox::Fact;
use crate::inbox::Filed;
use crate::inbox::LogInbox;
use crate::inbox::Note;

/// How often a client's push thread wakes, with no snapshot to send,
/// to see whether its client has gone.
///
/// The thread holds the client's slot, and it used to sleep until the
/// next snapshot.  With the receiver's link down there is no next
/// snapshot, so every client that came and went during an outage kept
/// its slot, and the web view's once-a-second connections filled all
/// sixteen within seconds -- refusing the monitor and the command line
/// at exactly the moment they were wanted.
const HANGUP_CHECK: Duration = Duration::from_secs(1);

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
    /// Allow raw SCPI that the table does not recognize at all.
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
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
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
    pub inbox: LogInbox,
}

/// The receiver daemon, as its socket's clients see it.
pub(crate) struct Daemon {
    /// The device task.
    pub(crate) handle: Handle,
    /// What to tell clients about the receiver and the policy.
    pub(crate) info: SharedInfo,
}

impl Service for Daemon {
    type Op = Op;

    /// A copy of the shared state, taken and released before the
    /// request runs.  A request can wait on the receiver for up to the
    /// request timeout, and holding the lock meanwhile queued every
    /// other client behind it -- and the reconnect path, which records
    /// the new receiver here before it starts the task that would
    /// answer the waiting request.  Taken per request rather than per
    /// connection, so a reconnect to a different receiver takes effect
    /// for the next command.
    fn answer(&self, id: String, op: Op) -> Message {
        let current = lock_or_poisoned(&self.info).clone();
        handle_request(id, op, &self.handle, &current)
    }

    /// Snapshots are pushed from a thread of their own, so a client
    /// slow to read cannot hold up its own requests.  It is the thread
    /// that holds resources -- a subscription, a socket, a thread -- so
    /// it holds the client's place.  Counting the request thread
    /// instead let a client half-close its sending side, end its
    /// requests, release its place, and leave the push thread running;
    /// repeating that accumulated subscriptions and threads without
    /// limit while the cap was never reached.
    fn connected(
        &self,
        writer: &Writer,
        serving: &Arc<AtomicBool>,
        held: Held,
    ) -> std::io::Result<Option<Held>> {
        // A client gets the current state immediately, so it can render
        // before the next poll rather than showing an empty screen.
        if let Some(snapshot) = self.handle.latest() {
            write_line(writer, &Message::event(&snapshot))?;
        }
        let updates = self.handle.subscribe();
        let pusher = Arc::clone(writer);
        let serving = Arc::clone(serving);
        thread::Builder::new()
            .name("smartclockd-push".to_owned())
            .spawn(move || {
                // Released when this thread ends, and the socket shut
                // with it, so the request thread's read returns rather
                // than waiting on a client nobody serves.
                let _held = held;
                loop {
                    // It notices the client has gone on its next
                    // snapshot or within `HANGUP_CHECK`.
                    let next = updates.recv_timeout(HANGUP_CHECK);
                    if !serving.load(Ordering::Relaxed) {
                        return;
                    }
                    match next {
                        Ok(snapshot) => {
                            if write_line(&pusher, &Message::event(&snapshot)).is_err() {
                                return;
                            }
                        }
                        Err(RecvTimeoutError::Timeout) => {}
                        Err(RecvTimeoutError::Disconnected) => return,
                    }
                }
            })?;
        Ok(None)
    }
}

fn handle_request(id: String, op: Op, handle: &Handle, info: &Info) -> Message {
    match op {
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
                "version": smartclock::VERSION,
                "identity": info.identity,
                "dialect": info.dialect.name(),
                "database": info.database,
                "allow_control": info.policy.control,
                "allow_dangerous": info.policy.dangerous,
                "allow_raw": info.policy.raw,
                "cadence_fast": info.cadence.fast.as_secs_f64(),
                "cadence_medium": info.cadence.medium.as_secs_f64(),
                "cadence_slow": info.cadence.slow.as_secs_f64(),
            }),
        ),
        // The screen itself, not the latest snapshot: the screen is
        // not kept there, and a snapshot read back afterwards could be
        // a later poll's with no screen in it.
        Op::Status => match handle.status() {
            Ok(screen) => Message::ok(id, serde_json::json!({ "screen": screen })),
            Err(e) => Message::err(id, e),
        },
        Op::Query { scpi } => send(id, &scpi, handle, info),
        Op::Note { text, at } => {
            let text = text.trim();
            if text.is_empty() {
                return Message::err(id, "a note needs some text");
            }
            written(id, info, |receiver| {
                info.inbox.note(Note {
                    at: at.unwrap_or_else(Timestamp::now),
                    receiver,
                    text: text.to_owned(),
                })
            })
        }
        Op::Fact { key, value, since } => {
            let (key, value) = (key.trim(), value.trim());
            if key.is_empty() || key.contains(char::is_whitespace) {
                return Message::err(id, "a fact's key is one word, such as ocxo.serial");
            }
            if value.is_empty() {
                return Message::err(id, "a fact needs a value");
            }
            written(id, info, |receiver| {
                info.inbox.fact(Fact {
                    since: since.unwrap_or_else(Timestamp::now),
                    receiver,
                    key: key.to_owned(),
                    value: value.to_owned(),
                })
            })
        }
    }
}

/// Write something for the attached receiver through the log thread,
/// answering with the unit it was filed under and whether it is written
/// yet or only queued.
fn written(
    id: String,
    info: &Info,
    write: impl FnOnce(String) -> std::result::Result<Filed, String>,
) -> Message {
    if info.identity.is_empty() {
        return Message::err(
            id,
            "no receiver has answered yet, so there is no log to write",
        );
    }
    match write(info.identity.clone()) {
        Ok(filed) => Message::ok(
            id,
            serde_json::json!({
                "receiver": info.identity,
                "written": filed == Filed::Written,
            }),
        ),
        Err(why) => Message::err(id, why),
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

    if let Some(Err(why)) = spec.map(|spec| check_argument(scpi, spec)) {
        return Message::err(id, why);
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
    info.inbox.audit(to_send, class, &note, &info.identity);

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
/// the same on firmware that does not chain, and desynchronizes the
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
/// defense in depth rather than a safety requirement.  What it buys is
/// a message naming the command and the bound, and an exchange that
/// never happened rather than one recorded in the audit trail as a
/// command that was sent and refused.
/// Takes the spec [`classify`] matched rather than looking one up
/// again: two searches with slightly different rules could disagree
/// about which entry a command is, and the gate must not be the place
/// that happens.
fn check_argument(scpi: &str, spec: &Spec) -> Result<(), String> {
    // An entry that carries its own argument, such as
    // ":GPS:POSition:SURVey:STATe ONCE", is already complete.  Only
    // such an entry: the check used to skip any command that matched a
    // table entry exactly, which let a command that requires a value
    // through with none -- :GPS:SATellite:TRACking:EMANgle on its own
    // passed the integer rule by never reaching it.
    if spec.scpi.eq_ignore_ascii_case(scpi) && spec.scpi.contains(char::is_whitespace) {
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
/// on, but even then an unrecognized command is treated as at least
/// control, and anything resembling the ones that can strand the link
/// as dangerous.  Guessing generously costs a client one more flag;
/// guessing kindly could erase the receiver.
fn raw_class(scpi: &str) -> Class {
    let upper = scpi.to_ascii_uppercase();
    // Matched keyword by keyword (`names`), so short forms cannot evade
    // these and a keyword merely beginning like one, `PRESent`, is not
    // taken for it.  COMMunicate, PRESet, ERASe and LANGuage strand the
    // link or the receiver; the rest are from the receiver's own keyword
    // table in docs/scpi/z3801-keywords.md: resets, anything that writes
    // non-volatile memory, and any other route to the UART, since the
    // gate's safety must not rest on COMMunicate being the only one.
    // Turning the prompt off would strand the link as surely as a baud
    // change, because the session frames on it.
    const STRANDS: [&str; 12] = [
        "COMMunicate",
        "PRESet",
        "ERASe",
        "LANGuage",
        "RST",
        "EEPRom",
        "WRITe",
        "SAVE",
        "BAUD",
        "UART",
        "PROMpt",
        "MEMory",
    ];
    if STRANDS.iter().any(|keyword| names(scpi, keyword)) {
        return Class::Dangerous;
    }
    // Reading an event register, or the standard event status register,
    // clears it -- and with it the front-panel Alarm LED, which belongs
    // to whoever is at the instrument.  Reading the error queue removes
    // the entry, which the daemon's journal is there to keep.  The
    // command table classes every one it knows as control; a spelling
    // it does not know is guessed the same way.  The mandatory
    // abbreviations are EVEN and ERR.
    if upper.contains(":EVEN") || upper.contains("*ESR") || upper.contains(":ERR") {
        return Class::Control;
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
    fn a_command_that_needs_a_value_is_refused_without_one() {
        // Matching a table entry exactly used to end the check, so a
        // command whose argument is required passed by never reaching
        // the rule that requires it.
        assert!(check_argument(":GPS:SATellite:TRACking:EMANgle", HP).is_err());
        assert!(check_argument(":SYSTem:COMMunicate:SERial1:BAUD", HP).is_err());
        assert!(check_argument(":PTIMe:TCODe:FORMat", HP).is_err());
        // An entry that is the whole command is still fine with none.
        assert!(check_argument(":GPS:POSition:SURVey:STATe ONCE", HP).is_ok());
        assert!(check_argument(":SYNChronization:HOLDover:INITiate", HP).is_ok());
        assert!(check_argument(":SYNChronization:TINTerval?", HP).is_ok());
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
            ":GPS:POSition N,+45,+0,+0.0,W,+100,+0,+0.0,+50.0",
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
    fn a_keyword_that_only_begins_like_a_stranding_one_is_not_taken_for_it() {
        assert_eq!(raw_class(":KENneth:PRESent?"), Class::Query);
        assert_eq!(raw_class(":KENneth:PRES?"), Class::Dangerous);
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
    fn a_read_that_clears_a_register_is_not_a_query() {
        // Reading an event register clears it and can put out the
        // front-panel Alarm LED, and reading the error queue takes the
        // entry, so the default policy must refuse them however they
        // are spelled: from the table, abbreviated, or raw.
        for scpi in [
            "*ESR?",
            ":STATus:OPERation:EVENt?",
            ":STAT:OPER:EVEN?",
            ":STATus:QUEStionable:EVENt?",
            ":STATus:OPERation:HARDware:EVENt?",
            ":STATus:OPERation:HOLDover:EVENt?",
            ":STATus:OPERation:POWerup:EVENt?",
            ":SYSTem:ERRor?",
            ":SYST:ERR?",
        ] {
            let class = classify(scpi, HP).map_or_else(|| raw_class(scpi), |(spec, _)| spec.class);
            assert_eq!(class, Class::Control, "{scpi}");
            assert!(!Policy::default().allows(class), "{scpi}");
        }
        // The condition registers are the non-destructive reads, and
        // stay open.
        let (spec, _) = classify(":STATus:OPERation:CONDition?", HP).expect("known");
        assert_eq!(spec.class, Class::Query);
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
    fn an_unrecognized_raw_command_is_guessed_generously() {
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

#[cfg(test)]
mod socket_tests {
    use super::Daemon as Served;
    use super::HANGUP_CHECK;
    use super::Info;
    use super::Policy;
    use super::SharedInfo;
    use crate::inbox::LogInbox;
    use smartclock::link::Scratch;
    use smartclock::link::listen_scratch;
    use smartclock::server::MAX_CLIENTS;
    use smartclock::server::serve;

    use smartclock::link::Stream;
    use std::io::BufRead;
    use std::io::BufReader;
    use std::io::Write;
    use std::sync::Arc;
    use std::sync::Mutex;
    use std::sync::mpsc::channel;
    use std::thread;
    use std::time::Duration;

    use smartclock::command::Dialect;
    use smartclock::task::Handle;
    use smartclock::task::Shared;

    /// A daemon listening on a socket of its own, with no receiver
    /// behind it.
    ///
    /// Nothing here needs one: every path under test is refused, capped
    /// or answered before a command would reach the device.  The
    /// request channel's receiver is deliberately kept alive, so a test
    /// that did reach the device would block rather than quietly get a
    /// "task stopped" and look like it passed.
    struct Daemon {
        scratch: Scratch,
        _requests: std::sync::mpsc::Receiver<smartclock::task::Request>,
        info: SharedInfo,
    }

    impl Daemon {
        fn start() -> Self {
            let (tx, rx) = channel();
            let (audit_tx, _audit_rx) = channel();
            let handle = Handle::new(tx, Shared::new());
            let info = Arc::new(Mutex::new(Info {
                identity: "HEWLETT-PACKARD,58503A,0000A00000,3704-C".to_owned(),
                dialect: Dialect::Hp58503,
                database: "/nowhere".to_owned(),
                policy: Policy::default(),
                inbox: LogInbox::new(audit_tx),
                cadence: smartclock::task::Cadence::default(),
            }));

            let shared_info = Arc::clone(&info);
            let (listener, scratch) = listen_scratch("smartclockd").expect("listen");
            thread::Builder::new()
                .name("test-serve".to_owned())
                .spawn(move || serve(vec![listener], "test", Arc::new(Served { handle, info })))
                .expect("serve thread");

            Self {
                scratch,
                _requests: rx,
                info: shared_info,
            }
        }

        fn connect(&self) -> Stream {
            let stream = smartclock::link::connect(self.scratch.endpoint()).expect("connect");
            stream
                .set_read_timeout(Some(Duration::from_secs(5)))
                .expect("timeout");
            stream
        }
    }

    #[test]
    fn a_request_waiting_on_the_receiver_does_not_hold_the_shared_state() {
        // Nothing serves the request queue here, so a query waits out
        // the request timeout.  Meanwhile the reconnect path, the
        // journal and every other client need the shared state, and a
        // reconnect that waited behind the query could not start the
        // task that would answer it.
        let daemon = Daemon::start();
        let mut client = daemon.connect();
        writeln!(
            client,
            r#"{{"v":1,"id":"1","op":{{"kind":"query","scpi":"*IDN?"}}}}"#
        )
        .expect("send");
        thread::sleep(Duration::from_millis(500));
        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        let free = loop {
            if daemon.info.try_lock().is_ok() {
                break true;
            }
            if std::time::Instant::now() >= deadline {
                break false;
            }
            thread::sleep(Duration::from_millis(20));
        };
        assert!(
            free,
            "the shared state was held while waiting on the receiver"
        );
    }

    /// The first line the daemon sends that answers `id`.
    fn reply_to(stream: &Stream, id: &str) -> String {
        let mut reader = BufReader::new(stream.try_clone().expect("clone"));
        for _ in 0..50 {
            let mut line = String::new();
            if reader.read_line(&mut line).expect("read") == 0 {
                return String::new();
            }
            if line.contains(&format!("\"id\":\"{id}\"")) {
                return line;
            }
        }
        panic!("no reply to {id}");
    }

    #[test]
    fn half_closing_does_not_hand_the_slot_back() {
        // The bug: the slot belonged to the request thread, so a client
        // could end that thread with a half-close, release its slot,
        // and leave the push thread holding a subscription and a
        // socket.  Repeating it accumulated both without limit while
        // the cap was never reached.
        let daemon = Daemon::start();
        let held: Vec<_> = (0..MAX_CLIENTS)
            .map(|_| {
                let stream = daemon.connect();
                stream
                    .shutdown(std::net::Shutdown::Write)
                    .expect("shutdown");
                stream
            })
            .collect();
        assert_eq!(held.len(), MAX_CLIENTS);

        // Long enough that the request threads have all seen EOF, and
        // short of `HANGUP_CHECK`, so the push threads -- which hold the
        // slots, the subscriptions and the sockets together, and let
        // them go together -- have not yet woken to notice.
        thread::sleep(Duration::from_millis(200));

        let extra = daemon.connect();
        let mut reader = BufReader::new(extra.try_clone().expect("clone"));
        let mut line = String::new();
        reader.read_line(&mut line).expect("read");
        assert!(
            line.contains("already connected"),
            "half-closed clients gave their slots back: {line:?}"
        );
    }

    #[test]
    fn clients_that_left_while_nothing_was_published_give_their_slots_back() {
        // Nothing publishes here, as nothing does while the receiver's
        // link is down.  The push threads used to wait for a snapshot
        // before noticing their clients had gone, so a full house of
        // departed clients held every slot until the link came back.
        let daemon = Daemon::start();
        for _ in 0..MAX_CLIENTS {
            drop(daemon.connect());
        }
        thread::sleep(HANGUP_CHECK + Duration::from_millis(500));

        let mut client = daemon.connect();
        writeln!(client, r#"{{"v":1,"id":"1","op":{{"kind":"info"}}}}"#).expect("send");
        let mut reader = BufReader::new(client.try_clone().expect("clone"));
        let mut line = String::new();
        reader.read_line(&mut line).expect("read");
        assert!(
            !line.contains("already connected") && line.contains(r#""id":"1""#),
            "a client was refused after every earlier one had left: {line:?}"
        );
    }

    #[test]
    fn a_dangerous_command_never_reaches_the_receiver() {
        // The proof that it did not is that this returns at all: the
        // request channel has no task behind it, so anything that got
        // as far as the device would block until the test timed out.
        let daemon = Daemon::start();
        let mut stream = daemon.connect();
        writeln!(
            stream,
            r#"{{"v":1,"id":"d","op":{{"kind":"query","scpi":":SYSTem:COMMunicate:SERial1:BAUD 1200"}}}}"#
        )
        .expect("write");
        let reply = reply_to(&stream, "d");
        assert!(
            reply.contains("allow-dangerous"),
            "expected a policy refusal, got {reply}"
        );
    }

    #[test]
    fn a_query_header_carrying_a_payload_never_reaches_the_receiver() {
        let daemon = Daemon::start();
        let mut stream = daemon.connect();
        writeln!(
            stream,
            r#"{{"v":1,"id":"p","op":{{"kind":"query","scpi":":SYSTem:LANGuage? \"INSTALL\""}}}}"#
        )
        .expect("write");
        let reply = reply_to(&stream, "p");
        assert!(
            reply.contains("takes no argument"),
            "expected an argument refusal, got {reply}"
        );
    }
}

#[cfg(test)]
mod note_tests {
    use super::Info;
    use super::Policy;
    use super::handle_request;
    use crate::inbox::LogInbox;
    use crate::inbox::LogRequest;

    use std::sync::mpsc::channel;
    use std::thread;

    use smartclock::command::Dialect;
    use smartclock::protocol::Message;
    use smartclock::protocol::Op;
    use smartclock::task::Handle;
    use smartclock::task::Shared;

    /// Ask `op` of a daemon attached to `identity`, with a log thread
    /// that writes everything it is sent.
    fn ask(identity: &str, op: Op) -> Message {
        let (inbox_tx, inbox) = channel();
        thread::spawn(move || {
            for request in inbox {
                match request {
                    LogRequest::Note(_, written) | LogRequest::Fact(_, written) => {
                        let _ = written.send(Ok(()));
                    }
                    LogRequest::Audit(_) => {}
                }
            }
        });
        let (requests, _unused) = channel();
        let info = Info {
            identity: identity.to_owned(),
            dialect: Dialect::Hp58503,
            database: "/nowhere".to_owned(),
            policy: Policy::default(),
            inbox: LogInbox::new(inbox_tx),
            cadence: smartclock::task::Cadence::default(),
        };
        handle_request(
            "1".to_owned(),
            op,
            &Handle::new(requests, Shared::new()),
            &info,
        )
    }

    fn error(message: Message) -> Option<String> {
        match message {
            Message::Reply { err, .. } => err,
            Message::Event { .. } => panic!("an event, not a reply"),
        }
    }

    const UNIT: &str = "HEWLETT-PACKARD,58503A,3710A01056,3704-C";

    #[test]
    fn a_note_is_filed_under_the_attached_unit() {
        let reply = ask(
            UNIT,
            Op::Note {
                text: "added a 20 dB LNA".to_owned(),
                at: None,
            },
        );
        match reply {
            Message::Reply { ok: Some(ok), .. } => {
                assert_eq!(ok["receiver"], UNIT);
                assert_eq!(ok["written"], true);
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn a_note_without_a_receiver_or_text_and_a_fact_with_a_spaced_key_are_refused() {
        let note = |text: &str| Op::Note {
            text: text.to_owned(),
            at: None,
        };
        assert!(error(ask("", note("before any receiver"))).is_some());
        assert!(error(ask(UNIT, note("   "))).is_some());
        let fact = Op::Fact {
            key: "ocxo serial".to_owned(),
            value: "1234".to_owned(),
            since: None,
        };
        assert!(error(ask(UNIT, fact)).is_some());
    }
}
