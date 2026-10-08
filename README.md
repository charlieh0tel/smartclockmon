# smartclockmon

A Rust library, logging daemon, terminal monitor and browser view for
HP / Symmetricom SmartClock GPS time and frequency receivers, over
RS-232.

![The live page: the 1 PPS interval, EFC and satellites of a locked Z3805A](docs/images/web-live.png)

| | |
| - | - |
| ![The status page: the receiver's screen beside a sky plot](docs/images/web-status.png) | ![The stability page: MDEV and ADEV with one-sigma bands](docs/images/web-stability.png) |
| ![The terminal monitor's dashboard](docs/images/tui-dashboard.png) | The status page and the monitor are shown against the simulator. |

## Status

In daily use, logging continuously.  [`PLAN.md`](PLAN.md) holds the
design, open questions and known defects.

## Parts

| | |
| - | - |
| `smartclock` | the library: transports, SCPI framing, the command table, parsers, the status screen scraper, the polling task, and the Allan deviation |
| `smartclockd` | holds the serial port, logs to SQLite, serves clients over a Unix socket and, if asked, TCP |
| `smartclockmon` | terminal monitor: dashboard, history graphs, journal, status screen and stability |
| `smartclock-cli` | queries, `diagnose`, notes and facts, the host's sensors, transcripts, sweeps for undocumented commands, ROM and EEPROM reads, and firmware loading ([notes](docs/firmware/restart.md#the-flasher)) |
| `smartclock-exporter` | Prometheus metrics for every receiver, from the daemons' readings |
| `smartclock-web` | browser view: live state, zoomable history, the status screen, stability, and receivers compared |
| `smartclock-sensord` | logs room temperature, humidity and pressure from hwmon, IIO and TEMPer USB sticks ([design](docs/sensors.md)) |
| `smartclock-log` | the log's schema and the readers the web view and monitor share |
| `smartclock-http` | the small HTTP server the exporter and web view share |
| `smartclock-sim` | a simulated receiver, in process for tests and over TCP for the real daemon |

## Running it

Releases carry Debian packages for amd64 and arm64, also in the APT
repository, and an unsupported Windows zip.  Build with `make`, or
`make deb` for a package.  The package runs one daemon per serial
port, named by the port: `systemctl enable --now smartclockd@ttyUSB0`.
[`docs/running.md`](docs/running.md) covers installing, settings,
remote clients and Windows.

Run by hand, the daemon holds the port and everything else is its
client:

    smartclockd --device /dev/serial/by-id/usb-... \
                --log-dir . \
                --socket /tmp/smartclockd.sock

    smartclockmon --daemon /tmp/smartclockd.sock
    smartclock-web --daemon /tmp/smartclockd.sock --log-dir .        # http://127.0.0.1:9980/
    smartclock-exporter --daemon /tmp/smartclockd.sock               # http://127.0.0.1:9979/metrics

Installed, the web view and exporter need no options: they find every
daemon under `/run/smartclockd` and every log under
`/var/lib/smartclockd`.  A client on another host names a daemon
started with `--listen HOST:PORT` as `--daemon tcp://HOST:PORT`.
[`docs/views.md`](docs/views.md) says what each view shows and why.

Without a receiver, use the simulator; every tool takes
`tcp://host:port` for a device path:

    smartclock-sim 127.0.0.1:5025            # --model z3801a --no-echo for the Z3801A's framing
    smartclockd     --device tcp://127.0.0.1:5025 ...
    smartclock-cli  --device tcp://127.0.0.1:5025 diagnose

`make ci` runs CI's checks; `make test-web` runs the browser tests,
after `make web-deps` once.

## Hardware

HP / Agilent / Symmetricom SmartClock receivers: GPS-disciplined OCXO
references with 10 MHz and 1 PPS outputs, reporting over a serial port
in SCPI.

| Model  | Command tree                 | Tested on hardware | Notes |
| ------ | ---------------------------- | ------------------ | ----- |
| 58503A | `:GPS:`, `:SYNC:`            | yes | Primary development target |
| Z3801A | `:PTIME:GPSYSTEM:`, `:ROSC:` | yes | Divergent tree; different response formats |
| Z3805A | `:PTIME:GPSYSTEM:`, `:ROSC:` | yes | Answers the Z3801A tree |
| 58503B | `:GPS:`, `:SYNC:`            | no; may work | Same tree as the 58503A by its manual |
| 59551A | `:GPS:`, `:SYNC:`            | no; may work | The 58503A tree; its pulse output and event timestamping are unused |
| Z3816A | `:PTIME:GPSYSTEM:`, `:ROSC:` | no; may work | Firmware image studied; assumed to answer as the Z3801A |

Their mid-1990s Motorola GPS engines predate the week rollovers of
1999 and 2019, so a unit reports a date 1024 weeks in the past.  Time
of day, 1 PPS and 10 MHz are unaffected; the tools correct the date.
Factory serial settings are 9600 8N1; the Z3801A's port is fixed at
19200 7O1.  When the configured settings get no answer, the daemon,
monitor and CLI probe 19200 and 9600 at 8N1 and 7O1.  [`docs/bench.md`](docs/bench.md)
describes the bench.

## Documentation

| File | Contents |
| ---- | -------- |
| [`docs/running.md`](docs/running.md) | installing, configuring, remote clients, Windows, and what the daemon does to the receiver |
| [`docs/views.md`](docs/views.md) | what the monitor, browser pages and exporter show, and why |
| [`docs/stability.md`](docs/stability.md) | how the stability curves are computed, checked and measured |
| [`docs/protocol.md`](docs/protocol.md) | how the receivers behave on the wire |
| [`docs/commands.md`](docs/commands.md) | the command table: each tree's commands, how far each is confirmed, and those in no manual; generated by `make docs` |
| [`docs/efc.md`](docs/efc.md) | how the receiver reports its control voltage, measured at the oscillator's EFC pin |
| [`docs/ocxo.md`](docs/ocxo.md) | the oscillator |
| [`docs/sensors.md`](docs/sensors.md) | logging the host's sensors beside the receivers |
| [`docs/firmware/`](docs/firmware/) | the firmware: the 1 PPS measurement, the disciplining loop, the GPS engine interface and the pForth console |
| [`docs/loop.html`](https://htmlpreview.github.io/?https://github.com/charlieh0tel/smartclockmon/blob/main/docs/loop.html) | the disciplining loop as a block diagram, with its update law, constants and closed-loop poles |
| [`docs/hardware-investigations.md`](docs/hardware-investigations.md) | what only a bench can settle |
| [`docs/z3801-keywords.md`](docs/z3801-keywords.md), [`docs/z3801-tree.md`](docs/z3801-tree.md), [`docs/58503a-tree.md`](docs/58503a-tree.md) | SCPI keywords and command paths read from the firmware |
| [`docs/screen-format-strings.md`](docs/screen-format-strings.md) | the status screen's printf templates |

Vendor manuals are in `third_party/`; `097-59551-02` (59551A/58503A)
and `097-z3801-01` (Z3801A) are the primary references, and
[`third_party/NOTICE`](third_party/NOTICE) lists the rest.

## License

Copyright © 2026 Christopher Hoover.  GPL-3.0-or-later.  See
[`LICENSE`](LICENSE).

The license does not cover `third_party/`; see
[`third_party/NOTICE`](third_party/NOTICE).
