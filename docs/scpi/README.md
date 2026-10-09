# SCPI trees

Every command path each firmware image's parser knows, one file per
image in `third_party/firmware/`, as `smartclock-cli dump-scpi IMAGE`
prints it.  A test regenerates each and fails if it differs from the
file here, so the files and the images cannot drift apart.

A path here means the parser knows it, not that a receiver answers it.
Nothing here was sent to a receiver.

[`models.md`](models.md) sets the trees side by side, path by path,
with the manuals that list each path and whether a receiver has
answered it; `smartclock-cli dump-scpi --models` writes it from the
images, the `manual-*.txt` lists beside it and the command table, and
a test checks it as it does the trees.  `make docs` regenerates all of
them.  It also lists the paths in no manual whose node has the same
handlers as a documented one, and the stubs: paths whose every handler
only refuses, which a receiver answers with -113, "Undefined header"
(`undocumented.md`).

Each `manual-*.txt` is the command paths a manual writes from the
root, one `LANGUAGE PATH` a line, PRIMARY or INSTALL, with a comment
saying how they were drawn from it.  A model counts against its own
manual: 097-59551-02 for the 58503A, 097-58503-13 for the 58503B,
097-z3801-01 for the Z3801A.  The Z3805A has none of its own and is
counted against the Z3801A's while its tree is the same, with a
footnote saying so; none here is the Z3815A's or Z3816A's.

## Format

One path a line, sorted without regard to case.  Common commands start
`*`, the rest `:`.  `?` marks a node with a query handler, `(set)` one
with a setter.  A node with neither is a branch the parser walks
through, or a keyword with no handler of its own (`:DIAGnostic:SLOG`,
which a 58503A answers as a query nonetheless).

`:SOURce` is an optional header (`z3801a.md`): a path under it
also answers without it, though only the form with it is listed.

## How it is read

Pointers in the image are absolute and it is not relocated, so a
stored pointer is a file offset as it stands.

A **node** is a record:

    +0   u32   pointer to the keyword pair
    +4   u32   pointer to the child list, zero for a leaf
    +8   u32   setter, zero if none
    +18  u32   query handler, zero if none

A **keyword pair** is the short form, a NUL, the rest of the long form
and a NUL, both upper case: `SYST\0EM\0` is `SYSTem`.

A **child list** is:

    +0   u16        an id
    +2   u16        how many children
    +4   u16        flags
    +6   u32 x n    pointers to the child nodes

The parser keeps two pointers side by side: one to the common-command
list (`*CLS` to `*WAI`) and one to the main tree.  `dump-scpi` finds
that pair by shape -- two child lists, the first holding `IDN` -- and
refuses an image where it is not found exactly once.  It follows every
parent's pointer, since several parents share one list (`SER`, `SER1`,
`SERIAL` and the rest under `:DIAGnostic` and `:SYSTem:COMMunicate`),
and refuses a children slot holding anything but zero or a child list,
and a handler slot holding anything but zero or an even address in the
image.

| Image | Paths | Query | Set | Root pair at |
| ----- | ----- | ----- | --- | ------------ |
| [Z3801A 3543](z3801a-3543.txt) | 595 | 414 | 303 | `0x5cfae` |
| [Z3805A 3543B](z3805a-3543b.txt) | 595 | 414 | 303 | `0x5d014` |
| [Z3816A 4001](z3816a-4001.txt) | 902 | 638 | 507 | `0x64ae8` |
| [58503A 3633](58503a-3633.txt) | 914 | 639 | 519 | `0x60d48` |
| [58503A 3704](58503a-3704.txt) | 931 | 646 | 533 | `0x61218` |
| [Z3815A 4010](z3815a-4010.txt) | 965 | 680 | 552 | `0x6cd16` |
| [58503B 1.01.04](58503b-1.01.04.txt) | 831 | 525 | 477 | `0x68316` |

## Between images

Counted by path, ignoring handlers.

- **Z3801A 3543 and Z3805A 3543B** have the same tree, line for line.
- **58503A 3633 to 3704** adds 17 and removes none: `:DIAGnostic:TCODe`
  and its error and status masks, `:DIAGnostic:ROSCillator:EFControl:
  DATA` and `:MODE`, `:SOURce:PTIMe:UTC`, and the four-letter tokens
  `DACP`, `EFER`, `ESSD`, `ESSN`, `GDOP`, `RSTG` and `TMD1`.
- **Z3801A 3543 against 58503A 3633**: 534 in common.  Only in the
  Z3801A's: `:LED:TMHValid` and `:SAMPle` with its 48 array and
  value words.  Only in the 58503A's: `:ALARm`, `:SYSTem:PON`, the
  `:DIAGnostic:REFerence`, `:TMODe`, `:TSET` and `:TVALid` branches,
  `:OUTPut:ACTive`, more of `:SYSTem:COMMunicate` and
  `:STATus:OPERation`, and the tokens `ANT1`, `AZEL`, `LEAP`, `MANI`,
  `MATThew`, `PAVG`, `PMD1`, `POS1`, `SIGQ`, `TIMD`, `TIME`, `UNSL`.
- **Z3815A 4010** is nearest the 58503A 3704: it has every path of
  3704 but the five under `:GARY`, and adds 39, 21 of them under
  `:DIAGnostic:ROSCillator`, with `:DIAGnostic:ADC`, `:CALibration`,
  `:TEST`, `:LED:NGPS`, `:LED:STANdby`, `:OUTPut:HPOWer` and
  `:OUTPut:PRIMary`.  Against the Z3816A 4001 it adds 68 and removes
  the same five.
- **58503B 1.01.04** against the 58503A 3704 removes 168 and adds 68.
  Gone: the `R...`/`W...` tokens, `CALA`, `CEQU`, `FMHO`, `IPSU`,
  `:GARY`'s children, the `:OUTPut:PINn` branches, the `SER*` ports
  under `:DIAGnostic`, and much of `:SOURce`, `:STATus` and
  `:SYSTem:COMMunicate`.  New: `:DIAGnostic:DOWNload`, `:ERASe` and
  `:DCOMplete` in the primary's own tree, `:SYSTem:SRESet`,
  `:STATus:AACKnowledge`, `:DIAGnostic:FAIL`, `:DIAGnostic:ROSCillator:
  LTIMe`, `:TEMP` and `CHOE`.  40 of the 68 are among the Z3815A's
  additions too: `:DIAGnostic:ROSCillator:PTESt` and its thresholds,
  `:DIAGnostic:ADC`, `:CALibration`, `:LED:NGPS`, `:OUTPut:HPOWer` and
  `:SOURce:PTIMe:TDATe` among them.

## Against the command table

Each `commands.toml` entry, looked for in the images of its models,
with `:SOURce` optional:

| Command table entries | Image | Found |
| --------------------- | ----- | ----- |
| 58503A/B | 58503A 3633 | 119 of 130 |
| 58503A/B | 58503A 3704 | 120 of 130 |
| 58503A/B | Z3815A 4010 | 120 of 130 |
| 58503A/B | 58503B 1.01.04 | 121 of 130 |
| Z3801A | Z3801A 3543 | 78 of 82 |
| Z3801A | Z3816A 4001 | 79 of 82 |

Missing from every image checked against the 58503A/B entries: the three
`:SYNChronization:HOLDover:DURation:THReshold` entries, whose node is
`:ROSCillator:HOLDover:DURation:MEASurement:THReshold`, and the six
`:SYSTem:COMMunicate:SERial1:...` entries, whose settings sit under
`:SER:RECeive:`; a 58503A answers both table spellings
(`58503a.md`).  `:DIAGnostic:ERASe` is missing from all but the
58503B, and `:DIAGnostic:ROSCillator:EFControl:DATA` from 3633 only.

Missing from the images checked against the Z3801A entries: the two
`:ROSCillator:HOLDover:DURation:THReshold` entries, whose node has
`MEASurement` between (`z3801a.md`), `:DIAGnostic:ERASe`, and in
3543 `:SYSTem:PON`.
