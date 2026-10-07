# Sensors beside the receivers

Proposed, not built.  An OCXO's EFC moves with the room, so the room's
temperature, and perhaps its humidity and pressure, belong beside the
receivers' readings.  None of the receivers reports them: the 58503A's
`:DIAGnostic:TEMPerature?` is an internal reading of 34 to 38 C, and
the z3801 receivers answer the same query with 0 or 1 count
(`docs/firmware/console.md`).

## Design

Sensors belong to the host, not to a receiver, so they get a service
and a log of their own.

- **`smartclock-sensord`**, one per host, in the same package as the
  rest and off until enabled (`smartclock-sensord.service`, settings in
  `/etc/default/smartclock-sensord`).  It runs as `smartclockd`, like
  the other services, with no serial access.  It reads sensors through
  the kernel's two interfaces for them, knowing nothing about the parts
  behind them, and logs whether or not any receiver is attached.
- **Its log** is `/var/lib/smartclock-sensord/sensors.sqlite`, apart
  from the receivers' logs, so nothing takes it for one.  Its schema is
  versioned from the first release; a later change is migrated forward
  on open, and a newer log is refused, as the receivers' logs are.
- **Its socket**, `/run/smartclock-sensord/socket`, uses the same
  framing and envelope as smartclockd's, with requests of its own and
  its own version: the sensors configured, and their latest readings.
  It pushes nothing on connect.

## Configuration

Repeatable switches, one per interface:

- `--hwmon NAME=PATH` (`SMARTCLOCK_SENSORD_HWMON`): a hwmon `*_input`,
  such as `room=/sys/class/hwmon/hwmon3/temp1_input`.
- `--iio NAME=CHANNEL` (`SMARTCLOCK_SENSORD_IIO`): an IIO channel's
  path less its suffix, such as
  `bench=/sys/bus/iio/devices/iio:device0/in_temp`.  A channel is named
  `in_<type>[index][_modifier]`, as `in_temp0` or `in_temp_ambient`.
  Its `_input` is read if there is one; otherwise (`_raw` + `_offset`)
  x `_scale`, each attribute taken for the channel or, failing that,
  for every channel of its type (`in_temp0_scale`, then
  `in_temp_scale`), with `_offset` 0 when absent and `_scale` required.
- `--every SECONDS` (`SMARTCLOCK_SENSORD_EVERY`): how often every sensor
  is read; 10 by default, the receivers' medium tier.

The environment forms take a comma-separated list, so a path may not
hold a comma.  A name matches `[a-z][a-z0-9_]{0,31}`, since it appears
in charts, addresses and metric labels.  A sensor is its name and
quantity together, so an SHT41's two readings can both be `room`; a
repeated pair is refused at startup.

`hwmonN`, `iio:deviceN` and I2C bus numbers can change across boots and
replugs.  A path may therefore go through a symlink kept by whatever
creates the device, such as `room=/run/sensors/room/in_temp`, or be a
glob, such as `/sys/bus/i2c/devices/*-0044/hwmon/hwmon*/temp1_input`.
Its last component must be literal, since the quantity is read from it.
It is resolved again on every read.  A glob matching more than one file
is a failed read, not a guess.

## Quantities

The quantity and unit come from the kernel's file name, not from the
configuration, and values are stored converted:

| Quantity | hwmon | IIO | Kernel unit | Stored |
| -------- | ----- | --- | ----------- | ------ |
| temperature | `temp*_input` | `in_temp*` | millidegree C | C |
| humidity | `humidity*_input` | `in_humidityrelative*` | milli-percent | %RH |
| pressure | none | `in_pressure*` | kPa | kPa |

Units are from `Documentation/ABI/testing/sysfs-class-hwmon` and
`sysfs-bus-iio` in the kernel.  A file of any other kind is refused at
startup.

Startup checks only the configuration's text.  A sensor whose file is
missing, or whose read fails, writes no reading, and the service's
journal says so once each time it starts or stops reading.  So a sensor
that is unplugged, or that udev creates late at boot, never stops the
service.

## Log

- `sensor`: `id`, `name`, `quantity`, `unit`, unique on `name` and
  `quantity`.
- `source`: `sensor_id`, `source` (the configured text), `device` (the
  hwmon or IIO device's `name` attribute), `since`.  A new row whenever
  either changes, so old readings keep the source they were read from.
- `reading`: `sensor_id`, `at`, `value`, one row per successful read,
  keyed on `sensor_id` and `at`, with `at` stored as the receivers'
  logs store times.
- `meta`: the schema version, the writer, and `every`, which readers use
  to tell a gap from the space between reads.

Growth is unbounded, like the receivers' logs: about 1 MB a day per
sensor at 10 s.

## Readers

- **History page.**  One chart per quantity, a line per sensor, named
  "room, C"; the temperature chart sits right below the receiver's
  internal temperature, then humidity and pressure.  Sensors are shown
  unless the address says otherwise (`sensors=`), apart from the
  receiver's `columns=`, so an older bookmark shows them too.  They come
  from their own endpoints (`/api/sensors`, `/api/sensors/history`),
  and the page snaps them onto the receiver's bucket grid, as the
  compare page joins receivers.  A gap is counted from `every`, not
  from the bucket width.  With no receiver log, the sensors are drawn
  alone.  Hover text gives each sensor's source.
- **Live strip.**  Each sensor's current value, from the socket, on
  every page.
- **Compare page.**  Sensor charts of their own, below the receivers',
  in colors outside the receivers' palette.
- **Exporter.**  `smartclock_sensor_temperature_celsius`,
  `smartclock_sensor_humidity_percent` and
  `smartclock_sensor_pressure_pascals`, labeled `sensor`, and
  `smartclock_sensord_up`.  A reading older than three read periods is
  not exported.
- **CLI.**  `smartclock-cli sensors` lists sensors and their latest
  readings.
- **TUI.**  Current values in the header.  Sensor history later.
- **Absent.**  With no sensor log, pages show no sensor charts and say
  nothing; with no socket, no live values.  The exporter reports
  `smartclock_sensord_up 0` once it has seen the service.

The web view takes `--sensor-log` and `--sensor-socket`
(`SMARTCLOCK_WEB_SENSOR_LOG`, `SMARTCLOCK_WEB_SENSOR_SOCKET`), defaulting
to the paths above; the exporter and the TUI take `--sensor-socket`.

## Order of work

One commit each.

1. Refactor: the socket protocol's envelope generic over its requests,
   and the client's `ask` with it.
2. Refactor: the socket server's scaffolding out of smartclockd into a
   library.
3. Refactor: the log writer's shared parts (persistent WAL, the version
   check and stamp) out of smartclockd, keeping one `unsafe` site.
4. Refactor: the compare page's grid snapping and join into
   `charts.js`, and the history page's paired chart into general
   multi-line chart groups.
5. Reading sensors: configuration, classification, conversion, IIO
   attribute fallback, globs.  Tested against a fake sysfs tree.
6. The sensor log's schema and reader, in `smartclock-log`.
7. `smartclock-sensord` logging, with its unit, settings, packaging and
   docs.
8. Its socket.
9. The exporter.
10. The history page and the live strip.
11. The compare page.
12. `smartclock-cli sensors`.
13. The TUI's current values.

## Sensors

Nothing below has been tried here.

The preferred route is a Qwiic or STEMMA QT sensor on an MCP2221A,
whose kernel driver (`hid-mcp2221`) presents its I2C bus.  Declaring
the sensor, as in
`echo sht4x 0x44 > /sys/bus/i2c/devices/i2c-N/new_device`, creates its
hwmon or IIO device.  The declaration does not survive a reboot and the
bus number can change, so a udev rule matching the MCP2221A, or a unit,
has to redo it.

| Sensor | Driver | Gives |
| ------ | ------ | ----- |
| SHT41, SHT40 | hwmon `sht4x` | temperature, humidity |
| MCP9808 | hwmon `jc42` | temperature |
| TMP117 | IIO `tmp117` | temperature, raw and scale |
| PCT2075 | hwmon `lm75` | temperature |

## Sources with no kernel driver

A helper outside this project can present one as an IIO device through
`/dev/uhid`, and keep a symlink to it under `/run`.  The service reads
it like any other IIO sensor.
