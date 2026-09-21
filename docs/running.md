# Running the daemon

## Installing

    sudo dpkg -i smartclockmon_0.1.0-1_amd64.deb

The package installs `smartclockd`, `smartclockmon` and `smartclock-cli`
to `/usr/bin`, creates a `smartclockd` system user in the `dialout`
group, and leaves the service disabled.  Edit the configuration first,
then start it:

    sudoedit /etc/default/smartclockd
    sudo systemctl enable --now smartclockd

## Configuring

Everything is set in `/etc/default/smartclockd`.  The systemd unit names
none of it, so there is no reason to edit the unit and no drop-in to
maintain when the adapter changes.  Each `SMARTCLOCKD_*` variable
matches the command line option of the same name, and `smartclockd
--help` is the reference for what they do.

`SMARTCLOCKD_DEVICE` is required and has no default, so **the service
will not start until you set it**.  That is deliberate: a default path
does not fail when it is wrong, it opens whatever else is on that path
and starts sending SCPI at it.  A failure to start, with the reason in
the journal, is the better outcome.

    ls -l /dev/serial/by-id/

Use a by-id path rather than `/dev/ttyUSB0`, which is assigned in
enumeration order and moves when another adapter is plugged in.

A boolean is enabled by setting it to `true`.  An empty value is a
mistake the daemon refuses at startup rather than reading as off.

`systemctl restart smartclockd` after any change; the file is read at
startup only.  It is a conffile, so package upgrades will not overwrite
your edits.

A database written by a newer smartclockd is refused rather than
opened: the schema stamp is now read back, not just written, since the
snapshots are the only record of a receiver's history and a bad write to
them cannot be undone.  That refusal exits 2 as well, so it stops rather
than reopening the file every five seconds.

A configuration mistake stops the service rather than looping.  systemd
cannot check the file itself -- `Condition=` and `Assert=` do not see
variables from an `EnvironmentFile` -- so the enforcement is by exit
code: the daemon exits 2 for a usage error, which covers an unset
device, an impossible baud and a malformed `ALLOW_` value, and
`RestartPreventExitStatus=2` in the unit means it fails once and stays
failed.  `systemctl status smartclockd` then shows the argument that is
wrong.  Anything that might succeed on a retry exits 1 and still
restarts.

## Where things live

| Path                                    | What                     |
| --------------------------------------- | ------------------------ |
| `/etc/default/smartclockd`              | all configuration        |
| `/var/lib/smartclockd/snapshots.sqlite` | the snapshot log         |
| `/run/smartclockd/socket`               | where clients connect    |

The log grows without bound, by a few MB a day at the default cadence.
Nothing rotates it; that is deliberate, because the point of the record
is to still have last year's holdover events.

## Watching it

`smartclockmon` finds the socket at its default path, so it needs no
arguments.  It has to be able to open that socket: `RuntimeDirectory` is
mode 0750 owned by `smartclockd`, which means running the monitor as
that user, or adding yourself to the group.  That is the whole of the
authorization model -- anyone who can open the socket may issue whatever
the daemon has been configured to allow.

## Unplugging the adapter

The daemon reconnects by itself when a USB adapter disappears and comes
back, and `Restart=on-failure` handles the failures it cannot absorb, so
the unit deliberately does not bind to a device.  Binding it means the
daemon stops dead when the adapter is unplugged and only returns when
systemd notices the device again, which is the wrong behaviour for a
receiver that is meant to be logging continuously.

If you want that behaviour anyway -- say, to keep the journal quiet
while an adapter is out for a long stretch -- add a drop-in naming your
device unit rather than editing the shipped unit:

    sudo systemctl edit smartclockd

    [Unit]
    BindsTo=dev-serial-by\x2did-usb\x2dYOUR_ADAPTER.device
    After=dev-serial-by\x2did-usb\x2dYOUR_ADAPTER.device

`systemd-escape --path --suffix=device /dev/serial/by-id/...` produces
the escaped unit name.

## Talking to a receiver over the network

`SMARTCLOCKD_DEVICE` also takes `tcp://host:port`, for a receiver behind
a serial-to-network adapter or for the simulator.  The shipped unit
allows the address families that needs, but if you have hardened it
further, `RestrictAddressFamilies=` has to list `AF_INET` and `AF_INET6`
or the daemon cannot open the connection and will restart forever.
