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

/// The shipped world data must actually carry the wordings whose absence
/// this whole exercise measured.
#[test]
fn the_shipped_data_covers_the_wordings_that_blinded_the_model() {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../re/mmud_wgnt.sqlite");
    let lex = DeathLexicon::load(std::path::Path::new(path)).expect("world data should load");
    assert!(
        lex.len() > 500,
        "expected the full death table, got {}",
        lex.len()
    );
    for (line, who) in [
        (
            "The acid slime dissolves into a puddle of bluish goo.",
            "acid slime",
        ),
        (
            "The filthbug collapses, its legs curling tightly around it.",
            "filthbug",
        ),
        ("The lashworm falls dead at your feet.", "lashworm"),
        (
            "The cave bear falls to the ground with a grunt!",
            "cave bear",
        ),
    ] {
        assert_eq!(lex.killed(line), Some(who), "for {line:?}");
    }
}
