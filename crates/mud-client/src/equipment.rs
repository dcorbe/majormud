//! What the character is currently wielding.
//!
//! Equipment is authoritative, unlike carried contents
//! ([`crate::sheet::Inventory`]): **nothing in the game force-unequips
//! gear** (`GAME_MECHANICS.md` §Equipment & gear, `[CONFIRMED]`), so
//! state changes only from commands we ourselves issue. That is what
//! lets this be tracked with confidence instead of re-asked for on
//! every decision.
//!
//! `eq`/`wield`/`arm` into an occupied slot swaps -- the displaced item
//! returns to the pack, no `rem` first -- and the board confirms the
//! swap with exactly one line, `You are now holding <new>.`, that
//! **never names what it took off**. The model remembers the
//! displacement from its own prior state; there is nothing on the wire
//! to read it from.
//!
//! Scoped to the wielded weapon only. Worn armour has the same
//! trade-places behaviour, but which slot a worn item occupies is a
//! `Content::items` fact this module has no dependency on (Task 2 runs
//! before the item-identity layer); the backstab decision this feeds
//! only ever needs to know what is wielded.

/// The wielded weapon, tracked from our own equip confirmations.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Equipment {
    weapon: Option<String>,
}

impl Equipment {
    pub fn new() -> Equipment {
        Equipment::default()
    }

    /// What is wielded now, as this client last confirmed it. `None`
    /// until a first equip is confirmed, or the character is genuinely
    /// unarmed.
    pub fn weapon(&self) -> Option<&str> {
        self.weapon.as_deref()
    }

    /// Confirmed by the board's own `You are now holding <name>.`: the
    /// new weapon is now wielded, and whatever was wielded before --
    /// if anything -- returned to the pack. Returns that displaced item
    /// so the caller can plan a swap-back, since the board's own line
    /// never names it.
    pub fn confirm_weapon(&mut self, name: &str) -> Option<String> {
        self.weapon.replace(name.to_string())
    }

    /// Feed one line from the board. Only a line that IS the equip
    /// confirmation updates the model -- a refusal ("You do not have %s
    /// left unequipped.", "You may not use that weapon!") does not match
    /// [`parse_now_holding`] and is left alone. That is the whole of
    /// "a refused equip must not update the model": there is no refusal
    /// branch to get wrong, because the model only ever moves off a
    /// positive confirmation in the first place.
    pub fn observe(&mut self, line: &str) -> Option<String> {
        parse_now_holding(line).and_then(|name| self.confirm_weapon(&name))
    }

    /// Seed the model from an `i`/`inventory` listing, for the one thing
    /// [`Self::observe`] can never learn on its own: what was already
    /// wielded before this client ever connected. Only takes if nothing
    /// has confirmed a weapon yet -- past the first listing, `"nothing
    /// in the game force-unequips gear"` (this module's own doc) makes
    /// our own equip confirmations the only trustworthy source, and a
    /// later listing re-parsed on top of them could only ever agree or
    /// go stale between the read and the moment it lands.
    pub fn seed(&mut self, items: &[String]) {
        if self.weapon.is_none() {
            self.weapon = wielded_from_listing(items);
        }
    }
}

/// Which of `items` (as printed by an `i`/`inventory` listing --
/// comma-split, wrap-rejoined, `sheet::Inventory::items`'s own shape)
/// names the WIELDED weapon.
///
/// The board marks an equipped item, worn or wielded, with a trailing
/// `" (<Slot>)"`; a carried-but-unequipped item prints none at all.
/// Established against MudPlay's own `i`-listing parser
/// (`archive/FujiTerm/MudPlay/Game/Inventory/InventoryManager.cs`'s
/// `EquippedSlotRegex`: an enumerated, closed set of 21 body slots plus
/// the two weapon-hand spellings) and its `GAME_MECHANICS.md`
/// "Equipment & gear" `[CONFIRMED]` notes -- nothing in this repo's own
/// fixtures settles whether a carried-but-unworn item ever gets a
/// marker too, so that source is the one that does. A one-handed weapon
/// marks `"(Weapon Hand)"`; a two-handed one marks `"(Two handed)"`
/// (`crates/mud-client/tests/contents.rs`'s `quarterstaff (Two
/// handed)`); both mean the same thing, wielded rather than merely
/// carried, so either one identifies the entry.
///
/// Returns the bare name, marker stripped -- the same shape
/// [`Equipment::confirm_weapon`] already stores off `"You are now
/// holding <name>."`.
pub fn wielded_from_listing(items: &[String]) -> Option<String> {
    items.iter().find_map(|item| {
        item.strip_suffix(" (Weapon Hand)")
            .or_else(|| item.strip_suffix(" (Two handed)"))
            .map(str::to_string)
    })
}

/// `mud_core::text::now_holding`'s exact wrapper, restated here rather
/// than imported -- the same choice `sheet::Inventory` already makes for
/// `CARRYING_PREFIX`-shaped lines: a wording change in `mud-core` is a
/// one-line fix in whichever crate actually notices, not a break the
/// compiler catches for free either way.
const NOW_HOLDING_PREFIX: &str = "You are now holding ";

fn parse_now_holding(line: &str) -> Option<String> {
    let rest = line.trim().strip_prefix(NOW_HOLDING_PREFIX)?;
    let name = rest.strip_suffix('.')?;
    if name.is_empty() {
        return None;
    }
    Some(name.to_string())
}
