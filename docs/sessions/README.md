# Sessions

Records of talking to the bench receivers, kept as they were captured.
Each pForth session opens with the unit, its serial, firmware and GPS
engine, and when and how it was taken.  The bench position in these
files is a placeholder (commit `89db482`).

| File | Unit | Firmware | What |
| ---- | ---- | -------- | ---- |
| `58503a-3704-pforth.txt` | 58503A 3710A01056 | 3704-C | pForth console session, 2026-09-26 (`firmware/console.md`) |
| `z3801a-3543-pforth.txt` | Z3801A 3542A01548 | 3543-A | pForth console sessions, from 2026-09-26 |
| `z3805a-3543b-pforth.txt` | Z3805A 3625A01487 | 3543B-A | pForth console sessions, from 2026-09-27 |
| `58503a-3704-idn.jsonl` | 58503A 3710A01056 | 3704-C | the first `*IDN?`, byte by byte |
| `58503a-3704-probe-01.jsonl`, `58503a-3704-probe-02.jsonl` | 58503A 3710A01056 | 3704-C | `smartclock-cli probe` runs, byte by byte |
| `58503a-3704-probe-03.txt` | 58503A 3710A01056 | 3704-C | a `probe` run's results, a line a command |

The four 58503A captures were kept on 2026-09-26 (`e5c25bd`), from
before then.  A `.jsonl` transcript is one JSON object a line: `t`,
seconds from the start, `dir`, `tx` or `rx`, and `data`.
