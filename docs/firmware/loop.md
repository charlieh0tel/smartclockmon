# The disciplining loop

## The stages

From the state strings at `0x2c5be` to `0x2c758`: `coarse`, then
`fine` -- with sub-states `fine start`, `fine sync meas`, `fine sync
1`, `fine sync 2`, `fine meas 1`, `fine meas delay`, `fine meas 2`,
`fine slew`, `fine slew meas`, `fine fine slew` -- then `startup pll`,
then `normal pll`.

The stage is the byte at `0x10282c`, the first of the loop's state
block.  The console word that prints `PLL state: ` switches on it at
`0x2bba8`:

| Value | Name |
| ----- | ---- |
| 0 | `invalid` |
| 1 | `powerup`, with sub-states from `start` through `checking time` (`0x2c67f` to `0x2c74c`) |
| 2 | `powerup recovery` |
| 3 | `holdover` |
| 4 | `holdover recovery` |
| 5 | `startup pll` |
| 6 | `normal pll` |
| 7 | `diag` |
| 8 | `idle` |
| 9 | `fatal error` |

The pSOS tasks are created at `0x231f0` to `0x232d4`: `gpsm` (entry
`0x50bf2`), `hmon` (`0x33dc4`), `pllp` (`FUN_0004b088`, task id at
`0x103d4e`) and `curv` (`0x450d4`, task id at `0x103d5e`); the SCPI
task, `sci` (entry `0x39706`, task id at `0x103d5a`), is created by
`FUN_00039732`.  The Z3805A's `:DIAGnostic:OS:PROCess?` lists the same
names (see `scpi/undocumented.md`).  The loop below runs in `pllp`; the fit
in "The aging fit" runs in `curv`.

## Fine acquisition

`FUN_00048db6` is a state machine that takes fifty one-second readings
of the interval (state 5, `DAT_00102845 < 0x33`), accumulates the sums
of a straight-line fit, and sets the EFC from the fitted slope -- the
frequency offset.  Its messages:

    i= %d, ti= %.1f, y0= %.2e, x0= %.2e
    y0 = %f, efc =  %d
    fine - ti too far off
    fine - frequency too far off
    Bad TI in frequency measurement

## The loop

[`loop.html`](https://htmlpreview.github.io/?https://github.com/charlieh0tel/smartclockmon/blob/main/docs/loop.html)
draws what follows as a block diagram, with the constants and the
closed-loop poles.

`FUN_0004824a` is `pll_normal`: its failure message is `pll_normal -
Error with measurement` (`0x487f8`) and its report is

    %d/%d/%d %02d:%02d:%02d efc= %d ti= %.1f loop= %d curr= %.2f

It keeps its state in a block at `0x10282c`.  Each second it adds that
second's reading (see `interval.md`, "One reading") to a sum at `0x10272c`; every
tenth second it runs the update below, in single-precision floating
point.  The values and their addresses:

| Symbol | Address | What it is |
| ------ | ------- | ---------- |
| x̄ | `0x102c0c` | the mean of the ten readings -- what the query returns |
| x₀ | `0x102c1c` | the setpoint subtracted from it: cleared at `0x4b1dc`, otherwise written only by the console word `phase_off` (`0x2b87c`), which nothing else calls |
| f | `0x102be0` | the filtered error, kept between updates |
| I | `0x102728` | the integrator |
| B | `0x102be4` | a base EFC, set once as `pll_normal` takes over |
| τ | `0x102548` | the loop's time constant |
| G | `0x102c28` | a gain; `0x102c30` and `0x102c34` hold 1/G |
| c | `0x1023cc` | the value `:DIAGnostic:ROSCillator:TCOefficient?` reports, from the checksummed block at `0x400080`; see "s, the oscillator current" |
| s | -- | the oscillator current: `FUN_000324b0(6)`, channel 6 of 8 measured channels |
| M | `0x102c38` | a clamp, set to 6.25 × 10⁻¹⁰ / \|G\| |
| u | `0x10285e` | the EFC value the update produces |

The update:

    e  = x̄ − x₀
    f ← (1 − a)·f + a·e                      a = 29.75 / τ
    d  = clamp(p·2700 / q + r, −M, M)        p = [0x102bdc], r = [0x102bd8],
                                             q = [0x100c08], seconds
    I ← I + 10·k·(f + d / (2700·k))          k = 1 / (4·G·τ²)
    u  = K·f + B + I + c·s                   K = 1 / (G·τ)

and `FUN_00033798(u)` converts the result.  On entry the integrator is
cleared and B is set to the EFC in force less c·s (`0x4835c`), so
starting the loop does not move the oscillator.  That is the only
store to B; the update (`0x4863a`) is the only read.  `startup_pll`,
holdover and the aging fit leave it alone, so B stays where the
hand-over put it, and the oscillator's drift accumulates in I.

q is the seconds counter `FUN_00023818` returns, which `FUN_00023624`
advances once a second; it lies in the region a warm restart preserves
(see "τ and G").  p and r are two coefficients of a fit to the EFC's
own history ("The aging fit"): with y = q / 2700, d = b + c / y is the
slope of the fitted curve a + b·y + c·ln y now, in EFC units per
2700 s.  The integrator's second term is then 10·d / 2700: the EFC
change the fit predicts over the ten seconds between updates.

The factor 10 matches the ten seconds between updates.  What kind of
loop this is -- proportional-integral on the prefiltered error, with
gains that alone would give a double pole at −1/(2τ), and closed-loop
poles at −0.37/τ and (−1.35 ± 0.51j)/τ once the prefilter is counted
-- is worked out in
[`loop.html`](https://htmlpreview.github.io/?https://github.com/charlieh0tel/smartclockmon/blob/main/docs/loop.html),
"What kind of loop that is"; this file is the evidence for its
numbers.

## Starting up

`startup_pll`, at `0x47bac`, runs before `pll_normal` and applies the
same update law, except that its time constant is its own, at
`0x102be8`, and it recomputes K, k and a from it every update rather
than once on entry.  That time constant is set to 150 s at `0x47af6`
and `0x4af58`.

Its first state counts ten-second means whose magnitude is below
150 ns -- the double 1.5 × 10⁻⁷, loaded as two halves at `0x47e60` and
`0x47e66` -- and clears the count at the first that is not; sixteen in
a row (`0x47e46`) end the state.  Then, at `0x47ede` to `0x47fce`, it
lengthens its constant by 5 s (`0x47efe`) each update until it is
within 5 s of τ, sets it to τ, and hands over to `pll_normal`.  The
Z3801A's `FUN_000442ca` does the same.

At `0x475b8` to `0x476b4` it also sets the EFC from the aging fit
below: u = FUN_00033798(a + b·y + c·ln y + c·s), with y = q / 2700 and
the ln term omitted when y ≤ 1.  Which of its states runs that code
was not traced.

## The aging fit

The `curv` task fits a curve to the EFC's history; the loop feeds the
curve's slope into its integrator.

**Sampling.**  `FUN_00044cbe`, which `pll_normal` calls on each pass
(`0x48a66`, `0x48be2`), works on a ring of 64 samples at `0x102876`:
tail byte at `0x102876`, head byte at `0x102877`, and four arrays of
64 -- e at `0x102880`, weight bytes at `0x102980`, y at `0x1029c0`, w at
`0x102ac0`.  While the stage is 6, or 5 with the byte at `0x102838` not
1, each call adds u − c·s -- the EFC in force at `0x10285e` less the
oscillator-current term -- to a sum at `0x102bc4` and counts it at
`0x10287e`.  When q reaches the deadline at `0x102bc0` the deadline
moves on by 2700 s and, if anything was counted, one sample is
written at the head:

    e = sum / count
    y = q / 2700
    w = y·ln(y / (y − 1)) + ln(y − 1) − 1        or −1 when q ≤ 2700
    weight = 3 if count = 2700; 2 if count ≥ 675; else 1;
             and 1 regardless while q < 10800

then the sum and count are cleared and event 0x100 is sent to `curv`
(`FUN_0004507a`).  With the debug byte at `0x102c13` set, each sample
is printed as `e_avg= %.1f time= %f weight= %d log= %.2f tfom= %.1e`
(`0x463a6`), time being y / 32 -- days.

w is ∫ ln t dt over the window from y − 1 to y, since ∫ ln t dt =
t·ln t − t: the mean of ln t over the 2700 s the sample averages.  So
the curve fitted below, a + b·y + c·w, is the window mean of
a + b·t + c·ln t.

**The fit.**  `FUN_00045116` runs on each event.  Its record is at
`0x1026a2`: mode byte at +0, a at +0x12, b at +0x16, c at +0x1a, rms at
+0x22, HQ at +0x26; its debug lines are `pts= %d a= %.1f b= %.1f c=
%.1f rms= %.1f` (`0x46415`), `HQ= %.1e mode= %d` (`0x46441`) and
`failed line fit` (`0x46455`).  It counts n₂, the samples with weight
2 or more, and n₁, weight 1 or more, and finds the oldest and newest y
(`FUN_00045680`).  Then, first match wins:

| Mode | When | Fit |
| ---- | ---- | --- |
| 4 | n₂ = 64 and the newest y > 128 | b = (Σe over the newest 32 − Σe over the oldest 32) / 1024, a = ē − b·(y_newest − 32.5), c = 0 (`FUN_00045392`) |
| 3 | the oldest y > 128 and n₂ > 2 | a straight line through the weight-2 samples (`FUN_00045ce0`), c = 0 |
| 3 or 2 | n₂ > 5 | the straight line, then the three-term fit (`FUN_000455de`); the latter is taken, and the mode is 2, when its rms is below 0.75 of the line's or n₂ < 16 |
| 3 or 2 | n₁ > 9 | the same on the weight-1 samples |
| 1 | n₁ > 2 | the straight line through the weight-1 samples |
| 0 | otherwise | a = ē, b = c = 0 (`FUN_00045570`) |

`FUN_00045ce0` is an unweighted least-squares line of e on y,
returning the slope, the intercept and the rms of the residuals over
n − 2; with fewer than three points or no spread in y it fails and
clears its results.  The three-term fit forms a first c and a step
from the line (`FUN_00045782`), refits the line to e − c·w with c
moved by that step while the rms falls (`FUN_0004587a`), and sets c
from the last three trials (`FUN_00045a78`).  Every fit but mode 0
ends by resetting a so the curve passes through the newest sample
(`FUN_0004571c`).  HQ is `FUN_00045f94`'s result for modes 2 to 4 and
4.32 × 10⁻⁴ for modes 0 and 1; what it measures was not traced.  128
in units of 2700 s is 96 hours.

**Back to the loop.**  `curv` then sends event 0x100 to `pllp`.  At the
start of each sampling pass `FUN_0004508e` asks for that event
(`FUN_000274d6`) and, when it has arrived, copies a to `0x102bd4`, b to
`0x102bd8` -- the loop's r -- c to `0x102bdc` -- its p -- and HQ to
`0x102866`.  All four sit in the loop block a warm restart preserves,
so after such a restart the loop starts with the last fit.  After a
power-up they are zero until a fit with at least three samples --
mode 1 or above -- so d is zero for the loop's first hours.  The
`holdover` stage, `FUN_000473de` (its return of 1 moves the stage byte
to 4 at `0x4b36a`), seeds a with u − c·s when a is zero (`0x47440` to
`0x47460`).

The console's `last efc average = %.1f` (`0x2c3a5`) prints the newest
e; `dmes_curv` (`0x2befe`) sets the byte at `0x103d8a`.

## s, the oscillator current

`FUN_000324b0(n)` returns channel n of eight measured channels, each a
48-byte record at `0x102258` + 0x30·n: the live value when the flag at
`0x10225c` + 0x30·n is set, otherwise a default from a ROM table at
`0x325b2` + 0x2a·n.  The channels' names follow that table at
`0x326e6`, in order:

    12B  5V  12C  -12B  -12C  -12D  Oscillator current  Antenna current

and their defaults are 12, 5, 12, −11.5, −11.5, −11.5, 250 and 50.  The
loop reads channel 6, so c·s is a stored constant times the oscillator
current.

The record does not hold the latest reading.  Each channel's
descriptor at `0x32596` + 0x2a·n names a conversion (`FUN_00031e66`
for channel 6) that `FUN_00031eac(n)` applies to a fresh ADC read, and
a writer that files the result: `FUN_00031f3e` for most channels,
which averages ten readings; `FUN_00031fc4` and, for channel 6,
`FUN_00032048`, which keep an exponential average, s ← 0.1·fresh +
0.9·s (`0x3dcccccd`, `0x3f666666`) on each of the health monitor's
passes.  So the loop's s lags the current by about ten passes, and a
reading flickering between two ADC levels barely moves it.  The SCPI
queries read different cells: `:DIAGnostic:ROSCillator:CURRent?`
(handler `0x3b18c`) returns `FUN_00031eac(6)`, a fresh conversion, not
the loop's average; `:DIAGnostic:ROSCillator:EFControl:ABSolute?`
(`0x3b1f0`) and `:RELative?` (`0x3b1a2`) both read u at `0x10285e`,
the value the update writes, so on this firmware the DAC word a
monitor logs is the whole update, c·s included.

c is what `:DIAGnostic:ROSCillator:TCOefficient` reads and writes.
The query's node names the handler `FUN_0003b248`, which formats the
record at `0x43324`: the cell `0x1023cc`, the getter `FUN_00022bb8`,
the limits −200 and 200 (`0x43314`, `0x43318`) and the setter
`FUN_000404be`, which writes the calibration block back to the EEPROM
at `0x400080`.  Nothing else writes the cell: the loop applies it every
update, `startup_pll` and the aging fit's sampler use it, and none
adjusts it.  So in this firmware the "temperature coefficient" is a
stored calibration constant multiplying the oscillator current -- in
EFC units per unit of that channel, nominally 250 -- applied all the
time; it is not a coefficient on temperature and is not learned while
locked.

The console word `xcal` (`0x2bf78`) measures it.  Given a number of
seconds, it prints `curr= %d efc= %.1f, sec remaining= %d` (`0x2c7db`)
every ten seconds for that long -- the raw current from the ADC
(`FUN_0002e3b0`, the reading shifted right by two) and the EFC in
force at `0x10285e` -- then `now do a least square line fit to
data...` and `tempco = %f` (`0x2c803`, `0x2c82f`), the slope of EFC on
current.  It stores nothing; the value is entered afterwards with the
SCPI command.

The 58503A images 3633 and 3704 apply the same term (`console.md`,
"The 58503A image"), though the bench 58503A's DAC word does not jump at a step of
its reported oven current (`efc.md`, "The regression").  The Z3801A's
image reads channel 3 of its own function, `FUN_00022fd2`; its report
strings list Temperature, 5V, +15V, −15V, Oven, Double oven and Antenna
current, but which is its channel 3 was not traced.

## τ and G

`FUN_0002b358` sets τ from its argument.  Nothing calls it directly; a
pointer to it sits in the console's word table at `0x2d368`, beside the
name `loop_time`, and the message `max loop time = %d` (`0x2c370`)
prints τ.  It is the only code that writes τ's address, and the
start-up code clears the RAM τ lives in, so its value in service comes
from a 25-byte block of defaults in ROM.

τ is at offset 0x14 of a 25-byte block at `0x102534`.  The
initialization routine `FUN_00022c7c` fills that block one of two ways:

- from ROM, `memcpy(0x102534, 0x40174, 0x19)` (`0x22dd8`, `0x22e06`),
  whose bytes at `0x40188` are `43 fa 00 00`, the float 500.0.  The same
  routine first copies a longer block of defaults, `0x400de` to
  `0x40173`, into `0x10249e` to `0x102532`;
- or from the region `0x100000` to `0x100c3b`, which the start-up code
  does not clear (its clear runs from `0x100c3c`) and into which the
  pllp task keeps writing its state (`0x4b5fc`: the τ block to
  `0x100b82`, the loop block to `0x100622`, another to `0x100004`).
  That copy is taken when a byte checksum of the region (`FUN_00040056`
  over 0xb9c bytes) matches the word stored at its start, the flag word
  at `0x100002` is 1, the byte at `0x10262c` is clear, and bits 7 and 6
  of the reset-status register at `0xfffa07` are clear -- EXT and POW
  in MC68331UM D.3.4: the reset was neither external nor a power-up
  (the other bits are SW, HLT, LOC, SYS and TST; `FUN_00022c7c` at
  `0x22dc4` also requires the register to be exactly SYS, a RESET
  instruction).  Then the loop block, the health-monitor records and
  the τ block are restored, event 0x19 is posted (`FUN_0003ec76`) and
  `Power on` is logged (`FUN_00041180(1)`, `0x4b188`).  Otherwise the
  defaults are loaded and `System preset` is logged
  (`FUN_00041180(0x1b)`, `0x4b196`).

So a value set with `loop_time` survives a reset that leaves RAM
intact, and the loop uses 500 s after a power-up.  The startup ramp
(above) runs its own constant from 150 s to τ − 5 in 5 s steps per
ten-second update, reaching 500 s about 700 s after it begins.

The Z3801A's image does the same with its block at `0x101e6e`, filled
from ROM `0x2f846` (`0x12ce0`), whose float at offset 0x14 is
`44 7a 00 00`: 1000.0.  Its ramp from 150 s to 1000 s takes about
1700 s.

The rest of the 25-byte block, from its ROM defaults
`00 00 00 01 00 00 01 00 | 00 00 00 00 | 00 00 00 00 | 00 00 00 00 |
43 fa 00 00 | 00` and the code that names its bytes by address:

| Offset | Default | Read and written by |
| ------ | ------- | ------------------- |
| +0, word | 0 | nothing but the block copies |
| +2 | 0 | the alarm LED: `FUN_00049f76`, called from `pllp`, gathers bits from `FUN_0003ed82`, the +0x18 flag, `FUN_000324fc` and `FUN_00049f06` into the byte at `0x102c16` and sets +2 when any is set (`0x4a008`); the health monitor folds it into a status bit (`0x33d12`); `:LED:ALARm?` and `:LED:ALARm:MAJor?` return it (`FUN_0003d3d0`) |
| +3 | 1 | a descriptor at `0x431bc` only |
| +4 | 0 | `:LED:ACTive?` returns it (`FUN_0003d3b6`) |
| +5 | 0 | `:LED:ENABled?` returns it (`FUN_0003d458`) |
| +6 | 1 | `:SYNChronization:HOLDover:RECovery:AUTO` (`FUN_0003f57a`; `:ROSCillator:HOLDover:RECovery:AUTO` is the same node): set to 1 when the loop starts (`0x4af60`); consulted when holdover begins (`0x472bc`, under the stage byte's move to 3) |
| +7 | 0 | set to 1 by the `powerup` sub-state machine `FUN_0004a34a` (`0x4a764`); tested by SCPI handlers at `0x3c330` and `0x3c6de` |
| +8, long | 0 | written by a SCPI setter (`0x3c35c`, through `FUN_00038f04`); its address is handed to the `powerup` sub-state machine (`0x4a382`) |
| +0xc, +0x10 | 0 | no reader found by address |
| +0x14, float | 500 | τ |
| +0x18 | 0 | a holdover-recovery flag: `FUN_000473ac`, called from the `holdover recovery` stage (`FUN_000478ce`, message `holdover recovery - Error with measurement`), counts its calls at `0x102bf6` and sets the flag with event 0x2e when the count passes the limit at `0x102566`; the `powerup` machine clears it (`0x4a094`) and so does the loop's start (`0x4afa0`); it is bit 0x20 of the alarm summary above |

Bytes +2 to +6 and +8 also appear, each with its address, in 28-byte
descriptor records at `0x43168` to `0x43228` and `0x4369e`, alongside
the same getter `FUN_00022bb8`; what those records serve was not
traced.  The event codes passed to `FUN_0003ec76` -- 0x19 to 0x5c,
posted to the queue at `0x10356e` -- are not the log's codes: the log
writer `FUN_00041180` takes a code from 0 to 0x1b, looks up its text --
`Log cleared`, `Power on`, `Re-boot`, ... `System preset` (`0x41562`
to `0x41834`) -- and writes the entry through `FUN_00041138` under the
lock at `0x1023fa`.

G is a constant.  `FUN_0004b088` reads the hardware word at
`0x302000` and passes −1.25 × 10⁻¹² if bit 8 is set and
−2.125 × 10⁻¹² if clear (`0x4b14c` to `0x4b166`); the Z3801A's image
passes a fixed +6.25 × 10⁻¹³ (`0x475bc`).  That word is all the
firmware does with chip select 7: the reset code makes `0x302000` a
2 KB, 16-bit, read-only block with one wait state (see the chip-select
table above), `0x4b14c` is the only access to it in the image -- every
other form of the address was searched for -- and only bit 8 is
examined.  So it is an input port the processor reads once, when the
loop task starts; what drives bit 8 -- a link, a switch, or a signal
from the oscillator or DAC board -- is on the board, not in the image.
The Z3801A's reset code sets no chip select there and its image never
reads the address.  What the sign of G stands for was not traced.

`FUN_0004b022(G)` stores G, sets both gain constants to 1/G, sets M to
6.25 × 10⁻¹⁰ / |G|, and sets a second limit at `0x102c3c` to
5.787 × 10⁻¹⁴ / G -- 5.787 × 10⁻¹⁴ being 5 × 10⁻⁹ per day expressed per
second.  Its one caller is `FUN_0004b088`.

## In every image

Every image holds the loop's strings -- the stage names through `fine
fine slew` and `normal pll`, `pll_normal - Error with measurement`,
the fit's `pts= %d a=` and `failed line fit`, the sampler's `e_avg= `,
the console's `loop_time` -- and the constants of the update and the
fit: 29.75, 2700.0, the clamp's 6.25 × 10⁻¹⁰ and HQ's 4.32 × 10⁻⁴.  The
update law above was transcribed from the Z3816A only; in the others
these show the same code is there, not that it is unchanged.  What
does differ is where τ and G come from.  Read on 2026-10-09.

| Image | τ, as the ROM block sets it | Startup's constant | G |
| ----- | --------------------------- | ------------------ | - |
| Z3816A 4001 | 500 s (`0x40174`) | 150 s | −1.25 × 10⁻¹² or −2.125 × 10⁻¹², bit 8 of the word at `0x302000` |
| Z3801A 3543 | 1000 s (`0x2f846`) | 150 s | +6.25 × 10⁻¹³ (`0x475be`) |
| Z3805A 3543B | 1000 s (`0x3094c`) | 150.0 present, not traced | +6.25 × 10⁻¹³ (`0x475f4`) |
| 58503A 3633 | 700 s (`0x305ae`) | 150.0 present, not traced | +6.25 × 10⁻¹³ (`0x476bc`) |
| 58503A 3704 | 700 s (`0x30840`) | 150.0 present, not traced | +6.25 × 10⁻¹³ (`0x477dc`) |
| Z3815A 4010 | 500 s (`0x44c9e`); 1000 or 500 s by bit 12 of the word at `0x302000` (`0x50be8`) | 150.0 present, not traced | +1.2 × 10⁻¹² or −2.125 × 10⁻¹², bit 12 of the same word (`0x4503c`) |
| 58503B 1.01.04 | 700 s, `:DIAGnostic:ROSCillator:LTIMe:MAX` | 150 s, `:LTIMe:INIT` | by oscillator type: −1.25 × 10⁻¹² or +6.25 × 10⁻¹³ |

**The update, compared.**  Each image's `pll_normal` and fit
dispatcher (`pts= %d a=`) were compared with the Z3816A's instruction
by instruction, with every address and branch target masked and each
call named by a hash of the routine it calls, so the same float
routine compares equal across images and a different one does not.

- The fit dispatcher is identical, callees included, in the Z3801A,
  Z3805A and both 58503As.
- `pll_normal` in 58503A 3633 differs in two places: it reads the
  oscillator current through channel 3 of another routine, and stores
  1 × 10⁻⁸ (`0x322bcc77`) where the Z3816A stores 1 × 10⁻⁷ at offset
  0x46 of the loop block.  The Z3801A and Z3805A differ the same way
  and in their hardware routines -- the reading (`0x489de` in the
  Z3816A), the DAC conversion (`0x33798`) and `0x4782e` -- with every
  float operation the same.  3704 adds a field that moves the block's
  later offsets by two.  So these four run the update above.
- The Z3815A and 58503B differ in the arithmetic.  Both compute K, k
  and a from a τ held in a register each pass, as `startup_pll` does,
  and both replace the drift term: where the Z3816A forms
  d = p·2700/q + r every ten seconds, they form the same expression
  once per fit: when the fit's event arrives, the 58503B's `0x4658c`
  (the Z3815A's at `0x4aa26`) copies a, b and c and stores
  x = c·2700/q + b (`0x10327a`).  Each update then moves a running
  value toward it, v ← ((τ − 10)·v + 10·x)/τ (`0x10327e`, at
  `0x49c38`), clamps v to ±M, and adds it to the integrator where the
  Z3816A adds d.  So a new fit's change of slope reaches the
  oscillator over about τ, not at once.  Their fit dispatchers differ
  from the Z3816A's too, and from each other.

The τ block has the 25-byte layout above in every image but the
58503B, whose block (ROM `0x407b4`, copied to `0x1028b8` at
`0x22d6e`) has 29 bytes: the startup constant at +0x14 and τ at +0x18.
Two SCPI parameters, undocumented, write them: `:DIAGnostic:
ROSCillator:LTIMe:INIT` (`0x1028cc`, 10 to 1000 s) and `:LTIMe:MAX`
(`0x1028d0`, 10 to 10000 s); each setter keeps the start at or below
the maximum (`0x4aa84`, `0x4aaf6`), and the time constant in force is
at `0x10328a`.  The Z3816A and Z3801A hard-code the start, 150 s.

The 58503B takes G from a table of 12-byte entries at `0x40dfc` --
G and two limits -- indexed by the byte at `0x102726` (`0x40b8e`).
That byte is what `:DIAGnostic:ROSCillator:TYPE?` returns, and its
setter accepts a value below the number of types (`0x3b58c`).  Two
entries are filled: G −1.25 × 10⁻¹², the Z3816A's, with limits
±10⁻¹¹, and +6.25 × 10⁻¹³, the 58503A's; the type selects which.  In both the 58503B and the Z3815A the cell holding G is also
`:DIAGnostic:ROSCillator:EFControl:ASLOPe`'s, so G can be read and set
over SCPI within its limits (`../scpi/undocumented.md`).

**The gain's second term and the measured uncertainty, Z3815A and
58503B only.**  Beside G (A, `ASLOPe`) these two carry a second
coefficient B (`BSLOPe`, the 58503B's `0x102722`) and a DAC-to-ADC
ratio (`DADC`, `0x10271a`).  An EFC calibration in both writes all
three: in the 58503B, `0x4d310` (called from `0x4c6b6`) and `0x4d1c8`,
whose messages are `point= %d delta_f= %e efc=  %d`, `a= %e b= %e`,
`low= %d high= %d dac/adc= %e`, `OCXO cal, a= %e` and `OCXO cal a out
of range`; choosing the oscillator type resets them (G from the type's
table, B to 0, the ratio to 16.0).  The older images have neither the
coefficients nor the calibration.  B's one use in arithmetic is the
measured holdover uncertainty: at each new aging fit, `0x45d56` runs
`0x45b26` over the last 32 samples, 24 hours, with the fit's
coefficients as they stood 32 samples back, summing for each later
sample (e − (a + b·y + c·ln y)) · (A + B·e²) · 2700 -- the old fit's
EFC error times an EFC-dependent gain times the window's seconds -- and
stores the result at `0x102bfc`, valid while `0x1032af` is set, before
saving the new fit's coefficients for the next run.
`:SYNChronization:HOLDover:TUNCertainty:MEASured?` returns it
(`0x4781c`).  So in these two models the oscillator's gain is A + B·e²
at EFC e for that measurement; whether the loop itself applies B was
not found.

The Z3815A reads the word at `0x302000` as the Z3816A does, but bit 12
where the Z3816A reads bit 8, and with it chooses τ as well as G.  A
second routine stores −2.125 × 10⁻¹² unconditionally (`0x50c14`);
which runs when was not traced.

Channel names differ.  The `Oscillator current` table of "s, the
oscillator current" is in the Z3816A, Z3815A and 58503B; the Z3801A,
Z3805A and 58503A 3633 name `Double oven` instead, and 3704 neither.
The console word `xcal` is in the Z3816A only, and its `tempco = `
message in all but the Z3815A and 58503B.

## How this was read

A software floating-point library does the arithmetic; the decompiler
shows it only as calls with the operands hidden.  Each routine was
identified by running it in an emulator (Unicorn, on a 68000 core) on
known inputs, and the update above was transcribed from the
disassembly of `0x4850c` to `0x48674`.  Operands go in D0 and D1 and
the result comes back in D0:

| Routine | Operation |
| ------- | --------- |
| `0x64b6a` | D0 − D1 |
| `0x64b68` | D1 − D0 |
| `0x64b8e` | D0 + D1 |
| `0x65df4` | D0 × D1 |
| `0x652ba` | D0 ÷ D1 |
| `0x652b8` | D1 ÷ D0: exchanges the two and falls into the divide |
| `0x234f2` | absolute value of the float on the stack |
| `0x65130` | compare D0 with D1 |
| `0x65ce2`, `0x65c3c` | integer to float |
| `0x65a90`, `0x65b22` | float to integer |
| `0x659f8` | float to double, in D0:D1 |
| `0x65bdc` | integer to double |
| `0x657a6` | double to float |
| `0x652ae` | divide the float at A0 by D1, in place |
| `0x64b5e` | add D1 to the float at A0, in place |
| `0x64b54` | subtract D1 from the float at A0, in place |
| `0x64e26` | double D0:D1 + double A0:A1 |
| `0x660d4` | double D0:D1 × double A0:A1 |
| `0x6553e` | double A0:A1 ÷ double D0:D1 |
| `0x6519a` | compare doubles: negative when A0:A1 < D0:D1 |
| `0x684e4` | natural log of the double on the stack |
| `0x693f0` | square root of the double on the stack |

The constants, as single-precision floats: `0x41ee0000` is 29.75,
`0x4528c000` 2700, `0x40800000` 4, `0x41200000` 10, `0x302bcc77`
6.25 × 10⁻¹⁰, `0x29824fff` 5.787 × 10⁻¹⁴, and `0x4e6e6b28` 10⁹, which
scales the interval to nanoseconds for the report.  In the aging fit:
`0x40a51800 00000000` is the double 2700, `0xbff00000 00000000` the
double −1, `0x3f400000` 0.75, `0x44800000` 1024, `0xc2020000` −32.5,
`0x42780000` 62, `0x42000000` 32, `0x39e27e0f` 4.32 × 10⁻⁴, and the
integers `0xa8c` 2700, `0x2a3` 675, `0x2a30` 10800.
