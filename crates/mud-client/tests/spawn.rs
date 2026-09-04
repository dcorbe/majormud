//! Spawn-table tests: the standing word the board prints in WHO.

use mud_client::spawn::Standing;

/// The tier word the board prints in WHO is enough: every branch tests
/// one threshold and each tier sits wholly on one side of it.
#[test]
fn standing_reads_back_from_the_legal_level_word() {
    for (word, notorious) in [
        ("Saint", false),
        ("Good", false),
        ("Neutral", false),
        ("Lawful", false),
        ("Seedy", false),
        ("Outlaw", true),
        ("Criminal", true),
        ("Villain", true),
        ("FIEND", true),
    ] {
        let s = Standing::from_legal_level(word).unwrap_or_else(|| panic!("{word}"));
        assert_eq!(s.notorious(), notorious, "{word}");
    }
    assert_eq!(Standing::from_legal_level("banana"), None);
}
