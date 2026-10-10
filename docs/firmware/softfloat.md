# The floating-point library

Every image does its arithmetic in a software floating-point library;
the decompiler shows it only as calls with the operands hidden.  The
routines were identified in the Z3816A by running each in an emulator
(Unicorn, on a 68000 core) on known inputs, and found in the other
images by their bytes: each image's routine matches the Z3816A's over
its first 24 bytes with absolute addresses and branch displacements
masked, at the same offset from the library's divide.  A few routines
also have a copy in the boot code (the Z3816A's multiply at
`0x108f6`); the table gives the primary's.

Operands go in D0 and D1 and the result comes back in D0.  A double is
D0:D1 or A0:A1, high long first; a double routine returns D0:D1 and
leaves a copy in A0:A1, which the next call may take as its other
operand.  The routines that take their operands the other way round
exchange them and run their neighbor: the float subtract and divide
fall into the next routine, the double divide jumps to it.

| Operation | Z3801A 3543 | Z3805A 3543B | 58503A 3633 | 58503A 3704 | Z3816A 4001 | Z3815A 4010 | 58503B 1.01.04 |
| --- | --- | --- | --- | --- | --- | --- | --- |
| D0 − D1 | `0x5d030` | `0x5d096` | `0x60dca` | `0x6129a` | `0x64b6a` | `0x6cd98` | `0x68398` |
| D1 − D0 | `0x5d02e` | `0x5d094` | `0x60dc8` | `0x61298` | `0x64b68` | `0x6cd96` | `0x68396` |
| D0 + D1 | `0x5d054` | `0x5d0ba` | `0x60dee` | `0x612be` | `0x64b8e` | `0x6cdbc` | `0x683bc` |
| D0 × D1 | `0x5e2ba` | `0x5e320` | `0x62054` | `0x62524` | `0x65df4` | `0x6e022` | `0x69622` |
| D0 ÷ D1 | `0x5d780` | `0x5d7e6` | `0x6151a` | `0x619ea` | `0x652ba` | `0x6d4e8` | `0x68ae8` |
| D1 ÷ D0 | `0x5d77e` | `0x5d7e4` | `0x61518` | `0x619e8` | `0x652b8` | `0x6d4e6` | `0x68ae6` |
| absolute value of the float on the stack | `0x133e2` | `0x133e2` | `0x1349e` | `0x135fa` | `0x234f2` | `0x235ee` | `0x235f4` |
| compare D0 with D1 | `0x5d5f6` | `0x5d65c` | `0x61390` | `0x61860` | `0x65130` | `0x6d35e` | `0x6895e` |
| integer to float | `0x5e1a8` | `0x5e20e` | `0x61f42` | `0x62412` | `0x65ce2` | `0x6df10` | `0x69510` |
| integer to float | `0x5e102` | `0x5e168` | `0x61e9c` | `0x6236c` | `0x65c3c` | `0x6de6a` | `0x6946a` |
| float to integer | `0x5df56` | `0x5dfbc` | `0x61cf0` | `0x621c0` | `0x65a90` | `0x6dcbe` | `0x692be` |
| float to integer | `0x5dfe8` | `0x5e04e` | `0x61d82` | `0x62252` | `0x65b22` | `0x6dd50` | `0x69350` |
| float to double, in D0:D1 | `0x5debe` | `0x5df24` | `0x61c58` | `0x62128` | `0x659f8` | `0x6dc26` | `0x69226` |
| integer to double | `0x5e0a2` | `0x5e108` | `0x61e3c` | `0x6230c` | `0x65bdc` | `0x6de0a` | `0x6940a` |
| double to float | `0x5dc6c` | `0x5dcd2` | `0x61a06` | `0x61ed6` | `0x657a6` | `0x6d9d4` | `0x68fd4` |
| divide the float at A0 by D1, in place | `0x5d774` | `0x5d7da` | `0x6150e` | `0x619de` | `0x652ae` | `0x6d4dc` | `0x68adc` |
| add D1 to the float at A0, in place | `0x5d024` | `0x5d08a` | `0x60dbe` | `0x6128e` | `0x64b5e` | `0x6cd8c` | `0x6838c` |
| subtract D1 from the float at A0, in place | `0x5d01a` | `0x5d080` | `0x60db4` | `0x61284` | `0x64b54` | `0x6cd82` | `0x68382` |
| double D0:D1 + double A0:A1 | `0x5d2ec` | `0x5d352` | `0x61086` | `0x61556` | `0x64e26` | `0x6d054` | `0x68654` |
| double D0:D1 × double A0:A1 | `0x5e59a` | `0x5e600` | `0x62334` | `0x62804` | `0x660d4` | `0x6e302` | `0x69902` |
| double A0:A1 ÷ double D0:D1 | `0x5da04` | `0x5da6a` | `0x6179e` | `0x61c6e` | `0x6553e` | `0x6d76c` | `0x68d6c` |
| double D0:D1 ÷ double A0:A1 | `0x5d9f2` | `0x5da58` | `0x6178c` | `0x61c5c` | `0x6552c` | `0x6d75a` | `0x68d5a` |
| pow(x, y), doubles on the stack, x the lower | `0x60ef8` | `0x60f5e` | `0x64cc0` | `0x65190` | `0x68a32` | `0x71308` | `0x6c5b6` |
| compare doubles: negative when A0:A1 < D0:D1 | `0x5d660` | `0x5d6c6` | `0x613fa` | `0x618ca` | `0x6519a` | `0x6d3c8` | `0x689c8` |
| natural log of the double on the stack | `0x609aa` | `0x60a10` | `0x64772` | `0x64c42` | `0x684e4` | `0x70dba` | `0x6c068` |
| square root of the double on the stack | `0x618b6` | `0x6191c` | `0x6567e` | `0x65b4e` | `0x693f0` | `0x71cc6` | `0x6cf74` |

`crash 7` uses two more, read rather than emulated: in the Z3801A it
subtracts a double from itself (`0x5d2a0`) and divides the zero by
itself (`0x5d9d8`, into `0x5da04`).  The start-up writes
`0x7480` to the library's control word at `0x10372e` (`0x1463e`),
which enables the invalid-operation trap: an invalid operation sets
bit 13 of the status word at `0x10372c` and reaches `FUN_00013e36`,
which prints `fp operand error: ` and the routine's name and stops in
the fatal routine (`restart.md`, "The watchdog", `crash 7`).
