# Plan

Build a Rust library for talking to HP / Symmetricom SmartClock GPS
receivers over serial, a logging daemon that runs as a system service,
and a TUI client.  A GUI is possible later but is not planned.

## Goals

- Read-only monitoring of a 58503A first; control commands later.
- Diagnose the development unit's suspected EFC / OCXO problem.
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

### Firmware reversing is deferred

`strings` over `the keyword table` already yielded the screen's
printf formats, the time code layouts and the log and tolerance
messages, which is recorded in `docs/screen-format-strings.md`.  Those
are string literals, and literals are what `strings` finds; reversing
would add control flow, not more of them.

The SCPI keyword table is in the image but stored in some structured
form that `strings` only fragments -- `ROSC`, `TINT`, `PTIM`, `ESHOLD`.
Recovering it properly would need a disassembler.  It is not worth one:
`097-z3801-01` documents that tree already, and a Z3801A on the line
would settle it in seconds.

The decisive point is that both images are Z3801A/Z3816A firmware while
the unit in front of us is a 58503A running 3704-C, which we do not
have an image of.  Reversing what we hold would describe a receiver we
do not own.  The live unit has been strictly more informative than the
documents so far: it is what revealed the real prompt, the SS column,
and the asterisk classification error.

Three things would justify it.  The 58503A's own EEPROMs, if they are
ever pulled, since that is the firmware actually being monitored.  A
Z3801A that contradicts its manual.  Or the one question hardware
cannot answer cheaply: what EFC value and averaging window set hardware
bit 6, "EFC near end of range", which would let the monitor warn before
the receiver does.  The firmware has `last efc average`, `tempco` and
`EFC near end of range` in it, so the answer is there.

Even that last one is better approached by logging EFC for months and
measuring the drift rate, which says when the rail will be reached
rather than only where it is.  It is 68k, so a disassembler would cope
whenever the case arises.

### The daemon owns the port

`smartclockd` runs as a systemd system service and holds
`/dev/ttyUSB0` open for as long as it runs.  Nothing else can open it,
so every other component is a client of the daemon.  This follows from
wanting multi-day EFC history on a unit that may be dying: collection
cannot depend on a TUI being up.

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
| Control   | holdover initiate and recover, survey, antenna delay, elevation mask | config opt-in |
| Dangerous | `:SYSTem:PRESet`, `:SYSTem:COMMunicate:SERial1:*`, `:DIAGnostic:ERASe`, `:SYSTem:LANGuage "INSTALL"` | config opt-in and a nonce |

Dangerous commands can strand the link or wipe configuration, since a
baud change persists across power cycles.  Requiring the client to echo
a nonce the daemon issued keeps them out of reach of a fat-finger or a
stray script.

Authorization is socket permissions and nothing else.  systemd's
`RuntimeDirectory` and `RuntimeDirectoryMode`, plus group ownership,
decide who can open the socket; anyone who can open it may issue
whatever the daemon's configuration allows.  No peer credentials, no
tokens.

This suits a single-operator machine and keeps the daemon free of an
authentication layer it would otherwise have to carry on every platform.
The cost is that the daemon cannot tell two connected clients apart, so
the audit trail records what was done but not by whom.

The nonce on dangerous commands stays, but it guards against accidents
rather than against people: it stops a fat-finger or a stray script, not
someone who already has socket access.

The TUI's raw SCPI console deliberately bypasses the dialect table, so
it is its own capability, `allow_raw`, off by default.  The daemon still
classifies the command prefix where it recognizes it, and logs every raw
command unconditionally.

Requests are serviced between commands, never mid-command, because a
partially read response desynchronizes the prompt framing.  Worst-case
control latency is therefore about one status screen read, roughly one
second.  If that matters, the fast tier keeps running and the status
tier is deferred while the request queue is non-empty.

Two consequences worth building in:

- **Force-refresh after control.**  A state-changing command should not
  wait up to 60 s to appear in snapshots.  After a successful control
  operation the `DeviceTask` immediately re-polls the affected tier;
  `:SYNC:HOLD:INIT` triggers a fast-tier and holdover refresh.
- **Audit trail beside the telemetry.**  Every non-scheduled command is
  recorded: timestamp, command text, response, classification, and an
  optional client-supplied label, which is untrusted and best-effort.  Being able to ask "what did I do to it, and what did
  EFC do afterwards" against a single database is the strongest argument
  for routing commands through the daemon rather than letting clients
  open the port.

### Disconnection is a first-class state

USB serial adapters drop, and the receiver may be power-cycled.  The
daemon reconnects rather than exiting, and records the gap.  `Snapshot`
carries an explicit freshness state so a client shows "disconnected"
rather than freezing on stale values.  For a monitoring tool aimed at a
suspect unit, silently displaying old numbers is the worst failure mode.

Use a `/dev/serial/by-id/...` path rather than `/dev/ttyUSB0`, which is
not stable across re-enumeration.

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

Tiers, to be measured against hardware before being fixed:

| Tier  | Contents                                                    |
| ----- | ----------------------------------------------------------- |
| ~1 s  | `:SYNC:TINT?`, `:SYNC:TFOM?`, `:SYNC:FFOM?`, `:DIAG:ROSC:EFC:REL?`, `:STAT:OPER:HARD:COND?`, `:SYNC:STATE?` |
| ~10 s | `:SYST:STAT?` (satellite table, health line), holdover duration and uncertainty |
| ~60 s | position, leap second state, lifetime count                 |
| event | `:DIAG:LOG:COUNT?` change -> `:DIAG:LOG:READ:ALL?`           |

A control request arriving on the socket must be able to preempt a
scheduled status screen read.

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

- `Type=notify`, signalling ready once the port is open and `*IDN?`
  has answered.
- `Restart=always` with a backoff.
- `BindsTo=` / `After=` the `.device` unit for the serial adapter, so a
  USB re-enumeration restarts the daemon cleanly.
- `StateDirectory=smartclockd` for the database,
  `RuntimeDirectory=smartclockd` for the socket.
- Dedicated user with `SupplementaryGroups=dialout` for port access;
  `RuntimeDirectoryMode` and socket group ownership set so the TUI runs
  unprivileged.
- Standard hardening: `ProtectSystem=strict`, `PrivateTmp`,
  `NoNewPrivileges`.

A `systemctl --user` unit is the simpler alternative if system-wide
installation proves annoying, at the cost of not starting until login.

With `interprocess` handling the IPC and no peer-credential code to
port, systemd is the only Linux-specific piece left in the daemon;
`serialport` and `rusqlite` are both portable.
Porting therefore means writing a launchd plist or a Windows service
wrapper and replacing `Type=notify`, not touching the protocol.

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

Two things fall out of having it: a test that every command reachable on
the active dialect has both a parser and a fixture, and the phase 7
per-model command matrix generated from the same source rather than
maintained by hand.

### CI is a Makefile

The Makefile holds the real targets; GitHub Actions only calls `make
ci`.  Local and CI then run the same thing by construction rather than
by two definitions kept in sync by hand.

```
make            build
make ci         fmt-check clippy test
make fmt        cargo fmt
make clippy     cargo clippy --all-targets -- -D warnings
make test       cargo test
make test-hw    cargo test -- --ignored
```

A `rust-toolchain.toml` pins the toolchain so a clippy lint that fires
in CI fires locally too.  The workflow is checkout,
`dtolnay/rust-toolchain` pinned to a commit sha, `Swatinem/rust-cache`,
then `make ci` -- no installer piped from a URL.

The one wrinkle is that some tests need the receiver.  Those are
`#[ignore]`d and reachable only through `make test-hw`, which CI never
runs, because they need hardware CI does not have and a daemon that must
be stopped first.  Everything else -- parsers, the status screen
scraper, session framing against `ReplayTransport` -- runs anywhere.
Once the phase 4 simulator exists, integration tests move back into
`make test` by talking to a PTY instead of a receiver, which is most of
the reason the simulator is worth building.

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

If a variant's tree cannot be pinned down from the manuals, the fallback
is the firmware in `third_party/`, or reading the
unit's EEPROM.

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

The development unit is in holdover with its antenna connected and may
be failing.  An OCXO aged past the range its EFC DAC can pull presents
exactly this way, and the receiver reports it directly.

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

## Phases

| # | Deliverable |
| - | ----------- |
| 0 | Workspace scaffold.  Rewrite `CLAUDE.md`, which still says "asl-dmr-bridge".  Transcribe chapter 5 of `097-59551-02` into the TOML command table -- id, class, citation, per-dialect command string, response parser, per-model availability -- and write the `build.rs` codegen for it.  Extract sample status screens into `tests/fixtures/`.  Makefile with a `ci` target, a pinned `rust-toolchain.toml`, and a GitHub Actions workflow that calls it. |
| 1 | `transport` + `session` + `smartclock-cli capture`, direct to device.  Confirm the documented tree against the live unit and record transcripts.  Verification, not discovery. |
| 2 | `dialect`, `types`, `parse`, status screen scraper, driven by the fixtures.  `smartclock-cli diagnose`: holdover reason, hardware condition bits, EFC, holdover duration and present uncertainty, tracked count, full log. |
| 3 | `Device`, `Snapshot`, `DeviceTask` with the tiered scheduler and request queue.  `smartclockd` with the SQLite logger, socket protocol, reconnection handling, and a systemd unit.  Queries only over the socket; control lands in phase 6.  Logging starts here and runs from here on. |
| 4 | `smartclock-sim`: PTY-backed emulator replaying recorded state, so client work needs no hardware and no daemon contention. |
| 5 | `smartclockmon`: socket client for live state, read-only SQLite for trends.  Panes for synchronization, acquisition and satellite table, health and EFC trend, position, log, raw SCPI console. |
| 6 | Control operations behind an explicit `Control` handle, proxied over the socket: holdover initiate and recover, survey, antenna delay, elevation mask, preset.  Command classification in the dialect table, socket-permission authorization, nonce confirmation for dangerous commands, audit rows, and force-refresh after a successful control operation.  `:SYSTem:COMMunicate:*` gated hard -- baud changes persist across power cycles and will strand the link. |
| 7 | Documentation: per-model command matrix generated from the command table, wire protocol notes, socket protocol, deployment. |

Suggest a commit at each phase boundary.

## Open questions

1. Which other SmartClock variants are on hand, so their dialects can be
   entered from the manuals rather than discovered later.
