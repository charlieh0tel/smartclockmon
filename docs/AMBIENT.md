# Ambient temperature

Proposed, not built.  An OCXO's EFC moves with the room, so the room's
temperature belongs beside the receivers' readings.  None of the
receivers reports it: the 58503A's `:DIAGnostic:TEMPerature?` is an
internal reading of 34 to 38 C, and the z3801 receivers answer the same
query with 0 or 1 count (`docs/firmware.md`).

## Design

The daemon reads a kernel sensor interface and knows nothing about
the sensor behind it.

- One of two switches, saying which kernel interface the reading
  comes from, rather than guessing it from a file name:
  - `--ambient-hwmon FILE` (`SMARTCLOCKD_AMBIENT_HWMON`): a hwmon
    `temp*_input`, millidegrees Celsius.  hwmon applies any
    `temp*_offset` itself.  A helper's file in the same form, such as
    a TEMPer's, goes here too.
  - `--ambient-iio CHANNEL` (`SMARTCLOCKD_AMBIENT_IIO`): an IIO
    channel's path less its suffix, such as
    `/sys/bus/iio/devices/iio:device0/in_temp`.  Its `_input` is read
    if there is one; otherwise (`_raw` + `_offset`) x `_scale`,
    millidegrees, with `_offset` 0 when absent and `_scale` required.
  The two are exclusive.  The daemon logs which it read and its first
  value.
- Read on the medium tier and logged as `ambient_c` in each snapshot
  (a schema bump).
- A reading from a file not modified for three medium periods is logged
  absent, so a writer that has stopped does not leave its last value
  standing.  hwmon files are generated on read and always pass.
- Shown as a chart on the live and compare pages and as a gauge in the
  exporter.
- Two daemons on one host log the same room each; each log stays
  self-contained.

## Sensors

Nothing below has been tried here, and the figures are typical values
as recalled, to be checked against each datasheet.

The preferred route is a Qwiic or STEMMA QT sensor on an MCP2221A,
whose kernel driver (`hid-mcp2221`) presents its I2C as `/dev/i2c-N`:

| Sensor | Accuracy | Driver | Form |
| ------ | -------- | ------ | ---- |
| SHT41, SHT40 | +/- 0.2 C | hwmon `sht4x` | `temp1_input`; humidity too |
| MCP9808 | +/- 0.25 C | hwmon `jc42` | `temp1_input` |
| TMP117 | +/- 0.1 C | IIO `tmp117` | `in_temp`, raw and scale |
| PCT2075 | +/- 1 C, 0.125 C steps | hwmon `lm75` | `temp1_input` |

The SHT41 is the first choice: hwmon, no glue, and humidity for a
second column later.

- **A hwmon sensor.**  An I2C temperature sensor with a kernel driver
  (TMP102, LM75, SHT3x) on a USB-to-I2C bridge (CP2112, MCP2221,
  `i2c-tiny-usb`) appears as `/dev/i2c-N`; declaring the sensor, as in
  `echo tmp102 0x48 > /sys/bus/i2c/devices/i2c-N/new_device`, creates a
  hwmon device whose `temp1_input` the option names directly.  The
  declaration does not survive a reboot and the bus number can change,
  so a udev rule or a unit has to redo it.
- **A TEMPer.**  A USB HID thermometer with no kernel driver; models
  differ in report format and scaling.  A helper outside this project,
  run by a systemd timer, writes its reading to a file such as
  `/run/ambient/temp1_input`, and the option names that.  Only the
  helper knows the device.

## Open

1. Whether to write the TEMPer helper, once the device is in hand to
   test against.
2. The staleness limit: three medium periods is a starting point, not a
   measured one.
