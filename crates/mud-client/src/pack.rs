//! What the character carries, by item id.
//!
//! The board prints display names ("ninjato (Weapon Hand)", "3 torch")
//! and the world database keys everything by id. Routing has to ask
//! "does this walker hold item 172" for a key door, and the bot has to
//! ask "is that thing on the floor a key I lack", and neither should be
//! comparing strings inside a loop. So the join is done once per
//! inventory reply, here, and the answer is shared through a
//! [`PackHandle`] the same way the toll log is shared: one handle per
//! character, refreshed by whichever reply arrives.
//!
//! Resolution is [`crate::items::resolve`]'s exact match. A name the
//! table cannot place is kept as text in `unresolved` so a bad parse
//! shows up rather than silently costing an item.

use std::sync::{Arc, Mutex};

use mud_core::content::{Content, Item, ItemId};

use crate::sheet::Inventory;

/// The item type of a key, `item.type` in the shipped table: 88 rows,
/// "adamantite key" through "yellow key".
pub const KEY_TYPE: i16 = 7;

/// Is this a key, the kind that lives on the ring and opens a type 2
/// door?
pub fn is_key(item: &Item) -> bool {
    item.item_type == KEY_TYPE
}

/// The character's carried contents, resolved to ids.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Pack {
    /// Loose items with their counts.
    pub carried: Vec<(ItemId, u32)>,
    /// Worn or wielded items, listed with a slot suffix.
    pub worn: Vec<ItemId>,
    /// The key ring.
    pub keys: Vec<ItemId>,
    /// Names the table could not place. Kept so a parse failure is
    /// visible, never counted as an item.
    pub unresolved: Vec<String>,
}

impl Pack {
    /// Resolve one inventory reading against the item table.
    ///
    /// Coins are skipped: they are the purse's business and the table
    /// has no rows for them. Everything else resolves exactly or lands
    /// in `unresolved`.
    pub fn resolve(content: &Content, inv: &Inventory) -> Pack {
        let mut pack = Pack::default();
        for entry in &inv.items {
            if crate::purse::parse_coin_line(entry).is_some() {
                continue;
            }
            let Some(item) = crate::items::resolve(content, entry) else {
                pack.unresolved.push(entry.clone());
                continue;
            };
            if is_worn(entry) {
                pack.worn.push(item.id);
                continue;
            }
            let count = leading_count(entry);
            match pack.carried.iter_mut().find(|(id, _)| *id == item.id) {
                Some((_, n)) => *n += count,
                None => pack.carried.push((item.id, count)),
            }
        }
        for key in &inv.keys {
            match crate::items::resolve(content, key) {
                Some(item) => pack.keys.push(item.id),
                None => pack.unresolved.push(key.clone()),
            }
        }
        pack
    }

    /// Does the character hold this item anywhere: loose, worn, or on
    /// the ring?
    pub fn has(&self, id: ItemId) -> bool {
        self.carried.iter().any(|(i, _)| *i == id)
            || self.worn.contains(&id)
            || self.keys.contains(&id)
    }
}

/// A worn or wielded entry carries its slot in a trailing parenthesis:
/// "silk gloves (Hands)", "ninjato (Weapon Hand)".
fn is_worn(entry: &str) -> bool {
    let entry = entry.trim_end();
    entry.ends_with(')') && entry.contains(" (")
}

/// "3 torch" is three torches. An entry with no count is one.
fn leading_count(entry: &str) -> u32 {
    entry
        .split_whitespace()
        .next()
        .and_then(|w| w.parse().ok())
        .unwrap_or(1)
}

/// One character's pack, shared.
///
/// `Arc` for the same reason the toll log is: every navigator and bot
/// built over a session's lifetime must see the same pack, and the
/// session refreshes it on every inventory reply. The content table
/// rides along so a holder can resolve a floor listing without being
/// handed the table separately.
#[derive(Clone)]
pub struct PackHandle {
    content: Arc<Content>,
    pack: Arc<Mutex<Pack>>,
}

impl std::fmt::Debug for PackHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PackHandle")
            .field("pack", &self.snapshot())
            .finish()
    }
}

impl PackHandle {
    /// An empty pack over this item table.
    pub fn new(content: Arc<Content>) -> PackHandle {
        PackHandle {
            content,
            pack: Arc::new(Mutex::new(Pack::default())),
        }
    }

    /// Replace the pack with this reading. Always a replacement, never
    /// a merge: the reply is the whole truth about the pack.
    pub fn refresh(&self, inv: &Inventory) {
        *self.pack.lock().expect("pack lock") = Pack::resolve(&self.content, inv);
    }

    pub fn snapshot(&self) -> Pack {
        self.pack.lock().expect("pack lock").clone()
    }

    pub fn has(&self, id: ItemId) -> bool {
        self.pack.lock().expect("pack lock").has(id)
    }

    pub fn content(&self) -> &Arc<Content> {
        &self.content
    }

    /// A handle over a different item table, sharing THIS handle's
    /// pack rather than minting a new one.
    ///
    /// `Session::set_content` uses this on every call after the first:
    /// a fresh `PackHandle::new` would give the new table its own
    /// empty `Arc<Mutex<Pack>>`, and every bot or navigator already
    /// holding the earlier handle would keep watching a pack nothing
    /// refreshes again.
    pub fn with_table(&self, content: Arc<Content>) -> PackHandle {
        PackHandle {
            content,
            pack: Arc::clone(&self.pack),
        }
    }
}
