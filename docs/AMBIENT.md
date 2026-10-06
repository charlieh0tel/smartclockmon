# Ambient temperature

Proposed, not built.  An OCXO's EFC moves with the room, so the room's
temperature belongs beside the receivers' readings.  None of the
receivers reports it: the 58503A's `:DIAGnostic:TEMPerature?` is an
internal reading of 34 to 38 C, and the z3801 receivers answer the same
query with 0 or 1 count (`docs/firmware.md`).

## Design

The daemon reads one file and knows nothing about the sensor behind it.

- `--ambient PATH` (`SMARTCLOCKD_AMBIENT`): a file holding a temperature
  in millidegrees Celsius, the form of hwmon's `temp*_input`.
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

Nothing below has been tried here.

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
