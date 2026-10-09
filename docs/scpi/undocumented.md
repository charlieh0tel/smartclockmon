# Undocumented commands

What is known of the commands no manual here has: what their handlers
do in the firmware, and what a receiver answered.

`models.md` says which paths each image's tree holds and which a
manual lists, and lists, generated from the images, the undocumented
paths whose node has the handlers of a documented one and the stubs,
whose every handler only refuses.  This file is written by hand; a test
checks that every path heading here names a path some image's tree
holds.

Each command below gives:

- **Images:** the firmware images whose tree holds the path.
- **Handler:** what its handler does, where it has been read, at its
  address in `58503a-3704.bin` unless another image is named.
- **Read:** what a receiver answered: model, firmware, date, reply.

The receivers are the bench 58503A (3704-C), Z3805A (3543B-A) and
Z3801A (3543-A).  Readings from the sweeps (see the end) carry the
date they were written up.

A read can still change state: a status group read with no leaf, or
anything ending `:EVENt?`, clears the event register it returns
(`z3801a.md`).

## Other names for documented commands

Each of these runs, query and setter, the handlers of a documented
branch (`models.md`, "Same handlers as a documented path"), in every
image that has it.

| Word | Same handlers as |
| ---- | ---------------- |
| `DOUGlas` | `:GPSystem:POSition:SURVey` |
| `GARY` | `:DIAGnostic:LOG` |
| `KENneth` | `:SYNChronization:HOLDover:TUNCertainty` |
| `MATThew` | `:ALARm` |
| `ROBin` | `:SENSe:DATA` |

### `:DOUGlas`

- **Images:** all seven.
- **Handler:** the position survey's; `:PROGress?` raises -221,
  "Settings conflict", when no survey is running.
- **Read:** on 2026-10-09, the same from all three receivers:

  | Query | Reply |
  | ----- | ----- |
  | `:DOUGlas:DURation?` | `+7.20000E+003` |
  | `:DOUGlas:STATe?` | `0` |
  | `:DOUGlas:PROGress?` | -221 |

### `:GARY`

- **Images:** all seven.  In the 58503B and Z3815A images `GARY` keeps
  only its own query, the handler of `:DIAGnostic:LOG:READ:ALL`.
- **Handler:** the diagnostic log's.
- **Read:** `:GARY:COUNt?` on 2026-10-09:

  | Receiver | Reply |
  | -------- | ----- |
  | 58503A 3704-C | `+24` |
  | Z3805A 3543B-A | `+64` |
  | Z3801A 3543-A | `+41` |

### `:KENneth`

- **Images:** all seven.
- **Handler:** the holdover time uncertainty's.  `:PREDicted?` is "an
  estimate of the time interval error that can be expected for a one
  day holdover" and `:PRESent?` "the current time interval error
  during holdover" (097-59551-02,
  `:SYNChronization:HOLDover:TUNCertainty`).  Both read a state byte
  (`0x102476`): `:PRESent?` answers only in states 3 and 4, and
  `:PREDicted?` in 3 to 6, its second field 1 in 3 and 4.
- **Read:** on 2026-10-09; the Z3805A's two replies were not read
  together:

  | Receiver | `:PREDicted?` | `:PRESent?` |
  | -------- | ------------- | ----------- |
  | 58503A 3704-C, locked | `+1.61E-005,0` | -221 |
  | Z3805A 3543B-A | `+3.2E-006,1` | -221 |
  | Z3801A 3543-A, 464 s into holdover | `+1.01E-005,1` | `+1.0E-006` |

### `:ROBin`

- **Images:** all seven.
- **Handler:** the measurement arrays', `:SENSe:DATA`.
- **Read:** `:ROBin:POINts?` answered `+4,+5,+6` from all three
  receivers on 2026-10-09.

## `:DIAGnostic`

### `:DIAGnostic:GPSystem:ACURrent`

- **Images:** 58503A 3633 and 3704, 58503B, Z3815A, Z3816A.
- **Handler:** returns a word from a table of eight channels
  (`0x222a6`): the value read where one has been, the image's default
  where not.  Its unit is not established.
- **Read:** 58503A 3704-C, 2026-10-09: `+2.08431E+002`.

### `:DIAGnostic:GPSystem:TIME`

- **Images:** all seven.
- **Read:** 58503A, written up 2026-09-20: `+21,+41,+53,+2007,+2,+4`,
  time then date.

### `:DIAGnostic:GPSystem:TRACking:LOG`

- **Images:** 58503A 3633 and 3704, 58503B, Z3815A, Z3816A.
- **Handler:** returns a flag.
- **Read:** 58503A 3704-C, 2026-10-09: `0`.

### `:DIAGnostic:GPSystem:UTC`

- **Images:** all seven.
- **Read:** 58503A, written up 2026-09-20: `1`.

### `:DIAGnostic:IDENtification:DEFault`

- **Images:** all seven.
- **Read:** 58503A, written up 2026-09-20: `"58503A","CQ"`.

### `:DIAGnostic:IDENtification:GPSystem`

- **Images:** all seven; 097-z3801-01 lists it for the Z3801A.
- **Read:** 58503A, written up 2026-09-20: the GPS engine, also as
  `:GPS?`.  It names a Motorola with `SOFTWARE DATE 06 Aug 1996`,
  before the 1024-week rollovers of 1999 and 2019, and so the source
  of the receiver's 1024-week date error (`firmware/gps.md`, "The
  engines on the bench").

### `:DIAGnostic:INPut:DATA`

- **Images:** 58503A 3633 and 3704, 58503B, Z3815A, Z3816A.
- **Handler:** returns zero whatever the state.
- **Read:** 58503A 3704-C, 2026-10-09: `+0`.

### `:DIAGnostic:OS`

- **Images:** all seven.
- **Read:** Z3805A, written up 2026-09-22.  `:DIAGnostic:OS:MEMory?`,
  `:PROCess?` and `:STACk?` dump the state of the real-time system,
  pSOS, and name its tasks:

      scpi   the command parser        pllp   the disciplining loop
      gpsm   GPS manager               curv   curve fitting
      hmon   health monitor            klok   clock
      spoo   spooler                   ROOT, IDLE

  with message exchanges `1pps`, `pllc`, `plla`, `gpsx`, `logr`,
  `eepx`, `sciR`/`sciW` and `drtR`/`drtW`, and about 13 percent of the
  CPU idle.  `pllp` runs the disciplining loop and `curv` the aging fit
  (`firmware/loop.md`, "The disciplining loop").

### `:DIAGnostic:PTIMe:TINTerval`

- **Images:** all seven.
- **Read:** Z3805A, written up 2026-09-22: the time interval, the
  unlocked one-second reading (`firmware/interval.md`).

### `:DIAGnostic:REFerence:GPSystem:QUEStionable:HYSTeresis`

- **Images:** 58503A 3633 and 3704, 58503B, Z3815A, Z3816A.
- **Handler:** returns a stored value, converted for the reply.  Its
  sibling under `:EXTernal` is a stub ("Stubs a receiver refused").
- **Read:** 58503A 3704-C, 2026-10-09: `+60`.

### `:DIAGnostic:ROSCillator:CURRent`

- **Images:** all seven.
- **Read:** 58503A, written up 2026-09-20: `+1.05882E+002`, oven
  current.
- **Of note:** the Z3801A image's strings include `Double oven`.  The
  58503A images carry it too (`firmware/console.md`, "The 58503A
  image"); the bench 58503A's oscillator is a 10811-60159 (`ocxo.md`).

### `:DIAGnostic:ROSCillator:EFControl`

- **Images:** all seven; 097-z3801-01 lists the bare node for the
  Z3801A.
- **Read:** the bare node answers as `:RELative?`: on the 58503A,
  written up 2026-09-20, and on the Z3805A, written up 2026-09-22.

### `:DIAGnostic:ROSCillator:EFControl:ABSolute`

- **Images:** all seven.
- **Read:** 58503A, written up 2026-09-20: `+713392`, DAC code.  See
  `efc.md` for what the EFC commands mean in volts and in frequency.

### `:DIAGnostic:ROSCillator:EFControl:DATA`

- **Images:** 58503A 3704, 58503B, Z3815A.
- **Read:** 58503A, written up 2026-09-20: `+0`.

### `:DIAGnostic:ROSCillator:EFControl:MODE`

- **Images:** 58503A 3704, 58503B, Z3815A; new in 3704.
- **Handler:** maps a mode byte (`0x10217a`) to one of three values.
- **Read:** 58503A 3704-C, 2026-10-09: `NORM`.

### `:DIAGnostic:ROSCillator:LTIMe`

- **Images:** 58503B.
- **Handler:** the loop's time constants, in `58503b-1.01.04.bin`
  (`firmware/loop.md`, "In every image").  `:INIT` is the constant the
  start-up ramp begins from (`0x1028cc`, 10 to 1000 s, default 150 s)
  and `:MAX` the one the loop settles at (`0x1028d0`, 10 to 10000 s,
  default 700 s); each setter keeps `:INIT` at or below `:MAX`.
  `:DATA?` returns the constant in force (`0x10328a`).
- **Read:** no 58503B on the bench.

### `:DIAGnostic:ROSCillator:TCOefficient`

- **Images:** 58503A 3633 and 3704, Z3801A, Z3805A, Z3815A, Z3816A.
- **Read:** 58503A, written up 2026-09-20: `-3.36500E+001`, the
  oven-current constant (`firmware/loop.md`).

### `:DIAGnostic:ROSCillator:TYPE`

- **Images:** 58503B.
- **Handler:** the oscillator type, the byte at `0x102726` in
  `58503b-1.01.04.bin`, which selects the loop's gain G from a table
  (`firmware/loop.md`, "In every image").  The setter accepts 0 or 1;
  type 0 has the Z3816A's G, type 1 the 58503A's.
- **Read:** no 58503B on the bench.

### `:DIAGnostic:SER1:RESTricted`

- **Images:** all seven, with `SER`, `SER2`, `SERIAL`, `SERIAL1` and
  `SERIAL2` beside `SER1`.
- **Read:** Z3805A, written up 2026-09-22: a restricted mode, on both
  ports.

### `:DIAGnostic:SLOG`

- **Images:** all seven.
- **Read:** 58503A, written up 2026-09-20: `:DIAGnostic:SLOG?` returns
  the oldest log entry, against `:LOG?` for the newest.  Z3805A,
  written up 2026-09-22: `:DIAGnostic:SLOG:COUNt?` answers, so the
  second log is real.

### `:DIAGnostic:SYSTem:PSTartup`

- **Images:** all seven, with `:DIAGnostic:SYSTem:DOUTput` beside it.
- **Read:** Z3805A, written up 2026-09-22: both answer.

### `:DIAGnostic:TCODe`

- **Images:** 58503A 3704, 58503B, Z3815A; new in 3704.
- **Handler:** each mask returns a byte.
- **Read:** 58503A 3704-C, 2026-10-09:

  | Query | Reply |
  | ----- | ----- |
  | `:DIAGnostic:TCODe:ERRor:AMASk?` | `+255` |
  | `:DIAGnostic:TCODe:ERRor:OMASk?` | `+0` |
  | `:DIAGnostic:TCODe:STATus:AMASk?` | `+255` |
  | `:DIAGnostic:TCODe:STATus:OMASk?` | `+0` |

### `:DIAGnostic:TEMPerature`

- **Images:** all seven; a stub in the 58503B, Z3815A and Z3816A
  (`models.md`, "Stubs").
- **Handler:** `firmware/console.md`, "`:DIAGnostic:TEMPerature?`".
- **Read:** 58503A, written up 2026-09-20: `+3.68550E+001`, degrees
  Celsius.

### `:DIAGnostic:TMODe`

- **Images:** 58503A 3633 and 3704, 58503B, Z3815A, Z3816A.
- **Handler:** `:DATA?` and `:STATe?` each return a byte.
- **Read:** 58503A 3704-C, 2026-10-09: `:DATA?` `+0`, `:STATe?` `0`.

### `:DIAGnostic:TOFFset`

- **Images:** all seven.
- **Read:** 58503A, written up 2026-09-20: `+0.00000E+000`.

## Elsewhere in the tree

### `:LED:ACTive`

- **Images:** all seven; 097-z3801-01 lists it for the Z3801A.
- **Read:** Z3805A, written up 2026-09-22: answers, as do
  `:LED:ALARm:MAJor?`, `:LED:ENABled?` and `:LED:TMHValid?`.

### `:LED:TMHValid`

- **Images:** Z3801A, Z3805A.
- **Read:** with `:LED:ACTive` above.

### `:SENSe:DATA:POINts`

- **Images:** all seven; 097-59551-02 and 097-58503-13 list it.
- **Read:** Z3805A, written up 2026-09-22: three measurement arrays.
  Its other name, `:ROBin:POINts?`, is above.

### `:SOURce:PTIMe:UTC`

- **Images:** 58503A 3704, 58503B, Z3815A; new in 3704.
- **Handler:** returns a flag (`0x102264`).
- **Read:** 58503A 3704-C, 2026-10-09: `:PTIMe:UTC?` `1`.

### `:SYSTem:PRINt`

- **Images:** all seven; the handlers of `:SYSTem:STATus`.
- **Read:** Z3805A, written up 2026-09-22: the whole status screen
  again.

## Stubs a receiver refused

| Query | Receivers | Reply |
| ----- | --------- | ----- |
| `:OUTPut:PIN1:DELay?` | 58503A 3704-C, Z3805A 3543B-A, Z3801A 3543-A | -113 |
| `:OUTPut:PIN1:FREQuency?` | 58503A 3704-C, Z3805A 3543B-A, Z3801A 3543-A | -113 |
| `:DIAGnostic:REFerence:EXTernal:QUEStionable:HYSTeresis?` | 58503A 3704-C | -113 |

All read on 2026-10-09.  -113 is "Undefined header", the refusal a
stub's handler makes (`models.md`, "Stubs").

## Sweeps

Candidate paths built from the keyword table and sent to a receiver
show which it implements: an unknown header returns -113 and changes
nothing.

- **58503A, written up 2026-09-20:** 1,530 candidates over the
  `:DIAGnostic` subtree.  What answered is under its own heading above.
- **Z3805A, written up 2026-09-22:** 282 tree paths put as queries;
  124 answered or declined and 158 came back undefined (`z3801a.md`).
  118 answered, 88 of them `:SYSTem:COMMunicate`: the serial settings,
  which are per port and per direction.
