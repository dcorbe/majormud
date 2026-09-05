//! What the character carries, by item id.
//!
//! The board prints display names and the world database has ids. The
//! pack is the join, done once per inventory reply, so routing and the
//! bot can ask "do I hold item 172" without a string compare in a loop.

use std::sync::Arc;

use mud_client::pack::{Pack, PackHandle, is_key};
use mud_client::sheet::Inventory;
use mud_core::content::{Content, Item, ItemId};

fn item(id: u16, name: &str, item_type: i16) -> Item {
    Item {
        id: ItemId(id),
        name: name.into(),
        item_type,
        ..Default::default()
    }
}

/// A slice of the shipped table: the Black House key, a weapon, a worn
/// piece, and a stackable.
fn content() -> Content {
    let mut c = Content::default();
    c.add_item(item(172, "black star key", 7));
    c.add_item(item(500, "ninjato", 1));
    c.add_item(item(501, "silk gloves", 0));
    c.add_item(item(502, "torch", 6));
    c
}

fn inventory(items: &[&str], keys: &[&str]) -> Inventory {
    Inventory {
        items: items.iter().map(|s| s.to_string()).collect(),
        keys: keys.iter().map(|s| s.to_string()).collect(),
        encumbrance: None,
    }
}

#[test]
fn a_key_is_item_type_seven() {
    assert!(is_key(&item(172, "black star key", 7)));
    assert!(!is_key(&item(500, "ninjato", 1)));
}

/// The live reply of 2026-09-05, trimmed: coins first, worn gear with
/// its slot suffix, loose gear, and the ring line.
#[test]
fn resolve_places_carried_worn_and_keys_by_id() {
    let pack = Pack::resolve(
        &content(),
        &inventory(
            &[
                "38 runic coins",
                "7 silver nobles",
                "silk gloves (Hands)",
                "ninjato (Weapon Hand)",
                "3 torch",
            ],
            &["black star key"],
        ),
    );
    assert_eq!(pack.carried, vec![(ItemId(502), 3)]);
    assert_eq!(pack.worn, vec![ItemId(501), ItemId(500)]);
    assert_eq!(pack.keys, vec![ItemId(172)]);
    assert!(pack.unresolved.is_empty(), "{:?}", pack.unresolved);
}

/// Coins are the purse's business and never an item.
#[test]
fn coins_are_not_items() {
    let pack = Pack::resolve(&content(), &inventory(&["38 runic coins"], &[]));
    assert!(pack.carried.is_empty());
    assert!(pack.unresolved.is_empty());
}

/// A name the table does not carry is kept as text, never guessed at.
#[test]
fn an_unknown_name_is_kept_unresolved() {
    let pack = Pack::resolve(&content(), &inventory(&["glowing orb"], &["bent key"]));
    assert!(pack.carried.is_empty());
    assert!(pack.keys.is_empty());
    assert_eq!(
        pack.unresolved,
        vec!["glowing orb".to_string(), "bent key".to_string()]
    );
}

/// A count prefix is a count, and two listings of one item add up.
#[test]
fn counts_accumulate_per_item() {
    let pack = Pack::resolve(&content(), &inventory(&["2 torch", "torch"], &[]));
    assert_eq!(pack.carried, vec![(ItemId(502), 3)]);
}

#[test]
fn has_answers_over_carried_worn_and_keys() {
    let pack = Pack::resolve(
        &content(),
        &inventory(&["torch", "silk gloves (Hands)"], &["black star key"]),
    );
    assert!(pack.has(ItemId(502)));
    assert!(pack.has(ItemId(501)));
    assert!(pack.has(ItemId(172)));
    assert!(!pack.has(ItemId(500)));
}

/// The handle every navigator and bot for one character shares. A
/// refresh on one clone is seen by every other.
#[test]
fn a_handle_refresh_is_shared_by_its_clones() {
    let handle = PackHandle::new(Arc::new(content()));
    let other = handle.clone();
    assert!(!other.has(ItemId(172)));
    handle.refresh(&inventory(&[], &["black star key"]));
    assert!(other.has(ItemId(172)));
    assert_eq!(other.snapshot().keys, vec![ItemId(172)]);
}

/// `with_table` swaps the item table but keeps the SAME shared pack —
/// the mechanism `Session::set_content` uses on a second call so a
/// handle taken out before it is not orphaned by the new table.
#[test]
fn with_table_shares_the_pack_with_a_second_table() {
    let handle = PackHandle::new(Arc::new(content()));
    handle.refresh(&inventory(&[], &["black star key"]));

    let mut richer = content();
    richer.add_item(item(600, "second key", 7));
    let retabled = handle.with_table(Arc::new(richer));

    // The table swapped, but the reading already resolved rides along
    // -- `with_table` shares the pack, it does not reset it.
    assert!(retabled.has(ItemId(172)));

    // And the two share one pack: a refresh through either is seen by
    // both.
    retabled.refresh(&inventory(&[], &["second key"]));
    assert!(
        handle.has(ItemId(600)),
        "the original handle did not see the shared refresh"
    );
}
