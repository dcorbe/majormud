//! Item identity against the real WG3-NT database -- the same `re/
//! mmud_wgnt.sqlite` `tests/graph.rs` loads, not invented fixture rows.
//!
//! Ground truth for the three rows this file leans on, straight from
//! the shipped `item` table:
//! - 68 "dagger" -- carries ability 0x74 (BSAccu): a real backstab
//!   weapon.
//! - 74 "sickle" -- also carries 0x74.
//! - 100 "quarterstaff" -- does not.

use mud_core::content::{Content, ItemId};

fn db_path() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../re/mmud_wgnt.sqlite")
}

fn content() -> &'static Content {
    use std::sync::OnceLock;
    static C: OnceLock<Content> = OnceLock::new();
    C.get_or_init(|| mud_core::content_db::load(&db_path()).expect("load content"))
}

#[test]
fn a_plain_name_resolves() {
    let item = mud_client::items::resolve(content(), "dagger").expect("dagger resolves");
    assert_eq!(item.id, ItemId(68));
    assert_eq!(item.name, "dagger");
}

/// The near-miss that MUST return `None`. "dagger of woe" is not a
/// shipped item -- it CONTAINS the real name "dagger" as a substring,
/// which is exactly the shape a "pick the nearest row" implementation
/// would wrongly resolve. See the mutation in this task's step 3.
#[test]
fn a_near_miss_containing_a_real_name_does_not_resolve() {
    assert!(
        mud_client::items::resolve(content(), "dagger of woe").is_none(),
        "a near-miss must read as unknown, never as the nearest real row"
    );
}

/// A plain typo, for the same reason: no shipped row is "daggar".
#[test]
fn a_misspelling_does_not_resolve() {
    assert!(mud_client::items::resolve(content(), "daggar").is_none());
}

/// An article survives resolution -- the board's own listings say "a
/// dagger" in running prose contexts even though `show_inventory`'s
/// carried list does not.
#[test]
fn an_article_still_resolves() {
    let item = mud_client::items::resolve(content(), "a dagger").expect("resolves with article");
    assert_eq!(item.id, ItemId(68));
}

/// A leading count, exactly as `show_inventory` groups loose items.
#[test]
fn a_leading_count_still_resolves() {
    let item = mud_client::items::resolve(content(), "3 sickle").expect("resolves with count");
    assert_eq!(item.id, ItemId(74));
}

/// A trailing worn/wielded suffix, exactly as `show_inventory` and
/// `sheet::Inventory::parse`'s own fixtures render it.
#[test]
fn a_worn_suffix_still_resolves() {
    let item = mud_client::items::resolve(content(), "quarterstaff (Two handed)")
        .expect("resolves past the hand suffix");
    assert_eq!(item.id, ItemId(100));

    let padded = mud_client::items::resolve(content(), "padded pants (Legs)");
    // Not asserting a specific id -- only that SOME real row is found,
    // proving the suffix strip works on worn armour too, not only the
    // weapon case.
    assert!(padded.is_some());
}

#[test]
fn resolution_is_case_insensitive() {
    let item = mud_client::items::resolve(content(), "DAGGER").expect("case-insensitive");
    assert_eq!(item.id, ItemId(68));
}

#[test]
fn an_empty_name_does_not_resolve() {
    assert!(mud_client::items::resolve(content(), "").is_none());
    assert!(mud_client::items::resolve(content(), "   ").is_none());
}

#[test]
fn dagger_and_sickle_are_backstab_capable() {
    let dagger = mud_client::items::resolve(content(), "dagger").unwrap();
    let sickle = mud_client::items::resolve(content(), "sickle").unwrap();
    assert!(mud_client::items::bs_capable(dagger));
    assert!(mud_client::items::bs_capable(sickle));
}

#[test]
fn a_quarterstaff_is_not_backstab_capable() {
    let staff = mud_client::items::resolve(content(), "quarterstaff").unwrap();
    assert!(!mud_client::items::bs_capable(staff));
}
