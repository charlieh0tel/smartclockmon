# Sensors beside the receivers

Proposed, not built.  An OCXO's EFC moves with the room, so the room's
temperature, and perhaps its humidity, belong beside the receivers'
readings.  None of the receivers reports them: the 58503A's
`:DIAGnostic:TEMPerature?` is an internal reading of 34 to 38 C, and
the z3801 receivers answer the same query with 0 or 1 count
(`docs/firmware/console.md`).

## Design

The daemon reads sensors through the kernel's two interfaces for them
and knows nothing about the parts behind them.

- **Configuration.**  Repeatable switches, one per interface, each
  naming its sensor:
  - `--hwmon NAME=FILE` (`SMARTCLOCKD_HWMON`): a hwmon `*_input`, such
    as `room=/sys/class/hwmon/hwmon3/temp1_input`.  hwmon applies any
    offset itself.
  - `--iio NAME=CHANNEL` (`SMARTCLOCKD_IIO`): an IIO channel's path
    less its suffix, such as
    `bench=/sys/bus/iio/devices/iio:device0/in_temp`.  Its `_input` is
    read if there is one; otherwise (`_raw` + `_offset`) x `_scale`,
    with `_offset` 0 when absent and `_scale` required.

  The environment forms take a comma-separated list.  Names are short
  identifiers; a repeated name is refused at startup.
- **Quantity and unit** come from the kernel's file name, not from the
  configuration, and values are stored converted: hwmon `temp*` and
  IIO `in_temp` in C, hwmon `humidity*` and IIO `in_humidityrelative`
  in %RH, IIO `in_pressure` in kPa.  A file of any other kind is
  refused at startup.  An SHT41's temperature and humidity are two
  named sensors.
- **Reading.**  Every sensor is read on the medium tier and written in
  the snapshot row with the rest of that tier, inheriting its staleness
  rule.  A failed read is logged absent.  The daemon logs each sensor's
  slot, kind and first reading at startup.
- **Display.**  The readers map slots to names, so the web view and the
  exporter say "room, C", not "sensor3".  The web view draws one chart
  per quantity with a line per sensor; the compare page can overlay
  them; the exporter adds `smartclock_sensor{name,quantity}`.

## Schema

Twelve columns in `snapshot`, `sensor1` to `sensor12`, rather than a
table of readings: the history reader, its gap breaks, the staleness
rule, the charts and the exporter all work on columns already, and a
sensor's reading lands in the same row as the EFC it is compared with.
An empty slot costs SQLite about a byte.

A `sensor` table says what each slot holds, one row per assignment:
`slot`, `name`, `quantity`, `unit`, `source`, `since`.  A changed path
for a name is a new row with a new `since`.

Slots are sticky.  A new name takes the lowest slot never assigned; a
name dropped from the configuration keeps its slot, which reads NULL
from then on; a slot is given to another name only by
`smartclock-cli sensor reassign SLOT NAME`, which the daemon records as
a new row.  With all twelve assigned, a new name is
refused at startup.  So a chart of one name never changes sensors
partway along.

Two daemons on one host assign slots independently and each log the
same room.  Readers match sensors by name, so that is harmless.

## Sensors

Nothing below has been tried here, and the figures are typical values
as recalled, to be checked against each datasheet.

The preferred route is a Qwiic or STEMMA QT sensor on an MCP2221A,
whose kernel driver (`hid-mcp2221`) presents its I2C as `/dev/i2c-N`.
Declaring the sensor, as in
`echo sht4x 0x44 > /sys/bus/i2c/devices/i2c-N/new_device`, creates its
hwmon or IIO device.  The declaration does not survive a reboot and the
bus number can change, so a udev rule matching the MCP2221A, or a unit,
has to redo it.

| Sensor | Accuracy | Driver | Gives |
| ------ | -------- | ------ | ----- |
| SHT41, SHT40 | +/- 0.2 C | hwmon `sht4x` | temperature, humidity |
| MCP9808 | +/- 0.25 C | hwmon `jc42` | temperature |
| TMP117 | +/- 0.1 C | IIO `tmp117` | temperature, raw and scale |
| PCT2075 | +/- 1 C, 0.125 C steps | hwmon `lm75` | temperature |

The SHT41 is the first choice: hwmon, no glue, and humidity too.

## Not covered

A source with no kernel driver, such as a TEMPer USB thermometer,
would need a helper writing a file and a check on that file's age,
since nothing else would show it had stopped.  Neither is designed
until such a source is chosen.
