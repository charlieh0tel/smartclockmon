//! Integrity checks on the generated command table.

use std::collections::HashSet;

use smartclock::command::Class;
use smartclock::command::Dialect;

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
fn every_spec_cites_a_manual() {
    for dialect in DIALECTS {
        for spec in dialect.specs() {
            assert!(
                spec.cite.starts_with("097-"),
                "{:?} {:?} cite {:?} is not a document number",
                dialect,
                spec.id,
                spec.cite
            );
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
    // the id set stays one vocabulary rather than two.
    for spec in Dialect::Z3801.specs() {
        assert!(
            Dialect::Hp58503.spec(spec.id).is_some(),
            "{:?} exists only on the z3801 tree",
            spec.id
        );
    }
}

#[test]
fn the_z3801_tree_is_still_unverified() {
    // No Z3801A hardware.  Those entries came from 097-z3801-01 and
    // nothing has confirmed them, so none may claim otherwise.
    for spec in Dialect::Z3801.specs() {
        assert!(
            !spec.verified,
            "{:?} claims verification, but no Z3801A has been on the line",
            spec.id
        );
    }
}

#[test]
fn the_primary_tree_has_been_probed() {
    // Phase 1 probed a 58503A (3710A01056, firmware 3704-C).  If this
    // drops to zero the table has been regenerated and lost its
    // provenance.
    let verified = Dialect::Hp58503
        .specs()
        .iter()
        .filter(|s| s.verified)
        .count();
    assert!(
        verified >= 60,
        "only {verified} entries survive from the probe"
    );
}

#[test]
fn no_control_or_dangerous_command_claims_verification() {
    // Probing only ever sends queries.  A verified flag on anything else
    // means something sent a state-changing command to the receiver.
    for dialect in DIALECTS {
        for spec in dialect.specs() {
            if spec.class != Class::Query {
                assert!(
                    !spec.verified,
                    "{:?} {:?} is {:?} but claims verification",
                    dialect, spec.id, spec.class
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
