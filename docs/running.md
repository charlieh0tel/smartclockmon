# Running the daemon

## Installing

    sudo dpkg -i smartclockmon_0.1.0-1_amd64.deb
    sudoedit /etc/default/smartclockd        # set SMARTCLOCKD_DEVICE
    sudo systemctl enable --now smartclockd
    sudo usermod -aG smartclockd $USER       # then log in again

The package puts the three binaries in `/usr/bin`, creates a
`smartclockd` system user in `dialout`, and leaves the service
disabled.

The last step is not optional: the socket is mode 0660 owned by
`smartclockd`, so `smartclockmon` and `smartclock-cli` cannot reach the
daemon until you are in that group.  Group membership is the whole of
the authorization model -- anyone who can open the socket may issue
whatever the daemon has been configured to allow.

Check it took:

    systemctl status smartclockd
    sudo -u smartclockd sqlite3 /var/lib/smartclockd/snapshots.sqlite \
        "select count(*), max(at) from snapshot;"

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
front-panel Alarm LED or the BITE output.  Reading an event register
clears it, which clears the alarm that summarises it.  That lamp is
yours: the daemon watches the same state through `*STB?`, which reports
it in real time and changes nothing, and the alarm stays lit until you
clear it at the instrument.  What the daemon saw is recorded and shown
in the monitor's header and the browser's status strip, so clearing the
lamp does not lose the history.

## Configuring

Everything is in `/etc/default/smartclockd`; the unit names none of it.
Each `SMARTCLOCKD_*` variable matches the command line option of the
same name, and `smartclockd --help` documents them.  Restart after a
change -- the file is read only at startup -- and note it is a conffile,
so upgrades will not overwrite your edits.

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

A configuration mistake fails the unit instead of looping.  systemd
cannot check the file itself, since `Condition=` and `Assert=` do not
see `EnvironmentFile` variables, so the enforcement is by exit code: the
daemon exits 2 for anything a retry cannot fix -- an unset device, an
impossible baud, a malformed `ALLOW_` value, a database written by a
newer version -- and `RestartPreventExitStatus=2` makes that final.
`systemctl status` then names what is wrong.

## Where things live

| Path                                    | What                  |
| --------------------------------------- | --------------------- |
| `/etc/default/smartclockd`              | all configuration     |
| `/var/lib/smartclockd/snapshots.sqlite` | the snapshot log      |
| `/run/smartclockd/socket`               | where clients connect |

The log grows without bound, a few MB a day.  Nothing rotates it: the
point of the record is to still have last year's holdover events.

## Unplugging the adapter

The daemon reconnects by itself, so the unit deliberately does not bind
to a device unit -- binding stops it dead while an adapter is out, which
is wrong for something meant to log continuously.  For that behaviour
anyway, add a drop-in rather than editing the shipped unit:

    sudo systemctl edit smartclockd

    [Unit]
    BindsTo=dev-serial-by\x2did-usb\x2dYOUR_ADAPTER.device
    After=dev-serial-by\x2did-usb\x2dYOUR_ADAPTER.device

`systemd-escape --path --suffix=device /dev/serial/by-id/...` gives the
escaped name.
