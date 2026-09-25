# Plan

Build a Rust library for talking to HP / Symmetricom SmartClock GPS
receivers over serial, a logging daemon that runs as a system service,
and a TUI client.  A GUI is possible later but is not planned.

## Status

Phases 0 to 10 are done; see **Phases** at the end for what each turned
out to involve, **Open questions** for what is undecided and **Known
defects** for what is wrong and unfixed.

192 tests, none needing hardware.  `make ci` is what CI runs; `make
test-hw` is the hardware-only set and CI never runs it.

Installed from the package and running as a service against the
development unit, logging to `/var/lib/smartclockd/snapshots.sqlite`.
`docs/running.md` is the deployment note.  The socket protocol is
documented in `smartclock::protocol`, where it is defined, and gets no
document of its own; the reasoning is under **Decisions**.

Two adversarial reviews in September 2026.  The first -- four Claude
reviewers and one Codex run over the whole tree -- found about forty
defects, five of them serious enough to fix at once.  The second went
over that day's diff and found eight more, several of them in the fixes
themselves: a bound that grew the queue it bounded, an unbounded channel
introduced to stop a silent drop, a freshness rule that called a
snapshot current when two thirds of it had never been read.  Fixing is
where defects come from, which is the argument for reviewing a diff
rather than a tree.  What they had in common is worth
recording: every one produced a **wrong value presented confidently**
rather than an error, and all 106 tests passed throughout, because the
fixtures only covered the layouts that happened to work.  They are
written up under "What the review found" below.

The simulator closes what used to be the largest hole.  `SimTransport`
puts a receiver behind the transport trait in process, so the device,
the polling task and the control handle are all exercised without
hardware; the same receiver listens on TCP so the real daemon and the
real monitor can be driven against it end to end.

## Goals

- Read-only monitoring of a 58503A first; control commands later.
- Diagnose the development unit's suspected EFC / OCXO problem.  Done,
  and the answer was no: see **EFC diagnosis**.
- Unattended long-term logging of EFC and holdover state for drift
  analysis, independent of whether anyone is watching.
- Support the wider SmartClock family, notably the Z3801A.

## Constraints

- **Single-operator machine.**  One person, one box, one receiver.  Held
  as a standing assumption, not a temporary simplification.  It is what
  justifies socket permissions as the whole of authorization, an audit
  trail with no notion of who, and a daemon that serves one device
  rather than a fleet.  Revisit these together if it ever stops being
  true.
- **Bringup targets the 58503A only.**  Other variants get table entries
  from their manuals, but nothing is verified against hardware until the
  58503A works end to end.

  A sick oscillator does not block this.  The SCPI interface works
  regardless of whether the OCXO is healthy, and a railed EFC with a
  stuck holdover is a better exercise of the alarm and error paths than
  a well behaved unit would be.  What would block bringup is a unit that
  does not answer on serial at all; in that case the simulator moves
  ahead of phase 3 and the fixtures carry development until another unit
  is available.

## Decisions

### No client-side SCPI crate

Surveyed: `scpi` + `scpi-contrib` (server side, for implementing an
instrument, no_std), `scpify` (TCP and HiSLIP only, no serial),
`scpi-client` 0.1.1 (thin, immature), `instrument-core` 0.1.0 (right
shape, but v0.1.0 and VISA/GPIB oriented).

None fit.  The receiver is not a VISA-style instrument: it echoes, it
prompts with `scpi> ` / `E-nnn> `, and its richest response is an ASCII
status screen rather than a SCPI response.  Generic clients assume a
clean write / read-to-terminator cycle, so we would spend more effort
defeating those assumptions than the framing costs to write directly.

The genuinely standard parts are shallow and already specified in
`097-59551-02`: `:SYSTem:ERRor?` returns `<code>,"<text>"`, and the
status registers are IEEE 488.2.

Revisit `scpi` at phase 4 for the simulator, where implementing an
instrument is the actual task.  Even there the prompt, echo and status
screen -- the parts worth emulating -- are what it does not model.

### No separate document for the socket protocol

Four programs speak it -- the monitor, the CLI, the exporter and the
browser view -- which was going to be the trigger for writing a
specification.  It is not worth one.

All four are built from this tree, in this language, and ship in one
package, so they cannot drift apart: the wire type is checked by the
compiler, which is a stronger specification than prose and cannot go
stale.  `smartclock::wire::Reading` is deliberately not `Snapshot` for
exactly this reason, so an internal rename is a compile error rather
than a silent protocol change.  What prose would add is a second
description of the same thing, to be forgotten at the first rename.

The parts a type cannot express are written where they belong, in
`smartclock::protocol`: newline-delimited JSON over a local socket, a
multiplexed stream where snapshots arrive unsolicited so every request
carries an id its reply echoes, and a `VERSION` the daemon refuses a
mismatch on.

This holds as long as the socket is internal.  A client outside this
tree -- another language, another repository, anyone else's -- makes it
an interface rather than an implementation detail, and then it needs
writing down and a stability commitment to go with it.

### The daemon owns the port

`smartclockd` runs as a systemd system service and holds the serial
port open for as long as it runs.  Nothing else can open it,
so every other component is a client of the daemon.  This follows from
wanting multi-day EFC history on a unit that might have been dying:
collection cannot depend on a TUI being up.  The unit turned out to
look healthy, which the history is what established -- so the reasoning
held even though its premise did not.

Within the daemon, a single `DeviceTask` thread owns the `Session`, and
therefore the fd.  It is the only thing that ever issues a command.
The framing forces this -- a half-read prompt from an interleaved
command desynchronizes the session for every later read.  `Session` is
therefore not `Sync` and is moved into the device thread at
construction, so a second writer is a compile error rather than runtime
corruption.

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

### The simulator is not PTY-backed

The plan called for a PTY.  That would have made the simulator
Unix-only, which undoes the reason `interprocess` was chosen over raw
AF_UNIX: systemd is meant to be the one Linux-specific piece.

Two things were wanted from it, and they separate cleanly.  Tests need a
receiver the library can talk to, which is an in-process `Transport`
implementation -- pure Rust, no operating system involved, portable
everywhere.  Driving the real daemon binary needs something reachable by
a device path, and a TCP listener does that on any platform.

The second half also pays for itself: it is the same `TcpTransport` the
plan already wanted for ser2net, so a network-attached serial adapter
works as a consequence rather than as extra work.

### The protocol stays JSON

Considered protobuf and gRPC and decided against both.

gRPC in Rust means `tonic`, which does not do named pipes, so it would
land on loopback TCP -- and that reopens the authentication question
that socket permissions answer for free.  On a single-operator machine
that is a straight regression.

Protobuf without gRPC, over the same socket, avoids that.  It buys a
schema and compactness, and costs a codegen step and the ability to
watch the socket with `socat`, for a message sent once a second between
two ends we control.  Not worth it yet.

Revisit if a client appears that is not written in Rust.

There is a real problem underneath that protobuf would not have fixed
and a wire type does: `Snapshot` was serialised straight from the
internal struct, so renaming a field silently changed the wire format.
The socket now carries its own type, converted from the internal one, so
the two can move independently and a rename is a compile error rather
than a client's problem.

### Two channels to the daemon

- **Local socket, via the `interprocess` crate** for the live snapshot
  stream and for command submission.  One API over AF_UNIX on Unix and
  named pipes on Windows, so the daemon is not pinned to Linux by its
  IPC.  Newline-delimited JSON, so it is debuggable with `socat` and
  does not bind clients to Rust.  A client subscribes on connect and
  receives the current snapshot followed by updates.

  Use a filesystem-path socket name rather than an abstract or
  namespaced one, so systemd's `RuntimeDirectory` owns its lifetime and
  ordinary file permissions gate access.
- **SQLite file, opened read-only** for history and trend charts.
  `journal_mode=WAL` lets readers run concurrently with the daemon's
  single writer.

Keeping live data on the socket rather than polling the database means
the TUI's live pane does not hammer SQLite at 1 Hz, and reconnect after
a daemon restart is trivial.

Both the socket protocol and the SQLite schema carry a version field,
since daemon and clients will be upgraded independently.

### Commands go through the daemon too

The library's `Control` handle is the same type whether the daemon calls
it locally or the CLI calls it over the socket, so the socket protocol
mirrors the library API rather than being a second design.

The socket is multiplexed -- snapshots stream unsolicited while replies
interleave -- so each request carries an id that the reply echoes:

```
-> {"v":1,"id":"7f3a","op":{"kind":"query","cmd":"holdover_waiting"}}
<- {"v":1,"id":"7f3a","ok":{"holdover_waiting":"LIMit"}}
<- {"v":1,"event":"snapshot","ts":"...","efc_pct":-94.2,...}
```

A reply reports what the device said, not that the operation finished.
A survey takes hours but acks in milliseconds; progress is observed
through later snapshots, so clients never block on long operations.

Commands are classified in the dialect table, as data rather than
scattered conditionals:

| Class     | Examples                                              | Gate                     |
| --------- | ----------------------------------------------------- | ------------------------ |
| Query     | `:GPS:POSition?`, `:SYNC:TINT?`                        | none                     |
| Control   | holdover initiate and recover, survey, antenna delay, elevation mask; reading an event register or `*ESR?`, which clears it; reading `:SYSTem:ERRor?`, which removes the entry | `--allow-control` |
| Dangerous | `:SYSTem:PRESet`, `:SYSTem:COMMunicate:SERial1:*`, `:DIAGnostic:ERASe`, `:SYSTem:LANGuage "INSTALL"` | `--allow-dangerous` |

Dangerous commands can strand the link or wipe configuration, since a
baud change persists across power cycles.  The gate is a daemon flag
rather than anything a client can present.  An earlier draft had the
client echo a nonce the daemon issued; a flag is better ceremony,
because enabling it is a deliberate act outside the client and it
survives no amount of fat-fingering at the socket.  A daemon started
without the flag cannot be talked into the command at all.

Commands are refused before classification if they could carry a second
one: a `;`, a control character or any non-ASCII.  SCPI chains program
message units with `;` and the transport appends only a terminator, so
without that check a permitted header smuggled anything after it --
`:SYSTem:STATus? ;:SYSTem:COMMunicate:SERial1:BAUD 1200` classified as
a query and ran on a daemon with no flags at all.  Arguments are
whitelisted rather than filtered, since every value this receiver takes
is a number, a word, a list or a quoted string.

Argument *ranges* belong in the command table beside the class, so the
string path validates what the typed `Control` handle already does.
`Control` stays as the library's typed API for embedders; it is not the
safety boundary, and its documentation should not claim to be.  The
boundary is the table, which is where the gate lives.

Authorization is socket permissions and nothing else.  systemd's
`RuntimeDirectory` and `RuntimeDirectoryMode`, plus group ownership,
decide who can open the socket; anyone who can open it may issue
whatever the daemon's configuration allows.  No peer credentials, no
tokens.

This suits a single-operator machine and keeps the daemon free of an
authentication layer it would otherwise have to carry on every platform.
The cost is that the daemon cannot tell two connected clients apart, so
the audit trail records what was done but not by whom.

Nothing guards against a fat-finger at the socket beyond the flags, and
nothing needs to: a daemon started without `--allow-dangerous` will not
run the command however it is asked.  Restarting it with the flag is the
deliberate act, and the audit trail records what followed.

The TUI's raw SCPI console deliberately bypasses the dialect table, so
it is its own flag, `--allow-raw`, off by default.  The daemon still
classifies the command prefix where it recognizes it, and logs every raw
command unconditionally.

Requests are serviced between commands, never mid-command, because a
partially read response desynchronizes the prompt framing.  Worst-case
control latency is therefore about one status screen read, roughly one
second.

Requests and polls take turns, and the turn is bounded in both
directions, which took two goes to get right.  Polls first had absolute
priority, so a tier as slow as its own period was always overdue and no
client command was ever served; the fix drained the whole queue before
each poll, which inverted it -- a handful of clients each keeping one
request outstanding published no snapshots at all, logged nothing, and
left the last one labelled `Live`.  At most `REQUESTS_PER_POLL` commands
are now served before the schedule gets its turn.

A command also carries the deadline of the caller that sent it.  A
request that times out client-side used to stay in the queue and run
whenever the task reached it, so a holdover the operator had been told
had failed began seconds later and the audit trail recorded it as a
failure.  Past its deadline a command is answered, not sent.

Two consequences worth building in:

- **Force-refresh after control.**  A state-changing command should not
  wait up to 60 s to appear in snapshots.  After a successful control
  operation the `DeviceTask` immediately re-polls the affected tier;
  `:SYNC:HOLD:INIT` triggers a fast-tier and holdover refresh.
- **Audit trail beside the telemetry.**  Every non-scheduled command is
  recorded: timestamp, command text, response, classification, and an
  optional client-supplied label, which is untrusted and best-effort.
  Being able to ask "what did I do to it, and what did EFC do
  afterwards" against a single database is the strongest argument for
  routing commands through the daemon rather than letting clients open
  the port.

### Disconnection is a first-class state

USB serial adapters drop, and the receiver may be power-cycled.  The
daemon reconnects rather than exiting, and records the gap.  `Snapshot`
carries freshness **per tier**, not per snapshot: when each group of
fields was last read and what went wrong with it.  One flag for the
whole reading was not enough, because a snapshot is built up a tier at
a time and the fastest one kept relabelling the others as current.  For
a monitoring tool aimed at a suspect unit, silently displaying old
numbers is the worst failure mode, and a single timestamp made that the
default behaviour rather than an edge case.

Use a `/dev/serial/by-id/...` path rather than `/dev/ttyUSB0`, which is
not stable across re-enumeration.

The device path has no default and both the daemon and the CLI refuse to
run without one.  A default is worse than a missing argument here: it
does not fail when it is wrong, it opens whatever else is on that path
and starts sending SCPI at it, which is the one thing this project's
rules are written to prevent.  The packaged configuration ships the
setting commented out, so a fresh install fails to start and says why.

### The receiver's own records

Three things the receiver keeps that the telemetry does not cover, all
added after asking what could be recorded and was not.

**The error queue is drained and written down.**  `:SYSTem:ERRor?`
*removes* the entry it returns, so the queue is not a view of anything:
whatever is not read is eventually discarded to make room.  The session
already read one entry after each failed command, to turn an error
prompt into a typed error, but that entry was used to explain the
failure and then dropped, and nothing at all read an error the receiver
raised on its own.  Those are the ones worth having.

A failed command can also read past its own error to one the receiver
raised earlier, which then exists nowhere but in the session.  The task
keeps such strays, each with the identity of the receiver that raised
it, and the daemon journals them before each pass; one from a receiver
no longer attached is reported and not filed under the unit that is.

A full queue replaces its last entry with -350 and discards the newest
errors (097-59551-02 5-31).  A command that fails then finds only the
marker, which describes the queue and not the command, so the command
reports its error as lost and the marker is kept as a stray.  The
journal records -350 as the receiver gave it and says that an unknown
number of errors went with it.

**The diagnostic log is copied out entry by entry.**  The count was
already polled, which recorded that seven things had happened and never
what.  `:DIAG:LOG:READ:ALL?` returns the lot in one reply, which for a
full log is about 56 KB and half a minute of wire time -- too much to do
between two snapshots -- so the copy is `:DIAG:LOG:READ? <n>`, sixteen
entries a pass, new ones first and then backwards through the history
that was already there.  Entries are stored by content rather than by
number, because clearing the log restarts the numbering and the same
number then means a different entry.

**Condition registers, and never the event registers.**  This went
back and forth twice and the third answer is the right one.

The hardware condition register was the only one read.  The operation,
holdover and powerup condition registers joined it, which is free of
side effects.  The event registers were then added too, on the grounds
that Time Reset -- the receiver stepping its own clock after a long
holdover, which invalidates every interval measurement across the step
-- is event-only and appears nowhere else.  097-59551-02 5-39.

That was wrong, because reading an event register clears it, clearing
the events clears the alarm condition register that summarises them,
and that puts out the front-panel Alarm LED and sets the BITE output
inactive.  At a ten second cadence the lamp would never be seen lit.
The lamp belongs to whoever is at the instrument.

What resolves it is that there *is* a non-destructive read, which the
first pass missed: `*STB?` returns the alarm condition register, and
5-44 says its bits "are updated in real time -- there is no latching or
buffering" and "Reading/Querying the Alarm Condition Register does not
change its contents".  It reports which groups have something latched
without taking the latch.  So the daemon polls that with the other
condition registers and reads no event register at all.

The cost is granularity: it names the group, not the bit.  For the
questionable group there is no such cost -- it holds Time Reset and the
user-reported bit, and nothing here sets the latter, so the group
summary names the bit exactly.  The thing the detour was for turns out
to be available without the detour.

Two consequences.  `*CLS` is not sent, and neither is anything else
that writes to the receiver: a logger stays read-only, and the operator
clears the alarm at the panel.  And the detail behind a summary -- which
bit of the operation group latched -- is available whenever somebody
wants it, by reading the event register, which is the same act as
acknowledging.  That is worth building when there is a reason to
acknowledge from the monitor; it is not worth doing behind the
operator's back on a timer.

For the same reason a client of the socket cannot read an event
register, or `*ESR?`, unless the daemon was started with
`--allow-control`.  They were classed as queries at first, which let
any client put the lamp out with no flag at all.  Being reads, they
still return their value; the class records that taking it is an act
on the receiver.

The transition filters are recorded beside the events, because the
events are uninterpretable without them.  A filter decides which
transitions latch; this 58503A answers positive 127 / 2 / 5087 / 15 / 7
with every negative filter at zero, so faults latch appearing and never
clearing.  A reader who did not know that would take a missing
clear-event as evidence a fault persisted.  The filters are
non-volatile and the only documented reset is `:SYSTem:PRESet`, which
is on the never-send list, so they are read and never written: a
logger should stay read-only on configuration.

All of this runs on the thread that owns the database, reaching the
receiver through the ordinary request queue, rather than in the poll
schedule.  It is the only arrangement with no window in which an entry
has been taken from the receiver and not yet written down: reading is
what removes it, and a channel between the read and the write is a gap
a shutdown can lose it in.

And it runs **per connection**, not per receiver.  A connection is the
boundary across which nothing is known -- the unit may have been power
cycled, swapped, or reconfigured by somebody else while the link was
down -- so carrying state across one assumes continuity over a gap
nobody observed.  That assumption is exactly what let a copied-log span
survive a receiver swap and judge two hundred of the new unit's entries
already copied.  The daemon counts connections and the journal resets
when the count moves; re-deriving costs one database query for the log
span and one read of the filters.

Erasing the receiver's log is the one thing that is neither per
connection nor per receiver but **condition-triggered**: it happens
when the copy here is complete and gap-free and the receiver's own
log-almost-full bit is set.  Clearing unsets that bit, so it cannot
happen twice and needs no state remembering that it already did -- a
flapping link cannot erase repeatedly.  It is gated behind
`--adopt-log` regardless, because erasing is irreversible and
non-volatile, and the entry count is passed with the command so the
receiver refuses with -222 if an entry arrived between the copy and the
clear.

### Rows belong to a receiver, not to a file

The log recorded no trace of its own subject: the identity was printed
to the journal at startup and kept nowhere that travelled with the data,
so a file handed to someone else did not say what it was a log of.  The
first fix was a metadata key, which was not a fix.  It answered "which
receiver wrote here most recently" when the question is "which receiver
wrote *this row*", and on a bench where units are swapped those differ.
Two oscillators' history in one file, indistinguishable, reads as one
oscillator with a step in it.

So: a `receiver` table, one row per unit, and a `receiver_id` on every
table that is per-unit.  Keyed on the serial alone.  Keying on the whole
of `*IDN?` -- the first attempt -- reports a firmware upgrade as a
different receiver, which is a false alarm on the single most likely
event.  Firmware is recorded as last seen, so the row describes what is
on the unit now rather than what was on it first.

A snapshot carries the identity of the receiver it was read from, and
is filed under that, not under the unit attached when it is written:
snapshots wait in a queue, and at a swap the ones read from the old
unit are still there after the new one is attached.  The journal's rows
are read from the receiver attached at the time and are filed under
it.

Rows written before any of this existed keep a NULL id.  Backfilling
them with the receiver attached today would put one unit's history under
another's name, which is the failure this exists to prevent.

### Storage: SQLite

Chosen over JSONL because EFC and holdover trending means range queries
over time, and JSONL forces a full rescan per chart.  `rusqlite` with
bundled SQLite, in `StateDirectory=smartclockd` (`/var/lib/smartclockd`).
Retention is unbounded for now; index on timestamp so it stays queryable
as it grows.

JSONL is retained for raw wire transcripts: append-only, greppable, and
directly reusable as parser fixtures.

### Poll scheduling is a link budget, not a preference

The status screen is roughly 24 lines of 76 columns, about 1.8 KB.  At
19200 8N1 that is about 0.95 s of wire time.  The status screen cannot
be polled at 1 Hz alongside anything else.

19200 is the ceiling, so the budget cannot be bought out with a faster
link.  `097-59551-02` 5-101 lists four rates up to 19200, and the
58503A answers `:SYSTem:COMMunicate:SERial1:BAUD 38400` with
`+0,"No error"` while keeping 19200 -- it neither applies the rate nor
reports refusing it.  `BaudRate` still knows 38400 and 115200, because
a receiver some other tool has moved to one has to be reachable to be
moved back.

The medium tier is therefore interleaved rather than run as one pass.
Each tier is a list of steps (`device::step_count` gives how many), the
scheduler takes one step per turn, and a tier's freshness is stamped
when the pass completes rather than when it starts.  The fast tier is
deliberately one step, because its fields are compared against each
other and a time interval from one second beside an EFC from the next
is a correlation nobody measured.

After each step short of the last, the tier is re-queued at the present
instant.  The scheduler takes the earliest deadline, and a tier left at
the deadline its pass started from has the earliest there is, so it won
every turn and ran its steps back to back -- steps in name only.
Re-queued, it yields to any tier that has come due since, and continues
when none has.  Giving the fast tier unconditional priority instead
would starve the slow one under a stream of refreshes, each of which
makes the fast tier due again.

Measured on a 58503A at 19200, as the marginal cost over a 0.67 s
open-and-synchronise, the medium tier is four steps of which one is
almost all of it:

| step                                    | cost   |
| --------------------------------------- | ------ |
| satellite counts                        | 0.16 s |
| oscillator temperature, current, DAC    | 0.13 s |
| condition registers                     | 0.12 s |
| holdover duration, predicted, present   | 0.16 s |
| status screen                           | 1.50 s |

A whole fast pass, all eight of its queries, is 0.38 s.  Interleaving
took the worst stall from 1.9 s to 1.5 s and could go no further,
because the screen is 1574 bytes: 0.94 s of wire time and another
0.5 s of the receiver composing it.  Nothing helps with a single read
that long except not making it.

The satellite counts do not come from the screen.  The command tree
extracted from the firmware has no per-satellite node, so elevation,
azimuth and signal strength are screen-only -- but the counts are
queryable: `:GPS:SATellite:TRACking:COUNt?` equals the screen's
`Tracking`, and `:GPS:SATellite:VISible:PREDicted:COUNt?` less that
equals its `Not Tracking`, verified against a screen showing 7 and 2.

So the screen is on no tier.  Nothing else it carries is unique to it
either: the health monitor line is the hardware condition register
rendered coarsely, and that register is on the fast tier, while the
bracketed synchronisation and acquisition text is
`:SYNChronization:STATe?` and the operation register.  The screen buys
one thing, a sky plot, and it is read only while someone is looking at
one -- `Op::Sky` on the socket, a view of its own in the monitor, and
`/sky` in the browser.  The medium tier, now four short steps
totalling 0.57 s, drops from thirty seconds to ten.

A screen read is returned to whoever asked for it, and is delivered to
the subscribers on one snapshot, so the log records that sky once.  It
is never stored as the latest snapshot.  Every poll starts from the
latest, so a screen kept there was copied into every snapshot after it,
logged afresh with each, and outlived a reconnect to a different
receiver.  The satellite counts the exporter serves come from the
medium tier's direct queries for the same reason.

Measured with the daemon running: 234 of 288 consecutive snapshots
were spaced 1.00 s apart, the rest being the medium and slow steps
publishing in between, and the only gaps over 1.02 s were the three
around a sky read.

A refresh moves the deadlines but not the step cursors: restarting a
pass in flight discards the steps already read, and under a stream of
refreshes no pass would reach its last step, so no tier would ever be
stamped as having run.

Tiers, to be measured against hardware before being fixed:

| Tier  | Contents                                                    |
| ----- | ----------------------------------------------------------- |
| ~1 s  | `:SYNC:TINT?`, `:SYNC:TFOM?`, `:SYNC:FFOM?`, `:DIAG:ROSC:EFC:REL?`, `:STAT:OPER:HARD:COND?`, `:SYNC:STATE?`, `:PTIM:TIME?` |
| ~10 s | satellite counts, oven temperature and current, the EFC DAC, `*STB?` and the operation and holdover condition registers, holdover duration and uncertainty |
| ~60 s | position, date, diagnostic log count, learned oscillator tempco, the powerup condition register |
|       | `:SYST:STAT?` (satellite table, health line) is on no tier: it costs 1.5 s and is read on request, by whoever is looking at a sky plot. |
|       | The receiver's UTC is on the fast tier, not with the date: a clock read once a minute and shown as a clock is wrong for the other fifty-nine seconds. |
| ~10 s | the error queue and any new diagnostic log entries, off the schedule; see below |

A control request arriving on the socket must be able to preempt a
scheduled status screen read.

### Allan deviation is computed over segments, not over a series

`:SYNChronization:TINTerval?` is a phase reading, so a run of them is
what the Allan deviation is defined over.  What it is the phase *of*
is worth being exact about: the interval measured is between the 1 PPS
from the GPS receiver and a 1 PPS derived from the OCXO -- "the time
difference between the 1-pps signal from the GPS engine to a similar
signal derived from the reference source" (`smartclock-dec96a9`,
Enhanced Learning).

The GPS receiver's 1 PPS is generated from its own crystal and is
quantized to it.  While locked, the OCXO is steered to follow that
1 PPS.  So the curve is of the pair and of the loop between them, and
not of the OCXO alone.

The estimator is the overlapping one, and gaps are handled by counting
only the second differences that exist rather than by filling anything in.  That
stays unbiased so long as what is missing is unrelated to what was
being measured -- readings lost to a daemon restart are; readings lost
*because* the receiver was misbehaving would not be, and no estimator
can rescue that.

Four things about gaps that are easy to get wrong, and were:

- **A second difference across a discontinuity is not a measurement.**
  A relock, a holdover, a power cycle or a swapped receiver may step
  the phase, and the estimator would read that step as enormous
  instability at every tau spanning it.  The run is cut into segments
  on the recorded mode and holdover flag, and on any absence longer
  than ten reading intervals -- of the readings, not of a grid
  coarsened for a long range, so a gap that cuts a short range cuts a
  long one too; second differences never cross a cut, and the
  segments are pooled so a broken run is still usable.  The state is
  checked across every logged row, including the held repeats that are
  dropped as readings: a short holdover during which the interval held
  one value is all repeats, and checking only the readings kept either
  side of it saw locked twice and no cut.  Rows where the interval was
  not read at all are kept for the same reason: a holdover in which the
  receiver refused it lies wholly in such rows, and leaving them out
  joined the locked readings either side.
- **A hole must stay a hole.** Readings go on a grid indexed by time
  rather than into a list, so a missing reading is an empty slot and
  not a closing-up of the ones after it -- which would turn every gap
  into a phase step.
- **A grid point with no reading near it must stay empty.** Nearest
  wins where two readings land on one point, but "nearest" is always
  *something*: the last point of a run has a nearest reading up to half
  a grid step away, and filling it from that one invents a measurement.
  The tolerance is half a *reading* interval, not half a grid step.
- **The spacing has to be better than one observed gap.** The grid
  places a reading at `round((t - t0) / tau0)`, so an error in `tau0`
  accumulates against the run.  The median of the gaps carries one
  gap's jitter -- a nominal second measured over a few hundred real
  polls came out as 0.999992636, seven parts per million, which reaches
  half a grid step within a day and then drops readings periodically as
  the drift wraps.  The median therefore only assigns whole-number
  positions, and the spacing is the least-squares slope of time against
  position, whose error falls off as `n^-3/2` rather than as
  `jitter / n`.  The slope is fitted within each run between absences,
  about that run's own means: a restarted daemon polls on a new phase,
  so no whole number of steps spans the absence, and one line through
  both runs was bent by the fraction -- four parts in ten thousand for
  a half-second shift between two runs of two hundred readings.

On a plain gapless record the estimator is checked against published
values and an independent implementation, in
`crates/smartclock/tests/adev_reference.rs`.  The 1000-point test data
set of NIST SP 1065 section 12.4 gives Table 31's overlapping Allan
deviation at tau of 1, 10 and 100 to the seven figures published.
allantools 2024.06's `oadev` on a thousand-reading phase record, with
a script beside the test to regenerate its values, agrees to one part
in 10^9 at every tau from 1 to 200 s, with the same count of
differences at each.

The web view's query thins the held readings in SQL, keeping a row
only where the interval or the state differs from the one before, so a
long range reads one row per update rather than one per poll.  It reads
at most the newest 500 000 such rows, about two months, and says when
a range held more.

A curve carries the count of readings, holes and segments with it,
because a curve alone cannot be judged: one drawn from a run that is
mostly holes looks exactly like one drawn from a clean day.  Points
below ten second differences are not emitted, and the sweep stops at a
third of the longest segment, so the curve ends where the data does
rather than trailing into noise.

Long ranges are gridded more coarsely rather than refused, which is a
subsample and not an average -- averaging readings before the estimator
sees them is the operation it exists to perform.  That costs the short
taus, which is the right trade for a month of history and the wrong one
for an hour, so the caller picks.

The reading itself is already an average.  `docs/firmware.md` shows
`:SYNChronization:TINTerval?` to be the mean of ten one-second
readings, updated every ten seconds, so the record is phase averaged
over contiguous 10 s windows.  That leaves the Allan deviation
band-limited below about five window lengths -- the 10 s and 20 s
points are low where white or flicker phase noise dominates, which is
where receiver noise sits -- and exact from about 100 s up.  The
modified Allan deviation has no such limit: its own averaging over n
consecutive samples of the 10 s means is the same window mean as over
10n one-second readings, so MDEV computed from the record equals MDEV
of the underlying one-second phase at every tau of 10 s and above,
with only the count of overlapping estimates reduced.  TDEV follows as
tau times MDEV over root three.  So MDEV and TDEV are the primary
curves, computed beside ADEV from the same gridded segments -- a hole
voids every window that spans it, `3m` triples rather than three --
and ADEV is kept, labelled, for comparison with the oscillator and
receiver data sheets, which quote it.  Both are checked against
allantools' `mdev` and `tdev` and against Table 31's modified Allan
and time deviation columns.

TDEV gets a chart of its own under the sigma-y chart, sharing its tau
axis and cursor, because it is in seconds and its slope is MDEV's plus
one: white PM falls as tau^-1/2, flicker PM is flat, white FM rises as
tau^1/2, flicker FM as tau, random-walk FM as tau^3/2.  So TDEV keeps
falling only while phase noise dominates and turns up as soon as the
oscillator's or the loop's frequency noise takes over, while MDEV and
ADEV are still falling or flat -- on one plot with a second axis that
read as a contradiction.  It is plotted anyway because it answers the
timing user's question directly: how far the 1 PPS wanders over tau,
in seconds, without multiplying sigma-y by tau in one's head.  `:PTIMe:TINTerval?`, which returns the latest one-second
reading, would extend the curves below 10 s, and is not going to be
polled: it costs a query a second and ten times the phase rows for
the receiver-noise floor alone.

## Architecture

Cargo workspace:

- `smartclock` -- library.  Typed errors via `thiserror`, no `anyhow`,
  blocking synchronous API.
- `smartclockd` -- the daemon.  Owns the port, logs, serves the socket.
- `smartclockmon` -- TUI client, `ratatui` + `crossterm`.
- `smartclock-cli` -- one-shot queries, `diagnose`, transcript capture.

`smartclock-cli` works two ways: `--device <path>` talks to the receiver
directly, which requires the daemon stopped, and `--socket <path>` goes
through the daemon.  Direct mode is what exists during phases 1 and 2,
before the daemon does; socket mode becomes the default once the socket
is present.

Library layers, bottom up:

1. `transport` -- `trait Transport: Read + Write`, with `SerialTransport`
   (`serialport`), `TcpTransport` (ser2net), `ReplayTransport`
   (fixtures) and `LoopbackTransport` (simulator).  Keeps the device a
   swappable dependency so most of the stack is testable without
   hardware.
2. `session` -- command framing.  Read-until-prompt, echo suppression,
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

- `Type=exec`.  `Type=notify` was planned, signalling ready once the
  port is open and `*IDN?` has answered, but the daemon is meant to
  come up and keep retrying with no receiver attached, so there is no
  moment that honestly counts as ready.
- `Restart=on-failure` with a backoff and a start limit, so a bad
  setting in `/etc/default/smartclockd` lands the unit in `failed`
  rather than restarting every five seconds forever.
- No `BindsTo=` / `After=` for the adapter's `.device` unit.  The
  daemon reconnects on its own and binding would stop it dead while an
  adapter is unplugged; naming a device in the unit also meant the
  operator had to edit the unit, which is the one thing the
  configuration file exists to avoid.  Offered as a drop-in recipe in
  `docs/running.md` instead.
- Every option readable from `SMARTCLOCKD_*` in the environment, so
  `/etc/default/smartclockd` is the only file an operator edits and
  `ExecStart=` names no settings at all.
- `StateDirectory=smartclockd` for the database,
  `RuntimeDirectory=smartclockd` for the socket.
- Dedicated user with `SupplementaryGroups=dialout` for port access;
  `RuntimeDirectoryMode` and socket group ownership set so the TUI runs
  unprivileged.
- Standard hardening: `ProtectSystem=strict`, `PrivateTmp`,
  `NoNewPrivileges`.  `RestrictAddressFamilies=` has to admit `AF_INET`
  and `AF_INET6` as well as `AF_UNIX`, or the `tcp://host:port` device
  form cannot open and the daemon restarts forever.  `PrivateDevices` is
  deliberately absent: it would hide the serial port.

A `systemctl --user` unit is the simpler alternative if system-wide
installation proves annoying, at the cost of not starting until login.

With `interprocess` handling the IPC and no peer-credential code to
port, systemd is the only Linux-specific piece left in the daemon;
`serialport` and `rusqlite` are both portable.
Porting therefore means writing a launchd plist or a Windows service
wrapper, not touching the protocol.

### The command table is data

Commands live in a TOML file, not in Rust source, and `build.rs`
generates the dialect code from it.  Each entry carries a stable logical
id, its classification, a citation, and one block per dialect:

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

The point is reviewability: the divergence between trees, the per-model
availability flags and the manual citations all sit in one table that
can be diffed against the documents, rather than being scattered through
code.

Codegen rather than runtime parsing, so the logical id set is an enum
and a typo is a compile error instead of a failed lookup.  `response`
names a parser; the parsers stay hand-written in `parse`, so the table
remains declarative and the awkward parsing stays real Rust.

An entry may also declare what argument it takes, which is what the
daemon checks a client's command against before anything reaches the
receiver:

```toml
  [command.argument]
  kind = "integer"   # or "word" with `allowed`, "none", or "free"
  min  = 0
  max  = 90
```

Omitting the block means the command takes no argument.  That default
was `free` until the September 2026 review, which is to say the gate
was fail-open: 101 of 113 entries had no block, so a query header --
permitted under the default policy -- could carry the set form's
payload beside it and be passed through untouched.  `:SYSTem:LANGuage?
"INSTALL"` reached the simulated receiver on a daemon started with no
flags at all.  `free` now has to be asked for by name, and a test pins
the list of entries that ask.

Two things fall out of having it: a test that every command reachable on
the active dialect has both a parser and a fixture, and the phase 7
per-model command matrix generated from the same source rather than
maintained by hand.

### The package version is derived

`make deb` builds `<version>-<commits>+g<sha>`, with `+dirty` appended
when the tree is not clean, rather than a revision written down by hand.
Two builds of different code then cannot carry the same version, which
matters because dpkg treats reinstalling an identical version as a
no-op: the binaries change or they do not, and nothing on the outside
says which.  That happened -- a package was reinstalled, the new
behaviour was absent, and the cause took a `--help` diff to find.

The hand-written `packaging/debian/changelog` stays as the record of
releases.  A derived version is a build, not a release.

### CI is shared; the Makefile is what you run

```
make            build
make ci         fmt-check clippy test
make fmt        cargo fmt
make clippy     cargo clippy --all-targets -- -D warnings
make test       cargo test
make test-hw    cargo test -- --ignored
make deb        a snapshot package
make release    tag the version Cargo.toml already names
```

CI called `make ci` at first, so that local and CI ran one definition
by construction.  It now calls the reusable `rust-ci.yml` shared with
the other repositories in this fleet, because one repository doing it
differently costs more than the symmetry is worth.  The inputs restore
what the shared defaults would drop: the toolchain is pinned rather
than tracking stable, `cargo fmt` is pinned to the same toolchain
because nightly rustfmt formats differently and would fail a check that
`make fmt` had just satisfied, and `--all-targets` is passed or clippy
never lints the tests, which are the larger half of this workspace.

Two things that were true of `make ci` are no longer enforced by CI and
are worth knowing.  The shared `cargo test` does not say `--workspace`;
harmless while there is no `default-members`, and adding one would
silently narrow CI to a subset, which is a trap this repository has
already fallen into once.  And the shared workflow pins its actions by
tag rather than by commit sha, which is the fleet's posture, not this
repository's preference.

`make ci` remains the local command and remains the stricter one.

The one wrinkle is that some tests need the receiver.  Those are
`#[ignore]`d and reachable only through `make test-hw`, which CI never
runs, because they need hardware CI does not have and a daemon that must
be stopped first.  Everything else -- parsers, the status screen
scraper, session framing against `ReplayTransport` -- runs anywhere.
Once the phase 4 simulator exists, integration tests move back into
`make test` by talking to a simulated receiver instead of a real one,
which is most of the reason it is worth building.

### Dialects

Two branches, not four.  `097-59551-02` shows the 58503A tree is
essentially identical to the 58503B:

| Family                  | Tree                                      |
| ----------------------- | ----------------------------------------- |
| 58503A / 58503B / 59551A| `:GPS:`, `:SYNChronization:`, `:PTIMe:`   |
| Z3801A / Z3816A         | `:PTIME:GPSYSTEM:`, `:ROSCillator:`       |

Availability is per-model, not just per-family: the 58503A has
`:GPS:SATellite:TRACking:IGNore` / `INCLude`, which the 58503B guide
marks 59551A-only.  Unsupported operations return a typed `Unsupported`
error rather than reaching the device.

Response formats differ across dialects too, so the table carries them.
`:DIAG:ROSC:EFC:REL?` returns `+-d.dEe` on the 58503A but is documented
as a plain integer on the Z3801A.

Z3801A entries go into the table as the manual describes them, but stay
unverified until hardware is available.  The table marks them as such,
so an unverified command is a known risk rather than a silent
assumption.

If a variant's tree cannot be pinned down from the manuals, the
fallback is the unit's EEPROM.

### The status screen scraper is mandatory

`:GPS:SATellite:TRACking?` returns PRN numbers only.  Per-satellite
elevation, azimuth and C/N exist solely in the `:SYSTem:STATus?` screen,
as does the health monitor line and survey detail.  The scraper is a
first-class parser, not a fallback.

`097-59551-02` chapter 3 contains five sample screens covering distinct
states -- survey in progress, `*nn Acq..` acquiring markers, `nn -- ---`
untracked rows, `Predict --` unavailable, "Locked to GPS: stabilizing
frequency".  More appear in `097-58503-13`.  These become scraper
goldens, so phases 2 and 3 do not block on hardware.

## EFC diagnosis

The development unit was thought to be failing: an OCXO aged past the
range its EFC DAC can pull presents as a unit stuck in holdover, and
the receiver reports that directly.

The evidence has since gone the other way.  It is locked, with a valid
reference, and has stayed locked for every sample since the log began
carrying the condition registers.  Its own diagnostic log, once copied
out, shows nineteen holdover-and-relock cycles across two days in March
2025 and nothing since -- a pattern that reads as GPS reception rather
than a crystal, since an oscillator drifting out of range does not
recover nineteen times.  And the EFC measurement below settles the
mapping in favour of the specification, which leaves about a decade of
tuning headroom rather than the year the pessimistic reading implied.

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
GPS or antenna fault immediately.

This moves `smartclock-cli diagnose` up to phase 2, so the unit can be
examined before either the daemon or the TUI exists.

## What the review found

Five defects fixed on the day, all of the same kind: the software
reported something wrong rather than reporting nothing.

**The scraper invented satellites.**  A blank signal cell let the next
satellite's asterisk be eaten as this one's reading, so that satellite
vanished and one was assembled from the leftovers -- `PRN 24` at an
elevation of 204 degrees, written to the log and drawn as a real
object.  A short tracked column made the first group claim the second
group's rows, reporting untracked satellites as in use.  An asterisk
now ends the preceding cell, and groups carry their heading column so a
blank cell can be told from a missing one.

**The tracked count was the untracked one.**  Every plain search for
`Tracking:` finds it inside `Not Tracking:` first, and the earlier fix
for this used `split_once`, which has the same flaw.  It passed because
every fixture prints the tracked count first.

**The screen now checks itself.**  It states how many satellites are
tracked and how many are not, so the table can be compared against
them.  All three faults above broke that invariant.  A disagreeing
table is still shown -- a partial sky beats none -- with the pane
saying the counts disagree.

**Freshness was per snapshot, not per tier.**  The one-second tier
succeeding re-stamped the whole snapshot `Live` and cleared the error,
while the status screen underneath went minutes stale; after a link
drop the first fast poll relabelled an hour-old sky as current.  That
is this document's own principle broken by the scheduler.  Each tier
now carries when it last succeeded and its own error, over the wire and
into the log, and each pane shows the age of what it displays.

A snapshot is `Live` only when every tier has succeeded and none is
carrying an error, so the first minute after startup reads `Stale`:
there is no sky, no position and no date yet, and saying otherwise told
a client the whole reading was current when two thirds of it did not
exist.

The whole-snapshot flag survived that rework and went on contradicting
it: still last-writer-wins, so with the medium tier failing and the fast
tier fine it alternated `Live` and `Stale` every second, and the log's
`freshness` column alternated with it.  It is now derived -- a snapshot
is as current as its least current part -- and the history graphs select
on `fast_at = at`, which asks the question they actually care about:
whether the fast tier measured this row or the row restates the last
one.  In the development database all 6836 rows were marked `live`,
including 535 that were restatements.

That question is the fast tier's.  A medium or slow column rides in
every fast row with whatever its own tier last read, so with that tier
failing and the fast tier fine the graphs drew the last value as a flat
line for as long as the failure lasted.  Such a column now counts in a
row only while its tier's timestamp is within three of its intervals of
the row, and is null past that, so the line breaks.  The daemon records
its cadence in `meta` for this, since the readers have only the
database; a log that predates that is read at the default cadence.  The
window rather than one row per read of that tier, because at an hour's
zoom a ten-second tier would leave most buckets empty and draw every
such trace dashed.  The cost is that a slow column's mean is weighted by
time rather than by read.

**A failed command could be answered by the next poll.**  A client
command that errored left the reply in flight, and the next scheduled
poll read it as its own: TFOM reported as FFOM, oven current as
temperature, recorded as measurements.  Compounding it, `drain`
returned success when it gave up, so `sync` matched the abandoned
reply's prompt and called itself fine one exchange behind.

**The authorization gate did not hold.**  Covered under "Commands go
through the daemon too" above; a query header carried a baud change
past a daemon started with no flags.

**The history window compared timestamps as text.**  `T` sorts after a
space, so `datetime('now')` never excluded anything: measured against
the development log, the pane titled "1 hour" returned the whole
database.

### What that says about the tests

Every one of these passed CI.  The pattern is that the fixtures were
drawn from screens the scraper already handled, and the parsers were
tested against replies the receiver had actually sent -- so the tests
confirmed the code against the cases it was written from.  What was
missing was the adversarial direction: a malformed screen, a tier that
fails while another succeeds, a command that fails mid-exchange.  The
invariant check on the satellite table is the most valuable single
change here, because it turns a class of misreadings into a visible
failure without anyone having to predict the shape of the next one.

## Phases

What each turned out to involve, which is the part worth keeping.

| # | Deliverable | What it turned out to involve |
| - | ----------- | ----------------------------- |
| 0 | Workspace, TOML command table with `build.rs` codegen, fixtures, Makefile CI | evidence became three-valued rather than a boolean |
| 1 | `transport`, `session`, `smartclock-cli` | the manuals had the prompt wrong, and an abandoned reply desynchronised everything after it |
| 2 | `types`, `parse`, screen scraper, `diagnose` | the scraper had to read by label, not column; the manuals' own ASCII does not line up |
| 3 | `Device`, `Snapshot`, `DeviceTask`, `smartclockd`, systemd unit, deb | reconnection meant handing the request channel back out of the task |
| 4 | `smartclock-sim`, in process and over TCP | TCP rather than a pseudo-terminal, which would have been Unix-only |
| 5 | `smartclockmon`, dashboard and history graphs | columns carry min and max as well as mean, or quantization steps vanish into a ramp |
| 6 | `Control` handle, daemon flags, audit trail, raw console | flags replaced the nonce; the classifier could not match a caller-supplied argument |
| 7 | `docs/commands.md`, generated | a test compares it against the table, so it cannot drift |
| 8 | `smartclock-exporter`, `smartclock-web` | a scrape must cost the receiver nothing, so both read what the daemon already polled |
| 9 | The receiver's own records: error queue, diagnostic log, condition registers | reading is what removes an entry, so the read and the write have to share a thread |
| 10 | Adoption per connection; the alarm watched, not taken | the event registers turned out to be the wrong answer twice before `*STB?` was the right one |

Suggest a commit at each phase boundary.

## Open questions

Things that are not decided, as distinct from the defects below.

1. **What Ghidra would say about the firmware.**  `strings` gives the
   tables -- the SCPI keywords, the screen formats, the state names --
   but not the logic, so three questions stay outside: which state
   machine drives the mode suffixes, how `:DIAGnostic:ROSCillator:
   TCOefficient?` is computed and where it is applied, and whether the
   scaling behind the undocumented `EFControl:ABSolute?` is visible in
   code.  Each has been inferred from the outside and none is settled.
   `third_party/z3801a-3543.bin` and `z3816a-4001.bin` are the images; there is
   no 58503A dump, which is also why the front panel's strings are
   unknown.

2. **Whether any other SmartClock variant is on hand.**  The Z3801A
   half of the command table has its spellings corroborated against the
   firmware's own keyword table, so they are right; no such receiver
   has ever been on the line, so its behaviour is unobserved.  Until
   one is, `evidence = "firmware"` is as far as those entries can go.

3. **What the receiver's `:STATus:<register>:ENABle` masks are set
   to.**  They decide which bits within a group reach the alarm, so
   they are what would say which events the receiver itself considers
   alarm-worthy.

   The reason recorded here for not having read them -- that it needs
   the daemon stopped -- was wrong.  `smartclock-cli --socket query`
   sends arbitrary SCPI through the running daemon; what stopped it is
   that an unrecognised command is treated as control, and
   `--allow-raw` is off.  The five queries are in the command table as
   of this commit, so they are now an ordinary read.  Nothing has been
   sent yet.

   `097-59551-02` 5-87 and 5-88 give the factory defaults, which are
   worth having either way:

   | Register | ENABle | PTRansition |
   | -------- | ------ | ----------- |
   | `OPERation` | 36 | 127 |
   | `OPERation:HARDware` | 8191 | 5119 |
   | `OPERation:HOLDover` | 8 | 15 |
   | `OPERation:POWerup` | 7 | 7 |
   | `QUEStionable` | 3 | 2 |

   with `*SRE` 136, `*ESE` 0 and every `NTRansition` 0.  All
   non-volatile.  This unit matches on every one we have read: `*SRE`
   136, `*ESE` 0, NTR all 0, and PTR 127 / 5087 / 15 / 7 / 2.  The lone
   difference is hardware PTR 5087 against a default of 5119, which is
   bit 5 -- documented "not used" in figure 5-1, so it signifies
   nothing.

4. **Asserting a known antenna position from the configuration.**  A
   receiver at a surveyed site does not need to survey: told where it
   is, it goes straight to position hold and starts serving time,
   where a survey leaves the 1 PPS invalid for as long as it runs.  A
   Z3805A brought up on a bench where a 58503A had already surveyed
   the same antenna spent its first hour unable to do either, and the
   position was known the whole time.

   So a position setting asserted on attach, taking any of three
   forms: a geodetic position, earth-centred x, y and z, or the word
   `survey` to say the receiver should determine its own.  The third
   is not a no-op -- it is how you ask for a survey deliberately rather
   than inheriting whatever the last operator left set.

   Four things make it more than a one-line setting.

   It is a control command, so it needs the same opt-in as the rest and
   must not fire under a daemon started read-only.

   It is non-volatile on the receiver, so a stale value outlives the
   configuration that set it.  Worse, holding a position also means
   turning off survey-on-powerup, or the next power cycle discards it;
   set both and a receiver whose antenna has moved will never notice,
   and will serve confident nonsense.  Whatever asserts a position
   should be able to release it too.

   The two models disagree about the vertical datum -- the 58503B
   reports height above the ellipsoid where the others use mean sea
   level -- so a geodetic setting has to say which it means rather than
   carry a bare number.  Earth-centred coordinates sidestep that
   entirely, which is half the reason to support them.

   Observed on a Z3805A: brought up on a bench where a 58503A had
   already surveyed the same antenna, it spent an hour unable to serve
   time while the position was known the whole time.  Asserting it by
   hand took two commands.

5. **Whether acknowledging from the monitor is wanted.**  Reading an
   event register would say which bit latched rather than which group,
   and clears the alarm as it does so.  That makes it exactly the right
   implementation of a deliberate acknowledgement and exactly the wrong
   thing to do on a timer.  Nothing needs it yet.

6. **Whether the stability chart should show one curve or three.**
   It shows MDEV, ADEV and TDEV together, with the legend to hide any;
   a reader who wants one curve has to click.  Left as is until
   someone reads it and objects.

## Known defects

Known, unfixed, and each here because the fix is not obviously worth
its cost yet.

**A receiver swapped without breaking the link is not noticed.**
`*IDN?` is read once, when the port is opened, and nothing re-reads it
while the link stays up.  Swap the cable between two receivers quickly
enough that no command fails and the daemon keeps the old identity, so
the new unit's rows are filed under the old one -- silently, and
exactly defeating the per-row attribution they exist for.

In practice the fast tier sends a command every second, so a swap
almost always breaks a read and is caught.  Almost always is not a
property to put a database on: **stop the daemon before moving the
cable between units.**

Not fixed by choice.  The fix is cheap -- ask `*IDN?` on the slow tier
and compare -- and can be done if swapping ever becomes routine.

**The receiver's log timestamps are not monotonic across a power
cycle.**  Its clock restarts at midnight on a stale date and runs free
until the first lock, so a log entry's stamp in that window is elapsed
time since boot wearing the costume of a wall clock.  The development
unit's log shows it plainly:

    3  20050528.00:01:08  Position hold mode started
    4  20050528.00:02:00  GPS reference valid at 20050529.04:09:25
    5  20050528.00:00:00  Power on
    6  20050528.00:00:28  Position hold mode started

Eighteen of the nineteen midnight stamps in that log are a power-on or
a preset.  Entry 4 carries the real time in its message, which is how
the receiver reports a time it did not have when the entry was written.

The stamp is therefore not an ordering key, and schema 7 stops using
it as one.  Ordering by `entry` alone is right within one run of the
numbering and wrong across a clear, which restarts it; ordering by `at`
is right across a clear and wrong within the bulk copy of history,
which is not fetched in entry order.  `receiver_log.generation` counts
the clears, so `(generation, entry)` is the receiver's own sequence and
orders the whole log.

Existing databases have their generations inferred on migration:
within a generation the receiver issues each entry number once, so
walking a unit's rows in the order they were read and starting a new
generation wherever a number comes round again recovers the
boundaries.

A clear is noticed by the count falling below the highest entry held.
A log cleared and refilled to at least that count while the link was
down numbers the same way, and with `--adopt-log` the old generation's
completeness would then authorise erasing entries never copied.  So
once per connection, before copying or erasing anything, three held
entries -- the newest, the oldest and one between -- are read again
and compared with what is stored; if any differs, what follows is a
new generation.  Three rather than one because the receiver repeats a
power-on stamp and message, so a single entry can match across two
logs.  Two logs matching at all three would still pass.

The stamps themselves are still what the receiver wrote, rollover and
all, and this entry stands as the reason not to read them as wall
clocks.

Our own timestamps are compared as text: every range bound and every
`ORDER BY at` in the readers does it, so the index on `at` can serve
them.  The default form of a timestamp trims trailing zeros, and as text
`00.11Z`, `00.1Z` and `00Z` sort in that order, the reverse of time;
two rows in one second could come back backwards, which the Allan
deviation reads as a negative gap.  Schema 8 stores every timestamp with
nine fractional digits, so text order is time order, and on opening an
older database rewrites the ones already there in one transaction.

What the September 2026 reviews found is either fixed or, where a
decision went the other way, recorded as a decision above.

Two things are deliberately not defended against, because the threat
model is a careless operator on a single-operator machine and not an
attacker.

Socket permissions are the whole of the authorization: group membership
decides who may connect, and whoever connects may issue whatever the
daemon was started to allow.

A client that connects and never speaks holds one of the sixteen slots
until the process ends.  There is no read timeout because
`interprocess` 2.4.4's portable `Stream` exposes no handle to set one
on, so closing this would mean `socket2` and a `cfg(unix)` arm for
`SO_RCVTIMEO` -- a dependency and platform-specific code in the part of
the daemon that was written the way it was to stay portable.  A program
that connects and goes silent is a bug in a program the operator wrote,
and the daemon says so in the journal rather than failing quietly.  The
half of this that was a real defect -- a client half-closing its sending
side, releasing its slot, and leaving the push thread running -- is
fixed and tested.
