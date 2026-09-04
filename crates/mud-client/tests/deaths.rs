//! The death lexicon: which monster does this line say died?
//!
//! `bot::is_kill_line` answers a different question — "did OUR fight
//! end" — and answers it with the experience award, which only fires for
//! kills we landed. Another player's kill produces no award and a
//! free-form per-monster wording, so it was invisible: measured at 10
//! overclaims across 27 blocks of a shared room (tests/world_corpus.rs).

use mud_client::deaths::DeathLexicon;

fn lexicon() -> DeathLexicon {
    DeathLexicon::from_pairs([
        (
            "acid slime".to_string(),
            "The acid slime dissolves into a puddle of bluish goo.".to_string(),
        ),
        (
            "filthbug".to_string(),
            "The filthbug collapses, its legs curling tightly around it.".to_string(),
        ),
    ])
}

#[test]
fn a_known_death_line_names_its_monster() {
    assert_eq!(
        lexicon().killed("The acid slime dissolves into a puddle of bluish goo."),
        Some("acid slime")
    );
}

#[test]
fn an_unrelated_line_names_nobody() {
    assert_eq!(lexicon().killed("The acid slime flails at you!"), None);
}

/// The parser hands over cleaned text, but leading spaces survive a
/// redraw and the board is inconsistent about case in its own tables.
#[test]
fn matching_tolerates_case_and_surrounding_space() {
    assert_eq!(
        lexicon().killed("  the FILTHBUG collapses, its legs curling tightly around it.  "),
        Some("filthbug")
    );
}

/// A death line names the TEMPLATE, never the rolled adjective — the
/// board prints "The acid slime dissolves..." while the room lists
/// "large acid slime" (verified, cwrun2.raw). So an exact-text lexicon
/// is sufficient and a per-instance one would be wrong.
#[test]
fn the_rolled_adjective_never_reaches_the_death_line() {
    assert_eq!(
        lexicon().killed("The large acid slime dissolves into a puddle of bluish goo."),
        None,
        "if this ever matches, the board changed and the lexicon needs the adjective stripped"
    );
}
