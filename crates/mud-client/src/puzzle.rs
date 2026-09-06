//! Buttons and levers: the remote actions that open an exit somewhere.
//!
//! A room's exit slot with type 12 is not an exit. It is a control: a
//! phrase spoken in that room clears bits of another exit's concealment
//! word, or toggles a gate's lock. The graph decodes every slot at load
//! and hangs the result on the exit it opens as
//! [`crate::graph::ExitRequirement::Puzzle`]. This module holds the
//! shape and the one pure question routing and walking both ask: which
//! actions does this walker have to perform, and in what order.
//!
//! The bit rules are mud-core's `remote_lever`, which follows the DLL at
//! 65973 to 66079. Action n clears bit n+3 of the word, so action 1
//! clears 0x10 and action 10 clears 0x2000. Action 0 clears every
//! puzzle bit at once. Unless the exit's para2 is negative, action n
//! only works while bit n+4 is already clear, so levers pull in
//! descending order. Descending is valid either way, which is why the
//! plan never asks whether the puzzle is ordered.
//!
//! A gate is different: `remote_gate_toggle` flips its lock on every
//! action regardless of number, and its word carries no puzzle bits. The
//! plan reads that as "any one action".

use mud_core::content::{ItemId, RoomId};

use crate::graph::{Capabilities, Cost};

/// The bits of a hidden exit's state word that a lever clears. Bit 2 is
/// the searchable flag and is not among them: a search never touches a
/// puzzle bit and a lever never touches bit 2.
pub const PUZZLE_BITS: u32 = 0x3ff0;

/// The highest action number a word can call for, the one that clears
/// bit 0x2000.
const MAX_ACTION: u8 = 10;

/// One phrase spoken in one room, and what it does to the exit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PuzzleAction {
    /// The room the phrase must be spoken in.
    pub room: RoomId,
    /// 0 clears every puzzle bit. 1 to 10 clear one bit each.
    pub number: u8,
    /// Line 1 of the slot's message, then line 2 when it is not blank.
    /// The walker speaks the first. The rest are aliases the board also
    /// accepts.
    pub phrases: Vec<String>,
    /// An item the actor must carry, when the slot names one.
    pub item: Option<ItemId>,
    /// Line 1 of the slot's response message: what the actor hears.
    pub reply: Option<String>,
    /// Steps from the exit's own room to `room`, counted at load.
    /// `None` when no walk reaches it. Zero for a button in the exit's
    /// own room, which is 200 of the 287 shipped slots.
    pub hops: Option<u32>,
}

impl PuzzleAction {
    /// The word bits this action clears.
    pub fn bit(&self) -> u32 {
        match self.number {
            0 => PUZZLE_BITS,
            n => 0x10 << (n - 1),
        }
    }

    /// Can this walker perform the action: no item needed, or the item
    /// in the pack.
    pub fn available(&self, caps: &Capabilities) -> bool {
        self.item.is_none_or(|id| caps.has_item(id))
    }
}

/// Everything that opens one exit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Puzzle {
    /// The exit's para1. On a hidden exit it is the concealment word.
    /// On a gate it is not a word at all and carries no puzzle bits.
    pub word: u32,
    /// Every action that targets this exit, nearest first.
    pub actions: Vec<PuzzleAction>,
}

impl Puzzle {
    /// The action numbers the word calls for, ascending.
    pub fn needed(&self) -> Vec<u8> {
        (1..=MAX_ACTION)
            .filter(|n| self.word & (0x10 << (n - 1)) != 0)
            .collect()
    }

    /// Which actions this walker performs, in order. `None` means the
    /// walker cannot open the exit at all.
    ///
    /// A word with no puzzle bits, which is every gate and a hidden exit
    /// nobody armed, opens on any one action. Otherwise an available
    /// action 0 does the whole job in one phrase, and failing that every
    /// needed bit must have its own available action. The order is
    /// descending by number.
    pub fn plan(&self, caps: &Capabilities) -> Option<Vec<&PuzzleAction>> {
        let needed = self.needed();
        if needed.is_empty() {
            return self
                .actions
                .iter()
                .find(|a| a.available(caps))
                .map(|a| vec![a]);
        }
        if let Some(all) = self
            .actions
            .iter()
            .find(|a| a.number == 0 && a.available(caps))
        {
            return Some(vec![all]);
        }
        needed
            .iter()
            .rev()
            .map(|n| {
                self.actions
                    .iter()
                    .find(|a| a.number == *n && a.available(caps))
            })
            .collect()
    }

    /// What routing pays to take the exit: `base` for the exit itself,
    /// then one step per phrase and the walk to each action room and
    /// back. `Impassable` when there is no plan, or when a planned
    /// action sits in a room no walk reaches.
    pub fn cost(&self, base: u32, caps: &Capabilities) -> Cost {
        let Some(plan) = self.plan(caps) else {
            return Cost::Impassable;
        };
        let mut total = base;
        for action in plan {
            let Some(hops) = action.hops else {
                return Cost::Impassable;
            };
            total += 1 + 2 * hops;
        }
        Cost::Steps(total)
    }
}
