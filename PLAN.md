# Plan

A Rust library for talking to HP / Symmetricom SmartClock GPS receivers
over serial, a logging daemon that runs as a system service, a TUI, and
browser and Prometheus views.  A GUI is not planned.

## Status

Phases 0 to 12 are done; **Phases** lists what is next.  Installed
from the package as a service, one daemon per port, logging to
`/var/lib/smartclockd/<model>-<serial>.sqlite`; `docs/running.md` is
the deployment note.  `make ci` is the local check; `make test-hw`, the
hardware-only set, never runs in CI.

Review diffs, not only the tree: the defects found in September 2026
each produced a wrong value rather than an error while every test
passed, because fixtures covered only the cases the code was written
from.

## Goals

- Read-only monitoring of a 58503A first; control commands later.
- Unattended long-term logging of EFC and holdover state for drift
  analysis, independent of whether anyone is watching.
- Support the wider SmartClock family, notably the Z3801A.

## Constraints

- **Single-operator machine**, as a standing assumption.  It justifies
  socket permissions as the whole of authorization, an audit trail with
  no notion of who, and one daemon per device rather than a fleet.
  Revisit these together if it stops being true.
- **58503A first.**  Other variants' table entries come from their
  manuals and firmware.  A Z3801A and a Z3805A on the bench answer the
  z3801 dialect.

## Decisions

### Flashing is a direct-port command of the CLI

`smartclock-cli flash` loads firmware with the library's session,
console and line-settings code, so there is one tool to learn.  The
forbidden-command check applies to commands typed to `query` and
`sweep`; `flash`, like `read-memory`, is a fixed procedure.  It
requires the port's daemon stopped and never connects to its socket.

A shared installer procedure uses audited image profiles for Z3801A,
Z3805A, 58503A and Z3816A, with their different protected flash regions
and checksum algorithms.  Before erase it checks the exact image
SHA-256, boot checksums, model, running revision and expected serial.
Unknown images and revisions are refused, with no force override.
Models without dumps, including 59551A, need an audited profile first;
filenames and embedded model strings cannot establish compatibility.

Installer revisions are checked against the model/layout allowlist, not
paired with a primary revision: boot flash survives upgrades.  The CLI
says so in check-only mode and just before erase.  Errors the receiver
held before the run stop the preflight, all listed; the session has
read them off the queue, so a second run proceeds.  The flasher never
sends `*CLS`.

The simulator optionally models the installer, flash contents, record
validation and boot checksums.  Tests compare the whole resulting
image, protected boot flash included, exercise interrupted-download
recovery through the real session framing, and cover both 58503A
revision changes (3633 to 3704 and back), reading the revision from the
new primary and preserving the boot region.  Final verification
requires the candidate revision, model and serial; a changed suffix is
reported separately, since its behavior across upgrades is unknown.
Hardware timing, wear and upgrade-induced settings changes are not
simulated.  Cross-revision flashing is untested on hardware; record
settings before and after the first upgrade.

After writing, the flasher reads the whole flash back through the debug
console, compares it with the image, and returns to SCPI with the
console's `halt`, or failing that through the installer.  `read-memory` and `flash` share one reader and one way back
in `smartclock::console`.

### No client-side SCPI crate

Surveyed: `scpi` + `scpi-contrib` (server side, no_std), `scpify` (TCP
and HiSLIP only), `scpi-client` 0.1.1 (thin, immature),
`instrument-core` 0.1.0 (VISA/GPIB oriented).  None fit: the receiver
echoes, prompts with `scpi> ` / `E-nnn> `, and its richest response is
an ASCII status screen, where generic clients assume a clean write /
read-to-terminator cycle.  The standard parts are shallow and specified
in `097-59551-02`: `:SYSTem:ERRor?` returns `<code>,"<text>"`, and the
status registers are IEEE 488.2.  The simulator skips `scpi` too: it
does not model the prompt, echo and screen, the parts worth emulating.

### No separate document for the socket protocol

All four clients -- monitor, CLI, exporter, browser view -- are built
from this tree, and the compiler checks the wire type.
`smartclock::wire::Reading` is not `Snapshot`, so an internal rename is
a compile error, not a protocol change.  `smartclock::protocol`
documents what a type cannot: newline-delimited JSON over a local
socket; unsolicited snapshots, so every request carries an id its reply
echoes; and a `VERSION` the daemon refuses a mismatch on.  A client
outside this tree would need a specification and a stability
commitment.

### The daemon owns the port

`smartclockd` runs as a systemd system service and holds the serial
port open, so every other component is its client and multi-day EFC
history does not depend on a TUI being up.

One `DeviceTask` thread owns the `Session`, and so the fd, and issues
every command: an interleaved command's half-read prompt desynchronizes
every later read.  `Session` is not `Sync` and moves into the device
thread at construction, so a second writer is a compile error.  A
command that errors must not leave its reply in flight, or the next
poll reads it as its own (TFOM recorded as FFOM); `drain` reports
failure when it gives up, so `sync` cannot match an abandoned reply's
prompt.

One daemon per socket: a stale socket file is removed before binding
only when nothing answers on it, and a second daemon pointed at a live
socket refuses to start.

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

Rate and framing belong to the unit, not the port, and ports
re-enumerate: the 58503A's are settable, the Z3801A's fixed at 19200
7O1 (`097-z3801-01` 1-8, 2-10).  `--baud` and `--framing` give the
settings to try first; with no identity, 19200 and 9600 at 8N1 and 7O1
are tried in turn (`smartclock::attach`).  The daemon, monitor and CLI
probe; `read-memory` (pForth console) and networked receivers do not.
Per-port framing in a drop-in only saves the probe.

A probe at the wrong framing queues errors on the receiver.  They are
read off at the working settings before anything else and counted in
the journal; `*CLS` would also clear the event registers, which drive
the front-panel alarm.

### The simulator is not PTY-backed

A PTY would make it Unix-only, undoing the reason `interprocess` was
chosen over raw AF_UNIX: systemd is to be the one Linux-specific piece.
Tests use `SimTransport` in `smartclock-sim`, an in-process
`Transport`; end-to-end runs use the same simulator on a TCP listener,
through the `TcpTransport` ser2net needs anyway.

### The protocol stays JSON

gRPC in Rust means `tonic`, which lacks named pipes, so it would land
on loopback TCP and reopen the authentication question socket
permissions answer.  Protobuf would add a codegen step and lose
watching the socket with `socat`, for a message a second between two
ends we control.  Revisit if a non-Rust client appears.  The socket
carries its own type, converted from `Snapshot`, so a field rename does
not change the wire format.

### Two channels to the daemon

- **Local socket, via `interprocess`**, for the live snapshot stream
  and commands: one API over AF_UNIX and Windows named pipes,
  newline-delimited JSON.  A client gets the current snapshot on
  connect, then updates.  The name is a filesystem path, so systemd's
  `RuntimeDirectory` owns its lifetime and file permissions gate
  access.
- **SQLite file, opened read-only**, for history.  `journal_mode=WAL`
  lets readers run beside the daemon's single writer.  A read-only
  reader needs `-shm` and cannot create it in `/var/lib/smartclockd`,
  so the daemon sets persistent WAL and closing a log leaves `-wal` and
  `-shm` in place.  rusqlite has no safe call for it; the workspace
  denies `unsafe_code` rather than forbidding it, and this one call
  carries the `expect`.  A log closed by anything else, such as a hand
  `sqlite3` session, loses `-shm`; the readers then open it immutable,
  which is safe because no writer holds it.

The socket keeps the TUI's live pane off SQLite at 1 Hz and makes
reconnection after a daemon restart trivial.  Protocol and schema each
carry a version, since daemon and clients upgrade independently.

### Commands go through the daemon too

`Control` is the same type whether the daemon calls it locally or the
CLI over the socket, so the protocol mirrors the library API:

```
-> {"v":1,"id":"7f3a","op":{"kind":"query","cmd":"holdover_waiting"}}
<- {"v":1,"id":"7f3a","ok":{"holdover_waiting":"LIMit"}}
<- {"v":1,"event":"snapshot","ts":"...","efc_pct":-94.2,...}
```

A reply reports what the device said, not that the operation finished;
a survey takes hours but acks in milliseconds, and clients watch
progress in later snapshots.

| Class     | Examples                                              | Gate                     |
| --------- | ----------------------------------------------------- | ------------------------ |
| Query     | `:GPS:POSition?`, `:SYNC:TINT?`                        | none                     |
| Control   | holdover initiate and recover, survey, antenna delay, elevation mask; reading an event register or `*ESR?`, which clears it; reading `:SYSTem:ERRor?`, which removes the entry | `--allow-control` |
| Dangerous | `:SYSTem:PRESet`, `:SYSTem:COMMunicate:SERial1:*`, `:DIAGnostic:ERASe`, `:SYSTem:LANGuage "INSTALL"` | `--allow-dangerous` |

Dangerous commands can strand the link or wipe configuration; a baud
change persists across power cycles.  The gate is a daemon flag, set
outside any client, so a daemon started without it cannot be talked
into the command.  On SIGTERM or SIGINT the daemon stops its task and
waits for the log thread to write held audit entries and snapshots; a
second signal exits at once.

A command containing `;`, a control character or any non-ASCII is
refused before classification, since SCPI chains units with `;` and
`:SYSTem:STATus? ;:SYSTem:COMMunicate:SERial1:BAUD 1200` would classify
as a query.  Arguments are whitelisted: every value this receiver takes
is a number, a word, a list or a quoted string.  Argument ranges sit in
the command table beside the class, so the table, not the typed
`Control` API, is the safety boundary.

Authorization is socket permissions alone: `RuntimeDirectory`,
`RuntimeDirectoryMode` and group ownership decide who opens the socket,
and whoever can may issue whatever the daemon's flags allow.  No peer
credentials or tokens; the audit trail records what, not who.

The TUI's raw SCPI console bypasses the table, so it has its own flag,
`--allow-raw`, off by default.  The daemon still classifies the prefix
where it can and logs every raw command.

Requests are serviced between commands; worst-case control latency is
one status screen read, roughly a second.  At most `REQUESTS_PER_POLL`
commands run before the schedule's turn, so neither clients nor polls
starve the other.  A command past its caller's deadline is answered,
not sent, so a holdover reported failed does not begin seconds later.

- **Force-refresh after control.**  After a successful control
  operation the `DeviceTask` re-polls the affected tier;
  `:SYNC:HOLD:INIT` triggers a fast-tier and holdover refresh.
- **Audit trail beside the telemetry.**  Every non-scheduled command is
  recorded: timestamp, text, response, classification, and an optional,
  untrusted client label, so one database answers "what did I do to
  it, and what did EFC do afterwards".

### Disconnection is a first-class state

USB serial adapters drop and receivers are power-cycled; the daemon
reconnects rather than exiting, and records the gap.

Freshness is per tier: each group of fields carries when it last
succeeded and its own error, over the wire and into the log, and each
pane shows the age of what it displays.  A snapshot is as fresh as its
least fresh part, so the first minute after startup, with no sky,
position or date yet, reads `Stale`.

History graphs select on `fast_at = at`: whether the fast tier
measured this row or the row restates the last one.  A medium or slow
column in a fast row counts while its tier's timestamp is within three
of its intervals of the row and is null past that, so a failing tier's
line breaks rather than running flat.  A window, not one row per read,
because at an hour's zoom a ten-second tier would leave most buckets
empty; a slow column's mean is therefore time-weighted.  The daemon
records its cadence in `meta` (absent: the default).  The monitor's
live panes and the exporter use the same window
(`Cadence::current_window`) with the cadence from `info`: a pane past
it shows its age, and the exporter omits its values.

Each opening of the receiver is an attachment (`AttachmentId`), which
starts from an empty snapshot.  A command that reaches a later
attachment than it was sent under is answered `Reattached` and not
sent, since it was meant and audited for the earlier unit.  Readings
carry their attachment; on a change the monitor ends its session and
re-fetches log path, policy and cadence, as after a daemon restart.  A
failed poll republishes the last snapshot unchanged, so clients see it
go stale, and the log writes that moment once, so the outage is a gap.

Use a `/dev/serial/by-id/...` path; `/dev/ttyUSB0` is not stable across
re-enumeration.  The device path has no default, since a wrong one
sends SCPI at whatever is there; the daemon and CLI refuse to run
without one, and the packaged configuration ships it commented out.

### The receiver's own records

**The error queue is drained and written down.**  `:SYSTem:ERRor?`
removes the entry it returns, and unread entries are eventually
discarded.  A failed command can read past its own error to an earlier
one; the task keeps such strays with the identity of the receiver that
raised them, and the daemon journals them before each pass, reporting,
not filing, any from a receiver no longer attached.  A full queue
replaces its last entry with -350 and discards the newest errors
(097-59551-02 5-31); a command that then fails reports its error as
lost and the marker is kept as a stray.  The journal records -350 and
notes that an unknown number of errors went with it.

**The diagnostic log is copied out entry by entry.**
`:DIAG:LOG:READ:ALL?` returns about 56 KB, half a minute of wire time,
so the copy uses `:DIAG:LOG:READ? <n>`, sixteen entries a pass, new
ones first, then backwards.  Entries are stored by content, not number,
because clearing the log restarts the numbering.

**Condition registers, never event registers.**  The daemon polls the
hardware, operation, holdover and powerup condition registers and
`*STB?`.  Reading an event register clears it, and with it the alarm
condition summary, the front-panel Alarm LED and the BITE output; the
lamp belongs to whoever is at the instrument.  Time Reset is event-only
(097-59551-02 5-39).  `*STB?` returns the alarm condition register,
whose bits "are updated in real time -- there is no latching or
buffering", and "Reading/Querying the Alarm Condition Register does not
change its contents" (5-44).  It names the group, not the bit; for the
questionable group that is exact, since it holds only Time Reset and
the user-reported bit, which nothing here sets.

So the logger never writes to the receiver, `*CLS` included; the
operator clears the alarm at the panel.  Reading the bit behind a
summary is acknowledging it (open question 4).  A socket client needs
`--allow-control` to read an event register or `*ESR?`; the value is
returned, and the class records that taking it acts on the receiver.

The transition filters are recorded beside the events, which mean
nothing without them.  This 58503A answers positive 127 / 2 / 5087 / 15
/ 7 with every negative filter at zero, so faults latch appearing and
never clearing: a missing clear-event is not evidence a fault
persisted.  The filters are non-volatile and the only documented reset
is `:SYSTem:PRESet`, so they are read, never written.

This runs on the thread that owns the database, reaching the receiver
through the request queue, because reading removes an entry and a
channel between read and write is a gap a shutdown can lose it in.  It
runs per connection: the unit may have been power cycled, swapped or
reconfigured, so no state carries over.  The journal resets when the
daemon's connection count moves, at the cost of one database query for
the log span and one read of the filters.

The receiver's log is erased when the copy here is complete and
gap-free and the log-almost-full bit is set; clearing unsets the bit,
so a flapping link cannot erase twice.  It is gated behind
`--adopt-log`, since erasing is irreversible, and the entry count goes
with the command so the receiver refuses with -222 if an entry arrived
between copy and clear.

Receiver log stamps are kept as written but do not order entries: after
a power cycle the clock runs from a stale midnight until first lock.
`at`, host UTC when read, is the best time an entry has.
`(generation, entry)` orders the log; `generation` counts clears.  Once
per connection three held entries are re-read, and any difference
starts a new generation, so a log cleared and refilled offline is not
mistaken for the old one.

### Rows belong to a receiver, not to a file

Units are swapped on a bench, and two oscillators' history in one file
reads as one oscillator with a step.  So a `receiver` table has one row
per unit and every per-unit table a `receiver_id`, keyed on the serial
alone; keying on all of `*IDN?` would make a firmware upgrade a new
receiver.  Firmware is recorded as last seen.

A snapshot is filed under the receiver it was read from, not the one
attached when it is written, since the old unit's snapshots are still
queued at a swap.  Journal rows go under the receiver attached at the
time.  Rows from before this keep a NULL id; backfilling would put one
unit's history under another's name.

The slow tier asks `*IDN?` first, and a different model or serial (not
firmware) stops the task, so the daemon reopens under the new serial
even when the link stays up.  Readings between the swap and that check,
at most one slow interval (a minute by default), are filed under the
old unit, so stop the daemon before moving a cable.  Asking more often
would cost a query on a faster tier.  The journal does not wait for
that check: it asks `*IDN?` at the start of every pass and again before
erasing the receiver's log, and skips on a different unit, so the new
unit's log is neither filed under the old one nor erased on the old
one's progress.

### Storage: SQLite

Chosen over JSONL because trending means range queries over time.
`rusqlite` with bundled SQLite, in `StateDirectory=smartclockd`
(`/var/lib/smartclockd`).  Retention is unbounded; the timestamp index
keeps it queryable.  JSONL is kept for raw wire transcripts:
append-only, greppable, reusable as parser fixtures.

Timestamps are compared as text, so the index on `at` serves every
range bound and `ORDER BY at`.  Schema 8 stores nine fractional digits,
because the default form trims trailing zeros and `00.11Z`, `00.1Z`,
`00Z` sort in reverse; opening an older database rewrites existing
stamps in one transaction.  Range bounds use the stored form: `T` sorts
after a space, so a bound from `datetime('now')` excludes nothing.

### Poll scheduling is a link budget

The status screen is roughly 24 lines of 76 columns, about 1.8 KB and
0.95 s at 19200 8N1, so it cannot be polled at 1 Hz beside anything
else.  19200 is the ceiling.  `097-59551-02` 5-101 lists four rates up to
19200, and the 58503A answers `:SYSTem:COMMunicate:SERial1:BAUD 38400`
with `+0,"No error"` while keeping 19200.  `BaudRate` still knows 38400
and 115200, so a receiver another tool moved there can be moved back.

Each tier is a list of steps (`device::step_count`); the scheduler
takes one step per turn and stamps a tier fresh when its pass
completes.  The fast tier is one step, because its fields are compared
against each other, and a time interval from one second beside an EFC
from the next is a correlation nobody measured.  After each step short
of the last, the tier is re-queued at the present instant, so it yields
to any tier come due rather than winning every turn; unconditional
fast-tier priority would starve the slow tier under refreshes.  A
refresh moves deadlines, not step cursors, or under a stream of
refreshes no tier would complete.

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
The medium tier, four steps totaling 0.57 s, runs every ten seconds.

So the screen is on no tier; only the sky plot is unique to it.
`:GPS:SATellite:TRACking:COUNt?` equals its `Tracking`, and
`:GPS:SATellite:VISible:PREDicted:COUNt?` less that equals its `Not
Tracking` (verified against a screen showing 7 and 2); the health line
is the hardware condition register; the bracketed synchronization text
is `:SYNChronization:STATe?` and the operation register.  The command
tree has no per-satellite node, so elevation, azimuth and signal
strength are screen-only.

The screen is read on request -- `Op::Status` (op `status`), the
monitor's status view, `/status` in the browser -- and every `--sky`
seconds (300 by default; 0 turns it off), so the satellite table is
logged without a viewer, at half a percent of the link.  `Screen` keeps
the text as well as what was parsed.  A read goes to whoever asked and
to subscribers on one snapshot, so the log records that sky once.  It
is never stored as the latest snapshot, which every poll starts from,
or it would be copied into every later snapshot and outlive a reconnect
to a different receiver; the exporter's satellite counts come from the
medium tier for the same reason.

Measured with the daemon running: 234 of 288 consecutive snapshots
were 1.00 s apart, the rest being medium and slow steps publishing in
between, and the only gaps over 1.02 s were the three around a sky
read.

| Tier  | Contents                                                    |
| ----- | ----------------------------------------------------------- |
| ~1 s  | `:SYNC:TINT?`, `:SYNC:TFOM?`, `:SYNC:FFOM?`, `:DIAG:ROSC:EFC:REL?`, `:STAT:OPER:HARD:COND?`, `:SYNC:STATE?`, `:PTIM:TIME?` |
| ~10 s | satellite counts, oven temperature and current, the EFC DAC, `*STB?` and the operation and holdover condition registers, holdover duration and uncertainty |
| ~60 s | position, date, diagnostic log count, the oscillator-current constant (`TCOefficient`), the powerup condition register |
|       | `:SYST:STAT?` (satellite table, health line) is on no tier; it is read on request by the status view. |
|       | The receiver's UTC is on the fast tier, not with the date: a clock read once a minute is wrong for the other fifty-nine seconds. |
| ~10 s | the error queue and any new diagnostic log entries, off the schedule; see "The receiver's own records" |

### Allan deviation is computed over segments, not over a series

`:SYNChronization:TINTerval?` is a phase reading: "the time difference
between the 1-pps signal from the GPS engine to a similar signal
derived from the reference source" (`smartclock-dec96a9`, Enhanced
Learning).  The GPS 1 PPS is quantized to its own crystal and the
locked OCXO is steered to follow it, so the curve is of the pair and
their loop, not of the OCXO alone.

The estimator is the overlapping one.  Gaps are handled by counting
only the second differences that exist, unbiased while what is missing
is unrelated to what was measured (a daemon restart, not a misbehaving
receiver).

- **No second difference across a discontinuity.**  A relock,
  holdover, power cycle or receiver swap may step the phase.  The run
  is cut into segments on the recorded mode and holdover flag, and on
  any absence over ten reading intervals (of the readings, not of a
  coarsened grid, so a gap that cuts a short range cuts a long one).
  Segments are pooled.  State is checked on every logged row, including
  held repeats and rows without an interval, where a short holdover can
  lie wholly.
- **A hole stays a hole.**  Readings go on a time-indexed grid, so a
  missing reading is an empty slot, not a phase step.
- **A grid point with no reading near it stays empty.**  Nearest wins
  within half a *reading* interval, not half a grid step; otherwise the
  last point of a run is filled from a reading up to half a step away.
- **The spacing must beat one observed gap.**  The grid places a
  reading at `round((t - t0) / tau0)`, so an error in `tau0`
  accumulates; a median nominal second of 0.999992636 drops readings
  periodically within a day.  The median only assigns whole-number
  positions; the spacing is the least-squares slope of time against
  position (error falling as `n^-3/2`), fitted per run between absences
  about that run's means, since a restarted daemon polls on a new
  phase: one line through two runs of two hundred readings half a
  second apart was bent by four parts in ten thousand.

`crates/smartclock/tests/adev_reference.rs` checks a gapless record:
the 1000-point data set of NIST SP 1065 section 12.4 gives Table 31's
overlapping Allan deviation at tau 1, 10 and 100 to the seven figures
published, and allantools 2024.06's `oadev` on a thousand-reading
record (regeneration script beside the test) agrees to one part in 10^9
at every tau from 1 to 200 s, with the same count of differences.

The web view's query keeps a row only where interval or state changed,
reads at most the newest 500 000 such rows (about two months), and says
when a range held more.  A curve carries its count of readings, holes
and segments, since a mostly-holes run looks like a clean day.  Points
below ten second differences are not emitted; the sweep stops at a
third of the longest segment.  Long ranges are subsampled onto a
coarser grid, not averaged (averaging is the estimator's job), at the
cost of short taus; the caller picks.

**The reading is already an average.**  `docs/firmware.md` shows
`:SYNChronization:TINTerval?` is the mean of ten one-second readings,
updated every ten seconds.  For white PM the Allan variance is 3σx²/τ²
at every tau, so that averaging lowers ADEV by √10 wherever white PM
dominates.  MDEV over n consecutive 10 s means is the window mean over
10n one-second readings, so MDEV of the record equals MDEV of the
one-second phase at every tau of 10 s and above, with fewer overlapping
estimates.  TDEV is tau times MDEV over root three.  So MDEV and TDEV
are the primary curves, from the same gridded segments (a hole voids
every window spanning it, `3m` triples rather than three); ADEV is
kept, labeled, because data sheets quote it.  Both are checked against
allantools' `mdev` and `tdev` and Table 31's modified Allan and time
deviation columns.

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
probability" and 1 PPS edge jitter as "<750 ps rms" (the divided-down
OCXO, not the interval to GPS).  The engine's pulse carries an
uncorrected sawtooth the Oncore reports as "-128 .. 127 ns"
(`VPCommands.pdf`, @@Bn/@@En); no path from that report into the
interval was found in the firmware.  A locked bench 58503A read -62 to +81 ns
over six hours, with reading-to-reading jumps up to 94 ns, within the
110 ns; so an MTIE of ~100 ns at short tau meets the specification,
and that jitter is the deviations' short-tau floor.

TDEV has its own chart under the sigma-y chart, sharing tau axis and
cursor, because it is in seconds and its slope is MDEV's plus one
(white PM tau^-1/2, flicker PM flat, white FM tau^1/2, flicker FM tau,
random-walk FM tau^3/2); on a shared plot TDEV rising while MDEV falls
reads as a contradiction.  TDEV says how far the 1 PPS wanders over
tau.

MTIE is on that chart too, computed, not inferred.  For Gaussian noise
it is a few times the rms, but it is set by the largest excursion -- a
sawtooth step, a relock, a holdover hop -- which deviations average
away, so a bound from TDEV can be an order of magnitude short.  It is a sliding
maximum and minimum over `m + 1` consecutive readings (SP 1065 section
5.2.9) through monotone deques, skipping windows spanning a hole or
cut.  Checked against allantools' `mtie`.

Each deviation carries a one-sigma confidence interval, drawn as a band
and tabulated as two asymmetric bounds.  One sigma because stability
plots use it, and a 95 % band on a log axis swallows the curve at long
tau.  The interval is SP 1065 equation 45 with Greenhall's equivalent
degrees of freedom (section 5.4.1, Greenhall and Riley 2003) for the
noise type the lag 1 autocorrelation method identifies at each tau
(sections 5.5.5 and 5.5.6, quadratic detrend as in allantools).  A tau
with too few decimated readings to identify noise takes the previous
tau's (section 5.3.2); a record too short for the edf algorithm (white
PM under three windows) gets no interval, as does MTIE.  Degrees of
freedom pool across segments.  The chi-squared quantile is Wilson and
Hilferty's, within 0.4 % of scipy's from two degrees of freedom up and
2.6 % at one and a half.  Tested against allantools'
`autocorr_noise_id`, `edf_greenhall` and `confidence_interval` on the
reference record, and edf and noise identification on every power law
from white PM to random walk FM, at lengths reaching each branch and
refusal of Greenhall's algorithm.  The TUI draws no band: ratatui has
no fill.

`:DIAGnostic:PTIMe:TINTerval?`, the latest one-second reading, would
extend the curves below 10 s and is not polled: it costs a query a
second and ten times the phase rows, for the receiver-noise floor
alone.  Plain `:PTIMe:TINTerval?` is the same mean as
`:SYNChronization:TINTerval?`, by the image's handler table and by 187
paired polls of the bench 58503A.

### One log per receiver, named by its serial

Several receivers on a host means one daemon per port,
`smartclockd@<name>`, each with its own device and socket, so nothing
in a daemon becomes concurrent.  The template is the only daemon unit,
even on a one-port host.  The instance is named by the port, as
`serial-getty@` is, and the unit derives device and socket from it;
anything else per port goes in a drop-in.  Shared settings are in
`/etc/default/smartclockd`.

The daemon and monitor have no socket default, which would guess an
instance name.  `smartclock-web` and the exporter are host-wide: they
scan `/run/smartclockd/*/socket` as the web view scans the log
directory, unless given a single socket.  The web's live strip follows
the selected receiver to its daemon; the exporter labels every sample
with the daemon instance and the receiver's serial and model.

The receiver, not the port, chooses the log: after `*IDN?` the daemon
opens `/var/lib/smartclockd/<model>-<serial>.sqlite`, and nothing
before.  A swap switches files, a unit moved to another port keeps one
history, and two instances cannot collide on a file.  A receiver whose
identity does not parse gets a file named after the device and a loud
log line, not a shared "unknown" file.  `receiver_id` and the
`receiver` table still record which unit a file's rows came from.

`smartclock-web` lists every `*.sqlite` in the log directory, live or
historical, and keys everything by serial (`?receiver=`).
`/api/receivers` lists each unit with the columns its model's command
table can measure, and the pages show only those.

Stored data is reshaped by hand with `sqlite3`, not by daemon migration
code: there is no fleet, and a migration nobody else runs is code
nobody tests.

## Architecture

Cargo workspace:

- `smartclock` -- library.  Typed errors via `thiserror`, no `anyhow`,
  blocking synchronous API.
- `smartclockd` -- the daemon.  Owns the port, logs, serves the socket.
- `smartclockmon` -- TUI client, `ratatui` + `crossterm`.
- `smartclock-cli` -- one-shot queries, `diagnose`, transcript capture,
  `read-memory`, `flash`.  `--device <path>` talks to the receiver
  directly, with the daemon stopped; `--socket <path>` goes through
  the daemon.
- `smartclock-sim` -- the simulated receiver, in process and over TCP.
- `smartclock-exporter` -- Prometheus metrics from the daemons' sockets.
- `smartclock-web` -- the browser views, over the logs and sockets.
- `smartclock-http` -- the minimal HTTP server the exporter and web
  view share.
- `smartclock-log` -- the log's schema and every query that reads it:
  table definitions, schema version, stored timestamp form and `meta`
  keys.  The daemon creates the tables from them and keeps its own
  writes and migrations; the monitor and web view read through it, so
  front ends cannot drift apart.  Its tests build every log from the
  daemon's definitions, so a table change that forgets a reader fails.

Library layers, bottom up:

1. `transport` -- `trait Transport: Read + Write`, with
   `SerialTransport` (`serialport`), `TcpTransport` (ser2net),
   `ReplayTransport` (fixtures) and `TeeTransport` (transcripts); the
   simulator's `SimTransport` is in `smartclock-sim`.
2. `session` -- command framing: read-until-prompt, echo suppression,
   per-command timeout, error queue drain.  Handles the `scpi> ` and
   `E-nnn> ` prompts, and reads the status screen by the line count
   from `:SYSTem:STATus:LENGth?`, not by timeout.
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

- `Type=exec`: the daemon retries with no receiver attached, so no
  moment honestly counts as ready for `Type=notify`.
- `Restart=on-failure` with a backoff and a start limit, so a bad
  setting lands the unit in `failed` rather than restarting forever.
- No `BindsTo=` / `After=` for the adapter's `.device` unit: the
  daemon reconnects on its own, binding would stop it while an adapter
  is unplugged, and naming a device would mean editing the unit.  A
  drop-in recipe is in `docs/running.md`.
- Every option readable from `SMARTCLOCKD_*`, so operators edit only
  `/etc/default/smartclockd` and drop-ins, and `ExecStart=` names no
  settings.  The unit sets the device and socket from the instance
  name.
- `StateDirectory=smartclockd` for the logs, shared by every instance;
  `RuntimeDirectory=smartclockd/<instance>` for the socket.
- Dedicated user with `SupplementaryGroups=dialout`;
  `RuntimeDirectoryMode` and socket group ownership let the TUI run
  unprivileged.
- Hardening: `ProtectSystem=strict`, `PrivateTmp`, `NoNewPrivileges`.
  `RestrictAddressFamilies=` admits `AF_INET` and `AF_INET6` as well as
  `AF_UNIX`, for the `tcp://host:port` device form.  No
  `PrivateDevices`: it would hide the serial port.

systemd is the only Linux-specific piece; `interprocess`, `serialport`
and `rusqlite` are portable.  Porting means a launchd plist or a
Windows service wrapper, not touching the protocol.

### The command table is data

Commands live in a TOML file from which `build.rs` generates the
dialect code.  Each entry has a stable logical id, a classification, a
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

Tree divergence, per-model availability and citations sit in one table
that can be diffed against the documents.  Codegen makes the logical
ids an enum, so a typo is a compile error.  `response` names a
hand-written parser in `parse`.

An entry may declare its argument, which the daemon checks before
anything reaches the receiver; no block means no argument:

```toml
  [command.argument]
  kind = "integer"   # or "word" with `allowed`, "none", or "free"
  min  = 0
  max  = 90
```

`free` must be asked for by name, and a test pins the entries that ask,
because as a default it let a query header carry the set form's
payload: `:SYSTem:LANGuage? "INSTALL"` reached the simulated receiver
on a daemon started with no flags.

A test checks that every command reachable on the active dialect has a
parser and a fixture.  The per-model command matrix
(`docs/commands.md`) is generated from the same source.

### The package version is derived

`make deb` builds `<version>-<commits>+g<sha>`, with `+dirty` for an
unclean tree, so different code never shares a version; dpkg treats
reinstalling an identical version as a no-op.  The hand-written
`packaging/debian/changelog` records releases; a derived version is a
build.

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
make release-notes  the GitHub release notes the tag will get
```

Pushing a release tag builds the packages, sets the GitHub release's
notes from the newest `packaging/debian/changelog` entry
(`packaging/release-notes.sh`), and rebuilds the apt repository.  The
changelog entry is written once, in `make release`, and is the release
notes everywhere.

CI calls the fleet's reusable `rust-ci.yml`, since one repository doing
it differently costs more than one definition run locally and in CI.
Its inputs pin the toolchain, pin `cargo fmt` to it (nightly rustfmt
formats differently), and pass `--all-targets`, or clippy skips the
tests.

`make ci` is stricter in two ways.  The shared `cargo test` omits
`--workspace`, harmless until a `default-members` silently narrows CI
(`Cargo.toml` says so).  The shared workflow pins actions by tag, not
commit sha: the fleet's posture, not this repository's preference.

`make test-web` runs Playwright tests, installed with Chromium into
`crates/smartclock-web/tests/browser` by `make web-deps`, so nothing
lands in the home directory.  They serve the pages from the real server
with a faked API, to reach what a live daemon cannot show on demand:
which receivers exist, when a daemon goes, how late an answer arrives.
Every test runs against every page, since pages diverge at those edges;
the pages share one lifecycle in `crates/smartclock-web/src/common.js`.
They need Node and a browser, so CI runs them in
`.github/workflows/web.yml` and `make ci` leaves them out.

Tests that need a receiver and a stopped daemon are `#[ignore]`d and
run only through `make test-hw`.  Everything else -- parsers, the
scraper, session framing against `ReplayTransport`, integration tests
against the simulator -- runs in `make test`.

### Dialects

Two branches, not four; `097-59551-02` shows the 58503A tree is
essentially the 58503B's:

| Family                  | Tree                                      |
| ----------------------- | ----------------------------------------- |
| 58503A / 58503B / 59551A| `:GPS:`, `:SYNChronization:`, `:PTIMe:`   |
| Z3801A / Z3816A         | `:PTIME:GPSYSTEM:`, `:ROSCillator:`       |

The Z3805A answers the z3801 dialect.  Availability is per model: the
58503A has `:GPS:SATellite:TRACking:IGNore` / `INCLude`, which the
58503B guide marks 59551A-only.  Unsupported operations return a typed
`Unsupported` error without reaching the device.  Response formats
differ too: `:DIAG:ROSC:EFC:REL?` returns `+-d.dEe` on the 58503A but
is documented as a plain integer on the Z3801A.

Each entry records its `evidence`: `manual`, `firmware` (every keyword
found in the firmware's own keyword table) or `hardware` (a receiver
answered it), so an unconfirmed command is a known risk.

The bench Z3801A (3543-A) differs from the 58503A on the wire in two
more ways.  It does not echo, so a command with no reply returns the
previous prompt's trailing space and the prompt alone, which the prompt
matcher allows.  It stamps its diagnostic log `Log NNN:H%08X: message`,
a hex count, where the 58503A and Z3805A write a date; the `H` notation
is seconds of GPS time since 1980-01-06 (`097-z3801-01` 4-13).  The
stamp is stored as written; the web view shows it decoded, the hex in
the tooltip.  Its `-230 Data corrupt or stale` on `:PTIMe:TINTerval?`
and `:PTIMe:FFOMerit?` while tracking no satellites is the manual's
answer for an unavailable value (4-6, 4-7), taken as a state refusal
like the 58503A's.

### The status screen scraper is mandatory

`:GPS:SATellite:TRACking?` returns PRN numbers only; per-satellite
elevation, azimuth and C/N exist only in the `:SYSTem:STATus?` screen,
so the scraper is a first-class parser.  `097-59551-02` chapter 3 has
five sample screens of distinct states -- survey in progress,
`*nn Acq..` acquiring markers, `nn -- ---` untracked rows, `Predict --`
unavailable, "Locked to GPS: stabilizing frequency" -- and
`097-58503-13` more.  These are scraper goldens.

The scraper reads by label, not column.  An asterisk ends the preceding
cell, and groups carry their heading column so a blank cell differs
from a missing one; otherwise a blank signal cell takes the next
satellite's asterisk, and a short tracked column lets the first group
claim the second's rows.  The tracked count is the `Tracking:` not
preceded by `Not `.

The table is checked against the screen's tracked and untracked
counts.  A disagreeing table is still shown, with the pane saying so,
which makes misreadings visible without predicting their shape.

## EFC diagnosis

An OCXO aged past its EFC DAC's pull range presents as a unit stuck in
holdover; the development unit was suspected of this.  It is not the
oscillator, on this evidence:

- Locked with a valid reference in every sample since the log began
  carrying the condition registers.
- The diagnostic log shows nineteen holdover-and-relock cycles across
  two days in March 2025 and nothing since, which reads as GPS
  reception; an oscillator drifting out of range does not recover
  nineteen times.
- The EFC measurement settles the mapping in favor of the
  specification, leaving about a decade of tuning headroom.
- On 2026-09-26 the crystal was retrimmed with the EFC input grounded;
  the receiver now locks with the pin near 0 V, and the retrim measured
  the pull, 3.94 × 10⁻¹³ per count, 0.63 of the loop's assumed G
  (`docs/efc.md`).

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
GPS or antenna fault.

## Phases

Phases 0 to 12 are done.  Suggest a commit at each phase boundary.

### Notes and receiver facts

Bench events and a unit's internals are invisible to the receiver and
live only in `docs/hardware-investigations.md`, where they cannot be
lined up against the log.  Two additions to each receiver's log,
written through the daemon and never sent to the receiver:

- *Notes.*  Timestamped free text -- "added a 20 dB LNA ahead of the
  Z3805A", "ran `master_reset`" -- for one receiver or the whole bench,
  such as a splitter change.  `smartclock-cli --socket ... note TEXT`,
  with `--at TIME` to backdate.  Shown as markers on the web history
  charts, with the text on hover, in a list beside the journal, and in
  the TUI's journal view.
- *Facts.*  Free-form `key=value` describing the unit:
  `ocxo.serial`, `ocxo.model`, `antenna.feed`, an engine swapped in.
  Each records when it became true, so a replaced part keeps its old
  value in the history.  `smartclock-cli --socket ... fact KEY VALUE`;
  setting one also leaves a note.  Current facts head `diagnose` and
  the web receiver info, beside what the unit reports itself.

Built: the `note` and `fact` tables (schema 11), the socket requests,
the CLI commands, notes in the web and TUI journals and as dashed
lines on the web charts with their text in the cursor readout, and
current facts in `diagnose` and the web view.  `diagnose` reads facts from the log
the daemon names, so it needs the log's group, as the monitor does.  To
do: entering the events in `hardware-investigations.md` at their
recorded times.  A
note is filed under the receiver attached when it is written, even
when `--at` backdates it past a swap.  A bench-wide note is written to
each daemon in turn.

Later: adding, editing and deleting notes from the web view, through
the daemon socket.  Needs POST bodies in `smartclock-http`, and notes
with ids rather than append-only.

### Next: sensors beside the receivers

Up to twelve named hwmon or IIO sensors, read on the medium tier into
sticky snapshot columns beside each receiver's readings;
`docs/SENSORS.md`.

### Comparing receivers

`/compare` shows every receiver over the same range: 1 PPS TI, EFC,
temperature, TFOM and FFOM overlaid, and ADEV and MDEV curves on one
plot, from the existing `/api/history`, `/api/adev` and
`/api/journal`, asked once per receiver and joined in the browser.
The live, status and stability pages stay one receiver each.

## Open questions

1. **Which state machine drives the mode suffixes.**  Still inferred
   from outside (`docs/screen-format-strings.md`, "Mode suffixes").
   The other firmware questions are settled in `docs/firmware.md`:
   `:DIAGnostic:ROSCillator:TCOefficient?` is a stored constant on the
   oscillator current in the loop's EFC, written only by its own setter
   ("s, the oscillator current"), and `RELative?` is (ABS − 2¹⁹) / 2¹⁹
   × 100 with ABS sixteen times a 16-bit DAC word ("The 58503A image").
   The images are `third_party/z3801a-3543.bin`, `z3805a-3543b.bin`,
   `z3816a-4001.bin`, `58503a-3633.bin` (assembled from others' dumps)
   and `58503a-3704.bin` (read from the bench unit through its pForth
   console with `smartclock-cli read-memory`), with EEPROM dumps of the
   bench units.  The 58503A images carry the front panel's strings
   (`10MHZ STABLE` is in both); they have not been enumerated.

2. **How far the z3801 dialect is confirmed.**  Of the z3801 entries,
   18 are `evidence = "hardware"`, 63 `firmware` (spellings
   corroborated against the firmware's keyword table) and one
   `manual`; those 64 stay unconfirmed until a receiver answers them.

3. **Asserting a known antenna position from the configuration.**  A
   receiver told where it is goes straight to position hold and serves
   time; a survey leaves the 1 PPS invalid while it runs.  A Z3805A on
   an antenna a 58503A had already surveyed spent an hour unable to
   serve time; asserting the position by hand took two commands.  So:
   a position setting asserted on attach, as a geodetic position,
   earth-centered x, y and z, or `survey` to ask for one deliberately
   rather than inherit the last operator's setting.

   - It is a control command, so it needs the same opt-in and must not
     fire under a daemon started read-only.
   - It is non-volatile, so a stale value outlives the configuration
     that set it.  Holding a position also means turning off
     survey-on-powerup, or the next power cycle discards it; with both
     set, a receiver whose antenna has moved never notices.  Whatever
     asserts a position must be able to release it.
   - The 58503B reports height above the ellipsoid where the others use
     mean sea level, so a geodetic setting must name its datum.
     Earth-centered coordinates sidestep that.

4. **Whether acknowledging from the monitor is wanted.**  Reading an
   event register says which bit latched rather than which group, and
   clears the alarm: right for a deliberate acknowledgment, wrong on a
   timer.  Nothing needs it yet.

5. **Maybe: a Host allowlist for the web view.**  `smartclock-http`
   checks neither `Host` nor `Origin`, so a page in the operator's
   browser can rebind its own name to the web view's address and read
   `/api/status`, position included, or force screen reads at 1.5 s of
   link each.  Likely shape: `--allow-host NAME`, repeatable, loopback
   always allowed, a non-loopback bind refused without one.  Deferred:
   no access-control work for now.

## Known defects

Known and unfixed, each because the fix is not yet worth its cost.  The
threat model is a careless operator on a single-operator machine, not
an attacker, so two things are not defended against:

- Socket permissions are the whole of the authorization.
- A client that connects and only reads holds one of the sixteen slots
  while connected; a watcher does exactly that, so it is not refused.
  One that stops reading is cut off once a write has waited
  `client::DEADLINE`, and when either half of a connection ends the
  socket is shut both ways (Unix; `interprocess`'s portable stream has
  neither).  A client half-closing its sending side releases its slot
  and stops the push thread, and is tested.

## Backlog

Open findings of the October 2026 whole-codebase review, in the agreed
order, kept here so any machine with the repo has the list.

Decided against: renaming exporter metrics or changing their labels
(`oven_tempco` stays; a stopped daemon's `up 0` carries only its
instance label).

**Web view.**  Host and Origin checks are open question 5.
