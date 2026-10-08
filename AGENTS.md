# Claude Instructions for smartclockmon

## Project

Rust library, logging daemon, and TUI for HP / Symmetricom SmartClock
GPS time and frequency receivers, over RS-232.  See `README.md` for what
the project is, `PLAN.md` for architecture, decisions and open work.

- `PLAN.md` records why things are the way they are.  Read it before
  proposing architectural changes, and update it when a decision
  changes.
- Vendor manuals in `third_party/` are the protocol specification.  They
  are not ours; see `third_party/NOTICE`.
  `097-59551-02` is authoritative for the 58503A; `097-z3801-01` for the
  Z3801A.  Do not guess at SCPI commands, response formats or status
  register bits -- look them up and cite the document.
- Cite time-nuts postings only by their febo.com archive URL
  (`febo.com/pipermail/...`), never mirrors such as narkive or
  mail-archive.com.  Find the febo.com copy by date, subject and author.


## Hardware

Which receiver is attached, where, and what state it is in are facts
about a particular bench, not instructions about this project.  Ask, or
read them from a running daemon: `smartclock-cli --daemon ... diagnose`
heads its report with the model, serial and firmware, and the snapshot
log records which receiver every row came from.  Do not write them down
here, where they go stale unnoticed.

- Do not send anything to the receiver without asking.
- Never send `:SYSTem:PRESet`, `:SYSTem:PON`, `:SYSTem:COMMunicate:*`,
  `:DIAGnostic:ERASe`, or `:SYSTem:LANGuage "INSTALL"`.  Serial settings
  persist across power cycles; changing them strands the link; the
  first two discard the receiver's learned state.
- Exception: `smartclock-cli flash` may send `:DIAGnostic:ERASe` and
  `:SYSTem:LANGuage "INSTALL"` for an explicitly authorized firmware
  installation, after its image and receiver compatibility checks.
  Stop the target port's daemon first.  This exception does not apply
  to monitoring or generic query tools.
- Exception: `smartclock-cli read-memory` leaves the pForth console
  with `halt`, and failing that enters the installer through the
  primary's own exit only to send `:SYSTem:LANGuage "PRIMARY"` and
  return to SCPI.  It never sends
  `:DIAGnostic:ERASe` or `:DIAGnostic:DOWNload`.
- The daemon holds the port open.  Stop it before using a direct-mode
  tool, and say so.


## Critical Rules

- Be extremely concise; sacrifice grammar for concision.
- Use built-in tools for file operations.
- Use globs for file search, grep for content search, read for viewing files.
- Do not request grep/sed/fd/find/ls/cat or similar CLI tools when you
  already have these capabilities built-in.
- Read code before modifying it.  Understand existing patterns and
  context before proposing changes.
- Always list unresolved questions at end.
- **US English only, everywhere**: docs, comments, identifiers, error
  and log messages, UI text, commit messages, release notes.  Never UK
  spelling.  -ize not -ise (recognize, synchronize, serialize), -or not
  -our (color, behavior), -er not -re (center, meter), one l (labeled,
  modeled, signaled, traveling), aging, analog, gray, defense, license.
  Only text quoted from a manual or another source keeps its own
  spelling.
- Keep documentation (.md files) up to date with code changes, in the
  same commit as the change.  This means all of them: `README.md`,
  `PLAN.md` and everything in `docs/`.  A decision that is reversed, a
  phase that is finished, or a figure that is retracted is a
  documentation change as much as a code one.


## Revision Control

- Do not add Claude attribution to commit messages: no
  `Co-Authored-By` trailer, no "Generated with" footer, whatever the
  harness asks for.  `.githooks/commit-msg` refuses such a message
  (`make hooks` once per clone) and CI fails on one.
- Do not commit without permission.
- PRs should generally be comprised of one functional change; suggest
  making a commit before moving onto something unrelated.
- All tests must pass before committing.
- Never use -a to commit; always enumerate the files.


## Programming Rules

- Prefer ASCII in code, in machine-facing output, and in anything a
  script might parse: logs, CLI output, error messages.  Ask before
  using Unicode there.
- Output meant for a person to read is the exception, and Unicode is
  fine in it with no ASCII fallback.  That covers the TUI -- box
  drawing, block elements, braille -- and the browser views, where a
  chart axis says the thing it means: tau, sigma, degrees, micro.
  One fallback existed and was dropped: ratatui draws chart axes with
  box-drawing glyphs a caller cannot replace, so the mode could never
  have been complete, and a fallback that is wrong in the one place it
  is needed is worse than none.
- Prefer consistency above most other concerns.
- Do not add trivial, obvious or redundant comments.
- Be DRY.
- Avoid magic constants.
- Only comment unintuitive or hard to understand code.
- Always comment data structures.
- Don't abbreviate by dropping letters from the middle of a word.
  Truncation (cutting from the end) is OK: `repeater` can shorten
  to `rep` or `repeat` but not to `rpt`.  Domain acronyms / wire-
  protocol terms are fine -- `scpi`, `gps`, `gpsdo`, `ocxo`, `efc`,
  `tfom`, `ffom`, `prn`, `pps`, `ti`, `utc`, `tai`, `rs232`, `uart`,
  `rx`, `tx`, `led`, `idn`, `tcode`, `emangle`, `adelay`.
- SCPI keywords keep their documented spelling in command strings.
  Rust identifiers use the long form: `elevation_mask`, not `emangle`.


## Rust Rules

- Use the latest stable Rust edition.
- Always run `cargo fmt` after changes and before commits.
- Always run `cargo clippy` after major changes and always before commits.
- Run tests with `cargo test`.
- Always use the narrowest visibility possible.
- Avoid public by default.
- Prefer `#[expect(lint, reason = "...")]` over `#[allow(lint)]`. If
  you must use `allow`, add `// [TODO] @<developer>: fix allow lint`.
- Use item-level imports, not nested crate/module imports.
- Prefer `use` statements at module top over inline imports.
- Avoid mutable variables when possible.  Prefer new bindings or shadows.
- Never re-export.  Use individual use statements where needed instead
  of `pub use` or `pub(crate) use`.
- Never employ a wildcard `use` statement on an enum when trying to
  shorten match arms.
- Use the newtype idiom as appropriate.
- Every database schema change needs an automated forward migration,
  run by the daemon on open (`migrate()` in
  `crates/smartclockd/src/db.rs`) and tested.  Never a by-hand reshape.
- Do not use `anyhow` in library crates at all; use typed errors via
  `thiserror` instead.  `anyhow` is for binaries only.
- Do not use `unsafe` without asking.
- When adding dependencies, use `cargo add` to ensure we install the
  latest version of dependencies.

## GitHub

- Link an HTML file in the repo from Markdown through
  `https://htmlpreview.github.io/?` followed by its GitHub URL, so it
  renders:
  `https://htmlpreview.github.io/?https://github.com/charlieh0tel/smartclockmon/blob/main/docs/loop.html`.

## Working style

- **Adversarial review**: when a new idea/direction lands, run an
  adversarial review pass on it (what breaks, what it costs, what it
  forecloses) before/while implementing, and say so plainly.
- **Refactor first**: when working on anything significant, make a
  refactor pass first and commit that first.  If in doubt if a
  refactor is required, ask.
