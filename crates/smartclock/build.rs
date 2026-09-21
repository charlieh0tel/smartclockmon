//! Generates the command table in `commands.toml` into Rust.
//!
//! Keeping the table as data means the two dialects, their per-model
//! availability and their manual citations stay diffable against the
//! documents.  Generating rather than parsing at runtime makes the
//! command id set an enum, so a typo fails to compile.

use std::collections::BTreeMap;
use std::collections::BTreeSet;
use std::env;
use std::fmt::Write as _;
use std::fs;
use std::path::Path;

use serde::Deserialize;

/// One command as written in `commands.toml`.
#[derive(Deserialize)]
struct Command {
    /// Stable logical name, independent of dialect.
    id: String,
    /// `query`, `control` or `dangerous`.
    class: String,
    /// One-line description, used as the generated doc comment.
    doc: String,
    /// Per-dialect spellings, keyed by dialect name.
    dialect: BTreeMap<String, Dialect>,
    /// What the argument may be, when the command takes one.
    argument: Option<Argument>,
}

/// A constraint on a command's argument, as written in the table.
#[derive(Deserialize)]
struct Argument {
    /// `none`, `integer` or `word`.
    kind: String,
    /// Smallest permitted value, for `integer`.
    min: Option<i64>,
    /// Largest permitted value, for `integer`.
    max: Option<i64>,
    /// The permitted words, for `word`.
    allowed: Option<Vec<String>>,
}

/// How one dialect spells a command.
#[derive(Deserialize)]
struct Dialect {
    /// The SCPI string to send.
    scpi: String,
    /// Name of the parser that reads the reply.
    response: String,
    /// Receivers exposing this command in this dialect.
    models: Vec<String>,
    /// Manual and page the entry was taken from.
    cite: String,
    /// How far the entry has been confirmed: `manual`, `firmware` or
    /// `hardware`.
    evidence: String,
}

/// Top level of `commands.toml`.
#[derive(Deserialize)]
struct Table {
    schema: u32,
    command: Vec<Command>,
}

/// The schema version this build script understands.
const SCHEMA: u32 = 4;

fn variant(id: &str) -> String {
    id.split('_')
        .map(|part| {
            let mut chars = part.chars();
            match chars.next() {
                Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
                None => String::new(),
            }
        })
        .collect()
}

/// Stamp the build with the commit it came from.
///
/// One string, used by every binary's `--version`, by the daemon's
/// startup line and its info reply, by the row the log records, and by
/// the Debian package's version, so that all of them name the same
/// build.  Falls back to the crate version alone where git cannot
/// answer -- a source tarball, or a build outside a checkout.
fn stamp_version() {
    let package = env::var("CARGO_PKG_VERSION").expect("CARGO_PKG_VERSION");

    // Rebuild when the checked-out commit changes, or the stamp is
    // whatever it was when this last ran.
    for path in [".git/HEAD", ".git/refs/heads"] {
        println!("cargo::rerun-if-changed=../../{path}");
    }

    let git = |args: &[&str]| {
        std::process::Command::new("git")
            .args(args)
            .output()
            .ok()
            .filter(|out| out.status.success())
            .map(|out| String::from_utf8_lossy(&out.stdout).trim().to_owned())
            .filter(|text| !text.is_empty())
    };

    let dirty = match git(&["status", "--porcelain"]) {
        Some(_) => "+dirty",
        None => "",
    };

    // A build standing exactly on its release tag, from a clean tree,
    // is that release and says so.  Everything else is a snapshot, and
    // has to sort BELOW the release it precedes: otherwise a local
    // build installed on a machine outranks the released package and
    // apt never offers the upgrade.  A `~` sorts before everything,
    // the empty string included, which is what makes that work.
    //
    //     0.1.0-0~git79.gf4bfa6f  <  0.1.0-1
    //     0.1.0-0~git79.gf4bfa6f  <  0.1.0-0~git80.g0badcafe
    let tagged = git(&["describe", "--exact-match", "--tags", "HEAD"])
        .filter(|tag| tag.trim_start_matches('v') == package && dirty.is_empty());
    let version = match (
        tagged,
        git(&["rev-list", "--count", "HEAD"]),
        git(&["rev-parse", "--short", "HEAD"]),
    ) {
        (Some(_), _, _) => package,
        (None, Some(count), Some(commit)) => format!("{package}-0~git{count}.g{commit}{dirty}"),
        _ => package,
    };
    println!("cargo::rustc-env=SMARTCLOCK_VERSION={version}");
}

fn main() {
    println!("cargo::rerun-if-changed=commands.toml");
    println!("cargo::rerun-if-changed=build.rs");
    stamp_version();

    let raw = fs::read_to_string("commands.toml").expect("read commands.toml");
    let table: Table = toml::from_str(&raw).expect("parse commands.toml");
    assert_eq!(table.schema, SCHEMA, "commands.toml schema is not {SCHEMA}");

    let dialects: BTreeSet<&str> = table
        .command
        .iter()
        .flat_map(|c| c.dialect.keys().map(String::as_str))
        .collect();

    let mut out = String::new();
    out.push_str("// Generated by build.rs from commands.toml.  Do not edit.\n\n");

    out.push_str("/// A command tree.  Receivers of one family share a tree.\n");
    out.push_str("#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]\n");
    out.push_str("pub enum Dialect {\n");
    for d in &dialects {
        let _ = writeln!(out, "    /// The `{d}` command tree.\n    {},", variant(d));
    }
    out.push_str("}\n\n");

    out.push_str("/// What a command may do, and so how it is gated.\n");
    out.push_str("#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]\n");
    out.push_str("pub enum Class {\n");
    out.push_str("    /// Read-only; always permitted.\n    Query,\n");
    out.push_str("    /// Changes receiver state; requires configuration opt-in.\n    Control,\n");
    out.push_str("    /// Can strand the link or wipe configuration.\n    Dangerous,\n");
    out.push_str("}\n\n");

    out.push_str("/// A logical operation, independent of how a dialect spells it.\n");
    out.push_str("#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]\n");
    out.push_str("pub enum CommandId {\n");
    for c in &table.command {
        let _ = writeln!(out, "    /// {}\n    {},", c.doc, variant(&c.id));
    }
    out.push_str("}\n\n");

    out.push_str("/// How far a table entry has been confirmed.\n");
    out.push_str("#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]\n");
    out.push_str("pub enum Evidence {\n");
    out.push_str("    /// Transcribed from the manual, nothing more.\n    Manual,\n");
    out.push_str(
        "    /// Every keyword found in the firmware's own keyword table,\n\
         \x20   /// so the spelling is right even though no such receiver has\n\
         \x20   /// been on the line.\n    Firmware,\n",
    );
    out.push_str("    /// The receiver answered it.\n    Hardware,\n");
    out.push_str("}\n\n");

    out.push_str("/// What a command's argument may be.\n");
    out.push_str("///\n");
    out.push_str(
        "/// Checked before the command reaches the receiver, so a client\n\
         /// sending a string gets the same validation the typed Control\n\
         /// handle performs.  The receiver would refuse an out-of-range\n\
         /// value itself, but refusing it here says which command and\n\
         /// which bound, and keeps the exchange out of the audit trail as\n\
         /// something that happened.\n",
    );
    out.push_str("#[derive(Debug, Clone, Copy, PartialEq, Eq)]\n");
    out.push_str("pub enum Argument {\n");
    out.push_str("    /// Anything the caller likes.  Named explicitly by a\n");
    out.push_str("    /// command whose argument is too various to describe;\n");
    out.push_str("    /// never what an unspecified entry falls back to.\n");
    out.push_str("    Free,\n");
    out.push_str("    /// The command takes no argument.\n    None,\n");
    out.push_str(
        "    /// A whole number within these bounds, inclusive.\n\
         \x20   Integer {\n\
         \x20       /// Smallest permitted value.\n        min: i64,\n\
         \x20       /// Largest permitted value.\n        max: i64,\n\
         \x20   },\n",
    );
    out.push_str("    /// One of these words, compared without case.\n");
    out.push_str("    Word(&'static [&'static str]),\n");
    out.push_str("}\n\n");

    out.push_str("/// How one dialect spells one command.\n");
    out.push_str("#[derive(Debug, Clone, Copy)]\n");
    out.push_str("pub struct Spec {\n");
    out.push_str("    /// The logical operation.\n    pub id: CommandId,\n");
    out.push_str("    /// Gating class.\n    pub class: Class,\n");
    out.push_str("    /// The SCPI string to send.\n    pub scpi: &'static str,\n");
    out.push_str(
        "    /// Name of the parser that reads the reply.\n    pub response: &'static str,\n",
    );
    out.push_str(
        "    /// Receivers exposing this command.\n    pub models: &'static [&'static str],\n",
    );
    out.push_str("    /// Manual and page the entry came from.\n    pub cite: &'static str,\n");
    out.push_str("    /// How far the entry has been confirmed.\n    pub evidence: Evidence,\n");
    out.push_str("    /// What the argument may be.\n    pub argument: Argument,\n");
    out.push_str("}\n\n");

    for d in &dialects {
        let entries: Vec<_> = table
            .command
            .iter()
            .filter_map(|c| c.dialect.get(*d).map(|spec| (c, spec)))
            .collect();
        let _ = writeln!(
            out,
            "/// Every command the `{d}` tree defines.\nstatic {}: [Spec; {}] = [",
            variant(d).to_uppercase(),
            entries.len()
        );
        for (c, spec) in entries {
            let class = match c.class.as_str() {
                "query" => "Query",
                "control" => "Control",
                "dangerous" => "Dangerous",
                other => panic!("unknown class {other:?} on command {:?}", c.id),
            };
            let models = spec
                .models
                .iter()
                .map(|m| format!("{m:?}"))
                .collect::<Vec<_>>()
                .join(", ");
            let evidence = match spec.evidence.as_str() {
                "manual" => "Manual",
                "firmware" => "Firmware",
                "hardware" => "Hardware",
                other => panic!("unknown evidence {other:?} on command {:?}", c.id),
            };
            // Absent means the command takes nothing.  The default has
            // to be the strict one: an entry whose argument nobody
            // thought about must not become a hole in the gate, and
            // every command that does take one is spelled out below.
            let argument = match &c.argument {
                None => "Argument::None".to_owned(),
                Some(a) => match a.kind.as_str() {
                    "none" => "Argument::None".to_owned(),
                    "free" => "Argument::Free".to_owned(),
                    "integer" => format!(
                        "Argument::Integer {{ min: {}, max: {} }}",
                        a.min.unwrap_or(i64::MIN),
                        a.max.unwrap_or(i64::MAX)
                    ),
                    "word" => {
                        let words = a
                            .allowed
                            .as_ref()
                            .map(|w| {
                                w.iter()
                                    .map(|s| format!("{s:?}"))
                                    .collect::<Vec<_>>()
                                    .join(", ")
                            })
                            .unwrap_or_default();
                        format!("Argument::Word(&[{words}])")
                    }
                    other => panic!("unknown argument kind {other:?} on {:?}", c.id),
                },
            };
            let _ = writeln!(
                out,
                "    Spec {{ id: CommandId::{}, class: Class::{class}, scpi: {:?}, \
                 response: {:?}, models: &[{models}], cite: {:?}, evidence: Evidence::{evidence}, \
                 argument: {argument} }},",
                variant(&c.id),
                spec.scpi,
                spec.response,
                spec.cite,
            );
        }
        out.push_str("];\n\n");
    }

    out.push_str("impl Dialect {\n");
    out.push_str("    /// The name this tree goes by in the command table.\n");
    out.push_str("    pub fn name(self) -> &'static str {\n        match self {\n");
    for d in &dialects {
        let _ = writeln!(out, "            Dialect::{} => {d:?},", variant(d));
    }
    out.push_str("        }\n    }\n\n");
    out.push_str("    /// Every command this dialect defines.\n");
    out.push_str("    pub fn specs(self) -> &'static [Spec] {\n        match self {\n");
    for d in &dialects {
        let _ = writeln!(
            out,
            "            Dialect::{} => &{},",
            variant(d),
            variant(d).to_uppercase()
        );
    }
    out.push_str("        }\n    }\n\n");
    out.push_str("    /// How this dialect spells `id`, if it has it at all.\n");
    out.push_str("    pub fn spec(self, id: CommandId) -> Option<&'static Spec> {\n");
    out.push_str("        self.specs().iter().find(|s| s.id == id)\n    }\n}\n");

    let dest = Path::new(&env::var("OUT_DIR").expect("OUT_DIR")).join("commands.rs");
    fs::write(&dest, out).expect("write generated commands");
}
