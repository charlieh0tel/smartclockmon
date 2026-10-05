# Running the daemon

## Installing

The daemon is a template unit, `smartclockd@.service`, one instance
per serial port and named by the port, the way `serial-getty@` is:
`smartclockd@ttyUSB0` opens `/dev/ttyUSB0`.  Instances are named for
ports and logs for receivers, since receivers move between ports.

    sudo dpkg -i smartclockmon_0.1.0-1_amd64.deb
    sudo systemctl enable --now smartclockd@ttyUSB0
    sudo usermod -aG smartclockd $USER        # then log in again

The package puts the binaries in `/usr/bin`, creates a `smartclockd`
system user in `dialout`, and enables nothing: you name the port.

`ttyUSB0` moves when another adapter is plugged in.  For a name that
stays put, either escape the by-id path into the instance name:

    sudo systemctl enable --now \
        "smartclockd@$(systemd-escape --path /dev/serial/by-id/usb-...-port0 | sed 's,^dev-,,')"

or give the adapter a short name with a udev rule and use that:

    # /etc/udev/rules.d/70-smartclock.rules
    SUBSYSTEM=="tty", ENV{ID_SERIAL}=="<from udevadm info>", SYMLINK+="smartclock/bench"

    sudo systemctl enable --now smartclockd@smartclock-bench   # /dev/smartclock/bench

The instance name is a path under `/dev` with `/` written as `-`;
`systemd-escape` does the rest.

The socket is mode 0660 owned by `smartclockd`, so `smartclockmon`
and `smartclock-cli` reach the daemon only for members of that group.
Group membership is the whole of the authorization model: anyone who
can open the socket may issue whatever the daemon is configured to
allow.

Check it took:

    systemctl status smartclockd@ttyUSB0
    smartclockmon --socket /run/smartclockd/ttyUSB0/socket
    sudo -u smartclockd ls /var/lib/smartclockd/
    sudo -u smartclockd sqlite3 /var/lib/smartclockd/<model>-<serial>.sqlite \
        "select count(*), max(at) from snapshot;"

The log file appears once the receiver has answered `*IDN?`, since it
is named after the receiver.

### Upgrading

Install the new package over the old one and restart each instance.
The daemon brings a log written by an earlier version up to its own
schema when it opens it, and stamps the log with that schema.

That cannot be undone.  A daemon refuses a log stamped with a schema
newer than its own rather than write into it, so once a newer daemon
has opened a log, an older one will not.  Copy the log first if you
may need to go back.  The readers, `smartclock-web` and
`smartclockmon`, open a log of any schema.

    sudo systemctl stop smartclockd@ttyUSB0
    sudo -u smartclockd cp /var/lib/smartclockd/<model>-<serial>.sqlite \
        /var/lib/smartclockd/<model>-<serial>.sqlite.bak

## What the daemon does to the receiver

It reads, and with one opt-in exception changes nothing, apart from
the garbled bytes a probe at the wrong line settings sends.  Three
things it does or does not read are worth knowing.

It drains the receiver's error queue.  Reading an entry removes it,
but the queue holds thirty and discards the newest when it overflows,
so an unread queue loses errors anyway.  The entries go to the journal.

When it found the receiver only by probing other line settings, it
reads the error queue empty before asking anything else, since the
probe's garbled bytes are queued there as errors, and says in the
journal how many it read.

It copies the receiver's diagnostic log out, entry by entry, and
optionally clears it; see `SMARTCLOCKD_ADOPT_LOG` below.

It does not read the event registers, and so does not touch the
front-panel Alarm LED or the BITE output, nor will it read one for a
client unless started with `--allow-control`.  Reading an event
register clears it, which clears the alarm that summarizes it.  The
daemon watches the same state through `*STB?`, which changes nothing,
so the alarm stays lit until it is cleared at the instrument.  What the
daemon saw is recorded and shown in the monitor's header and the
browser's status strip, so clearing the lamp loses no history.

## Configuring

The unit derives two things from the instance name: the device,
`/dev/<port>`, and the socket, `/run/smartclockd/<port>/socket`.
Everything else has a default.  Each `SMARTCLOCKD_*` variable matches
the command line option of the same name, and `smartclockd --help`
documents them.

A setting for every instance goes in `/etc/default/smartclockd`, a
conffile the package ships with every line commented out.  A setting
for one instance goes in its drop-in:

    sudo systemctl edit smartclockd@ttyUSB0

    [Service]
    Environment=SMARTCLOCKD_ALLOW_RAW=true

A drop-in's `Environment=` beats the unit's own, so it can also
replace the device -- `SMARTCLOCKD_DEVICE=tcp://127.0.0.1:5025` for
the simulator -- but it cannot beat the file: systemd applies
`EnvironmentFile=` over every `Environment=` wherever it is written.
So make a setting in one place or the other, never both.  Restart
after a change; both are read only at startup.

`smartclockd` itself has no default device: run by hand, it needs
`--device`, since a wrong default would open whatever else is on that
path and send SCPI at it.  The unit supplies `/dev/<port>` from the
instance name.  A by-id path from `ls -l /dev/serial/by-id/` does not
move when another adapter is plugged in, as `/dev/ttyUSB0` does.  The
form `tcp://host:port` also works, for a serial-to-network adapter or
the simulator.

Booleans are enabled with `true`; an empty value is refused rather than
read as off.

`SMARTCLOCKD_ADOPT_LOG` is the one setting that changes the receiver
rather than the daemon, and it is off by default.  The receiver's own
diagnostic log holds 222 entries and then stops recording, so a unit
that has been running a long time has usually stopped logging; turning
this on erases that log once every entry of it is safely copied here
and the receiver reports it nearly full, which gets it recording again.
It is irreversible, and on a unit somebody is investigating that log is
evidence.  Nothing is erased unless the copy is complete and gap-free,
and the entry count is sent with the command so the receiver refuses if
an entry arrived in between.

The receiver does not signal a full log.  "Log Almost Full" is bit 6
of the operation group, and the factory default for
`:STATus:OPERation:ENABle` is 36 -- bits 2 and 5, Holdover Summary and
Hardware Summary (`097-59551-02` 5-88).  Bit 6 is not among them, so
the condition never reaches the alarm.  The bench 58503A's log filled
in March 2025 and recorded nothing for the next eighteen months.
`smartclockd` reads the condition register directly, where the enable
mask does not apply, and reports it.

A configuration mistake fails the unit instead of looping.  systemd
cannot check the file itself, since `Condition=` and `Assert=` do not
see `EnvironmentFile` variables, so the enforcement is by exit code: the
daemon exits 2 for anything a retry cannot fix -- an unset device, an
impossible baud, a malformed `ALLOW_` value, a database written by a
newer version -- and `RestartPreventExitStatus=2` makes that final.
`systemctl status` then names what is wrong.

The daemon refuses anything that changes the receiver unless started
with `--allow-control` -- which includes reading an event register,
since that clears it, and reading the error queue, since that takes
the entry the daemon's journal would have kept -- refuses what can
strand the link without `--allow-dangerous`, and refuses commands the
table does not know without `--allow-raw`.  All three are off by
default, and every command that is not a scheduled poll is recorded in
the log.

`smartclock-cli` talking to the receiver directly has no such flags,
and refuses, before it opens the port, to send `:SYSTem:PRESet`, the
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

A log is named after the receiver that answered on the port, and is
opened only once one has: a unit moved to another port or another
host's adapter keeps one continuous history, and a different unit
plugged into the same port gets a file of its own.  Set
`SMARTCLOCKD_DATABASE` to a file to log every receiver seen on the
port into that one file instead.

Every row says which receiver it came from.  A `receiver` table holds
one row per unit that has written to the file, keyed on the serial from
`*IDN?` -- the serial alone, because firmware changes under it and an
upgrade is not a different instrument -- and the snapshots, satellites,
errors, diagnostic log entries and audit trail all carry its id, so
a file that has logged two units keeps their rows apart.

A log grows without bound, a few MB a day.  Nothing rotates it, so
last year's holdover events stay in the record.

## More than one receiver

A second port is a second instance:

    sudo systemctl enable --now smartclockd@ttyUSB1

    smartclockmon --socket /run/smartclockd/ttyUSB1/socket

A Z3801A's port is fixed at seven data bits and odd parity
(`097-z3801-01` 1-8 and 2-10), where the daemon opens 8N1 unless told
otherwise.  A receiver that does not answer at the configured settings
is looked for at 19200 and 9600, 8N1 and 7O1, and the journal says
where it was found, so no drop-in is needed.  Naming the framing in
the instance's drop-in skips the probe, and the errors it leaves in
the receiver's queue, on every start:

    sudo systemctl edit smartclockd@ttyUSB1

    [Service]
    Environment=SMARTCLOCKD_FRAMING=7O1

`smartclockmon` and `smartclock-cli` take `--framing 7O1` for direct
mode the same way, and probe the same way; `smartclock-cli read-memory`
(and `read-flash`, `read-eeprom`) does not, since its port is at the debug console, not at SCPI.

Anything that differs between the ports -- the framing, and whether
to adopt that receiver's log -- goes in the drop-ins, not in
`/etc/default/smartclockd`: a setting in the shared file applies to
every instance and a drop-in cannot override it.

With two adapters, `ttyUSB0` and `ttyUSB1` can swap when either is
replugged or the host reboots, and each instance then opens the other
receiver's port at the other's framing.  It finds the receiver by
probing, and since logs are named for the receiver, not the port, no
history goes astray; but it probes on every start until the ports are
put back.  Giving each adapter a stable name (see "Installing" for the
udev rule or the escaped by-id path) and naming the instances after
those keeps each instance on its own receiver.

The instances share `/var/lib/smartclockd`, and since each writes the
file named for the receiver on its own port, and a receiver is on one
port at a time, they never write the same file.  The viewers are
host-wide and need no telling: `smartclock-web` reads every log in
the directory and every socket under `/run/smartclockd`, offers every
receiver it finds in its selector, and shows the live strip from
whichever daemon is attached to the one selected -- a receiver with
history and no daemon shows the history and says so in the strip.
`smartclock-exporter` scrapes every daemon into one `/metrics`, each
sample labeled `daemon="<instance>"`, `serial` and `model`.

## Unplugging the adapter

The daemon reconnects by itself, so the unit does not bind to a
device unit, which would stop it while an adapter is out.  To bind it
anyway, add a drop-in rather than editing the shipped unit:

    sudo systemctl edit smartclockd@ttyUSB0

    [Unit]
    BindsTo=dev-serial-by\x2did-usb\x2dYOUR_ADAPTER.device
    After=dev-serial-by\x2did-usb\x2dYOUR_ADAPTER.device

`systemd-escape --path --suffix=device /dev/serial/by-id/...` gives the
escaped name.

Moving the cable to another receiver without unplugging the adapter is
noticed within a minute, when the slow tier asks `*IDN?` again, and the
daemon reopens as the new unit.  Readings taken in that minute are
filed under the old one, so stop the daemon first and start it again
once the cable is moved:

    sudo systemctl stop smartclockd@ttyUSB0
    sudo systemctl start smartclockd@ttyUSB0
