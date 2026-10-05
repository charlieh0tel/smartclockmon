# Running the daemon

## Installing

The daemon is a template unit, `smartclockd@.service`, one instance
per serial port, named by the port as `serial-getty@` is:
`smartclockd@ttyUSB0` opens `/dev/ttyUSB0`.  Instances are named for
ports and logs for receivers, since receivers move between ports.

    sudo dpkg -i smartclockmon_0.1.0-1_amd64.deb
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

Check the install:

    systemctl status smartclockd@ttyUSB0
    smartclockmon --socket /run/smartclockd/ttyUSB0/socket
    sudo -u smartclockd ls /var/lib/smartclockd/
    sudo -u smartclockd sqlite3 /var/lib/smartclockd/<model>-<serial>.sqlite \
        "select count(*), max(at) from snapshot;"

The log file, named after the receiver, appears once the receiver
has answered `*IDN?`.

### Upgrading

Install the new package over the old one and restart each instance.
On opening a log written by an earlier version, the daemon upgrades it
to its own schema and stamps it with that schema.

This is irreversible.  A daemon refuses to write a log stamped with a
newer schema than its own, so once a newer daemon has opened a log,
an older one will not.  Copy the log first if you may need to go
back.  The readers, `smartclock-web` and `smartclockmon`, open a log
of any schema.

    sudo systemctl stop smartclockd@ttyUSB0
    sudo -u smartclockd cp /var/lib/smartclockd/<model>-<serial>.sqlite \
        /var/lib/smartclockd/<model>-<serial>.sqlite.bak

## What the daemon does to the receiver

It reads and, with one opt-in exception, changes nothing, apart from
the garbled bytes a probe at the wrong line settings sends.  Note
three things:

It drains the receiver's error queue into the journal.  Reading an
entry removes it, but the queue holds thirty and discards the newest
on overflow, so an unread queue loses errors anyway.

When it found the receiver only by probing other line settings, it
first empties the error queue, where the probe's garbled bytes sit as
errors, and logs to the journal how many entries it read.

It copies the receiver's diagnostic log out, entry by entry, and
optionally clears it; see `SMARTCLOCKD_ADOPT_LOG` below.

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
`:DIAGnostic:ERASe`, or a `:SYSTem:LANGuage` setting.  The one
exception is `read-memory`, with `read-flash` and `read-eeprom`, which
enters the debug console with `:SYSTem:LANGuage "PFORTH"` and returns
with the console's `halt`, or failing that through the installer with
`:SYSTem:LANGuage "PRIMARY"` (`firmware.md`, "Reading memory through
it").

## Where things live

| Path                                             | What                       |
| ------------------------------------------------ | -------------------------- |
| `/etc/default/smartclockd`                       | settings for every instance |
| `/etc/systemd/system/smartclockd@<port>.service.d/` | one instance's settings |
| `/var/lib/smartclockd/<model>-<serial>.sqlite`   | one log per receiver       |
| `/run/smartclockd/<port>/socket`                 | where clients connect      |

A log is named after the receiver that answered on the port and is
opened only once one has: a unit moved to another port or another
host's adapter keeps one continuous history, and a different unit on
the same port gets its own file.  Set `SMARTCLOCKD_DATABASE` to a file
to log every receiver seen on the port into that one file instead.

Every row names its receiver.  A `receiver` table holds one row per
unit that has written to the file, keyed on the serial from `*IDN?` --
the serial alone, since a firmware upgrade is not a different
instrument -- and the snapshots, satellites, errors, diagnostic log
entries and audit trail all carry its id, so a file that has logged
two units keeps their rows apart.

A log grows without bound, a few MB a day.  Nothing rotates it, so
last year's holdover events stay in the record.

## Notes and facts

What the receiver cannot report -- a new amplifier, a moved antenna,
the oscillator's serial -- goes into its log through the daemon:

    smartclock-cli --socket /run/smartclockd/ttyUSB0/socket note added a 20 dB LNA
    smartclock-cli --socket ... note --at 2026-09-25T14:00:00-07:00 ran master_reset
    smartclock-cli --socket ... fact ocxo.model 10811-60159

Both are filed under the receiver attached now; nothing is sent to it.
Notes show in the journal of the web view and the monitor, and as
dashed lines across the web view's charts; current facts head
`diagnose` and sit under the web view's history.
A fact keeps its history, so a replaced part's old value stays, and
each one also leaves a note.  Rows are not edited through the daemon;
fix a mistake with `sqlite3` on the log.

## More than one receiver

A second port is a second instance:

    sudo systemctl enable --now smartclockd@ttyUSB1

    smartclockmon --socket /run/smartclockd/ttyUSB1/socket

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
