//! What the character is carrying and what it knows how to cast.
//!
//! Both are read from the board's own listings rather than tracked
//! incrementally, because incremental tracking drifts: items are dropped
//! by the board on death, consumed, stolen, and burned out, and none of
//! those announce themselves reliably. Asking is cheap and always right.
//!
//! The immediate use is darkness. `1/2156` Small Cavern has `light =
//! -200`, and a character with no light gets "The room is very dark - you
//! can't see anything" and NO room block at all — so verified navigation
//! cannot confirm an arrival and the bot cannot see what is in the room.
//! Knowing whether a torch or a light spell is available is what turns
//! that from a dead end into a decision.

/// Light-source items are `item.type == 6` in the shipped data. These are
/// the ones that exist: torch (175, 800 uses), lantern (176, 2400), brass
/// lamp (286, 1800), moon-lamp (1153, 4000), scaled lantern (1233, 6000).
///
/// Matched on the trailing noun of the display name, so a rolled or
/// adjectived instance ("a battered torch") still counts — the same rule
/// the bot's targeting uses, and for the same reason.
const LIGHT_ITEMS: [&str; 4] = ["torch", "lantern", "lamp", "moon-lamp"];

/// Spells that light a room. `starlight` (spell 26) is the one the
/// shipped book actually offers; the others are named for when a caster
/// turns up with them.
const LIGHT_SPELLS: [&str; 3] = ["starlight", "light", "continual light"];

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Inventory {
    /// Carried items as the board lists them, wrapping rejoined.
    pub items: Vec<String>,
    /// `Encumbrance: 119/2880` — carried and capacity.
    pub encumbrance: Option<(i64, i64)>,
}

impl Inventory {
    /// Parse the reply to `inv`.
    pub fn parse(text: &str) -> Inventory {
        let mut inv = Inventory::default();
        let mut carrying: Option<String> = None;
        for line in text.lines() {
            let line = line.trim_end();
            if let Some(rest) = line.trim_start().strip_prefix("You are carrying ") {
                carrying = Some(rest.to_string());
                continue;
            }
            if let Some(rest) = line.trim_start().strip_prefix("Encumbrance: ") {
                inv.encumbrance = rest
                    .split_once(&['/', ' '][..])
                    .and_then(|(a, b)| {
                        let cap = b.split(|c: char| !c.is_ascii_digit()).next()?;
                        Some((a.trim().parse().ok()?, cap.parse().ok()?))
                    });
                continue;
            }
            // The carried list wraps at the terminal width, mid-item, so
            // anything before the next known heading belongs to it.
            if let Some(acc) = carrying.as_mut() {
                let ends = line.trim_start();
                if ends.starts_with("You have")
                    || ends.starts_with("Wealth:")
                    || ends.starts_with("Encumbrance:")
                    || ends.is_empty()
                {
                    carrying = Some(std::mem::take(acc));
                    // Fall through so the heading is still examined.
                    if ends.starts_with("Encumbrance:") {
                        continue;
                    }
                    continue;
                }
                acc.push(' ');
                acc.push_str(ends);
            }
        }
        if let Some(list) = carrying {
            let list = list.trim().trim_end_matches('.');
            if !list.is_empty() && list != "nothing" {
                inv.items = list
                    .split(',')
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
                    .collect();
            }
        }
        inv
    }

    /// The first carried thing that would light a dark room, named as the
    /// board would want it referred to.
    pub fn light_source(&self) -> Option<String> {
        self.items.iter().find_map(|item| {
            let noun = item.split_whitespace().next_back()?.to_lowercase();
            LIGHT_ITEMS
                .iter()
                .find(|l| noun == **l)
                .map(|l| (*l).to_string())
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KnownSpell {
    pub level: i16,
    pub mana: i16,
    /// The short name, which is what `cast` takes.
    pub short: String,
    pub name: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Spellbook {
    pub spells: Vec<KnownSpell>,
}

impl Spellbook {
    /// Parse the reply to `spells`.
    ///
    /// Rows are fixed-width (`mud_core::text::spell_row`: level in 3,
    /// mana in 4, four spaces, short in 6, name in 30) but parsed by
    /// whitespace rather than by column, so a differently padded build
    /// still reads. The name may contain spaces, so it is whatever
    /// remains after the first three fields.
    pub fn parse(text: &str) -> Spellbook {
        let mut book = Spellbook::default();
        for line in text.lines() {
            let line = line.trim();
            if line.is_empty()
                || line.starts_with("You have the following")
                || line.starts_with("Level")
                || line.starts_with("You have no spells")
            {
                continue;
            }
            let mut parts = line.split_whitespace();
            let (Some(level), Some(mana), Some(short)) =
                (parts.next(), parts.next(), parts.next())
            else {
                continue;
            };
            let (Ok(level), Ok(mana)) = (level.parse::<i16>(), mana.parse::<i16>()) else {
                continue;
            };
            let name = parts.collect::<Vec<_>>().join(" ");
            if name.is_empty() {
                continue;
            }
            book.spells.push(KnownSpell {
                level,
                mana,
                short: short.to_string(),
                name,
            });
        }
        book
    }

    /// The short name of a spell that would light a dark room.
    pub fn light_spell(&self) -> Option<String> {
        self.spells
            .iter()
            .find(|s| LIGHT_SPELLS.contains(&s.name.to_lowercase().as_str()))
            .map(|s| s.short.clone())
    }
}
