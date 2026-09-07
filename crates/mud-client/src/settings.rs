//! Live settings: the profile as an editable TOML document.
//!
//! The document is the source of truth. `/set` writes into it, then the
//! whole document is re-parsed into a [`Profile`] and validated, so there
//! is one parser, one validator and no per-key setter. A `/save` writes
//! the document text, which is why the file's comments and key order
//! survive.

use std::path::{Path, PathBuf};
use std::str::FromStr;

use toml_edit::{DocumentMut, Item, Table};

use crate::profile::Profile;

/// Every key `/set` accepts, in file order. The listing walks it in this
/// order and completion offers it. Hand-written because Rust has no way
/// to enumerate struct fields, and pinned to the structs by the test
/// `every_key_the_profile_serialises_is_in_keys`.
pub const KEYS: &[&str] = &[
    "target",
    "host",
    "port",
    "username",
    "password",
    "pace_ms",
    "disable_evil_warnings",
    "scrollback_lines",
    "reconnect",
    "reconnect_delay_seconds",
    "bot.auto_combat",
    "bot.auto_heal",
    "bot.auto_get",
    "bot.take_keys",
    "bot.ignore_coins",
    "bot.auto_flee",
    "bot.minor_heal_at_percent",
    "bot.major_heal_at_percent",
    "bot.rest_at_percent",
    "bot.mana_rest_at_percent",
    "bot.rest_until_percent",
    "bot.meditate",
    "bot.flee_at_percent",
    "bot.rest_command",
    "bot.minor_heal_spell",
    "bot.major_heal_spell",
    "bot.hp_regen_spell",
    "bot.heal_spells",
    "bot.buffs",
    "bot.ignore",
    "bot.max_hp",
    "bot.max_mana",
    "bot.combat_idle_prompts",
    "bot.assist_play",
    "farm.content",
    "farm.start",
    "farm.circuit",
    "farm.finish_at",
    "farm.loops",
    "farm.max_seconds",
    "farm.stop_seconds",
    "farm.dwell_empty_seconds",
    "farm.depart_at_percent",
    "farm.slowdown_backoff_ms",
    "farm.idle_poke_ms",
    "farm.heal_retry_prompts",
    "farm.heal_refused",
    "farm.fight_while_travelling",
    "farm.interrupt_at_percent",
    "farm.travel_interrupts",
    "farm.max_rest_seconds",
    "farm.defend_seconds",
    "farm.nav.step_timeout_ms",
    "farm.nav.bash_doors",
    "farm.nav.search_hidden",
    "bank.auto_deposit",
    "bank.deposit_at_coins",
    "bank.deposit_on_weight_class",
    "bank.keep_gold",
    "bank.at",
];

/// Keys that were renamed, and what they are called now. Deserialisation
/// accepts both, so this exists to SAY SO: a profile that keeps working
/// while its vocabulary has moved on is a profile whose owner never finds
/// out about the new knob next to it. A warning, never a refusal.
const RENAMED_KEYS: [(&str, &str); 4] = [
    ("heal_at_percent", "rest_at_percent"),
    ("heal_command", "rest_command"),
    ("spell_at_percent", "minor_heal_at_percent"),
    ("heal_spells", "minor_heal_spell and major_heal_spell"),
];

/// The serde aliases: old spelling, current spelling. Setting the current
/// spelling removes the old one from the same table, because serde
/// rejects a table that carries both as a duplicate field.
const ALIASES: [(&str, &str); 3] = [
    ("heal_at_percent", "rest_at_percent"),
    ("heal_command", "rest_command"),
    ("spell_at_percent", "minor_heal_at_percent"),
];

/// Cloneable because `/new` opens a window on a copy of the lobby's
/// settings. `detached` is the copy it takes: the same document and the
/// same dirty flag, with no file behind it.
#[derive(Clone)]
pub struct Settings {
    doc: DocumentMut,
    profile: Profile,
    /// Where the text came from, or where it was last saved.
    path: Option<PathBuf>,
    /// Edited since the last load or save.
    dirty: bool,
}

impl Default for Settings {
    /// An empty document, which is the default profile.
    fn default() -> Self {
        Settings {
            doc: DocumentMut::new(),
            profile: Profile::default(),
            path: None,
            dirty: false,
        }
    }
}

impl Settings {
    pub fn parse(text: &str) -> Result<Settings, String> {
        let doc: DocumentMut = text
            .parse()
            .map_err(|e: toml_edit::TomlError| e.to_string())?;
        let profile = profile_of(&doc).map_err(ParseFailure::whole)?;
        Ok(Settings {
            doc,
            profile,
            path: None,
            dirty: false,
        })
    }

    pub fn profile(&self) -> &Profile {
        &self.profile
    }

    pub fn dirty(&self) -> bool {
        self.dirty
    }

    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    /// A copy with no file behind it. What `/new` opens a window on: the
    /// settings come from the lobby, but the file they came from is one
    /// character's profile, and a bare `/save` in the new window would
    /// write the new character over it. Without a path that `/save`
    /// refuses and asks for a name. Everything else is kept, the dirty
    /// flag included, so a window opened from unsaved edits knows it
    /// holds unsaved edits.
    pub fn detached(&self) -> Settings {
        Settings {
            path: None,
            ..self.clone()
        }
    }

    /// The document as it would be saved.
    pub fn text(&self) -> String {
        self.doc.to_string()
    }

    /// Read a profile file. The path is remembered for `save`.
    pub fn load(path: &Path) -> Result<Settings, String> {
        let text = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
        let mut settings = Settings::parse(&text)?;
        settings.path = Some(path.to_path_buf());
        Ok(settings)
    }

    /// Write the document. `None` means the remembered path, and with
    /// none remembered the caller has to name one. A path given here is
    /// remembered. A path that exists and is not the remembered one is
    /// refused: it is another character's file until it is loaded.
    pub fn save(&mut self, path: Option<&Path>) -> Result<PathBuf, String> {
        let path = match path.map(Path::to_path_buf).or_else(|| self.path.clone()) {
            Some(p) => p,
            None => return Err("no file to save to: /save <file>".into()),
        };
        // A file these settings did not come from is somebody's. The
        // resolver in `profile` turns a bare name into a path beside
        // every other profile, which is exactly where an unrelated
        // character's file already sits.
        if path.exists() && self.path.as_deref() != Some(path.as_path()) {
            return Err(format!(
                "{} exists and these settings were not loaded from it. /load it first, or pick another name.",
                path.display()
            ));
        }
        // Written to a sibling and renamed on, never onto the target
        // itself. A plain write truncates first, so a failure partway
        // through leaves the profile empty and the only copy of a
        // hand-written file gone. The temporary is a sibling so the
        // rename stays on one filesystem, which is where it is atomic.
        let temp = temp_path(&path);
        if let Err(e) = std::fs::write(&temp, self.doc.to_string()) {
            let _ = std::fs::remove_file(&temp);
            return Err(format!("{}: {e}", path.display()));
        }
        // The rename installs a new file, so the target's own mode has
        // to be carried onto it first. A profile chmod'd to 600 for the
        // password it holds would otherwise come back readable by
        // everyone on the machine.
        if let Ok(meta) = std::fs::metadata(&path) {
            let _ = std::fs::set_permissions(&temp, meta.permissions());
        }
        if let Err(e) = std::fs::rename(&temp, &path) {
            let _ = std::fs::remove_file(&temp);
            return Err(format!("{}: {e}", path.display()));
        }
        self.path = Some(path.clone());
        self.dirty = false;
        Ok(path)
    }

    /// One line per renamed key the document still uses.
    pub fn warnings(&self) -> Vec<String> {
        let mut out = Vec::new();
        collect_renamed(self.doc.as_table(), &mut out);
        out
    }

    /// Write one key. `text` is parsed as a TOML value. When that fails
    /// the whole of it is a string, so `rest` and `sit down` both work
    /// without quotes. Any failure, a bad type or a validator refusal,
    /// leaves the document as it was.
    pub fn set(&mut self, key: &str, text: &str) -> Result<(), String> {
        if !KEYS.contains(&key) {
            return Err(format!("unknown key {key}"));
        }
        let value: toml_edit::Value = match toml_edit::Value::from_str(text) {
            Ok(v) => v,
            Err(_) => toml_edit::Value::from(text),
        };
        let before = self.doc.clone();
        let (path, leaf) = split_key(key);
        let table = table_at(&mut self.doc, &path)?;
        table.insert(leaf, Item::Value(value));
        drop_aliases(table, leaf);
        self.reparse(key, before)
    }

    /// Remove one key so its default applies again. Removing a key that
    /// is not there is fine.
    pub fn unset(&mut self, key: &str) -> Result<(), String> {
        if !KEYS.contains(&key) {
            return Err(format!("unknown key {key}"));
        }
        let before = self.doc.clone();
        let (path, leaf) = split_key(key);
        if let Some(table) = existing_table(&mut self.doc, &path) {
            table.remove(leaf);
            drop_aliases(table, leaf);
        }
        self.reparse(key, before)
    }

    /// The document is a profile again, or it goes back to what it was.
    /// `key` names the edit, for a refusal that has to say what it was
    /// refusing without a span.
    fn reparse(&mut self, key: &str, before: DocumentMut) -> Result<(), String> {
        match profile_of(&self.doc) {
            Ok(profile) => {
                self.profile = profile;
                // Only a real change is unsaved work. Setting a key to
                // the value it already carries, or unsetting one that
                // was never there, used to arm the /quit refusal over a
                // document nobody had touched.
                if self.doc.to_string() != before.to_string() {
                    self.dirty = true;
                }
                Ok(())
            }
            Err(e) => {
                self.doc = before;
                Err(e.one_line(key))
            }
        }
    }

    /// Every key the pattern matches, with its value rendered as TOML,
    /// in `KEYS` order. Absent tables show their defaults, because that
    /// is what the character gets. The password shows as stars.
    pub fn list(&self, pattern: &str) -> Vec<(String, String)> {
        let table = self.effective_table();
        KEYS.iter()
            .filter(|key| glob_match(pattern, key))
            .map(|key| (key.to_string(), render(&table, key)))
            .collect()
    }

    /// One key's rendered value, or `None` for a key `/set` does not know.
    pub fn value(&self, key: &str) -> Option<String> {
        if !KEYS.contains(&key) {
            return None;
        }
        Some(render(&self.effective_table(), key))
    }

    /// The profile with its optional tables filled in, as a TOML table,
    /// so the listing reads what the character will actually get.
    fn effective_table(&self) -> toml::Table {
        let mut profile = self.profile.clone();
        profile.bot.get_or_insert_with(Default::default);
        profile.farm.get_or_insert_with(Default::default);
        let text = toml::to_string(&profile).expect("a profile serialises");
        text.parse().expect("a serialised profile parses")
    }
}

/// Where `save` writes before it renames onto the target: the target's
/// own name with `.tmp` on the end, in the target's directory.
fn temp_path(path: &Path) -> PathBuf {
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(".tmp");
    path.with_file_name(name)
}

/// Why a document is not a profile. The two are told apart because they
/// are read in different places: toml's error carries a span, which is
/// worth printing against a file the operator can open and is noise
/// against a `/set` they just typed.
enum ParseFailure {
    Toml(toml::de::Error),
    Invalid(String),
}

impl ParseFailure {
    /// For a file. The span names the line and column to go and look at.
    fn whole(self) -> String {
        match self {
            ParseFailure::Toml(e) => e.to_string(),
            ParseFailure::Invalid(s) => s,
        }
    }

    /// For one edit. Toml's rendering is five lines with a caret ruler
    /// through a document the operator never sees, so the key it was
    /// typed against stands in for the span.
    fn one_line(self, key: &str) -> String {
        match self {
            ParseFailure::Toml(e) => format!("{key}: {}", e.message()),
            ParseFailure::Invalid(s) => s,
        }
    }
}

/// The one parse and the one validation, shared by every edit and every
/// load. `Profile::load` used to hold this.
fn profile_of(doc: &DocumentMut) -> Result<Profile, ParseFailure> {
    let mut profile: Profile =
        toml::from_str(&doc.to_string()).map_err(ParseFailure::Toml)?;
    if let Some(bot) = &mut profile.bot {
        bot.normalise();
        bot.validate().map_err(ParseFailure::Invalid)?;
    }
    profile.bank.validate().map_err(ParseFailure::Invalid)?;
    Ok(profile)
}

/// `bot.rest_at_percent` is the table path `["bot"]` and the leaf
/// `rest_at_percent`. A bare key has an empty path.
fn split_key(key: &str) -> (Vec<&str>, &str) {
    let mut parts: Vec<&str> = key.split('.').collect();
    let leaf = parts.pop().unwrap_or(key);
    (parts, leaf)
}

/// The table at `path`, created on the way down. A created table is
/// implicit, so `[farm]` gets no header of its own when only
/// `[farm.nav]` has keys.
fn table_at<'a>(doc: &'a mut DocumentMut, path: &[&str]) -> Result<&'a mut Table, String> {
    let mut table = doc.as_table_mut();
    for seg in path {
        let item = table.entry(seg).or_insert_with(|| {
            let mut t = Table::new();
            t.set_implicit(true);
            Item::Table(t)
        });
        table = item
            .as_table_mut()
            .ok_or_else(|| format!("{seg} is not a table in this profile"))?;
    }
    Ok(table)
}

fn existing_table<'a>(doc: &'a mut DocumentMut, path: &[&str]) -> Option<&'a mut Table> {
    let mut table = doc.as_table_mut();
    for seg in path {
        table = table.get_mut(seg)?.as_table_mut()?;
    }
    Some(table)
}

fn drop_aliases(table: &mut Table, leaf: &str) {
    for (old, new) in ALIASES {
        if new == leaf {
            table.remove(old);
        }
    }
}

fn collect_renamed(table: &Table, out: &mut Vec<String>) {
    for (key, item) in table.iter() {
        if let Some((_, new)) = RENAMED_KEYS.iter().find(|(old, _)| *old == key) {
            out.push(format!("`{key}` is now `{new}` (still accepted)"));
        }
        if let Some(t) = item.as_table() {
            collect_renamed(t, out);
        }
    }
}

/// irssi's `/set` pattern: `*` matches anything and a trailing `*` is
/// implied, so `bot` lists the bot table and `*heal*` finds every heal
/// key in every table.
pub fn glob_match(pattern: &str, key: &str) -> bool {
    let mut rest = key;
    for (i, part) in pattern.split('*').enumerate() {
        if i == 0 {
            match rest.strip_prefix(part) {
                Some(r) => rest = r,
                None => return false,
            }
        } else {
            match rest.find(part) {
                Some(at) => rest = &rest[at + part.len()..],
                None => return false,
            }
        }
    }
    true
}

fn render(table: &toml::Table, key: &str) -> String {
    match lookup(table, key) {
        Some(value) if key == "password" => match value.as_str() {
            Some("") => "\"\"".to_string(),
            _ => "\"****\"".to_string(),
        },
        Some(value) => value.to_string(),
        None => "unset".to_string(),
    }
}

fn lookup<'a>(table: &'a toml::Table, key: &str) -> Option<&'a toml::Value> {
    let mut parts = key.split('.');
    let mut value = table.get(parts.next()?)?;
    for part in parts {
        value = value.as_table()?.get(part)?;
    }
    Some(value)
}

/// What Tab does to the word under the cursor. `start..end` are
/// character offsets of that word. `text` replaces it. `list` is empty
/// unless the word could not grow, in which case it is the candidates
/// to print.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Completion {
    pub start: usize,
    pub end: usize,
    pub text: String,
    pub list: Vec<String>,
}

/// Complete the word under the cursor. The first word of a slash line
/// completes against `verbs`. The second word after `/set` or `/unset`
/// completes against [`KEYS`]. Anything else is `None`.
pub fn complete(line: &str, cursor: usize, verbs: &[&str]) -> Option<Completion> {
    let chars: Vec<char> = line.chars().collect();
    let cursor = cursor.min(chars.len());
    let start = chars[..cursor]
        .iter()
        .rposition(|c| c.is_whitespace())
        .map_or(0, |i| i + 1);
    let end = chars[cursor..]
        .iter()
        .position(|c| c.is_whitespace())
        .map_or(chars.len(), |i| cursor + i);
    let word: String = chars[start..end].iter().collect();
    let before: String = chars[..start].iter().collect();
    let index = before.split_whitespace().count();
    let verb = before.split_whitespace().next().unwrap_or_default();
    let candidates: Vec<String> = if index == 0 && word.starts_with('/') {
        verbs
            .iter()
            .filter(|v| v.starts_with(&word))
            .map(|v| v.to_string())
            .collect()
    } else if index == 1 && matches!(verb, "/set" | "/unset") {
        KEYS.iter()
            .filter(|k| k.starts_with(&word))
            .map(|k| k.to_string())
            .collect()
    } else {
        return None;
    };
    match candidates.as_slice() {
        [] => None,
        [one] => Some(Completion {
            start,
            end,
            text: format!("{one} "),
            list: Vec::new(),
        }),
        many => {
            let prefix = common_prefix(many);
            let list = if prefix.chars().count() > word.chars().count() {
                Vec::new()
            } else {
                many.to_vec()
            };
            Some(Completion {
                start,
                end,
                text: prefix,
                list,
            })
        }
    }
}

/// The longest prefix every word shares. Empty for no words.
pub fn common_prefix(words: &[String]) -> String {
    let Some(first) = words.first() else {
        return String::new();
    };
    first
        .chars()
        .enumerate()
        .take_while(|(i, c)| {
            words
                .iter()
                .all(|w| w.chars().nth(*i) == Some(*c))
        })
        .map(|(_, c)| c)
        .collect()
}

