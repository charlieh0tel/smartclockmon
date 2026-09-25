# Running the daemon

## Installing

The daemon is a template unit, `smartclockd@.service`: one instance
per serial port, named by you, even where there is one port.  The
name is the port's, not the receiver's, since receivers move between
ports and the log follows the receiver.

    sudo dpkg -i smartclockmon_0.1.0-1_amd64.deb
    sudo cp /usr/share/doc/smartclockmon/examples/smartclockd.instance \
        /etc/default/smartclockd.bench
    sudoedit /etc/default/smartclockd.bench   # set SMARTCLOCKD_DEVICE
    sudo systemctl enable --now smartclockd@bench
    sudo usermod -aG smartclockd $USER        # then log in again

The package puts the binaries in `/usr/bin`, creates a `smartclockd`
system user in `dialout`, and enables nothing: an instance exists
once its environment file does.

The last step is not optional: the socket is mode 0660 owned by
`smartclockd`, so `smartclockmon` and `smartclock-cli` cannot reach the
daemon until you are in that group.  Group membership is the whole of
the authorization model -- anyone who can open the socket may issue
whatever the daemon has been configured to allow.

Check it took:

    systemctl status smartclockd@bench
    smartclockmon --socket /run/smartclockd/bench/socket
    sudo -u smartclockd ls /var/lib/smartclockd/
    sudo -u smartclockd sqlite3 /var/lib/smartclockd/<model>-<serial>.sqlite \
        "select count(*), max(at) from snapshot;"

The log file appears once the receiver has answered `*IDN?`, since it
is named after the receiver.

## What the daemon does to the receiver

Reads, and with one opt-in exception nothing else.  Worth knowing
because two of the reads would otherwise be surprising, and one thing
it deliberately does *not* read.

It drains the receiver's error queue.  Reading an entry is what removes
it, so this is destructive by nature -- but the queue holds thirty and
discards the newest when it overflows, so an unread queue loses errors
anyway, and nothing else was ever going to read them.

It copies the receiver's diagnostic log out, entry by entry, and
optionally clears it; see `SMARTCLOCKD_ADOPT_LOG` below.

It does **not** read the event registers, and so does not touch the
front-panel Alarm LED or the BITE output; nor will it read one for a
client unless started with `--allow-control`.  Reading an event register
clears it, which clears the alarm that summarises it.  That lamp is
yours: the daemon watches the same state through `*STB?`, which reports
it in real time and changes nothing, and the alarm stays lit until you
clear it at the instrument.  What the daemon saw is recorded and shown
in the monitor's header and the browser's status strip, so clearing the
lamp does not lose the history.

## Configuring

Everything is in `/etc/default/smartclockd.<instance>`; the unit
names none of it but the socket, `/run/smartclockd/<instance>/socket`,
which it sets from the instance name.  Each `SMARTCLOCKD_*` variable
matches the command line option of the same name, and `smartclockd
--help` documents them.  Restart after a change -- the file is read
only at startup.  The file is yours, not the package's: upgrades never
touch it.

`SMARTCLOCKD_DEVICE` is required and has no default, so the service will
not start until you set it.  A default path does not fail when it is
wrong: it opens whatever else is on that path and starts sending SCPI at
it.  Use a by-id path from `ls -l /dev/serial/by-id/` rather than
`/dev/ttyUSB0`, which moves when another adapter is plugged in.  The
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

Nothing on the receiver will tell you the log has filled.  "Log Almost
Full" is bit 6 of the operation group, and the factory default for
`:STATus:OPERation:ENABle` is 36 -- bits 2 and 5, Holdover Summary and
Hardware Summary (`097-59551-02` 5-88).  Bit 6 is not among them, so
the condition never reaches the alarm and the front panel stays dark
while the log quietly stops recording.  The development unit filled in
March 2025 and lost eighteen months that way.  `smartclockd` reads the
condition register directly, where the enable mask does not apply, and
surfaces it.

A configuration mistake fails the unit instead of looping.  systemd
cannot check the file itself, since `Condition=` and `Assert=` do not
see `EnvironmentFile` variables, so the enforcement is by exit code: the
daemon exits 2 for anything a retry cannot fix -- an unset device, an
impossible baud, a malformed `ALLOW_` value, a database written by a
newer version -- and `RestartPreventExitStatus=2` makes that final.
`systemctl status` then names what is wrong.

## Where things live

| Path                                             | What                       |
| ------------------------------------------------ | -------------------------- |
| `/etc/default/smartclockd.<instance>`            | one instance's configuration |
| `/var/lib/smartclockd/<model>-<serial>.sqlite`   | one log per receiver       |
| `/run/smartclockd/<instance>/socket`             | where clients connect      |
| `/usr/share/doc/smartclockmon/examples/smartclockd.instance` | the file to copy |

A log is named after the receiver that answered on the port, and is
opened only once one has: a unit moved to another port or another
host's adapter keeps one continuous history, and a different unit
plugged into the same port gets a file of its own.  Set
`SMARTCLOCKD_DATABASE` to a file to log every receiver seen on the
port into that one file instead.

A log grows without bound, a few MB a day.  Nothing rotates it: the
point of the record is to still have last year's holdover events.

## More than one receiver

A second port is a second instance, exactly like the first:

    sudo cp /usr/share/doc/smartclockmon/examples/smartclockd.instance \
        /etc/default/smartclockd.second
    sudoedit /etc/default/smartclockd.second    # set SMARTCLOCKD_DEVICE
    sudo systemctl enable --now smartclockd@second

    smartclockmon --socket /run/smartclockd/second/socket

The instances share `/var/lib/smartclockd`, and since each writes the
file named for the receiver on its own port, and a receiver is on one
port at a time, they never write the same file.  `smartclock-web`
reads the whole directory and offers every receiver it finds, live or
historical, in its selector; it takes one daemon's socket for the live
strip, so `SMARTCLOCK_WEB_SOCKET` names whichever instance the strip
should show.  The exporter likewise scrapes one socket, so it is one
exporter per daemon.

## Unplugging the adapter

The daemon reconnects by itself, so the unit deliberately does not bind
to a device unit -- binding stops it dead while an adapter is out, which
is wrong for something meant to log continuously.  For that behaviour
anyway, add a drop-in rather than editing the shipped unit:

    sudo systemctl edit smartclockd@bench

    [Unit]
    BindsTo=dev-serial-by\x2did-usb\x2dYOUR_ADAPTER.device
    After=dev-serial-by\x2did-usb\x2dYOUR_ADAPTER.device

`systemd-escape --path --suffix=device /dev/serial/by-id/...` gives the
escaped name.
