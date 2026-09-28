# Status screen fixtures

Sample `:SYSTem:STATus?` screens, used as goldens for the scraper:
transcriptions from the vendor manuals, and one capture from hardware
(below).

- `58503a-*` from `097-59551-02` chapter 3.
- `58503b-*` from `097-58503-13` and `097-58503-12`.

Between them the manual samples cover:

- `1PPS TI --` and `Predict --`, where a value is unavailable.
- `UTC 12:00:00[?] 01 Jan 1996`, where the time is suspect.
- `LAT`, `AVG LAT` and `INIT LAT`, three spellings of the position block
  depending on survey state.
- `*2 71 316` and `*1 -- ---`, satellites being acquired, with and
  without elevation and azimuth.
- `Survey: 0% complete` alongside `Suspended:track <4 sats`.
- `Synchronized to UTC`, `Inaccurate: not tracking` and
  `Invalid: not tracking`.
- Column spacing in the health monitor line that varies between screens.

## From hardware

`58503a-live-01.txt` was captured from the development 58503A, firmware
3704-C.  It differs from every manual sample:

- The satellite table's fourth column is headed `SS`, not `C/N`, and
  carries a different range: 28 to 114 here against 36 to 49 in the
  manuals.
- Section headings are padded with underscores (`Reference Outputs
  ______`) where the manuals show spaces.
- The screen is 79 columns wide; the manual samples are 75.
- An acquiring satellite is `* 1  24 204`, with a space after the star,
  against `*26` in the manuals, and `* 9  Acq ..` against `*26 Acq..`.
- Position reads `LAT` / `LON` / `HGT` with `(MSL)`, not `AVG LAT` with
  `(GPS)`, when the receiver is in position hold rather than surveying.
- Several lines carry trailing whitespace.

Both sets are kept: the manual samples cover states the unit was not
in, such as survey and power-up, and the capture is what firmware
3704-C sends.
