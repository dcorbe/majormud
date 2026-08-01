//! Which monster does this line say died?
//!
//! Deliberately NOT [`crate::bot::is_kill_line`], which answers "did OUR
//! fight end". That one leans on the experience award, and an award only
//! fires for a kill we landed; its phrase half ("falls to the ground")
//! covers 67 of 1085 templates. Another player's kill therefore produced
//! no signal at all, and the maintained room model kept the corpse
//! forever — 10 overclaims across 27 blocks of a room shared with one
//! other player (`tests/world_corpus.rs`).
//!
//! The two questions must stay apart. Broadening `is_kill_line` to cover
//! anyone's kill would unlatch [`crate::bot::Bot`] from a fight that is
//! still going the moment somebody else finished something nearby.
//!
//! The wordings are DATA, not grammar: `monster.deathmsg` indexes the
//! `message` table and the text is its third line, beside the two attack
//! forms. A death line names the TEMPLATE and never the rolled adjective
//! ("The acid slime dissolves..." while the room lists "large acid
//! slime", verified in cwrun2.raw), so exact text is enough to identify
//! it.

use std::collections::HashMap;
use std::path::Path;
use std::sync::OnceLock;

/// The run's lexicon.
///
/// Process-wide because it is immutable content read once from the world
/// database, and because the alternative is threading it through
/// `farm_stop`'s sixteen parameters to reach the one place a
/// [`crate::world::Here`] is built — and through the TUI to reach the
/// other. `mmc` runs one profile per process, so there is nothing to
/// key it by.
///
/// Uninitialised it answers `None`, which leaves every consumer on the
/// behaviour it had before this module existed.
static LEXICON: OnceLock<DeathLexicon> = OnceLock::new();

/// Load the world data's death table for the rest of the process.
///
/// Idempotent by construction: the second caller's data is dropped, so a
/// TUI that starts a farm after already having one does not reload.
pub fn init(db: &Path) -> Result<(), String> {
    if LEXICON.get().is_some() {
        return Ok(());
    }
    let lex = DeathLexicon::load(db)?;
    let _ = LEXICON.set(lex);
    Ok(())
}

/// The monster this line announces the death of, per the loaded lexicon.
pub fn killed(line: &str) -> Option<&'static str> {
    LEXICON.get()?.killed(line)
}

/// Test seam: install a lexicon when there is no world database to read.
/// Same one-shot semantics as [`init`].
pub fn init_with(lex: DeathLexicon) {
    let _ = LEXICON.set(lex);
}

/// Every death wording the shipped world data knows, and whose it is.
#[derive(Debug, Default)]
pub struct DeathLexicon {
    /// Death line, lowercased and trimmed -> monster name. Several
    /// monsters share a wording (606 distinct texts across 1006
    /// monsters); the last one loaded wins, which costs nothing —
    /// consumers use the name to pick an occupant out of a room the
    /// board already listed.
    by_line: HashMap<String, String>,
}

fn key(line: &str) -> String {
    line.trim().to_lowercase()
}

impl DeathLexicon {
    pub fn from_pairs(pairs: impl IntoIterator<Item = (String, String)>) -> Self {
        DeathLexicon {
            by_line: pairs
                .into_iter()
                .map(|(name, line)| (key(&line), name))
                .collect(),
        }
    }

    /// Read the death table out of the shipped world database.
    ///
    /// Read-only, like every other content load here. A monster with no
    /// death message (95 of 1101) simply contributes nothing.
    pub fn load(db: &Path) -> Result<Self, String> {
        let conn =
            rusqlite::Connection::open_with_flags(db, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
                .map_err(|e| format!("open {}: {e}", db.display()))?;
        let mut stmt = conn
            .prepare(
                "select lower(m.name), g.messageline3 \
                 from monster m join message g on g.number = m.deathmsg \
                 where m.name != '' and trim(coalesce(g.messageline3,'')) != ''",
            )
            .map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })
            .map_err(|e| e.to_string())?;
        let mut by_line = HashMap::new();
        for row in rows {
            let (name, line) = row.map_err(|e| e.to_string())?;
            by_line.insert(key(&line), name);
        }
        Ok(DeathLexicon { by_line })
    }

    /// The monster this line announces the death of, if any.
    pub fn killed(&self, line: &str) -> Option<&str> {
        self.by_line.get(&key(line)).map(String::as_str)
    }

    pub fn len(&self) -> usize {
        self.by_line.len()
    }

    pub fn is_empty(&self) -> bool {
        self.by_line.is_empty()
    }
}
