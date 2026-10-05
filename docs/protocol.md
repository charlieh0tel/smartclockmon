# The protocol

SCPI over RS-232, but not a VISA-style instrument.  The receiver
behaves as an interactive terminal:

- It echoes received characters, one at a time as they arrive.
- It prompts with `scpi > ` while its error queue is empty, and with
  `E-nnn> ` while it holds anything, whatever the last command did
  ("The error prompt", below).  The manuals render the prompt `scpi>`,
  but the wire carries `scpi > `, with a space before the angle
  bracket.  The error prompt has none.
- The prompt's trailing space often arrives late, at the head of the
  next reply.
- A reply abandoned part-read leaves the receiver still sending.  The
  next prompt seen belongs to the abandoned reply, and every exchange
  after it reads one reply behind, so a session must drain before
  resynchronizing.
- `:SYSTem:STATus?` returns a multi-line formatted ASCII status screen
  rather than a SCPI response.  `:SYSTem:STATus:LENGth?` gives that
  screen's line count.
- Per-satellite elevation, azimuth and C/N appear only in that status
  screen.  No SCPI query returns them, and the command tree extracted
  from the firmware has no per-satellite node.  The counts are the
  exception: `:GPS:SATellite:TRACking:COUNt?` is the screen's
  `Tracking`, and `:GPS:SATellite:VISible:PREDicted:COUNt?` less that
  is its `Not Tracking`.
- The health monitor line is a coarse rendering of the hardware
  condition register and adds nothing to it.

Error reporting and the status registers do follow the standards:
`:SYSTem:ERRor?` returns the conventional `<code>,"<description>"`, and
the status register structure is IEEE 488.2.

## The error prompt

The prompt reflects the error queue, not the last command
(097-59551-02, 5-40: `*CLS` clears "the error queue (and corresponding
serial port prompt)").  On 2026-10-04 the bench Z3805A (3625A01487,
3543B-A), with its queue empty, was sent:

| Sent | Reply | Prompt after |
| ---- | ----- | ------------ |
| `:NO:SUCH?` | (none) | `E-113> ` |
| `*IDN? 1` | (none) | `E-108> ` |
| `*IDN?` | its identity | `E-108> ` |
| `:SYSTem:ERRor?` | `-113,"Undefined header"` | `E-108> ` |
| `:SYSTem:ERRor?` | `-108,"Parameter not allowed"` | `scpi > ` |

The prompt carries the newest queued error while `:SYSTem:ERRor?`
returns the oldest first, and a command that succeeds while errors are
queued answers in full under an error prompt, so an error prompt alone
does not mean the last command failed.  A query that fails returns no
response, only the prompt (097-59551-02, A-6).

So when the last prompt showed queued errors, the session reads the
queue before sending anything but `:SYSTem:ERRor?` or `*CLS`, keeping
what it read as strays; the daemon logs them, and `smartclock-cli`
prints them on stderr.  An answer to `:SYSTem:ERRor?` leaves the rest
of the queue for the next read rather than setting it aside.

