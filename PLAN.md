# Plan

A Rust library for talking to HP / Symmetricom SmartClock GPS receivers
over serial, a logging daemon that runs as a system service, a TUI, and
browser and Prometheus views.  A GUI is not planned.

## Status

Phases 0 to 12 are done; see **Phases** for what each involved, **Open
questions** for what is undecided and **Known defects** for what is
wrong and unfixed.

`make ci` is the local check; CI runs the shared `rust-ci.yml` and the
browser tests in `web.yml` (see "CI is shared").  `make test-hw` is the
hardware-only set and CI never runs it.

Installed from the package and running as a service, one daemon per
port, logging to `/var/lib/smartclockd/<model>-<serial>.sqlite`.
`docs/running.md` is the deployment note.  The socket protocol is
documented in `smartclock::protocol`, where it is defined; see "No
separate document for the socket protocol".

Two adversarial reviews in September 2026, one over the whole tree and
one over a day's diff, found about forty defects and then eight more,
several in the first round's fixes -- the argument for reviewing a diff
and not only a tree.  Every one produced a wrong value presented
confidently rather than an error, and every test passed throughout,
because the fixtures covered only the cases the code was written from.
The fixes are recorded with the decisions they belong to.

## Goals

- Read-only monitoring of a 58503A first; control commands later.
- Diagnose the development unit's suspected EFC / OCXO problem.  Done;
  it is not the oscillator: see **EFC diagnosis**.
- Unattended long-term logging of EFC and holdover state for drift
  analysis, independent of whether anyone is watching.
- Support the wider SmartClock family, notably the Z3801A.

## Constraints

- **Single-operator machine.**  One person, one box.  A standing
  assumption, not a temporary simplification.  It justifies socket
  permissions as the whole of authorization, an audit trail with no
  notion of who, and daemons that each serve one device rather than a
  fleet.  Revisit these together if it stops being true.
- **Bringup targeted the 58503A first.**  Other variants got table
  entries from their manuals and firmware, and nothing was verified
  against them until the 58503A worked end to end.  A Z3801A and a
  Z3805A are now on the bench and answer the z3801 dialect.

## Decisions

### Flashing is a direct-port command of the CLI

`smartclock-cli flash` loads firmware.  It was a separate
`smartclock-flash` binary; it moved into the CLI once its session,
console and line-settings code were the library's, leaving one tool to
learn.  The CLI's forbidden-command check still applies to commands a
user types to `query` and `sweep`; `flash`, like `read-memory`, is a
fixed procedure of its own.  It requires the port's daemon to be
stopped and never connects to its socket.  A shared installer procedure uses audited image profiles for
Z3801A, Z3805A, 58503A and Z3816A, including their different protected
flash regions and checksum algorithms.  Exact image SHA-256, boot
checksums, model, running revision and expected serial are checked
before erase.  Unknown images and receiver revisions are refused; no
force override is provided.  Models without dumps, including 59551A,
need an audited profile before they can be flashed.  Filename and
embedded model-string guesses cannot establish compatibility.

Installer revisions are checked against the model/layout allowlist, not
paired with a particular primary revision: boot flash survives upgrades.
The CLI states this limitation in check-only mode and immediately before
erase.  A nonempty error queue stops preflight and asks the operator to
review and clear it; the flasher does not issue `*CLS` or silently
discard the remaining errors.

The existing simulator optionally models the installer, flash contents,
record validation and boot checksums.  Tests compare the entire
resulting image, including protected boot flash, and exercise
interrupted-download recovery through the real session framing.  Both
58503A revision changes (3633 to 3704 and back) are tested, reading the
revision from the newly programmed primary and preserving the original
boot region.  Final boot verification requires the candidate revision,
model and serial; a changed suffix is reported separately, since its
behavior across upgrades is unknown.  Hardware timing, wear and
upgrade-induced settings changes remain outside the simulation.
Cross-revision flashing has not been tested on hardware; record settings
before and after the first upgrade.

After writing, the flasher reads the whole flash back through the
debug console and compares it with the image, then returns the port to
SCPI through the installer.  The console code lives in the library
(`smartclock::console`) so the CLI's memory reads and `flash` share
one reader and one way back.

### No client-side SCPI crate

Surveyed: `scpi` + `scpi-contrib` (server side, for implementing an
instrument, no_std), `scpify` (TCP and HiSLIP only, no serial),
`scpi-client` 0.1.1 (thin, immature), `instrument-core` 0.1.0 (right
shape, but v0.1.0 and VISA/GPIB oriented).

None fit.  The receiver echoes, prompts with `scpi> ` / `E-nnn> `, and
its richest response is an ASCII status screen.  Generic clients assume
a clean write / read-to-terminator cycle, and defeating that costs more
than writing the framing.  The standard parts are shallow and specified
in `097-59551-02`: `:SYSTem:ERRor?` returns `<code>,"<text>"`, and the
status registers are IEEE 488.2.

The simulator does not use `scpi` either: the prompt, echo and status
screen, the parts worth emulating, are what it does not model.

### No separate document for the socket protocol

Four programs speak it -- the monitor, the CLI, the exporter and the
browser view -- all built from this tree, in this language, in one
package, so they cannot drift apart.  The wire type is checked by the
compiler, which is a stronger specification than prose.
`smartclock::wire::Reading` is not `Snapshot`, so an internal rename is
a compile error rather than a silent protocol change.

What a type cannot express is written in `smartclock::protocol`:
newline-delimited JSON over a local socket, a multiplexed stream where
snapshots arrive unsolicited so every request carries an id its reply
echoes, and a `VERSION` the daemon refuses a mismatch on.

This holds while the socket is internal.  A client outside this tree
makes it an interface, needing a specification and a stability
commitment.

### The daemon owns the port

`smartclockd` runs as a systemd system service and holds the serial
port open for as long as it runs, so every other component is a client
of the daemon.  Multi-day EFC history cannot depend on a TUI being up.

Within the daemon, a single `DeviceTask` thread owns the `Session`, and
therefore the fd, and is the only thing that issues a command.  A
half-read prompt from an interleaved command desynchronizes every later
read.  `Session` is not `Sync` and is moved into the device thread at
construction, so a second writer is a compile error.

For the same reason a command that errors must not leave its reply in
flight: the next poll would read it as its own and record, say, TFOM as
FFOM.  `drain` reports failure when it gives up, so `sync` cannot match
an abandoned reply's prompt and call itself current one exchange behind.

```
                      smartclockd (system service)
                  +----------------------------------+
/dev/serial/by-id | DeviceTask (sole Session owner)   |
      <---------> |   poll schedule + request queue   |
                  +----+------------------------+-----+
                       | broadcast<Snapshot>    | commands
                  +----v-----+            +-----v------+
                  |  Logger  |            | local sock |
                  | sqlite W |            | /run/...   |
                  +----------+            +-----+------+
                       |                        |
                  sqlite file               live stream
                  (WAL, read-only)          + control
                       |                        |
                  +----+------------------------+-----+
                  | smartclockmon (TUI) / smartclock-cli|
                  +------------------------------------+
```

### A receiver's line settings are found, not assumed

Rate and framing belong to the unit, not the port: the 58503A's are
settable, the Z3801A's fixed at 19200 7O1 (`097-z3801-01` 1-8, 2-10).
`--baud` and `--framing` give the settings to try first.  When they get
no identity, 19200 and 9600 at 8N1 and 7O1 are tried in turn
(`smartclock::attach`).  The daemon, the monitor and the CLI all do
this; `read-memory`, which talks to the pForth console, does not.  A
receiver on the network is not probed.  Per-port framing in a drop-in
is optional and only saves the probe.

A port's settings were once fixed per port and never probed; after a
power cut the bench's two USB adapters came back in the other order and
each daemon sat unanswered at the other unit's framing.

A probe at the wrong framing reaches the receiver as garbage it queues
as errors.  Those are read off the queue at the settings that work,
before anything else is asked, and counted in the journal.  `*CLS`
would clear them too, but also the event registers, which drive the
front-panel alarm.

### The simulator is not PTY-backed

A PTY would make the simulator Unix-only, which undoes the reason
`interprocess` was chosen over raw AF_UNIX: systemd is meant to be the
one Linux-specific piece.

Tests need a receiver the library can talk to: `SimTransport` in
`smartclock-sim`, an in-process `Transport`, portable everywhere.
Driving the real daemon and monitor end to end needs something
reachable by a device path: the same simulator on a TCP listener.  That
is the `TcpTransport` ser2net needs anyway, so a network-attached
serial adapter works at no extra cost.

### The protocol stays JSON

gRPC in Rust means `tonic`, which does not do named pipes, so it would
land on loopback TCP and reopen the authentication question socket
permissions answer for free.  Protobuf over the same socket buys a
schema and compactness, and costs a codegen step and the ability to
watch the socket with `socat`, for a message a second between two ends
we control.  Revisit if a client appears that is not written in Rust.

The real problem, which protobuf would not have fixed, was serializing
`Snapshot` straight from the internal struct, so a field rename
changed the wire format.  The socket carries its own type, converted
from the internal one.

### Two channels to the daemon

- **Local socket, via `interprocess`**, for the live snapshot stream
  and command submission.  One API over AF_UNIX and Windows named
  pipes.  Newline-delimited JSON, debuggable with `socat`.  A client
  subscribes on connect and receives the current snapshot followed by
  updates.  The name is a filesystem path rather than an abstract
  name, so systemd's `RuntimeDirectory` owns its lifetime and file
  permissions gate access.
- **SQLite file, opened read-only**, for history and trend charts.
  `journal_mode=WAL` lets readers run concurrently with the daemon's
  single writer.

Live data on the socket keeps the TUI's live pane off SQLite at 1 Hz
and makes reconnection after a daemon restart trivial.  Both the socket
protocol and the schema carry a version, since daemon and clients are
upgraded independently.

### Commands go through the daemon too

The library's `Control` handle is the same type whether the daemon
calls it locally or the CLI calls it over the socket, so the socket
protocol mirrors the library API.  Each request carries an id that the
reply echoes:

```
-> {"v":1,"id":"7f3a","op":{"kind":"query","cmd":"holdover_waiting"}}
<- {"v":1,"id":"7f3a","ok":{"holdover_waiting":"LIMit"}}
<- {"v":1,"event":"snapshot","ts":"...","efc_pct":-94.2,...}
```

A reply reports what the device said, not that the operation finished.
A survey takes hours but acks in milliseconds; progress is observed in
later snapshots, so clients never block on long operations.

Commands are classified in the command table:

| Class     | Examples                                              | Gate                     |
| --------- | ----------------------------------------------------- | ------------------------ |
| Query     | `:GPS:POSition?`, `:SYNC:TINT?`                        | none                     |
| Control   | holdover initiate and recover, survey, antenna delay, elevation mask; reading an event register or `*ESR?`, which clears it; reading `:SYSTem:ERRor?`, which removes the entry | `--allow-control` |
| Dangerous | `:SYSTem:PRESet`, `:SYSTem:COMMunicate:SERial1:*`, `:DIAGnostic:ERASe`, `:SYSTem:LANGuage "INSTALL"` | `--allow-dangerous` |

Dangerous commands can strand the link or wipe configuration; a baud
change persists across power cycles.  The gate is a daemon flag, not
anything a client presents (an earlier draft had the client echo a
nonce): enabling it is a deliberate act outside the client, and a
daemon started without it cannot be talked into the command at all.
The audit trail records what followed.  On SIGTERM or SIGINT the
daemon stops its task, then waits for the log thread to write the
audit entries and snapshots it holds, so `systemctl stop` does not
lose the record of a command that ran; a second signal exits at once.

Commands are refused before classification if they could carry a second
one: a `;`, a control character or any non-ASCII.  SCPI chains program
message units with `;` and the transport appends only a terminator, so
without that check `:SYSTem:STATus? ;:SYSTem:COMMunicate:SERial1:BAUD
1200` classified as a query.  Arguments are whitelisted rather than
filtered, since every value this receiver takes is a number, a word, a
list or a quoted string.

Argument ranges are in the command table beside the class, so the
string path validates what the typed `Control` handle does.  `Control`
is the typed API for embedders, not the safety boundary; the table is.

Authorization is socket permissions and nothing else: `RuntimeDirectory`,
`RuntimeDirectoryMode` and group ownership decide who can open the
socket, and anyone who can may issue whatever the daemon's flags allow.
No peer credentials, no tokens.  This keeps the daemon free of an
authentication layer on every platform; the cost is that the audit
trail records what was done but not by whom.

The TUI's raw SCPI console bypasses the command table, so it has its
own flag, `--allow-raw`, off by default.  The daemon still classifies
the prefix where it recognizes it, and logs every raw command.

Requests are serviced between commands, never mid-command.  Worst-case
control latency is about one status screen read, roughly a second.
Requests and polls take turns, bounded both ways: at most
`REQUESTS_PER_POLL` commands before the schedule gets its turn.  Polls
with absolute priority served no client command; draining the whole
queue before each poll let a few clients stop all snapshots and logging.

A command carries its caller's deadline and past it is answered, not
sent, so a holdover the operator was told had failed does not begin
seconds later.

- **Force-refresh after control.**  After a successful control
  operation the `DeviceTask` re-polls the affected tier;
  `:SYNC:HOLD:INIT` triggers a fast-tier and holdover refresh.
- **Audit trail beside the telemetry.**  Every non-scheduled command is
  recorded: timestamp, command text, response, classification, and an
  optional client-supplied label, which is untrusted.  Asking "what did
  I do to it, and what did EFC do afterwards" of one database is the
  strongest argument for routing commands through the daemon.

### Disconnection is a first-class state

USB serial adapters drop and receivers are power-cycled.  The daemon
reconnects rather than exiting, and records the gap.

Freshness is **per tier**: each group of fields carries when it last
succeeded and its own error, over the wire and into the log, and each
pane shows the age of what it displays.  A whole-snapshot flag let the
one-second tier relabel a minutes-stale sky as current, including after
a link drop.  A snapshot's overall freshness is derived -- as current as
its least current part -- so the first minute after startup, with no
sky, position or date yet, reads `Stale`.

The history graphs select on `fast_at = at`: whether the fast tier
measured this row or the row restates the last one.  A medium or slow
column rides in every fast row with whatever its tier last read, so it
counts only while its tier's timestamp is within three of its intervals
of the row, and is null past that, so a failing tier's line breaks
rather than running flat.  A window rather than one row per read,
because at an hour's zoom a ten-second tier would leave most buckets
empty; the cost is that a slow column's mean is weighted by time, not
by read.  The daemon records its cadence in `meta` for this; a log
without it is read at the default cadence.  The monitor's live panes
and the exporter use the same three intervals
(`Cadence::current_window`), with the daemon's cadence taken from its
`info` reply: a pane past it is labeled with its age, and the exporter
leaves its values out.

Use a `/dev/serial/by-id/...` path, not `/dev/ttyUSB0`, which is not
stable across re-enumeration.  The device path has no default and the
daemon and CLI refuse to run without one: a wrong default opens
whatever else is on that path and sends SCPI at it.  The packaged
configuration ships the setting commented out, so a fresh install fails
to start and says why.

### The receiver's own records

Three things the receiver keeps that the telemetry does not cover.

**The error queue is drained and written down.**  `:SYSTem:ERRor?`
removes the entry it returns, and what is not read is eventually
discarded.  The errors worth having are the ones the receiver raises on
its own.  A failed command can read past its own error to one raised
earlier; the task keeps such strays with the identity of the receiver
that raised it, and the daemon journals them before each pass; one from
a receiver no longer attached is reported, not filed under the unit
that is.

A full queue replaces its last entry with -350 and discards the newest
errors (097-59551-02 5-31).  A command that fails then finds only the
marker, so it reports its error as lost and the marker is kept as a
stray.  The journal records -350 as given and notes that an unknown
number of errors went with it.

**The diagnostic log is copied out entry by entry.**
`:DIAG:LOG:READ:ALL?` returns a full log in about 56 KB, half a minute
of wire time, so the copy is `:DIAG:LOG:READ? <n>`, sixteen entries a
pass, new ones first and then backwards through the history.  Entries
are stored by content, not number, because clearing the log restarts
the numbering.

**Condition registers, never event registers.**  The daemon polls the
hardware, operation, holdover and powerup condition registers and
`*STB?`.  Reading an event register clears it, which clears the alarm
condition register that summarizes it, which puts out the front-panel
Alarm LED and sets the BITE output inactive; the lamp belongs to
whoever is at the instrument.  (Event registers were polled for a time,
for Time Reset, which is event-only: 097-59551-02 5-39.)  `*STB?`
returns the alarm condition register, whose bits "are updated in real
time -- there is no latching or buffering", and "Reading/Querying the
Alarm Condition Register does not change its contents" (5-44).  It
names the group, not the bit; for the questionable group that is
exact, since it holds only Time Reset and the user-reported bit, which
nothing here sets.

So `*CLS` is not sent, nor anything else that writes to the receiver:
the logger is read-only and the operator clears the alarm at the panel.
The bit behind a summary is available by reading the event register,
which is the same act as acknowledging (open question 4).  A socket
client cannot read an event register or `*ESR?` without
`--allow-control`; they still return their value, and the class records
that taking it is an act on the receiver.

The transition filters are recorded beside the events, because the
events are uninterpretable without them.  This 58503A answers positive
127 / 2 / 5087 / 15 / 7 with every negative filter at zero, so faults
latch appearing and never clearing: a missing clear-event is not
evidence a fault persisted.  The filters are non-volatile and the only
documented reset is `:SYSTem:PRESet`, so they are read, never written.

All of this runs on the thread that owns the database, reaching the
receiver through the request queue: reading is what removes an entry,
and a channel between the read and the write is a gap a shutdown can
lose it in.

It runs **per connection**, not per receiver.  Across a connection the
unit may have been power cycled, swapped or reconfigured, so no state
carries over; a copied-log span carried across a swap once judged two
hundred of the new unit's entries already copied.  The daemon counts
connections and the journal resets when the count moves; re-deriving
costs one database query for the log span and one read of the filters.

Erasing the receiver's log is **condition-triggered**: when the copy
here is complete and gap-free and the receiver's log-almost-full bit is
set.  Clearing unsets the bit, so a flapping link cannot erase twice
and no state is needed.  It is gated behind `--adopt-log`, because
erasing is irreversible, and the entry count is passed with the command
so the receiver refuses with -222 if an entry arrived between the copy
and the clear.

### Rows belong to a receiver, not to a file

A metadata key answers "which receiver wrote here most recently"; the
question is "which receiver wrote this row", and on a bench where units
are swapped those differ.  Two oscillators' history in one file reads
as one oscillator with a step in it.

So there is a `receiver` table, one row per unit, and a `receiver_id`
on every per-unit table, keyed on the serial alone.  Keying on the
whole of `*IDN?` reports a firmware upgrade as a different receiver.
Firmware is recorded as last seen.

A snapshot carries the identity of the receiver it was read from and is
filed under that, not under the unit attached when it is written:
snapshots wait in a queue, and at a swap the old unit's are still there
after the new one is attached.  Journal rows are filed under the
receiver attached at the time.

Rows written before this existed keep a NULL id; backfilling them with
today's receiver would put one unit's history under another's name.

### Storage: SQLite

Chosen over JSONL because EFC and holdover trending means range queries
over time, which JSONL answers only by rescanning.  `rusqlite` with
bundled SQLite, in `StateDirectory=smartclockd` (`/var/lib/smartclockd`).
Retention is unbounded; the timestamp index keeps it queryable.  JSONL
is kept for raw wire transcripts: append-only, greppable, and reusable
as parser fixtures.

Timestamps are compared as text, so that the index on `at` serves every
range bound and `ORDER BY at`.  Schema 8 stores every timestamp with
nine fractional digits, because the default form trims trailing zeros
and `00.11Z`, `00.1Z`, `00Z` sort in the reverse of time; opening an
older database rewrites the ones already there in one transaction.
Range bounds are written in the stored form: `T` sorts after a space,
so a bound from `datetime('now')` excludes nothing.

### Poll scheduling is a link budget

The status screen is roughly 24 lines of 76 columns, about 1.8 KB,
about 0.95 s at 19200 8N1.  It cannot be polled at 1 Hz alongside
anything else.

19200 is the ceiling.  `097-59551-02` 5-101 lists four rates up to
19200, and the 58503A answers `:SYSTem:COMMunicate:SERial1:BAUD 38400`
with `+0,"No error"` while keeping 19200.  `BaudRate` still knows 38400
and 115200, so a receiver some other tool has moved there can be
reached to be moved back.

Each tier is a list of steps (`device::step_count`), the scheduler
takes one step per turn, and a tier's freshness is stamped when its
pass completes.  The fast tier is one step, because its fields are
compared against each other and a time interval from one second beside
an EFC from the next is a correlation nobody measured.

After each step short of the last, the tier is re-queued at the present
instant, so it yields to any tier that has come due since.  Left at the
deadline its pass started from, it would win every turn and run its
steps back to back.  Unconditional priority for the fast tier would
starve the slow one under a stream of refreshes.  A refresh moves the
deadlines but not the step cursors: restarting a pass in flight
discards its steps, and under a stream of refreshes no tier would ever
complete.

Measured on a 58503A at 19200, as the marginal cost over a 0.67 s
open-and-synchronize:

| step                                    | cost   |
| --------------------------------------- | ------ |
| satellite counts                        | 0.16 s |
| oscillator temperature, current, DAC    | 0.13 s |
| condition registers                     | 0.12 s |
| holdover duration, predicted, present   | 0.16 s |
| status screen                           | 1.50 s |

A whole fast pass, eight queries, is 0.38 s.  The screen is 1574
bytes: 0.94 s of wire time and 0.5 s of the receiver composing it.

So the screen is on no tier.  Nothing it carries but the sky plot is
unique to it: `:GPS:SATellite:TRACking:COUNt?` equals its `Tracking`,
and `:GPS:SATellite:VISible:PREDicted:COUNt?` less that equals its `Not
Tracking` (verified against a screen showing 7 and 2); the health line
is the hardware condition register rendered coarsely; the bracketed
synchronization text is `:SYNChronization:STATe?` and the operation
register.  The firmware's command tree has no per-satellite node, so
elevation, azimuth and signal strength are screen-only.  The medium
tier, four steps totaling 0.57 s, runs every ten seconds.

The screen is on no tier, but it is read two ways: when someone is
looking at it -- `Op::Status` on the socket (op `status`), the status
view in the monitor, and `/status` in the browser, which show the
receiver's screen text beside the sky plot -- and every `--sky`
seconds (300 by default; 0 turns it off), through the same request,
so the log's satellite table is kept without a viewer.  At 1.5 s a
read, the default costs half a percent of the link.  `Screen` keeps the text as well as what was parsed from
it.  A screen read is returned to whoever asked and delivered to the
subscribers on one snapshot, about 1.8 KB per read and nothing between,
so the log records that sky once.  It is never stored as the latest
snapshot: every poll starts from the latest, so a screen kept there
would be copied into every later snapshot and outlive a reconnect to a
different receiver.  The exporter's satellite counts come from the
medium tier's queries for the same reason.

Measured with the daemon running: 234 of 288 consecutive snapshots
were 1.00 s apart, the rest being medium and slow steps publishing in
between, and the only gaps over 1.02 s were the three around a sky
read.

| Tier  | Contents                                                    |
| ----- | ----------------------------------------------------------- |
| ~1 s  | `:SYNC:TINT?`, `:SYNC:TFOM?`, `:SYNC:FFOM?`, `:DIAG:ROSC:EFC:REL?`, `:STAT:OPER:HARD:COND?`, `:SYNC:STATE?`, `:PTIM:TIME?` |
| ~10 s | satellite counts, oven temperature and current, the EFC DAC, `*STB?` and the operation and holdover condition registers, holdover duration and uncertainty |
| ~60 s | position, date, diagnostic log count, learned oscillator tempco, the powerup condition register |
|       | `:SYST:STAT?` (satellite table, health line) is on no tier; it is read on request by the status view. |
|       | The receiver's UTC is on the fast tier, not with the date: a clock read once a minute is wrong for the other fifty-nine seconds. |
| ~10 s | the error queue and any new diagnostic log entries, off the schedule; see "The receiver's own records" |

### Allan deviation is computed over segments, not over a series

`:SYNChronization:TINTerval?` is a phase reading, so a run of them is
what the Allan deviation is defined over.  The interval is between the
GPS receiver's 1 PPS and a 1 PPS derived from the OCXO -- "the time
difference between the 1-pps signal from the GPS engine to a similar
signal derived from the reference source" (`smartclock-dec96a9`,
Enhanced Learning).  The GPS receiver's 1 PPS is quantized to its own
crystal, and while locked the OCXO is steered to follow it, so the
curve is of the pair and the loop between them, not of the OCXO alone.

The estimator is the overlapping one.  Gaps are handled by counting
only the second differences that exist, not by filling anything in,
which stays unbiased while what is missing is unrelated to what was
measured: readings lost to a daemon restart are; readings lost because
the receiver misbehaved would not be, and no estimator can rescue that.

Four rules about gaps:

- **A second difference across a discontinuity is not a measurement.**
  A relock, a holdover, a power cycle or a swapped receiver may step
  the phase.  The run is cut into segments on the recorded mode and
  holdover flag, and on any absence longer than ten reading intervals
  (of the readings, not of a grid coarsened for a long range, so a gap
  that cuts a short range cuts a long one too).  Second differences
  never cross a cut, and segments are pooled.  The state is checked
  across every logged row, including held repeats dropped as readings
  and rows where the interval was not read at all, since a short
  holdover can lie wholly in either.
- **A hole stays a hole.**  Readings go on a grid indexed by time, so a
  missing reading is an empty slot, not a closing-up of the ones after
  it, which would turn every gap into a phase step.
- **A grid point with no reading near it stays empty.**  Nearest wins
  where two readings land on one point, within half a *reading*
  interval, not half a grid step; otherwise the last point of a run is
  filled from a reading up to half a step away.
- **The spacing must be better than one observed gap.**  The grid
  places a reading at `round((t - t0) / tau0)`, so an error in `tau0`
  accumulates.  The median gap carries one gap's jitter -- a nominal
  second came out as 0.999992636 over a few hundred polls, which drops
  readings periodically within a day -- so the median only assigns
  whole-number positions, and the spacing is the least-squares slope of
  time against position, whose error falls as `n^-3/2`.  The slope is
  fitted within each run between absences, about that run's own means:
  a restarted daemon polls on a new phase, and one line through two
  runs of two hundred readings half a second apart was bent by four
  parts in ten thousand.

On a gapless record the estimator is checked in
`crates/smartclock/tests/adev_reference.rs`: the 1000-point data set of
NIST SP 1065 section 12.4 gives Table 31's overlapping Allan deviation
at tau 1, 10 and 100 to the seven figures published, and allantools
2024.06's `oadev` on a thousand-reading record (the script that
regenerates its values is beside the test) agrees to one part in 10^9
at every tau from 1 to 200 s, with the same count of differences.

The web view's query thins held readings in SQL, keeping a row only
where the interval or state differs from the one before.  It reads at
most the newest 500 000 such rows, about two months, and says when a
range held more.

A curve carries its count of readings, holes and segments, because a
curve from a run that is mostly holes looks like one from a clean day.
Points below ten second differences are not emitted, and the sweep
stops at a third of the longest segment.

Long ranges are gridded more coarsely rather than refused: a subsample,
not an average, since averaging before the estimator is the operation
it exists to perform.  That costs the short taus, so the caller picks.

**The reading is already an average.**  `docs/firmware.md` shows
`:SYNChronization:TINTerval?` to be the mean of ten one-second
readings, updated every ten seconds.  For white PM the Allan variance
is 3σx²/τ² at every tau, so averaging ten readings lowers the Allan
deviation by √10 across the whole range that noise dominates.  The
modified Allan deviation has no such limit: its averaging over n
consecutive 10 s means is the window mean over 10n one-second readings,
so MDEV from the record equals MDEV of the one-second phase at every
tau of 10 s and above, with fewer overlapping estimates.  TDEV is tau
times MDEV over root three.  So MDEV and TDEV are the primary curves,
computed from the same gridded segments (a hole voids every window that
spans it, `3m` triples rather than three), and ADEV is kept, labeled,
for comparison with data sheets, which quote it.  Both are checked
against allantools' `mdev` and `tdev` and Table 31's modified Allan and
time deviation columns.

Measured on the bench 58503A, 25 September 2026: one hour of
`:DIAGnostic:PTIMe:TINTerval?` (the one-second reading) beside
`:SYNChronization:TINTerval?`, one pass a second through the daemon.

- The ten-second value is the mean of the ten one-second readings
  ending one poll before it appears, to 0.15 ns rms over 352 holds --
  the reply's 0.1 ns rounding.
- The one-second readings run −72.5 to +45 ns, standard deviation
  30.6 ns, the GPS engine's sawtooth uncorrected.
- allantools on both series:

  | tau, s | ADEV, 1 s readings | ADEV, 10 s means | MDEV, 1 s readings | MDEV, 10 s means |
  | ------ | ------------------ | ---------------- | ------------------ | ---------------- |
  | 10 | 5.34e-9 | 1.68e-9 | 1.69e-9 | 1.68e-9 |
  | 20 | 2.60e-9 | 7.44e-10 | 5.57e-10 | 5.39e-10 |
  | 50 | 1.03e-9 | 3.18e-10 | 1.68e-10 | 1.64e-10 |
  | 100 | 5.25e-10 | 1.67e-10 | 6.89e-11 | 6.72e-11 |
  | 200 | 2.71e-10 | 8.19e-11 | 2.76e-11 | 2.74e-11 |
  | 500 | 1.08e-10 | 3.06e-11 | 6.02e-12 | 6.02e-12 |

  MDEV agrees to 1 to 3 % at every tau, the difference being the count
  of windows.  ADEV from the means is 3.1 to 3.5 times lower at every
  tau to 500 s (√10 is 3.16), because white phase noise dominates the
  whole range: the one-second ADEV falls as 1/τ from 5.2 × 10⁻⁸ at 1 s
  all the way out.  On this receiver the plain Allan deviation of the
  record is not that of the 1 PPS anywhere measured; MDEV is exact
  throughout.

The short end of every curve is the receiver's 1 PPS against the GPS
engine's.  The 58503B specifications (097-58503-12, chapter 4) give
time accuracy as "<110 ns with respect to UTC (USNO MC), 95%
probability" and the 1 PPS output's edge jitter as "<750 ps rms" (the
divided-down OCXO, not the interval to GPS).  The GPS engine's pulse
carries an uncorrected sawtooth the Oncore reports as "-128 .. 127 ns"
(`VPCommands.pdf`, @@Bn/@@En), and no path from that report into the
interval was found in the firmware.  A locked 58503A on the bench over
six hours read between -62 and +81 ns, with reading-to-reading jumps up
to 94 ns, within the manual's 110 ns; so an MTIE of ~100 ns at short
tau is the receiver meeting its specification, and the deviations'
short-tau floor is that jitter.

TDEV has its own chart under the sigma-y chart, sharing its tau axis
and cursor, because it is in seconds and its slope is MDEV's plus one:
white PM falls as tau^-1/2, flicker PM is flat, white FM rises as
tau^1/2, flicker FM as tau, random-walk FM as tau^3/2.  On one plot
with a second axis, TDEV turning up while MDEV still falls reads as a
contradiction.  It answers the timing user's question directly: how far
the 1 PPS wanders over tau.

MTIE is on that chart too, computed rather than inferred.  For Gaussian
noise it is a few times the rms, but it is set by the largest
excursion -- a sawtooth step, a relock, a holdover hop -- which the
deviations average away, so a bound from TDEV can be an order of
magnitude short.  MTIE is a sliding maximum and minimum over windows of
`m + 1` consecutive readings (SP 1065 section 5.2.9), linear through
monotone deques; a window spanning a hole or a segment cut is not
examined, since a relock's step is not wander.  Checked against
allantools' `mtie`.

Each deviation carries a one-sigma confidence interval, drawn as a band
and tabulated as two bounds, since a chi-squared interval is asymmetric
and the upper side matters.  The interval is SP 1065 equation 45 with
Greenhall's equivalent degrees of freedom (section 5.4.1, Greenhall and
Riley 2003) for the noise type the lag 1 autocorrelation method
identifies at each tau (sections 5.5.5 and 5.5.6, with a quadratic
detrend as allantools applies).  One sigma because stability plots are
drawn with it, and a 95 % band on a log axis swallows the curve at long
tau.  Where a tau has too few decimated readings to identify its noise,
the previous tau's answer is carried (section 5.3.2); where the record
is too short for the edf algorithm -- white PM with under three windows
-- there is no interval.  Degrees of freedom are pooled across segments
as the differences are.  MTIE has no standard interval and gets none.
The chi-squared quantile is Wilson and Hilferty's approximation, within
0.4 % of scipy's in the deviation from two degrees of freedom up and
2.6 % at one and a half; the chain is tested against allantools'
`autocorr_noise_id`, `edf_greenhall` and `confidence_interval` on the
reference record.  The TUI draws no band: ratatui has no fill.

`:DIAGnostic:PTIMe:TINTerval?`, the latest one-second reading, would
extend the curves below 10 s and is not polled: it costs a query a
second and ten times the phase rows, for the receiver-noise floor
alone.  Plain `:PTIMe:TINTerval?` is the same mean as
`:SYNChronization:TINTerval?`, by the image's handler table and by 187
paired polls of the bench 58503A.

### One log per receiver, named by its serial

More than one receiver on a host means one daemon per port --
`smartclockd@<name>`, each with its own device and socket -- so nothing
in a daemon becomes concurrent.  The template is the only daemon unit,
even on a host with one port: two ways to run the same daemon was one
too many.  The instance is named by the port, as `serial-getty@` is,
and the unit derives the device and socket from the name; anything
else per port goes in a drop-in.  Shared settings are in
`/etc/default/smartclockd`, which every instance reads.

The daemon and the monitor have no socket default, since a default
would guess an instance name.  The viewers are host-wide:
`smartclock-web` and the exporter scan `/run/smartclockd/*/socket` as
the web view scans the log directory; the web's live strip follows the
selected receiver to its daemon, and the exporter labels every sample
with the daemon instance and the receiver's serial and model.  Neither
takes a single socket unless told to.

The log a daemon writes is chosen by the receiver, not the port: after
`*IDN?` it opens `/var/lib/smartclockd/<model>-<serial>.sqlite`.
Nothing is opened until a unit has answered.  A swap on a port switches
files; a unit moved to another port keeps one history; two instances
cannot collide on a file.  A receiver whose identity does not parse
gets a file named after the device and a loud log line, not a shared
"unknown" file.  The schema is unchanged: `receiver_id` and the
`receiver` table still record which unit a file's rows came from.

`smartclock-web` takes the log directory, lists every `*.sqlite` in it,
live or historical, and keys everything by serial: `?receiver=` is the
serial.  `/api/receivers` lists each unit with the columns its model's
command table can measure, and the pages show only those.  Comparing
two units on one chart would be two queries bucketed the same way on
one time axis; not built.

The one log that held two serials was split by hand, a one-off
`sqlite3` job by `receiver_id`: there is no fleet to migrate, and a
migration nobody else runs is code nobody tests.

## Architecture

Cargo workspace:

- `smartclock` -- library.  Typed errors via `thiserror`, no `anyhow`,
  blocking synchronous API.
- `smartclockd` -- the daemon.  Owns the port, logs, serves the socket.
- `smartclockmon` -- TUI client, `ratatui` + `crossterm`.
- `smartclock-cli` -- one-shot queries, `diagnose`, transcript capture,
  `read-memory`.
- `smartclock-sim` -- the simulated receiver, in process and over TCP.
- `smartclock-exporter` -- Prometheus metrics from the daemons' sockets.
- `smartclock-web` -- the browser views, over the logs and sockets.
- `smartclock-http` -- the minimal HTTP server the exporter and web
  view share.
- `smartclock-log` -- the log's schema and every query that reads it.
  The table definitions, the schema version, the stored timestamp form
  and the `meta` keys live here; the daemon creates the tables from
  them and keeps its own writes and migrations, and the monitor and
  the web view read through it.  One copy of the readers rather than
  one per front end: two copies had drifted, the monitor's scanning
  the whole table on every refresh where the web view's used the
  index.  Its tests build every log from the same definitions the
  daemon creates, so a table change that forgets a reader fails there.

`smartclock-cli` works two ways: `--device <path>` talks to the
receiver directly, which requires the daemon stopped, and `--socket
<path>` goes through the daemon.

Library layers, bottom up:

1. `transport` -- `trait Transport: Read + Write`, with
   `SerialTransport` (`serialport`), `TcpTransport` (ser2net),
   `ReplayTransport` (fixtures) and `TeeTransport` (transcripts); the
   simulator's `SimTransport` is in `smartclock-sim`.  Most of the stack
   is testable without hardware.
2. `session` -- command framing: read-until-prompt, echo suppression,
   per-command timeout, error queue drain.  Handles the `scpi> ` and
   `E-nnn> ` prompts, and uses `:SYSTem:STATus:LENGth?` to read the
   status screen by line count rather than by timeout.
3. `dialect` -- per-model command tree.  A `Dialect` trait resolves
   logical operations to model-specific command strings, response
   formats and availability.  Model detected from `*IDN?`, overridable.
4. `types` -- newtypes and enums: `Tfom`, `Ffom`, `Prn`, `EfcPercent`,
   `TimeInterval`, `SmartClockMode`, `HoldoverState`,
   `HoldoverWaitReason`, `HardwareCondition`, `Position`,
   `SatelliteInfo`, `LeapPending`.  Units converted at the boundary.
5. `parse` -- response parsers, the status screen scraper, and the
   TCODe T1/T2 parser with checksum verification.
6. `client` -- `Device` with typed methods, plus `poll() -> Snapshot`.
7. `task` -- `DeviceTask`: owns the session, runs the schedule, serves
   the request queue, broadcasts snapshots.

### systemd unit

- `Type=exec`.  The daemon comes up and keeps retrying with no receiver
  attached, so there is no moment that honestly counts as ready for
  `Type=notify`.
- `Restart=on-failure` with a backoff and a start limit, so a bad
  setting in `/etc/default/smartclockd` lands the unit in `failed`
  rather than restarting forever.
- No `BindsTo=` / `After=` for the adapter's `.device` unit.  The
  daemon reconnects on its own, binding would stop it while an adapter
  is unplugged, and naming a device in the unit would mean editing the
  unit.  Offered as a drop-in recipe in `docs/running.md`.
- Every option readable from `SMARTCLOCKD_*`, so
  `/etc/default/smartclockd` and a drop-in are all an operator edits and
  `ExecStart=` names no settings.  The unit sets two variables itself,
  the device and the socket, from the instance name.
- `StateDirectory=smartclockd` for the logs, shared by every instance;
  `RuntimeDirectory=smartclockd/<instance>` for the socket.
- Dedicated user with `SupplementaryGroups=dialout`;
  `RuntimeDirectoryMode` and socket group ownership set so the TUI runs
  unprivileged.
- Hardening: `ProtectSystem=strict`, `PrivateTmp`, `NoNewPrivileges`.
  `RestrictAddressFamilies=` admits `AF_INET` and `AF_INET6` as well as
  `AF_UNIX`, or the `tcp://host:port` device form cannot open.
  `PrivateDevices` is absent: it would hide the serial port.

With `interprocess` handling the IPC and no peer-credential code,
systemd is the only Linux-specific piece; `serialport` and `rusqlite`
are portable.  Porting means a launchd plist or a Windows service
wrapper, not touching the protocol.

### The command table is data

Commands live in a TOML file, and `build.rs` generates the dialect code
from it.  Each entry carries a stable logical id, its classification, a
citation, and one block per dialect:

```toml
[[command]]
id    = "holdover_waiting"
class = "query"
cite  = "097-59551-02 5-36"

  [command.dialect.hp58503]
  scpi     = ":SYNChronization:HOLDover:WAITing?"
  response = "enum:HoldoverWaitReason"
  models   = ["58503A", "58503B", "59551A"]

  [command.dialect.z3801]
  scpi     = ":ROSCillator:HOLDover:WAITing?"
  response = "enum:HoldoverWaitReason"
  models   = ["Z3801A", "Z3816A"]
```

The divergence between trees, per-model availability and the manual
citations sit in one table that can be diffed against the documents.
Codegen rather than runtime parsing, so the logical ids are an enum and
a typo is a compile error.  `response` names a parser; the parsers stay
hand-written in `parse`.

An entry may declare what argument it takes, which the daemon checks a
client's command against before anything reaches the receiver:

```toml
  [command.argument]
  kind = "integer"   # or "word" with `allowed`, "none", or "free"
  min  = 0
  max  = 90
```

Omitting the block means the command takes no argument.  `free` must be
asked for by name, and a test pins the list of entries that ask: when
it was the default, a query header could carry the set form's payload
and `:SYSTem:LANGuage? "INSTALL"` reached the simulated receiver on a
daemon started with no flags.

A test checks that every command reachable on the active dialect has a
parser and a fixture, and the per-model command matrix
(`docs/commands.md`) is generated from the same source.

### The package version is derived

`make deb` builds `<version>-<commits>+g<sha>`, with `+dirty` when the
tree is not clean.  Two builds of different code cannot carry the same
version, which matters because dpkg treats reinstalling an identical
version as a no-op.  The hand-written `packaging/debian/changelog` is
the record of releases; a derived version is a build, not a release.

### CI is shared; the Makefile is what you run

```
make            build
make ci         fmt-check clippy test
make fmt        cargo fmt
make clippy     cargo clippy --all-targets -- -D warnings
make test       cargo test
make test-hw    cargo test -- --ignored
make web-deps   install the browser tests' Playwright and Chromium
make test-web   the browser tests
make deb        a snapshot package
make release    tag the version Cargo.toml already names
```

CI calls the reusable `rust-ci.yml` shared with the other repositories
in this fleet, because one repository doing it differently costs more
than running one definition locally and in CI.  The inputs restore what
the shared defaults drop: the toolchain is pinned; `cargo fmt` is
pinned to the same toolchain, because nightly rustfmt formats
differently; and `--all-targets` is passed, or clippy never lints the
tests.

Two things `make ci` enforces that CI does not.  The shared `cargo
test` does not say `--workspace`, which is harmless while there is no
`default-members`; adding one would silently narrow CI (`Cargo.toml`
says so).  And the shared workflow pins its actions by tag rather than
by commit sha, the fleet's posture rather than this repository's
preference.  `make ci` remains the stricter local command.

The browser pages have Playwright tests, `make test-web`, installed
into `crates/smartclock-web/tests/browser` by `make web-deps`, Chromium
included, so nothing lands in the home directory.  They serve the pages
from the real server and fake its API, because the cases worth testing
are the ones a live daemon cannot show on demand: which receivers
exist, when a daemon goes, how late an answer arrives.  Every test runs
against every page, because the pages had drifted apart at exactly
those edges.  The pages share one lifecycle in
`crates/smartclock-web/src/common.js`, to which each page describes its
content once.  CI runs them in `.github/workflows/web.yml`, since they
need Node and a browser the fleet's workflow does not have; `make ci`
leaves them out for the same reason.

Tests that need a receiver are `#[ignore]`d and reachable only through
`make test-hw`, since they need hardware and a stopped daemon.
Everything else -- parsers, the scraper, session framing against
`ReplayTransport`, integration tests against the simulator -- runs in
`make test`.

### Dialects

Two branches, not four.  `097-59551-02` shows the 58503A tree is
essentially identical to the 58503B:

| Family                  | Tree                                      |
| ----------------------- | ----------------------------------------- |
| 58503A / 58503B / 59551A| `:GPS:`, `:SYNChronization:`, `:PTIMe:`   |
| Z3801A / Z3816A         | `:PTIME:GPSYSTEM:`, `:ROSCillator:`       |

The Z3805A answers the z3801 dialect.  Availability is per model, not
just per family: the 58503A has `:GPS:SATellite:TRACking:IGNore` /
`INCLude`, which the 58503B guide marks 59551A-only.  Unsupported
operations return a typed `Unsupported` error without reaching the
device.

Response formats differ too, so the table carries them:
`:DIAG:ROSC:EFC:REL?` returns `+-d.dEe` on the 58503A but is documented
as a plain integer on the Z3801A.

Each entry records its `evidence`: `manual`, `firmware` (every keyword
found in the firmware's own keyword table) or `hardware` (a receiver
answered it), so an unconfirmed command is a known risk rather than a
silent assumption.

The bench Z3801A (3543-A) differs from the 58503A on the wire in two
more ways.  It does not echo, so a command with no reply comes back as
the previous prompt's trailing space and the prompt alone, which the
prompt matcher allows for.  And it stamps its diagnostic log
`Log NNN:H%08X: message`, a hex count, where the 58503A and the Z3805A
write a calendar date; the manual defines the `H` notation as seconds
of GPS time since 1980-01-06 (`097-z3801-01` 4-13).  The stamp is
stored as written and the web view shows it decoded, the hex in the
tooltip.  Its `-230 Data corrupt or stale` on `:PTIMe:TINTerval?` and
`:PTIMe:FFOMerit?` while it tracks no satellites is the manual's answer
for an unavailable value (4-6, 4-7), taken as a state refusal like the
58503A's.

### The status screen scraper is mandatory

`:GPS:SATellite:TRACking?` returns PRN numbers only.  Per-satellite
elevation, azimuth and C/N exist only in the `:SYSTem:STATus?` screen,
so the scraper is a first-class parser, not a fallback.

`097-59551-02` chapter 3 has five sample screens covering distinct
states -- survey in progress, `*nn Acq..` acquiring markers, `nn -- ---`
untracked rows, `Predict --` unavailable, "Locked to GPS: stabilizing
frequency".  More are in `097-58503-13`.  These are scraper goldens.

The scraper reads by label, not column.  An asterisk ends the
preceding cell, and groups carry their heading column so a blank cell
can be told from a missing one; without both, a blank signal cell let
the next satellite's asterisk be read as this one's signal, assembling
`PRN 24` at an elevation of 204 degrees, and a short tracked column let
the first group claim the second group's rows.  The tracked count is
the `Tracking:` not preceded by `Not `, since a plain search finds it
inside `Not Tracking:` first.

The screen states how many satellites are tracked and how many are
not, and the table is checked against those counts.  A disagreeing
table is still shown, with the pane saying the counts disagree.  This
check turns a class of misreadings into a visible failure without
anyone having to predict the next one's shape.

## EFC diagnosis

The development unit was thought to be failing: an OCXO aged past the
range its EFC DAC can pull presents as a unit stuck in holdover.

The evidence says otherwise.  It is locked with a valid reference, and
has been for every sample since the log began carrying the condition
registers.  Its diagnostic log shows nineteen holdover-and-relock
cycles across two days in March 2025 and nothing since, which reads as
GPS reception; an oscillator drifting out of range does not recover
nineteen times.  The EFC measurement settles the mapping in favor of
the specification, leaving about a decade of tuning headroom.  On
2026-09-26 the crystal was retrimmed with the EFC input grounded; the
receiver now locks with the pin near 0 V, and the retrim measured the
pull, 3.94 × 10⁻¹³ per count, 0.63 of the loop's assumed G
(`docs/efc.md`).

The diagnosis is "not the oscillator, on this evidence".

`:STATus:OPERation:HARDware:CONDition?` bits:

| Bit | Condition                        |
| --- | -------------------------------- |
| 0   | Selftest Failure                 |
| 1   | +15V Supply Exceeds Tolerance    |
| 2   | -15V Supply Exceeds Tolerance    |
| 3   | +5V Supply Exceeds Tolerance     |
| 4   | Oven Supply Exceeds Tolerance    |
| 6   | EFC Voltage Near Full-Scale      |
| 7   | EFC Voltage Full-Scale           |
| 8   | GPS 1 PPS Failure                |
| 9   | GPS Failure                      |
| 10  | TI Measurement Failed            |
| 11  | EEPROM Write Failed              |
| 12  | Internal Reference Failure       |

`:STATus:OPERation:HOLDover:CONDition?` bits: 0 Holding, 1 Waiting to
Recover, 2 Recovering, 3 Exceeding Threshold.

Bits 6 and 7 plus `:SYNChronization:HOLDover:WAITing?`, which returns
`HARDware | GPS | LIMit | NONE`, separate an oscillator fault from a
GPS or antenna fault.  This is why `smartclock-cli diagnose` came in
phase 2, before the daemon or the TUI.

## Phases

| # | Deliverable | What it involved |
| - | ----------- | ---------------- |
| 0 | Workspace, TOML command table with `build.rs` codegen, fixtures, Makefile CI | evidence became three-valued rather than a boolean |
| 1 | `transport`, `session`, `smartclock-cli` | the manuals had the prompt wrong, and an abandoned reply desynchronized everything after it |
| 2 | `types`, `parse`, screen scraper, `diagnose` | the scraper had to read by label, not column; the manuals' own ASCII does not line up |
| 3 | `Device`, `Snapshot`, `DeviceTask`, `smartclockd`, systemd unit, deb | reconnection meant handing the request channel back out of the task |
| 4 | `smartclock-sim`, in process and over TCP | TCP rather than a pseudo-terminal, which would have been Unix-only |
| 5 | `smartclockmon`, dashboard and history graphs | columns carry min and max as well as mean, or quantization steps vanish into a ramp |
| 6 | `Control` handle, daemon flags, audit trail, raw console | flags replaced the nonce; the classifier could not match a caller-supplied argument |
| 7 | `docs/commands.md`, generated | a test compares it against the table, so it cannot drift |
| 8 | `smartclock-exporter`, `smartclock-web` | a scrape must cost the receiver nothing, so both read what the daemon already polled |
| 9 | The receiver's own records: error queue, diagnostic log, condition registers | reading is what removes an entry, so the read and the write share a thread |
| 10 | Adoption per connection; the alarm watched, not taken | `*STB?` reads the alarm summary without taking the latch; event registers are not polled |
| 11 | One log per receiver, one daemon per port | the log opens after `*IDN?`, named by model and serial; `smartclockd@.service` is the only daemon unit; the web view and exporter read every log and socket on the host, keyed by serial |
| 12 | Line settings probed; one lifecycle for the web pages | a unit's rate and framing are its own, not the port's; the pages share `common.js` and Playwright tests fake the API to reach the edges a live daemon cannot show |

Suggest a commit at each phase boundary.

### Next: notes and receiver facts

Bench events and what is inside a unit are not visible from the
receiver, and so far live only in `docs/hardware-investigations.md`,
where they cannot be lined up against the log.  Two additions to each
receiver's log, written through the daemon and never sent to the
receiver:

- *Notes.*  Timestamped free text -- "added a 20 dB LNA ahead of the
  Z3805A", "ran `master_reset`" -- for one receiver or for the whole
  bench, such as a splitter change.  `smartclock-cli --socket ... note
  TEXT`, with `--at TIME` to backdate.  Shown as markers on the web
  history charts, with the text on hover, in a list beside the
  journal, and in the TUI's journal view.
- *Facts.*  Free-form `key=value` describing the unit:
  `ocxo.serial`, `ocxo.model`, `antenna.feed`, an engine swapped in.
  Each records when it became true, so a replaced part keeps its old
  value in the history.  `smartclock-cli --socket ... fact KEY VALUE`;
  setting one also leaves a note.  Current facts head `diagnose` and
  the web receiver info, beside what the unit reports itself.

Once built, the events in `hardware-investigations.md` are entered as
notes at their recorded times.

### Next: comparing receivers

When the host logs more than one receiver, a page of its own shows
them together over the same range: 1 PPS TI, EFC, temperature, TFOM
and FFOM overlaid, and ADEV and MDEV curves on one plot.  The live,
status and stability pages stay one receiver each.  Notes and facts
mark what differs between the units.

## Open questions

Things not decided, as distinct from the defects below.

1. **Which state machine drives the mode suffixes.**  Still inferred
   from the outside (`docs/screen-format-strings.md`, "Mode
   suffixes").  The other firmware questions are settled in
   `docs/firmware.md`: `:DIAGnostic:ROSCillator:TCOefficient?` is a
   stored constant on the oscillator current in the loop's EFC,
   written only by its own setter ("s, the oscillator current"), and
   `RELative?` is (ABS − 2¹⁹) / 2¹⁹ × 100 with ABS sixteen times a
   16-bit DAC word ("The 58503A image").  The images are
   `third_party/z3801a-3543.bin`, `z3805a-3543b.bin`,
   `z3816a-4001.bin`, `58503a-3633.bin` (assembled from others' dumps)
   and `58503a-3704.bin` (read from the bench unit through its pForth
   console with `smartclock-cli read-memory`), with EEPROM dumps of the
   bench units.  The 58503A images carry the front panel's strings
   (`10MHZ STABLE` is in both); they have not been enumerated.

2. **How far the z3801 dialect is confirmed.**  A Z3801A and a Z3805A
   are on the bench and answer it.  Of the z3801 entries, 18 are
   `evidence = "hardware"`, 63 `firmware` (spellings corroborated
   against the firmware's keyword table) and one `manual`; those 64
   stay unconfirmed until a receiver answers them.

3. **Asserting a known antenna position from the configuration.**  A
   receiver at a surveyed site, told where it is, goes straight to
   position hold and serves time, where a survey leaves the 1 PPS
   invalid while it runs.  A Z3805A brought up on a bench where a
   58503A had already surveyed the same antenna spent an hour unable to
   serve time; asserting the position by hand took two commands.

   So a position setting asserted on attach, in one of three forms: a
   geodetic position, earth-centered x, y and z, or `survey` to ask
   for a survey deliberately rather than inheriting whatever the last
   operator left set.

   - It is a control command, so it needs the same opt-in and must not
     fire under a daemon started read-only.
   - It is non-volatile on the receiver, so a stale value outlives the
     configuration that set it.  Holding a position also means turning
     off survey-on-powerup, or the next power cycle discards it; with
     both set, a receiver whose antenna has moved never notices.
     Whatever asserts a position must be able to release it.
   - The models disagree about the vertical datum -- the 58503B reports
     height above the ellipsoid where the others use mean sea level --
     so a geodetic setting must say which it means.  Earth-centered
     coordinates sidestep that.

4. **Whether acknowledging from the monitor is wanted.**  Reading an
   event register says which bit latched rather than which group, and
   clears the alarm as it does so: the right implementation of a
   deliberate acknowledgment and the wrong thing to do on a timer.
   Nothing needs it yet.

5. **Maybe: a Host allowlist for the web view.**  `smartclock-http`
   checks neither `Host` nor `Origin`, so a page in the operator's
   browser can rebind its own name to the web view's address and read
   `/api/status`, position included, or force screen reads at 1.5 s of
   link each.  The likely shape is `--allow-host NAME`, repeatable,
   with loopback always allowed and a non-loopback bind refused
   without one.  Deferred: no access-control work for now.

## Known defects

Known and unfixed, each because the fix is not yet worth its cost.

**A receiver swapped without breaking the link is not noticed.**
`*IDN?` is read when the port is opened and not again while the link
stays up.  Swap the cable between two receivers quickly enough that no
command fails and the new unit's rows are filed under the old one.  The
fast tier sends a command every second, so a swap almost always breaks
a read and is caught; still, stop the daemon before moving the cable
between units.  The fix is cheap -- ask `*IDN?` on the slow tier and
compare -- if swapping becomes routine.

**The receiver's log timestamps are not monotonic across a power
cycle.**  Its clock restarts at midnight on a stale date and runs free
until the first lock, so a stamp in that window is elapsed time since
boot.  From the development unit's log:

    3  20050528.00:01:08  Position hold mode started
    4  20050528.00:02:00  GPS reference valid at 20050529.04:09:25
    5  20050528.00:00:00  Power on
    6  20050528.00:00:28  Position hold mode started

Eighteen of the nineteen midnight stamps in that log are a power-on or
a preset.  Entry 4 carries the real time in its message.  The stamps
are stored as written and are not an ordering key.

Ordering by `entry` alone is wrong across a clear, which restarts the
numbering; ordering by `at` is wrong within the bulk copy of history,
which is not fetched in entry order.  Since schema 7,
`receiver_log.generation` counts the clears, so `(generation, entry)`
orders the whole log.  Migration infers generations by walking a
unit's rows in read order and starting a new one wherever an entry
number comes round again.

A clear is noticed by the count falling below the highest entry held.
A log cleared and refilled to at least that count while the link was
down numbers the same way, and with `--adopt-log` the old generation's
completeness would authorize erasing entries never copied.  So once per
connection, before copying or erasing, three held entries -- the
newest, the oldest and one between -- are read again and compared;
any difference starts a new generation.  Three because the receiver
repeats a power-on stamp and message.  Two logs matching at all three
would still pass.

Two things are not defended against, because the threat model is a
careless operator on a single-operator machine, not an attacker.

- Socket permissions are the whole of the authorization.
- A client that connects and never speaks holds one of the sixteen
  slots until the process ends.  `interprocess` 2.4.4's portable
  `Stream` exposes no handle to set a read timeout on, so closing this
  would mean `socket2` and a `cfg(unix)` arm for `SO_RCVTIMEO` in the
  part of the daemon written to stay portable.  The daemon says so in
  the journal.  A client half-closing its sending side releases its
  slot and stops the push thread, and is tested.

## Backlog

What a review of the whole codebase in October 2026 (two reviewers,
each finding checked against the code) found and has not yet been
done, in the order agreed.  Done items are in the history, not here.
Kept here so that any machine with the repo has the list.

Decided and not to be done: renaming exporter metrics or changing their
labels (`oven_tempco` stays; a stopped daemon's `up 0` carries only its
instance label).

**Daemon and library, receiver identity.**  A swap is noticed only when
the link breaks (see "Known defects"), and three things go wrong around
one even then:

- After a reconnect the first poll starts from the previous unit's last
  snapshot, so its fields can be logged as the new unit's until each
  tier has read.  Start each attachment from an empty snapshot.
- A command queued while one unit was attached can run against the
  next, audited under the first; a journal pass can run across the
  change.  Bind requests to an attachment and check it before sending.
- A failed poll republishes the previous snapshot under the same `at`,
  so the log holds the row twice and history weights it double.
- Clients are not told of a new attachment: the monitor keeps the old
  unit's screen, and in per-receiver mode keeps reading the old unit's
  log.  Push the attachment to subscribers.
- `receiver.last_seen` is updated only on attach; it should follow the
  last row logged, which is what every reader takes it to mean.

**Daemon, other.**

- `query()` treats an `E-nnn>` prompt as this command's failure, but the
  prompt shows the error queue (097-59551-02 pp. 3-6, A-6): a valid
  reply is dropped while an older error is queued, and draining a queue
  of two or more loses entries.  Confirm on the bench, and make the
  simulator's prompt reflect its queue first.
- A subscriber dropped for falling behind keeps its socket open, and no
  write has a timeout, so a client that stops reading holds a slot and
  two threads for good.  The library now sets socket timeouts with
  `socket2` under `cfg(unix)`; the same would close the "client that
  never speaks" defect above.
- Starting a second daemon on a live socket unlinks it; probe with a
  connect first.  The accept loop spins on EMFILE; back off.
- A log that fails to open is retried, and reported, on every snapshot.
- A journal pass ignores its time budget except in two steps.
- A screen request has no deadline, and one discarded with the queue is
  dropped without an answer.
- One failure path per request kind; a plain refusal from the receiver
  needlessly resynchronizes the link.
- The medium tier aborts when `holdover_duration` is refused, where
  every other field is left absent.
- The fixed five-second prompt timeout is too short for the status
  screen below 4800 baud.

**Web view.**  Host and Origin checks are open question 5.  The query
string is parsed by hand in two places; move it into `smartclock-http`.
`smartclock-http` uses `anyhow`, which AGENTS.md forbids in a library.

**Simulator.**  Its prompt is always `E-113`; it should carry the real
queue state, and a Z3801A with no echo should exist so both framings
are tested.  Its binary takes arguments by position.

**Smaller.**

- The `oven_tempco` HELP text repeats a claim `docs/efc.md` retracted;
  reword it, keeping the name.
- Comments that describe something else: `JOURNAL_EVERY` says passes
  read the event registers; `adev::at_with` says holes are not
  subtracted; doc comments sit on the wrong items in `smartclockd/db.rs`
  and `smartclock-web`.
- The audit `label` column is always NULL.
- Allan deviation confidence references cover one noise type; add white
  and flicker phase and random walk, and the boundary cases.
- Refactors: the rollover and date wording is written three times;
  status register newtypes by macro; `Screen` fields as the existing
  newtypes; the normal quantile code computes only plus or minus one;
  `adev` internals made private; the daemon's test databases through
  one fixture; `Info::generation` counts connections, not generations.
