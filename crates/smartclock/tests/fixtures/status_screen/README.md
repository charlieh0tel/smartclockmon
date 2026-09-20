# Status screen fixtures

Sample `:SYSTem:STATus?` screens transcribed from the vendor manuals, used
as goldens for the scraper.  None came from hardware; captures from the
attached 58503A land here too once phase 1 records them.

- `58503a-*` from `097-59551-02` chapter 3.
- `58503b-*` from `097-58503-13` and `097-58503-12`.

Between them they cover the cases a naive scraper gets wrong:

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
