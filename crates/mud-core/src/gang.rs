//! Gangs / guild houses (`gangs.md`).
//!
//! A gang is one WCCGANG2 record (§0): leadership is string equality with
//! the leader-name field — there is no leader flag — and members carry the
//! gang's display name on their player record. This module holds the pure
//! record; membership commands, the exp-pool feed, and the guild-house
//! surfaces live in `game.rs`.

/// Gang record flag bits (`gang+0x50`, gangs.md §0).
pub const GANG_DISBANDED: u16 = 0x1;
/// Hidden from the top-gangs listing (the sysop DISABLE/ENABLE toggle —
/// schema-only in M7, gangs.md §7).
pub const GANG_HIDDEN: u16 = 0x4;
/// Primary exp pool saturated; the secondary pool is live (§2.1).
pub const GANG_SATURATED: u16 = 0x8;

/// Player gang/rank word bits (`player+0x7d4`, gangs.md §0 — word-relative
/// values from the 2026-07-30 decompile pass; the high byte is `+0x7d5`).
pub const GF_LIEUTENANT: u16 = 0x0100;
/// Pending promote for an offline target, applied at login (§0 handler
/// step 1).
pub const GF_PENDING_PROMOTE: u16 = 0x0200;
/// Pending demote, applied at login (step 2).
pub const GF_PENDING_DEMOTE: u16 = 0x0400;
/// Pending "Your ganghouse has been closed down!!" notice (step 3).
pub const GF_NOTICE_HOUSE_CLOSED: u16 = 0x0800;
/// Pending "Gang house items have dissappeared from your inventory!"
/// notice (step 4; the misspelling is the DLL's).
pub const GF_NOTICE_ITEMS_GONE: u16 = 0x1000;
/// Outstanding gang-shop paperwork — deed-purchase refusal 7 (§3.1). Set
/// by selling to the deed shop; cleared by the unported tax lifecycle
/// (§8, M8).
pub const GF_PAPERWORK: u16 = 0x4000;
/// Roster view online-only (clear = all members); toggled by SET (§0).
pub const GF_ROSTER_ONLINE_ONLY: u16 = 0x0008;

/// One gang — the WCCGANG2 record (gangs.md §0), persisted in the
/// state.sqlite `gang` table.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Gang {
    /// `+0x00` — the uppercase lookup key (Btrieve key 0).
    pub name_key: String,
    /// `+0x14` — the display name (≤19 chars, caller-cased); this string
    /// is what members carry in `player.gang`.
    pub display: String,
    /// `+0x28` — primary experience pool; saturates at `u32::MAX` (§2.1).
    pub exp_pool: u32,
    /// `+0x58` — secondary (overflow) pool, live once saturated.
    pub secondary_pool: u32,
    /// `+0x5c` — secondary-pool wrap counter ("×N times").
    pub wrap: u16,
    /// `+0x2c` — leader character name; leadership IS this string.
    pub leader: String,
    /// `+0x4a` — creation date, as wall-clock seconds in the port (the
    /// DLL stores a `today()` date word; ours carries second precision
    /// through `CoreConfig.wall_base`).
    pub created: i64,
    /// `+0x4e` — member count. Bookkeeping only: the DLL enforces no cap
    /// anywhere (§1.3), and offline members of a disbanded gang decrement
    /// it one by one at login (§0 step 5).
    pub member_count: u16,
    /// `+0x50` — the `GANG_*` flag bits.
    pub flags: u16,
}

impl Gang {
    /// Build the record `cmd_create` writes (§1.1): count 1, zero pools,
    /// zero flags, creator as leader. Name validation (length, printable,
    /// not "None", unique) is the command's job, not the record's.
    pub fn new(display: &str, leader: &str, created: i64) -> Gang {
        Gang {
            name_key: display.to_uppercase(),
            display: display.to_string(),
            exp_pool: 0,
            secondary_pool: 0,
            wrap: 0,
            leader: leader.to_string(),
            created,
            member_count: 1,
            flags: 0,
        }
    }

    pub fn is_disbanded(&self) -> bool {
        self.flags & GANG_DISBANDED != 0
    }

    pub fn is_hidden(&self) -> bool {
        self.flags & GANG_HIDDEN != 0
    }

    pub fn is_saturated(&self) -> bool {
        self.flags & GANG_SATURATED != 0
    }

    /// gangs.md §0: leadership ≡ `gang+0x2c == player+0x1e`.
    pub fn is_leader(&self, player_name: &str) -> bool {
        self.leader == player_name
    }

    /// The per-kill pool feed (§2.1, award block 11154). Unsaturated:
    /// primary accumulates; crossing `u32::MAX` pins the primary, rolls
    /// the excess (the amount beyond the max) into the secondary, and
    /// sets flag 0x8. Saturated: the secondary accumulates, wrapping with
    /// `wrap` counting each wrap.
    pub fn add_exp(&mut self, award: u32) {
        if !self.is_saturated() {
            match self.exp_pool.checked_add(award) {
                Some(v) => self.exp_pool = v,
                None => {
                    let excess = award - (u32::MAX - self.exp_pool);
                    self.exp_pool = u32::MAX;
                    self.secondary_pool = excess;
                    self.flags |= GANG_SATURATED;
                }
            }
        } else {
            match self.secondary_pool.checked_add(award) {
                Some(v) => self.secondary_pool = v,
                None => {
                    self.secondary_pool = self.secondary_pool.wrapping_add(award);
                    self.wrap += 1;
                }
            }
        }
    }
}
