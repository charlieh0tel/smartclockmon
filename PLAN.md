# Plan

A Rust library for talking to HP / Symmetricom SmartClock GPS receivers
over serial, a logging daemon that runs as a system service, a TUI, and
browser and Prometheus views.  A GUI is not planned.

## Status

Installed from the package as a service, one daemon per port, logging
to `/var/lib/smartclockd/<model>-<serial>.sqlite`; `docs/running.md` is
the deployment note.  **Next** lists pending work.  `make ci` is the
local check; `make test-hw`, the hardware-only set, never runs in CI.

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
console and line-settings code, so there is one tool to learn.  Like
`read-memory` it is a fixed procedure, outside the forbidden-command
check, run with the port's daemon stopped.

An image must match an audited profile (Z3801A, Z3805A, 58503A,
Z3816A), each with its own protected regions and checksums; filenames
and model strings cannot establish compatibility.  Before erase it
checks the image SHA-256, boot checksums, model, running revision and
expected serial, and refuses anything unknown, with no force override.
It never sends `*CLS`.  Afterwards it reads the whole flash back
through the debug console, compares it with the image, and returns to
SCPI with `halt`, or failing that through the installer, by the same
code as `read-memory` (`smartclock::console`).  Revision changes have
simulator coverage, not hardware validation
(`docs/firmware/restart.md`, "The flasher").

### No client-side SCPI crate

Surveyed: `scpi` + `scpi-contrib` (server side, no_std), `scpify` (TCP
and HiSLIP only), `scpi-client` 0.1.1 (thin, immature),
`instrument-core` 0.1.0 (VISA/GPIB oriented).  None fit: the receiver
echoes, prompts with `scpi > ` / `E-nnn> `, and its richest response is
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
every later read.  `Session` moves into that thread at construction, so
a second writer is a compile error.  A command that errors must not
leave its reply in flight, or the next poll reads it as its own (TFOM
recorded as FFOM).  Client requests are served between polls, at most
`REQUESTS_PER_POLL` before the schedule's turn, so neither starves the
other.

One daemon per socket: a stale socket file is removed before binding
only when nothing answers on it, and a second daemon pointed at a live
socket refuses to start.

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

A client sends SCPI text (op `query`; the others are `latest`,
`status`, `info`, `note` and `fact`), which the daemon classifies
against the command table and gates by class.  A reply reports what the
device said, not that the operation finished: a survey takes hours but
acks in milliseconds, and clients watch progress in later snapshots.

| Class     | Examples                                              | Gate                     |
| --------- | ----------------------------------------------------- | ------------------------ |
| Query     | `:GPS:POSition?`, `:SYNC:TINT?`                        | none                     |
| Control   | holdover initiate and recover, survey, antenna delay, elevation mask; reading an event register or `*ESR?`, which clears it; reading `:SYSTem:ERRor?`, which removes the entry | `--allow-control` |
| Dangerous | `:SYSTem:PRESet`, `:SYSTem:COMMunicate:SERial1:*`, `:DIAGnostic:ERASe`, `:SYSTem:LANGuage "INSTALL"` | `--allow-dangerous` |

Each gate is a daemon flag, set outside any client, so a daemon
started without it cannot be talked into the command.  A command
containing `;`, a control character or non-ASCII is refused before
classification, since a query chained to a setter would classify as a
query.  Argument ranges sit in the table beside the class, so the
table, not the typed `Control` API, is the safety boundary.  The TUI's
raw console bypasses the table, so it has its own flag, `--allow-raw`.

Authorization is socket permissions alone (`RuntimeDirectoryMode`,
group ownership); no peer credentials or tokens.  Every command that is
not a scheduled poll is audited -- time, text, class, outcome, receiver
-- so one log answers "what did I do to it, and what did EFC do
afterwards".  A successful non-query brings every tier's next poll
forward.

### Disconnection is a first-class state

USB serial adapters drop and receivers are power-cycled; the daemon
reconnects rather than exiting, and records the gap.  A failed poll
republishes the last snapshot unchanged, so clients see it go stale,
and the log writes that moment once, so the outage is a gap.

Freshness is per tier: each group of fields carries when it last
succeeded and its own error, over the wire and into the log, and each
pane shows the age of what it displays.  A medium or slow value counts
while its tier's timestamp is within three of its intervals
(`Cadence::current_window`) and is null past that, so a failing tier's
line breaks rather than running flat.  History queries, the monitor and
the exporter share that window, with the cadence the daemon records in
`meta`.

Each opening of the receiver is an attachment (`AttachmentId`), which
starts from an empty snapshot.  A command that reaches a later
attachment than it was sent under is answered `Reattached` and not
sent, since it was meant and audited for the earlier unit.  The daemon
has no default device, since a wrong one sends SCPI at whatever is
there.

### The receiver's own records

The receiver keeps two records that are lost unread: its error queue,
where `:SYSTem:ERRor?` removes the entry it returns and a full queue
replaces its last entry with -350 and discards the newest (097-59551-02
5-31), and its diagnostic log, which holds 222 entries and then stops
recording.  The daemon journals both, each under the receiver that
wrote it; how the log is copied and ordered is in `docs/running.md`.
The journal runs on the thread that owns the database, through the
request queue, since a channel between reading an entry and writing it
is a gap a shutdown can lose it in.  It starts afresh on each
attachment: the unit may have been power cycled, swapped or
reconfigured.

**Condition registers, never event registers.**  Reading an event
register clears it, and with it the front-panel Alarm LED and the BITE
output, which belong to whoever is at the instrument.  The daemon polls
the condition registers and `*STB?`, which reading "does not change"
(5-44); Time Reset is event-only (5-39).  So the logger never writes to
the receiver, `*CLS` included, and a client needs `--allow-control` to
read an event register.  The transition filters, without which events
mean nothing, are recorded once per attachment and never written: they
are non-volatile and only `:SYSTem:PRESet` resets them.  The bench
58503A's negative filters are zero, so a missing clear-event is not
evidence a fault persisted.

**`--adopt-log` erases the receiver's log** once it is copied, so the
log records again.  It is opt-in because erasing is irreversible, and
fires only when the copy is complete and gap-free, the log-almost-full
bit is set, and `*IDN?` confirms the unit is the one copied.  Clearing
unsets the bit, so a flapping link cannot erase twice, and the entry
count goes with the command so the receiver refuses with -222 if an
entry arrived in between.

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

The status screen is about 1.6 KB: 0.94 s of wire time at 19200 8N1
and 0.5 s of the receiver composing it, so it cannot be polled at 1 Hz
beside anything else.  19200 is the ceiling: `097-59551-02` 5-101 lists
four rates up to 19200, and the 58503A answers
`:SYSTem:COMMunicate:SERial1:BAUD 38400` with `+0,"No error"` while
keeping 19200.  `BaudRate` still knows 4800, 38400, 57600 and 115200,
so a receiver another tool moved there can be moved back.

On a 58503A a whole fast pass costs 0.38 s.  So the screen is on no
tier: everything on it but per-satellite elevation, azimuth and signal
strength has its own query.  It is read on request and every `--sky`
seconds (300 by default), half a percent of the link, and never stored
as the latest snapshot, which every poll starts from.

Each tier is a list of steps, one taken per turn.  The fast tier is one
step, because its fields are compared against each other, and a time
interval from one second beside an EFC from the next is a correlation
nobody measured.  A part-done tier yields to any tier come due, and a
refresh moves deadlines, not step cursors, so no tier starves.

| Tier  | Contents                                                    |
| ----- | ----------------------------------------------------------- |
| ~1 s  | `:SYNC:TINT?`, `:SYNC:TFOM?`, `:SYNC:FFOM?`, `:DIAG:ROSC:EFC:REL?`, `:STAT:OPER:HARD:COND?`, `:SYNC:STATE?`, `:SYNC:HOLD:WAIT?`, `:PTIM:TIME?` |
| ~10 s | satellite counts, oven temperature and current, the EFC DAC, `*STB?` and the operation and holdover condition registers, holdover duration and uncertainty |
| ~60 s | `*IDN?`, position, date, diagnostic log count, the oscillator-current constant (`TCOefficient`), the powerup condition register |
|       | `:SYST:STAT?` (satellite table, health line) is on no tier; it is read every `--sky` seconds and on request. |
|       | The receiver's UTC is on the fast tier, not with the date: a clock read once a minute is wrong for the other fifty-nine seconds. |
| ~10 s | the error queue and any new diagnostic log entries, off the schedule; see "The receiver's own records" |

### Allan deviation is computed over segments, not over a series

The phase reading, `:SYNChronization:TINTerval?`, is already a mean of
ten one-second readings, so the overlapping estimators run over
segments cut at every discontinuity -- a relock, holdover, power cycle,
swap or long absence -- with holes left as holes rather than bridged.
Because of that averaging, MDEV and TDEV are the primary curves; ADEV is
kept, labeled, because data sheets quote it.  Each carries a one-sigma
interval from Greenhall's equivalent degrees of freedom; MTIE is
computed, not inferred.  The method, its checks against NIST SP 1065
and allantools, and the bench measurements are in
`docs/stability.md`.

### One log per receiver, named by its serial

Several receivers on a host means one daemon per port,
`smartclockd@<port>`, each with its own device and socket, so nothing
in a daemon becomes concurrent.  The instance is named by the port, as
`serial-getty@` is, and the unit derives device and socket from it;
anything else per port goes in a drop-in, and shared settings in
`/etc/default/smartclockd`.  The daemon and monitor have no socket
default, which would guess an instance name; `smartclock-web` and the
exporter scan `/run/smartclockd/*/socket` unless given one.

The receiver, not the port, chooses the log: after `*IDN?` the daemon
opens `/var/lib/smartclockd/<model>-<serial>.sqlite`, and nothing
before.  A swap switches files, a unit moved to another port keeps one
history, and two instances cannot collide on a file.  A receiver whose
identity does not parse gets a file named after the device and a loud
log line, not a shared "unknown" file.  `smartclock-web` lists every
`*.sqlite` in the log directory and keys everything by serial
(`?receiver=`).

A schema change is migrated by the daemon on open, in `migrate()`, and
tested: released logs live on hosts other than the bench.

### Exporter metrics keep their names and labels

Exporter metrics are not renamed and their labels are not changed:
`oven_tempco` stays, and a stopped daemon's `up 0` carries only its
instance label.

### Notes and facts live in the receiver's log

Bench events and a unit's internals are invisible to the receiver, and
kept only in `docs/hardware-investigations.md` they cannot be lined up
against the log.  So each log has a `note` table, timestamped free text
such as "ran `master_reset`", and a `fact` table of `key=value` about
the unit, such as `ocxo.serial`; a fact records when it became true, so
a replaced part keeps its old value, and setting one also leaves a
note.  Both are written through the daemon (ops `note`, `fact`) and
never sent to the receiver.  A note is filed under the receiver
attached when it is written, even when `--at` backdates it past a swap;
a bench-wide note, such as a splitter change, is written to each daemon
in turn.  `docs/running.md`, "Notes and facts", says where they show.

### Comparing receivers is a page over the existing API

`/compare` shows every receiver over the same range -- 1 PPS TI, EFC,
temperature, TFOM and FFOM overlaid, and ADEV and MDEV curves on one
plot -- from `/api/history`, `/api/adev` and `/api/notes`, asked once
per receiver and joined in the browser.  The live, status and stability
pages stay one receiver each.

### The time range follows Grafana

The browser view's range control copies Grafana's dashboards -- half-
window steps, zoom out about the center, the `t` keys, Back undoing a
change of range, a refresh picker with Auto -- because people who read
time series know them.  It departs twice: a range moved to end near now
becomes the moving one rather than sliding into the future, and a fixed
range is not read again once a read has started after its end.  Each
page sets its fastest refresh by what one read costs.  `docs/views.md`
has the details.

### Sensors are the host's

Room temperature and the like are read by `smartclock-sensord`, a
service of its own with its own log and socket, not by smartclockd into
the receivers' logs.  A sensor belongs to the host: in a receiver's log
it would go dark with every outage and swap, a swap would start it
afresh in another file, and two daemons would log it twice.  Reading
it on the device thread would also put sysfs reads, which can block
for seconds on a wedged bus, in the path of the serial link.
`docs/sensors.md` has the design.

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

Commands live in `crates/smartclock/commands.toml`, from which
`build.rs` generates the `Dialect` and `CommandId` enums and the specs.
Each entry has a stable id and a class, and one block per dialect:

```toml
[[command]]
id = "holdover_waiting"
class = "query"

  [command.dialect.hp58503]
  scpi = ":SYNChronization:HOLDover:WAITing?"
  response = "enum:HoldoverWaitReason"
  models = ["58503A", "58503B", "59551A"]
  cite = "097-59551-02 5-36"
  evidence = "hardware"
```

Tree divergence, per-model availability and citations sit in one table
that can be diffed against the documents, and a typo in an id is a
compile error.  `evidence` is `manual`, `firmware` (every keyword found
in the firmware's own keyword table) or `hardware` (a receiver answered
it), so an unconfirmed command is a known risk.

An entry may declare its argument -- `integer` with `min` and `max`,
`word` with `allowed`, `none` or `free` -- which the daemon checks
before anything reaches the receiver; none declared means none taken.
`free` must be asked for by name, and a test pins the entries that ask:
as a default it let `:SYSTem:LANGuage? "INSTALL"` through a daemon
started with no flags.  `docs/commands.md` is generated from the table,
and a test fails if they disagree.

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
make docs       regenerate docs/commands.md from the command table
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
it differently costs more than one definition; `make ci` is stricter.
The browser tests need Node and Chromium, so CI runs them in
`.github/workflows/web.yml` and `make ci` leaves them out.

### Dialects

Two branches, not four; `097-59551-02` shows the 58503A tree is
essentially the 58503B's:

| Family                  | Tree                                      |
| ----------------------- | ----------------------------------------- |
| 58503A / 58503B / 59551A| `:GPS:`, `:SYNChronization:`, `:PTIMe:`   |
| Z3801A / Z3816A         | `:PTIME:GPSYSTEM:`, `:ROSCillator:`       |

The Z3805A answers the z3801 dialect.  Availability is per model, and
an unsupported operation returns a typed `Unsupported` error without
reaching the device.  Response formats differ too: `:DIAG:ROSC:EFC:REL?`
returns `+-d.dEe` on the 58503A but is documented as a plain integer on
the Z3801A.

The bench Z3801A (3543-A) does not echo, and stamps its diagnostic log
in hex seconds of GPS time (`097-z3801-01` 4-13) where the others write
a date; the stamp is stored as written.  Its `-230 Data corrupt or
stale` while tracking no satellites is the manual's answer for an
unavailable value (4-6, 4-7), taken as a state refusal.

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

## Architecture

The crates are listed in `README.md`.  `smartclock-log` holds the log's
schema and every query that reads it, so the daemon, monitor and web
view cannot drift apart.  The library, bottom up:

1. `transport` -- `Transport`: serial, TCP, replay, tee.
2. `session` -- framing: prompts, echo, timeouts, error queue drain;
   the status screen by its `:SYSTem:STATus:LENGth?` line count.
3. `command` -- `Dialect`, `CommandId` and specs from `commands.toml`.
4. `types`, `parse`, `screen` -- newtypes, parsers, the scraper.
5. `device` -- `Device`: typed queries and poll steps; `control`.
6. `task` -- `DeviceTask`: the schedule, requests, snapshots.
7. `protocol`, `wire`, `client` -- the socket and its client.

## Next

- **Later: compacting old logs**, perhaps into Parquet.  The logs grow
  without bound.
- **Bench work:** the open items in `docs/hardware-investigations.md`,
  each with its TODO.
- **Editing notes from the web view**, through the daemon socket.
  Needs POST bodies in `smartclock-http`, `/api/notes` returning each
  note's `id`, and edit requests keyed by it.

## Open questions

1. **Which state machine drives the mode suffixes.**  Still inferred
   from outside (`docs/screen-format-strings.md`, "Mode suffixes").
   The rest of what the firmware leaves open is in
   `docs/firmware/README.md`, "What is not established".

2. **How far the z3801 dialect is confirmed.**  Of the z3801 entries,
   18 are `evidence = "hardware"`, 63 `firmware` and one `manual`;
   those 64 stay unconfirmed until a receiver answers them.  The
   keyword table they were checked against is `docs/z3801-keywords.md`.

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
