//! Integrity checks on the generated command table.

use std::collections::HashSet;

use smartclock::command::Class;
use smartclock::command::CommandId;
use smartclock::command::Dialect;
use smartclock::command::Evidence;

const DIALECTS: [Dialect; 2] = [Dialect::Hp58503, Dialect::Z3801];

#[test]
fn ids_are_unique_within_a_dialect() {
    for dialect in DIALECTS {
        let mut seen = HashSet::new();
        for spec in dialect.specs() {
            assert!(seen.insert(spec.id), "{:?} repeats {:?}", dialect, spec.id);
        }
    }
}

#[test]
fn every_spec_names_at_least_one_model() {
    for dialect in DIALECTS {
        for spec in dialect.specs() {
            assert!(
                !spec.models.is_empty(),
                "{:?} {:?} lists no models",
                dialect,
                spec.id
            );
        }
    }
}

#[test]
fn every_spec_says_where_it_came_from() {
    // A manual and page, a note that it was found by sweeping a
    // receiver, or the firmware image whose command tree lists it.
    // Several commands exist in no manual at all, and some of those
    // were read out of the parser's own tables before any receiver
    // confirmed them -- which is weaker than a reply and stronger than
    // a guess, so it says so rather than borrowing either wording.
    for dialect in DIALECTS {
        for spec in dialect.specs() {
            assert!(
                spec.cite.starts_with("097-")
                    || spec.cite.starts_with("discovered on ")
                    || spec.cite.starts_with("firmware image "),
                "{:?} {:?} cite {:?} is neither a document nor a discovery",
                dialect,
                spec.id,
                spec.cite
            );
        }
    }
}

#[test]
fn an_undocumented_command_must_have_been_seen_on_hardware() {
    // A discovered entry has no manual behind it, so nothing but the
    // receiver's own answer justifies it being in the table.
    for dialect in DIALECTS {
        for spec in dialect.specs() {
            if spec.cite.starts_with("discovered on ") {
                assert_eq!(
                    spec.evidence,
                    Evidence::Hardware,
                    "{:?} is undocumented but not hardware-confirmed",
                    spec.id
                );
            }
        }
    }
}

#[test]
fn queries_end_in_a_question_mark() {
    for dialect in DIALECTS {
        for spec in dialect.specs() {
            if spec.class == Class::Query {
                assert!(
                    spec.scpi.ends_with('?'),
                    "{:?} {:?} is a query but sends {:?}",
                    dialect,
                    spec.id,
                    spec.scpi
                );
            }
        }
    }
}

#[test]
fn non_queries_expect_no_response() {
    for dialect in DIALECTS {
        for spec in dialect.specs() {
            if spec.class != Class::Query && !spec.scpi.ends_with('?') {
                assert_eq!(
                    spec.response, "none",
                    "{:?} {:?} sends no query but expects {:?}",
                    dialect, spec.id, spec.response
                );
            }
        }
    }
}

#[test]
fn the_z3801_tree_is_a_subset_of_the_primary_one() {
    // Every Z3801A entry exists as a logical operation on the 58503A, so
    // the id set stays one vocabulary rather than two.  The one
    // exception is a keyword only the Z3816A image has, `:SYSTem:PON`,
    // carried so the table can refuse it by name; no 58503A is known to
    // accept it and nothing here invents an entry to say so.
    for spec in Dialect::Z3801.specs() {
        if spec.id == CommandId::SystemPon {
            continue;
        }
        assert!(
            Dialect::Hp58503.spec(spec.id).is_some(),
            "{:?} exists only on the z3801 tree",
            spec.id
        );
    }
}

#[test]
fn a_z3801_entry_claiming_hardware_names_the_unit() {
    // This used to forbid hardware evidence outright, because no
    // receiver of that family had ever been on the line.  One has: a
    // Z3805A, 3625A01487, firmware 3543B-A.  The guard therefore
    // becomes a weaker but still real one -- an entry may claim
    // hardware only if it says which unit answered, so the claim stays
    // checkable rather than becoming a habit.
    for spec in Dialect::Z3801.specs() {
        if spec.evidence != Evidence::Hardware {
            continue;
        }
        assert!(
            spec.cite.contains("Z380") || spec.cite.contains("Z381"),
            "{:?} claims hardware without naming the unit: {:?}",
            spec.id,
            spec.cite
        );
    }
}

#[test]
fn most_of_the_z3801_tree_is_firmware_confirmed() {
    // The keyword table extracted from the keyword table covers all but
    // :DIAGnostic:ERASe, which belongs to the INSTALL language and so
    // is not in the PRIMARY image.
    let firmware = Dialect::Z3801
        .specs()
        .iter()
        .filter(|s| s.evidence == Evidence::Firmware)
        .count();
    // Entries promoted to hardware after the Z3805A answered them are
    // still firmware-confirmed; they just record the stronger evidence.
    let hardware = Dialect::Z3801
        .specs()
        .iter()
        .filter(|s| s.evidence == Evidence::Hardware)
        .count();
    assert!(
        firmware + hardware >= 55,
        "only {firmware} firmware-confirmed and {hardware} hardware-confirmed"
    );
}

#[test]
fn the_primary_tree_has_been_probed() {
    // Phase 1 probed a 58503A (0000A00000, firmware 3704-C).  If this
    // drops to zero the table has been regenerated and lost its
    // provenance.
    let verified = Dialect::Hp58503
        .specs()
        .iter()
        .filter(|s| s.evidence == Evidence::Hardware)
        .count();
    assert!(
        verified >= 60,
        "only {verified} entries survive from the probe"
    );
}

#[test]
fn no_control_or_dangerous_command_claims_verification() {
    // Probing only ever sends queries.  Hardware evidence on a command
    // that changes a setting means something sent it to the receiver.
    //
    // A command that returns a reading is exempt even when it is not a
    // query: reading an event register clears it, which makes it an act
    // rather than a look, but it was confirmed by being read, not by
    // changing anything.
    for dialect in DIALECTS {
        for spec in dialect.specs() {
            if spec.class != Class::Query && spec.response == "none" {
                assert_ne!(
                    spec.evidence,
                    Evidence::Hardware,
                    "{:?} {:?} is {:?} but claims hardware",
                    dialect,
                    spec.id,
                    spec.class
                );
            }
        }
    }
}

#[test]
fn the_commands_that_can_strand_the_link_are_marked_dangerous() {
    let dangerous: HashSet<&str> = Dialect::Hp58503
        .specs()
        .iter()
        .filter(|s| s.class == Class::Dangerous)
        .map(|s| s.scpi)
        .collect();
    for expected in [
        ":SYSTem:COMMunicate:SERial1:BAUD",
        ":SYSTem:COMMunicate:SERial1:PARity",
        ":SYSTem:PRESet",
        ":DIAGnostic:ERASe",
        ":SYSTem:LANGuage",
    ] {
        assert!(dangerous.contains(expected), "{expected} is not dangerous");
    }
}

#[test]
fn the_generated_matrix_matches_the_table() {
    // docs/commands.md is generated from commands.toml, so the two can
    // only disagree if someone changed the table and did not regenerate.
    // Failing here is what stops the documentation drifting, which is
    // the whole reason it is generated rather than written.
    let checked_in = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../docs/commands.md")
        .canonicalize()
        .expect("docs/commands.md should exist; run `make docs`");
    let on_disk = std::fs::read_to_string(&checked_in).expect("read docs/commands.md");
    let generated = smartclock::matrix::markdown();
    assert!(
        on_disk == generated,
        "docs/commands.md is out of date with commands.toml; run `make docs`"
    );
}
