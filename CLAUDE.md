# Claude Instructions for smartclockmon

## Project

Rust library, logging daemon, and TUI for HP / Symmetricom SmartClock
GPS time and frequency receivers, over RS-232.  See `README.md` for what
the project is, `PLAN.md` for architecture, decisions and phases.

- `PLAN.md` records why things are the way they are.  Read it before
  proposing architectural changes, and update it when a decision
  changes.
- Vendor manuals in `third_party/` are the protocol specification.
  `097-59551-02` is authoritative for the 58503A; `097-z3801-01` for the
  Z3801A.  Do not guess at SCPI commands, response formats or status
  register bits -- look them up and cite the document.


## Hardware

A live 58503A is attached at `/dev/ttyUSB0`, 19200 8N1.  It is in
holdover and may have a failing OCXO.

- Do not send anything to the receiver without asking.
- Never send `:SYSTem:PRESet`, `:SYSTem:COMMunicate:*`,
  `:DIAGnostic:ERASe`, or `:SYSTem:LANGuage "INSTALL"`.  Serial settings
  persist across power cycles; changing them strands the link.
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
- Keep documentation (.md files) up to date with code changes.


## Revision Control

- Do not add Claude attribution to commit messages.
- Do not commit without permission.
- PRs should generally be comprised of one functional change; suggest
  making a commit before moving onto something unrelated.
- All tests must pass before committing.
- Never use -a to commit; always enumerate the files.


## Programming Rules

- Prefer ASCII in all code and user-facing strings (logs, CLI output,
  error messages).  Ask before using Unicode.
- The TUI is the exception: box drawing, block elements and other
  Unicode are fine there.  It keeps an ASCII mode for terminals that
  cannot render them, so anything drawn with Unicode needs an ASCII
  fallback.
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
- Do not use `anyhow` in library crates at all; use typed errors via
  `thiserror` instead.  `anyhow` is for binaries only.
- Do not use `unsafe` without asking.
- When adding dependencies, use `cargo add` to ensure we install the
  latest version of dependencies.
