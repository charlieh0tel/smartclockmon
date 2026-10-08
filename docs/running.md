# Running the daemon

## Installing

The daemon is a template unit, `smartclockd@.service`, one instance
per serial port, named by the port as `serial-getty@` is:
`smartclockd@ttyUSB0` opens `/dev/ttyUSB0`.  Instances are named for
ports and logs for receivers, since receivers move between ports.

    sudo apt install ./smartclockmon_<version>_amd64.deb
    sudo systemctl enable --now smartclockd@ttyUSB0
    sudo usermod -aG smartclockd $USER        # then log in again

The package puts the binaries in `/usr/bin`, creates a `smartclockd`
system user in `dialout`, and enables nothing: you name the port.

`ttyUSB0` moves when another adapter is plugged in.  For a stable
name, either escape the by-id path into the instance name:

    sudo systemctl enable --now \
        "smartclockd@$(systemd-escape --path /dev/serial/by-id/usb-...-port0 | sed 's,^dev-,,')"

or give the adapter a short name with a udev rule and use that:

    # /etc/udev/rules.d/70-smartclock.rules
    SUBSYSTEM=="tty", ENV{ID_SERIAL}=="<from udevadm info>", SYMLINK+="smartclock/bench"

    sudo systemctl enable --now smartclockd@smartclock-bench   # /dev/smartclock/bench

The instance name is a path under `/dev` with `/` written as `-`;
`systemd-escape` does the rest.

The socket is mode 0660, group `smartclockd`, so only members of that
group reach the daemon with `smartclockmon` and `smartclock-cli`.
Group membership is the entire authorization model: anyone who can
open the socket may issue whatever the daemon is configured to allow.

`--listen HOST:PORT` (`SMARTCLOCKD_LISTEN`) has the daemon listen on
TCP as well, for clients on other hosts, which name it
`--daemon tcp://HOST:PORT`.  A client's `--daemon` takes the socket's
path or a `tcp://` address.  Nothing decides who may connect over TCP:
anyone who can reach the port may issue whatever the daemon allows.
The port is the daemon's alone, so give each instance its own in a
drop-in.  `smartclock-sensord` takes the same flag,
`SMARTCLOCK_SENSORD_LISTEN`.

Check the install:

    systemctl status smartclockd@ttyUSB0
    smartclockmon --daemon /run/smartclockd/ttyUSB0/socket
    sudo -u smartclockd ls /var/lib/smartclockd/
    sudo -u smartclockd sqlite3 /var/lib/smartclockd/<model>-<serial>.sqlite \
        "select count(*), max(at) from snapshot;"

The log file, named after the receiver, appears once the receiver
has answered `*IDN?`.

Room temperature and the like come from a service of their own,
`smartclock-sensord`, one per host and also not enabled.  Name the
sensors in `/etc/default/smartclock-sensord` first, then
`sudo systemctl enable --now smartclock-sensord`, and check it with
`smartclock-cli sensors`; `docs/sensors.md` has the details.

### Upgrading

On opening a log written by an earlier version, the daemon upgrades it
to its own schema and stamps it with that schema.  This is
irreversible: a daemon refuses a log stamped with a newer schema than
its own, exiting with status 2, which the unit does not restart.  So
if you may need to go back, copy each log before the new daemon first
opens it:

    sudo systemctl stop smartclockd@ttyUSB0
    sudo -u smartclockd cp /var/lib/smartclockd/<model>-<serial>.sqlite \
        /var/lib/smartclockd/<model>-<serial>.sqlite.bak

Then install the new package and restart each instance.  The readers,
`smartclock-web` and `smartclockmon`, open a log of any schema since
12, the first with integer timestamps; an older one they
refuse, saying which service to start once to convert it.  Converting
takes a few seconds per log, during which the receiver is not read, and
leaves the file smaller.  The same holds for the sensor log and
`smartclock-sensord`, whose first schema with integer timestamps is 2.

## What the daemon does to the receiver

It reads and, with one opt-in exception, changes nothing, apart from
the garbled bytes a probe at the wrong line settings sends.  Note
these:

It drains the receiver's error queue into the journal.  Reading an
entry removes it, but the queue holds thirty and discards the newest
on overflow, so an unread queue loses errors anyway.

When it found the receiver only by probing other line settings, it
first empties the error queue, where the probe's garbled bytes sit as
errors, and logs to the journal how many entries it read.

It copies the receiver's diagnostic log out, entry by entry, and
optionally clears it; see `SMARTCLOCKD_ADOPT_LOG` below.
`:DIAG:LOG:READ:ALL?` returns about 56 KB, half a minute of wire time,
so each pass reads up to sixteen entries with `:DIAG:LOG:READ? <n>`,
new ones first, then backwards.  The receiver's stamps are kept as
written but do not order entries: after a power cycle its clock runs
from a stale midnight until first lock.  An entry's `at`, host UTC when
it was read, is the best time it has.  Clearing the log restarts its
numbering, so entries are ordered by `(generation, entry)`, where
`generation` counts clears.  A count below the highest entry held is a
clear; so is any difference when three held entries are re-read, once
per attachment, which catches a log cleared and refilled while the
daemon was away.

It reads the status screen every `SMARTCLOCKD_SKY` seconds, 300 by
default, for the satellite table only the screen carries.  A read holds
the link about 1.5 s, leaving a gap in the once-a-second readings; 0
reads it only when a client asks.

It does not read the event registers, so it leaves the front-panel
Alarm LED and the BITE output alone, and reads one for a client only
when started with `--allow-control`.  Reading an event register clears
it, which clears the alarm that summarizes it.  The daemon watches the
same state through `*STB?`, which changes nothing, so the alarm stays
lit until cleared at the instrument.  The monitor's header and the
browser's status strip show what the daemon recorded, so clearing the
lamp loses no history.

## Configuring

The unit derives the device, `/dev/<port>`, and the socket,
`/run/smartclockd/<port>/socket`, from the instance name.  Everything
else has a default.  Each `SMARTCLOCKD_*` variable matches the command
line option of the same name; `smartclockd --help` documents them.

A setting for every instance goes in `/etc/default/smartclockd`, a
conffile the package ships with every line commented out.  A setting
for one instance goes in its drop-in:

    sudo systemctl edit smartclockd@ttyUSB0

    [Service]
    Environment=SMARTCLOCKD_ALLOW_RAW=true

A drop-in's `Environment=` overrides the unit's own, so it can also
replace the device -- `SMARTCLOCKD_DEVICE=tcp://127.0.0.1:5025` for
the simulator -- but not the file: systemd applies `EnvironmentFile=`
over every `Environment=` wherever it is written.  Make each setting
in one place, never both.  Restart after a change; both are read only
at startup.

`smartclockd` has no default device: run by hand, it needs `--device`,
since a wrong default would send SCPI to whatever else is on that
path.  The unit supplies `/dev/<port>` from the instance name.  A
by-id path from `ls -l /dev/serial/by-id/` stays put when another
adapter is plugged in; `/dev/ttyUSB0` does not.  `tcp://host:port`
also works, for a serial-to-network adapter or the simulator.

Booleans are enabled with `true`; an empty value is refused, not
read as off.

`SMARTCLOCKD_ADOPT_LOG`, off by default, is the one setting that
changes the receiver rather than the daemon.  The receiver's
diagnostic log holds 222 entries and then stops recording, so a
long-running unit has usually stopped logging.  This setting erases
that log once every entry is copied here and the receiver reports it
nearly full, which restarts recording.  The erase is irreversible, and
on a unit under investigation that log is evidence.  Nothing is erased
unless the copy is complete and gap-free, and the command carries the
entry count, so the receiver refuses if an entry arrived in between.

The receiver does not signal a full log.  "Log Almost Full" is bit 6
of the operation group, and the factory default for
`:STATus:OPERation:ENABle` is 36 -- bits 2 and 5, Holdover Summary and
Hardware Summary (`097-59551-02` 5-88) -- so the condition never
reaches the alarm.  The bench 58503A's log filled in March 2025 and
recorded nothing for eighteen months.  `smartclockd` reads the
condition register directly, where the enable mask does not apply, and
reports it.

A configuration mistake fails the unit instead of looping.  systemd
cannot check the file, since `Condition=` and `Assert=` do not see
`EnvironmentFile` variables, so the daemon enforces it by exit code: 2
for anything a retry cannot fix -- an unset device, an impossible
baud, a malformed `ALLOW_` value, a database written by a newer
version -- and `RestartPreventExitStatus=2` stops the restarts.
`systemctl status` then names the fault.

The daemon refuses:

- anything that changes the receiver without `--allow-control`,
  including reading an event register, which clears it, and reading
  the error queue, which takes the entry the daemon's journal would
  have kept;
- what can strand the link without `--allow-dangerous`;
- commands the table does not know without `--allow-raw`.

All three are off by default, and the log records every command that
is not a scheduled poll.

`smartclock-cli` talking to the receiver directly has no such flags.
Before it opens the port, it refuses to send `:SYSTem:PRESet`, the
undocumented `:SYSTem:PON`, anything under `:SYSTem:COMMunicate`,
`:DIAGnostic:ERASe`, or a `:SYSTem:LANGuage` setting.  There are two
exceptions.  `flash` sends `:SYSTem:LANGuage "INSTALL"` and
`:DIAGnostic:ERASe` to install firmware, after its image and receiver
checks ([the flasher](firmware/restart.md#the-flasher)).
`read-memory`, with `read-flash` and `read-eeprom`, enters the debug console with `:SYSTem:LANGuage "PFORTH"` and returns
with the console's `halt`, or failing that through the installer with
`:SYSTem:LANGuage "PRIMARY"` (`firmware/console.md`, "Reading memory through
it").

## Where things live

| Path                                             | What                       |
| ------------------------------------------------ | -------------------------- |
| `/etc/default/smartclockd`                       | settings for every instance |
| `/etc/systemd/system/smartclockd@<port>.service.d/` | one instance's settings |
| `/var/lib/smartclockd/<model>-<serial>.sqlite`   | one log per receiver       |
| `/run/smartclockd/<port>/socket`                 | where clients connect      |
| `/etc/default/smartclock-sensord`                | the sensors to log          |
| `/var/lib/smartclock-sensord/sensors.sqlite`     | the host's sensor log       |
| `/run/smartclock-sensord/socket`                 | the sensors' latest readings |

A log is named after the receiver that answered on the port and is
opened only once one has: a unit moved to another port or another
host's adapter keeps one continuous history, and a different unit on
the same port gets its own file.  Set `SMARTCLOCKD_DATABASE` to a file
to log every receiver seen on the port into that one file instead.

Every row names its receiver.  A `receiver` table holds one row per
unit that has written to the file, keyed on the serial from `*IDN?` --
the serial alone, since a firmware upgrade is not a different
instrument -- and the snapshots, alarm changes, transition filters,
errors, diagnostic log entries, audit trail, notes and facts all carry
its id, satellites through their snapshot, so a file that has logged
two units keeps their rows apart.

A log grows without bound, a few MB a day.  Nothing rotates it, so
last year's holdover events stay in the record.

## Notes and facts

What the receiver cannot report -- a new amplifier, a moved antenna,
the oscillator's serial -- goes into its log through the daemon:

    smartclock-cli --daemon /run/smartclockd/ttyUSB0/socket note added a 20 dB LNA
    smartclock-cli --daemon ... note --at 2026-09-25T14:00:00-07:00 ran master_reset
    smartclock-cli --daemon ... fact ocxo.model 10811-60159

Both are filed under the receiver attached now; nothing is sent to it.
Notes show in the journal of the web view and the monitor, and as
dashed lines across the web view's charts, with the text beside the
pointer on a line; clicking a note in the web journal shows an hour
either side of it; current facts head
`diagnose` and sit under the web view's history.
A fact keeps its history, so a replaced part's old value stays, and
each one also leaves a note.  Rows are not edited through the daemon;
fix a mistake with `sqlite3` on the log.  A daemon busy reading the
receiver's log answers "queued": the note is written shortly, so do
not send it again.

## More than one receiver

A second port is a second instance:

    sudo systemctl enable --now smartclockd@ttyUSB1

    smartclockmon --daemon /run/smartclockd/ttyUSB1/socket

A Z3801A's port is fixed at seven data bits and odd parity
(`097-z3801-01` 1-8 and 2-10); the daemon opens 8N1 by default.  A
receiver that does not answer at the configured settings is probed for
at 19200 and 9600, 8N1 and 7O1, and the journal says where it was
found, so no drop-in is needed.  Naming the framing in the instance's
drop-in skips the probe, and the errors it leaves in the receiver's
queue, on every start:

    sudo systemctl edit smartclockd@ttyUSB1

    [Service]
    Environment=SMARTCLOCKD_FRAMING=7O1

`smartclockmon` and `smartclock-cli` take `--framing 7O1` for direct
mode and probe the same way; `smartclock-cli read-memory` (and
`read-flash`, `read-eeprom`) does not probe, since its port is at the
debug console, not at SCPI.

Anything that differs between the ports -- the framing, and whether
to adopt that receiver's log -- goes in the drop-ins, not in
`/etc/default/smartclockd`: a setting in the shared file applies to
every instance and a drop-in cannot override it.

With two adapters, `ttyUSB0` and `ttyUSB1` can swap when either is
replugged or the host reboots, and each instance then opens the other
receiver's port at the other's framing.  It finds the receiver by
probing, and since logs are named for the receiver, not the port, no
history goes astray; but it probes on every start until the ports are
put back.  To keep each instance on its own receiver, give each
adapter a stable name (the udev rule or escaped by-id path under
"Installing") and name the instances after those.

The instances share `/var/lib/smartclockd`.  Each writes the file
named for the receiver on its own port, and a receiver is on one port
at a time, so no two write the same file.  The viewers are host-wide
and need no options: `smartclock-web` reads every log in the directory
and every socket under `/run/smartclockd`, offers every receiver it
finds in its selector, and shows the live strip from whichever daemon
is attached to the one selected -- a receiver with history and no
daemon shows the history and says so in the strip.
`smartclock-exporter` scrapes every daemon into one `/metrics`, each
sample labeled `daemon="<instance>"`, `serial` and `model`.

Both can be given the daemons instead, with `--daemon` repeated or
comma-separated, each `[NAME=]ENDPOINT`: a socket's path or
`tcp://HOST:PORT`, so one collector can ask daemons on other hosts.
The name is the exporter's `daemon` label; an unnamed one goes by its
endpoint as written.

## Unplugging the adapter

The daemon reconnects by itself, so the unit does not bind to a
device unit, which would stop it while an adapter is out.  To bind it
anyway, add a drop-in; do not edit the shipped unit:

    sudo systemctl edit smartclockd@ttyUSB0

    [Unit]
    BindsTo=dev-serial-by\x2did-usb\x2dYOUR_ADAPTER.device
    After=dev-serial-by\x2did-usb\x2dYOUR_ADAPTER.device

`systemd-escape --path --suffix=device /dev/serial/by-id/...` gives the
escaped name.

The daemon notices the cable moved to another receiver, with the
adapter still plugged in, within a minute, when the slow tier asks
`*IDN?` again, and reopens as the new unit.  Readings taken in that
minute are filed under the old one, so stop the daemon before moving
the cable and start it after:

    sudo systemctl stop smartclockd@ttyUSB0
    sudo systemctl start smartclockd@ttyUSB0

## Windows

`make windows` cross-builds everything for `x86_64-pc-windows-gnu`; it
needs that Rust target (`rustup target add x86_64-pc-windows-gnu`) and mingw-w64.
The test suite passes under Wine.  Each release carries a zip of the
Windows programs, unsupported and untried on a receiver.  There is no
installer and no service: the programs run from a console.

Windows has no Unix sockets, so the daemon listens on TCP alone,
`127.0.0.1:9978` unless given `--listen`, and refuses `--socket`.  The
monitor, exporter and web view ask `tcp://127.0.0.1:9978` unless given
`--daemon`; the exporter and web view refuse `--run-dir`.  A second
receiver needs a daemon on a port of its own, and the collectors told
both:

    smartclockd --device COM3 --listen 127.0.0.1:9976
    smartclock-exporter --daemon bench=tcp://127.0.0.1:9978,lab=tcp://127.0.0.1:9976

The sensor service likewise listens on `127.0.0.1:9977`, where its
clients look by default, and refuses `--socket`.  There are no hwmon or
IIO sensors there, so it offers neither.

Ctrl-C or Ctrl-Break stops the daemon; a second exits at once,
without writing what it holds.  Closing its console, logging off or
shutting down gives it four seconds to write it, Windows ending the
process at five.

Logs default to `C:\ProgramData\smartclockmon\log`.  Anything that
can reach a daemon's port may connect to it.
