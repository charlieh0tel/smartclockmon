# pForth words

Every word each firmware image's pForth console knows, one file per
image in `third_party/firmware/`, as `smartclock-cli dump-pforth IMAGE`
prints it, and [`models.md`](models.md) setting them side by side,
as `dump-pforth --models` writes it.  A test regenerates each and fails
if it differs from the file here; `make docs` rewrites them all.
`../console.md` is the console itself: getting in and out, reading
memory, and what particular words do.

A word here is one the image's tables hold, not one tried on a
receiver.  The console defines a few more when it starts (`ps`,
`mem_rep`, `s_rep`; `../console.md`, "Getting to it"), which no table
in the image holds.

## Format

One word a line: its table, its name and its code's address, sorted by
table and then by name.

## How it is read

**The kernel** is the interpreter's dictionary, a list of entries each
linking to the one before:

    +0   u32   the previous entry, zero for the first
    +4   u32   the word's code
    +8   u16   two fields not read here
    +10  u16
    +12        the name, NUL-terminated

`dump-pforth` finds the entry named `halt`, which every image has,
follows the links back to the first entry, and forward to the last by
the entry within 64 bytes that links to the one before.  It refuses an
image where `halt` is not found once, where the links loop, or where
two entries claim the same predecessor.

**The diagnostic words** are the firmware's own, a table of 38-byte
records:

    +0   u32       the word's code
    +4   28 bytes  the name, NUL-padded
    +32  6 bytes   not read here

It finds the record named `loop_time`, which every image has, and takes
records before and after it for as long as each has an even code
address inside the image and a padded name.

Every image has 154 kernel words, the same names; the diagnostic words
number 78 to 89 and are where the images differ.  `models.md` lists the
words every image has in one run and tables only the rest.
