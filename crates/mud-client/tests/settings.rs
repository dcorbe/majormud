//! Live settings: the profile as an editable document.

use mud_client::settings::{glob_match, KEYS, Settings};

const COMMENTED: &str = r#"# my character
target = "mbbs"
host = "127.0.0.1"   # the local emulator
port = 2327
username = "dan"
password = "pw"

[bot]
auto_combat = true   # fight back
rest_at_percent = 60
"#;

#[test]
fn a_scalar_lands_in_the_profile_and_the_text() {
    let mut s = Settings::parse(COMMENTED).unwrap();
    assert!(!s.dirty());
    s.set("bot.rest_at_percent", "45").unwrap();
    assert_eq!(s.profile().bot.as_ref().unwrap().rest_at_percent, 45);
    assert!(s.text().contains("rest_at_percent = 45"));
    assert!(s.dirty());
}

#[test]
fn a_list_is_a_toml_array() {
    let mut s = Settings::parse(COMMENTED).unwrap();
    s.set("bot.ignore_coins", r#"["copper", "silver"]"#).unwrap();
    assert_eq!(
        s.profile().bot.as_ref().unwrap().ignore_coins,
        vec!["copper".to_string(), "silver".to_string()]
    );
}

#[test]
fn a_bare_word_is_a_string_and_so_is_a_phrase() {
    let mut s = Settings::parse(COMMENTED).unwrap();
    s.set("bot.rest_command", "rest").unwrap();
    assert_eq!(s.profile().bot.as_ref().unwrap().rest_command, "rest");
    s.set("bot.rest_command", "sit down").unwrap();
    assert_eq!(s.profile().bot.as_ref().unwrap().rest_command, "sit down");
    assert!(s.text().contains(r#"rest_command = "sit down""#));
}

#[test]
fn a_key_in_an_absent_table_creates_the_table() {
    let mut s = Settings::default();
    s.set("bank.keep_gold", "5").unwrap();
    assert_eq!(s.profile().bank.keep_gold, 5);
    s.set("bot.auto_get", "true").unwrap();
    assert!(s.profile().bot.as_ref().unwrap().auto_get);
    assert!(s.text().contains("[bot]"));
}

#[test]
fn a_nested_table_key_is_reachable() {
    let mut s = Settings::default();
    s.set("farm.nav.step_timeout_ms", "9000").unwrap();
    assert_eq!(s.profile().farm.as_ref().unwrap().nav.step_timeout_ms, 9000);
    assert!(s.text().contains("[farm.nav]"));
}

/// One line, naming the key. Toml's own rendering is a span with a
/// caret ruler through a document the operator never typed and cannot
/// open.
#[test]
fn a_wrong_type_is_refused_and_nothing_changes() {
    let mut s = Settings::parse(COMMENTED).unwrap();
    let before = s.text();
    let err = s.set("bot.rest_at_percent", "soon").unwrap_err();
    assert_eq!(err.lines().count(), 1, "{err}");
    assert!(err.contains("bot.rest_at_percent"), "{err}");
    assert!(err.contains("expected u32"), "{err}");
    assert_eq!(s.text(), before);
    assert_eq!(s.profile().bot.as_ref().unwrap().rest_at_percent, 60);
    assert!(!s.dirty());
    // A password of digits parses as an integer and lands here too.
    let err = s.set("password", "1234").unwrap_err();
    assert_eq!(err.lines().count(), 1, "{err}");
    assert!(err.starts_with("password: "), "{err}");
}

/// Dirty means unsaved work, so a write that changed no text is not
/// one. The /quit refusal reads this.
#[test]
fn a_write_that_changes_nothing_leaves_the_settings_clean() {
    let mut s = Settings::parse(COMMENTED).unwrap();
    s.set("bot.rest_at_percent", "60").unwrap();
    assert!(!s.dirty(), "the key already said 60:\n{}", s.text());
    s.unset("bot.max_hp").unwrap();
    assert!(!s.dirty(), "the key was never there");
    s.set("bot.rest_at_percent", "45").unwrap();
    assert!(s.dirty(), "this one is a change");
}

/// A file keeps the span. The operator can open the file and go to the
/// line and column it names, which is exactly what a /set has none of.
#[test]
fn a_bad_profile_file_reports_the_line_and_column() {
    let err = match Settings::parse("port = \"twenty three\"\n") {
        Err(e) => e,
        Ok(_) => panic!("a port that is a phrase is not a port"),
    };
    assert!(err.contains("line 1"), "{err}");
    assert!(err.lines().count() > 1, "{err}");
}

#[test]
fn an_unknown_key_is_refused_before_the_text_changes() {
    let mut s = Settings::parse(COMMENTED).unwrap();
    let before = s.text();
    let err = s.set("bot.ignore_currency", "[]").unwrap_err();
    assert!(err.contains("unknown key bot.ignore_currency"), "{err}");
    assert_eq!(s.text(), before);
}

#[test]
fn a_value_the_validator_rejects_is_refused() {
    let mut s = Settings::parse(COMMENTED).unwrap();
    let err = s.set("bot.ignore_coins", r#"["pennies"]"#).unwrap_err();
    assert!(err.contains("pennies"), "{err}");
    assert!(s.profile().bot.as_ref().unwrap().ignore_coins.is_empty());
}

#[test]
fn unset_returns_a_key_to_its_default() {
    let mut s = Settings::parse(COMMENTED).unwrap();
    s.set("bank.at", r#""1/297""#).unwrap();
    assert_eq!(s.profile().bank.at.as_deref(), Some("1/297"));
    s.unset("bank.at").unwrap();
    assert_eq!(s.profile().bank.at, None);
    // Not `.contains("at =")`: `auto_combat = true`, already in COMMENTED,
    // matches that substring on its own (`...combat = true`). The `at` key
    // removed here would be a line of its own.
    assert!(!s.text().contains("\nat = "));
    s.unset("bot.rest_at_percent").unwrap();
    assert_eq!(s.profile().bot.as_ref().unwrap().rest_at_percent, 60);
}

#[test]
fn setting_the_new_name_drops_the_old_alias() {
    let mut s = Settings::parse(
        r#"
[bot]
heal_at_percent = 50
"#,
    )
    .unwrap();
    s.set("bot.rest_at_percent", "40").unwrap();
    assert_eq!(s.profile().bot.as_ref().unwrap().rest_at_percent, 40);
    assert!(!s.text().contains("heal_at_percent"));
}

/// The key list is hand-written. This pins it to the structs: a field
/// added to any config table without a `KEYS` entry fails here.
#[test]
fn every_key_the_profile_serialises_is_in_keys() {
    use mud_client::bank::BankConfig;
    use mud_client::bot::BotConfig;
    use mud_client::dialect::Target;
    use mud_client::farm::FarmConfig;
    use mud_client::profile::Profile;

    // Every new optional field has to be filled in here. An `Option`
    // left at `None` serialises as nothing, so it never reaches `found`
    // and its missing `KEYS` entry escapes this test.
    let full = Profile {
        target: Target::MbbsEmu,
        host: "h".into(),
        port: 1,
        username: "u".into(),
        password: "p".into(),
        pace_ms: Some(1),
        disable_evil_warnings: true,
        bot: Some(BotConfig {
            heal_spells: vec!["x".into()],
            ..Default::default()
        }),
        farm: Some(FarmConfig {
            finish_at: Some("1/1".into()),
            depart_at_percent: Some(1),
            ..Default::default()
        }),
        bank: BankConfig {
            at: Some("1/1".into()),
            ..Default::default()
        },
        ..Default::default()
    };
    let text = toml::to_string(&full).unwrap();
    let table: toml::Table = text.parse().unwrap();
    let mut found = Vec::new();
    flatten(&table, "", &mut found);
    found.sort();
    let mut want: Vec<String> = KEYS.iter().map(|k| k.to_string()).collect();
    want.sort();
    assert_eq!(found, want);
}

fn flatten(table: &toml::Table, prefix: &str, out: &mut Vec<String>) {
    for (k, v) in table {
        let key = if prefix.is_empty() {
            k.clone()
        } else {
            format!("{prefix}.{k}")
        };
        match v {
            toml::Value::Table(t) => flatten(t, &key, out),
            _ => out.push(key),
        }
    }
}

fn scratch(name: &str) -> std::path::PathBuf {
    let dir = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join("settings");
    std::fs::create_dir_all(&dir).unwrap();
    dir.join(name)
}

#[test]
fn save_keeps_comments_and_order() {
    let path = scratch("commented.toml");
    std::fs::write(&path, COMMENTED).unwrap();
    let mut s = Settings::load(&path).unwrap();
    assert_eq!(s.path(), Some(path.as_path()));
    s.set("bot.rest_at_percent", "45").unwrap();
    let saved = s.save(None).unwrap();
    assert_eq!(saved, path);
    assert!(!s.dirty());
    let text = std::fs::read_to_string(&path).unwrap();
    assert_eq!(
        text,
        COMMENTED.replace("rest_at_percent = 60", "rest_at_percent = 45"),
        "every comment and every line stays where it was"
    );
}

/// The save is a write to a sibling and a rename, so the target is
/// never truncated and the sibling never survives a good save.
#[test]
fn a_save_replaces_the_file_and_leaves_no_temporary() {
    let path = scratch("replaced.toml");
    let temp = scratch("replaced.toml.tmp");
    std::fs::write(&path, "host = \"old\"\n").unwrap();
    let mut s = Settings::load(&path).unwrap();
    s.set("host", "\"new\"").unwrap();
    s.save(None).unwrap();
    assert!(std::fs::read_to_string(&path).unwrap().contains("host = \"new\""));
    assert!(!temp.exists(), "the temporary is renamed away, not left behind");
}

/// The rename installs a new file, and the profile holds a password in
/// plain text, so the mode the operator set has to come with it.
#[test]
#[cfg(unix)]
fn a_save_keeps_the_files_mode() {
    use std::os::unix::fs::PermissionsExt;

    let path = scratch("private.toml");
    std::fs::write(&path, COMMENTED).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
    let mut s = Settings::load(&path).unwrap();
    s.set("bot.rest_at_percent", "45").unwrap();
    s.save(None).unwrap();
    let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o600, "the password in it is still nobody else's");
}

/// A write that cannot happen reports the profile, which is the file
/// the operator named, and takes the temporary with it rather than
/// leaving a half written sibling next to a good profile.
#[test]
#[cfg(unix)]
fn a_failed_write_reports_the_profile_and_drops_the_temporary() {
    use std::os::unix::fs::PermissionsExt;

    let dir = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join("blocked");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("profile.toml");
    let temp = dir.join("profile.toml.tmp");
    std::fs::write(&path, COMMENTED).unwrap();
    std::fs::write(&temp, "half a document\n").unwrap();
    std::fs::set_permissions(&temp, std::fs::Permissions::from_mode(0o400)).unwrap();
    // Root ignores the mode bits, and then the test can say nothing.
    if std::fs::write(&temp, "x").is_ok() {
        return;
    }
    let mut s = Settings::load(&path).unwrap();
    s.set("port", "2400").unwrap();
    let err = s.save(None).unwrap_err();
    assert!(
        err.starts_with(&format!("{}: ", path.display())),
        "the error names the profile, not the sibling: {err}"
    );
    assert!(!temp.exists(), "the temporary does not survive a failed write");
    assert_eq!(std::fs::read_to_string(&path).unwrap(), COMMENTED);
}

/// What the rename buys: a save that cannot finish leaves the profile
/// exactly as it was. A directory nobody may write to refuses the new
/// file, where a write straight onto the target would have emptied it
/// first and then failed.
#[test]
#[cfg(unix)]
fn a_failed_save_leaves_the_old_file_whole() {
    use std::os::unix::fs::PermissionsExt;

    let dir = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join("readonly");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("profile.toml");
    std::fs::write(&path, COMMENTED).unwrap();
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o500)).unwrap();
    // Root ignores the mode bits, and then the test can say nothing.
    let enforced = std::fs::write(dir.join("probe"), "x").is_err();
    let mut s = Settings::load(&path).unwrap();
    s.set("port", "2400").unwrap();
    let result = s.save(None);
    let after = std::fs::read_to_string(&path).unwrap();
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700)).unwrap();
    if !enforced {
        return;
    }
    assert!(result.is_err(), "the directory refuses the write");
    assert_eq!(after, COMMENTED, "the old profile is still whole");
    assert!(
        !dir.join("profile.toml.tmp").exists(),
        "a failed write takes its temporary with it"
    );
    assert!(s.dirty(), "nothing was saved, so the edit is still unsaved");
}

#[test]
fn save_without_a_path_needs_one_once() {
    let mut s = Settings::default();
    s.set("host", "\"127.0.0.1\"").unwrap();
    let err = s.save(None).unwrap_err();
    assert!(err.contains("/save <file>"), "{err}");
    let path = scratch("fresh.toml");
    let _ = std::fs::remove_file(&path);
    s.save(Some(&path)).unwrap();
    assert_eq!(s.path(), Some(path.as_path()));
    s.set("port", "2327").unwrap();
    s.save(None).unwrap();
    assert!(std::fs::read_to_string(&path).unwrap().contains("port = 2327"));
}

/// `/new` opens a window on a detached copy: the settings without the
/// file. A bare `/save` there asks for a name rather than writing the
/// second character over the first character's profile.
#[test]
fn a_detached_copy_keeps_the_settings_and_drops_the_file() {
    let mut s = Settings::default();
    s.set("host", "\"127.0.0.1\"").unwrap();
    let path = scratch("template.toml");
    let _ = std::fs::remove_file(&path);
    s.save(Some(&path)).unwrap();
    s.set("username", "\"dan\"").unwrap();
    let copy = s.detached();
    assert_eq!(copy.path(), None, "the copy has no file to save over");
    assert!(copy.dirty(), "the copy holds the unsaved edit it was made from");
    assert_eq!(copy.profile().host, "127.0.0.1");
    assert_eq!(copy.profile().username, "dan");
    assert_eq!(copy.text(), s.text());
    assert_eq!(s.path(), Some(path.as_path()), "the template keeps its own file");
    let mut copy = copy;
    let err = copy.save(None).unwrap_err();
    assert!(err.contains("/save <file>"), "{err}");
}

#[test]
fn load_reports_renamed_keys_and_still_parses() {
    let s = Settings::parse(
        r#"
[bot]
heal_at_percent = 50
heal_command = "rest"
"#,
    )
    .unwrap();
    let warnings = s.warnings();
    assert_eq!(warnings.len(), 2, "{warnings:?}");
    assert!(warnings[0].contains("`heal_at_percent` is now `rest_at_percent`"));
    assert_eq!(s.profile().bot.as_ref().unwrap().rest_at_percent, 50);
}

#[test]
fn a_missing_file_is_an_error_not_a_default() {
    assert!(Settings::load(&scratch("nowhere.toml")).is_err());
}

#[test]
fn a_glob_matches_by_prefix_with_a_trailing_star_implied() {
    assert!(glob_match("bot", "bot.rest_at_percent"));
    assert!(glob_match("bot.*", "bot.rest_at_percent"));
    assert!(glob_match("bot.rest", "bot.rest_at_percent"));
    assert!(glob_match("bot.rest*", "bot.rest_until_percent"));
    assert!(glob_match("*heal*", "bot.minor_heal_at_percent"));
    assert!(glob_match("*heal*", "farm.heal_refused"));
    assert!(glob_match("", "host"));
    assert!(!glob_match("bot", "bank.at"));
    assert!(!glob_match("*heal*", "bank.at"));
    assert!(!glob_match("bot", "my.bot.thing"));
}

#[test]
fn the_listing_walks_keys_in_file_order_and_renders_toml() {
    let s = Settings::parse(COMMENTED).unwrap();
    let rows = s.list("bot.rest");
    assert_eq!(
        rows,
        vec![
            ("bot.rest_at_percent".to_string(), "60".to_string()),
            ("bot.rest_until_percent".to_string(), "95".to_string()),
            ("bot.rest_command".to_string(), "\"rest\"".to_string()),
        ],
        "a prefix pattern takes every key it starts, rest_command included"
    );
    let all = s.list("");
    assert_eq!(all.len(), KEYS.len());
    assert_eq!(all[0].0, "target");
    assert_eq!(all[0].1, "\"mbbs\"");
}

#[test]
fn an_absent_table_lists_its_defaults() {
    let s = Settings::default();
    assert_eq!(s.value("bot.rest_at_percent").as_deref(), Some("60"));
    assert_eq!(s.value("bank.at").as_deref(), Some("unset"));
    assert_eq!(s.value("farm.circuit").as_deref(), Some("[]"));
}

#[test]
fn the_password_is_masked() {
    let s = Settings::parse(COMMENTED).unwrap();
    assert_eq!(s.value("password").as_deref(), Some("\"****\""));
    let empty = Settings::default();
    assert_eq!(empty.value("password").as_deref(), Some("\"\""));
}

use mud_client::settings::complete;

const VERBS: &[&str] = &["/set", "/save", "/quit", "/unset"];

#[test]
fn a_unique_verb_completes_with_a_space() {
    let c = complete("/sa", 3, VERBS).unwrap();
    assert_eq!((c.start, c.end, c.text.as_str()), (0, 3, "/save "));
    assert!(c.list.is_empty());
}

#[test]
fn a_unique_key_completes_after_set() {
    let c = complete("/set bot.ignore_c", 17, VERBS).unwrap();
    assert_eq!((c.start, c.end, c.text.as_str()), (5, 17, "bot.ignore_coins "));
    assert!(c.list.is_empty());
    let c = complete("/unset bank.at", 14, VERBS).unwrap();
    assert_eq!(c.text, "bank.at ");
}

#[test]
fn several_candidates_grow_to_the_common_prefix_and_then_list() {
    let c = complete("/set bot.rest_", 14, VERBS).unwrap();
    assert_eq!(c.text, "bot.rest_");
    assert_eq!(
        c.list,
        vec![
            "bot.rest_at_percent".to_string(),
            "bot.rest_until_percent".to_string(),
            "bot.rest_command".to_string()
        ]
    );
    // The order is KEYS order, which the docs quote.
    let c = complete("/set bot.ignore", 15, VERBS).unwrap();
    assert_eq!(
        c.list,
        vec!["bot.ignore_coins".to_string(), "bot.ignore".to_string()]
    );
    let c = complete("/set bot.min", 12, VERBS).unwrap();
    assert_eq!(c.text, "bot.minor_heal_", "the word grew, so nothing is listed");
    assert!(c.list.is_empty());
}

#[test]
fn completion_works_mid_line_and_only_on_slash_lines() {
    let c = complete("/set bot.ign 5", 12, VERBS).unwrap();
    assert_eq!((c.start, c.end), (5, 12));
    assert!(complete("set bot.ign", 11, VERBS).is_none());
    assert!(complete("/go bot.ign", 11, VERBS).is_none());
    assert!(complete("/set bot.rest_at_percent 6", 26, VERBS).is_none());
    assert!(complete("/set bot.rest_at_percent bot.re", 31, VERBS).is_none());
    assert!(complete("/zz", 3, VERBS).is_none());
}

#[test]
fn common_prefix_of_nothing_is_empty() {
    use mud_client::settings::common_prefix;
    assert_eq!(common_prefix(&[]), "");
    assert_eq!(common_prefix(&["abc".into(), "abd".into()]), "ab");
}

/// The stray `beef` in the repo root was a bare lobby with three keys.
/// Under the resolver alone that save would have replaced the real
/// profile, password and all. A file the settings did not come from is
/// somebody's, and a save onto it is refused.
#[test]
fn a_save_onto_a_file_the_settings_did_not_come_from_is_refused() {
    let path = scratch("theirs.toml");
    std::fs::write(&path, COMMENTED).unwrap();
    let mut s = Settings::default();
    s.set("host", "\"127.0.0.1\"").unwrap();
    let err = s.save(Some(&path)).unwrap_err();
    assert!(
        err.starts_with(&format!("{} exists and these settings were not loaded from it.", path.display())),
        "{err}"
    );
    assert!(err.contains("/load it first, or pick another name."), "{err}");
    assert_eq!(std::fs::read_to_string(&path).unwrap(), COMMENTED, "untouched");
    assert!(s.dirty(), "nothing was saved");
}

/// The remembered path is the file these settings came from, so a save
/// onto it by name is the same as a bare save.
#[test]
fn a_save_onto_the_remembered_path_by_name_writes() {
    let path = scratch("mine.toml");
    std::fs::write(&path, COMMENTED).unwrap();
    let mut s = Settings::load(&path).unwrap();
    s.set("port", "2400").unwrap();
    s.save(Some(&path)).unwrap();
    assert!(std::fs::read_to_string(&path).unwrap().contains("port = 2400"));
}

/// `dan.toml` and `./dan.toml` name the same file. A save through a
/// different spelling of the remembered path is still a save onto the
/// file these settings came from, not onto somebody else's.
#[test]
fn a_save_onto_a_different_spelling_of_the_remembered_path_writes() {
    let path = scratch("spelled.toml");
    let _ = std::fs::remove_file(&path);
    std::fs::write(&path, COMMENTED).unwrap();
    let mut s = Settings::load(&path).unwrap();
    s.set("port", "2400").unwrap();
    let alt = path.parent().unwrap().join("..").join("settings").join("spelled.toml");
    s.save(Some(&alt)).unwrap();
    assert!(std::fs::read_to_string(&path).unwrap().contains("port = 2400"));
}

/// A name nobody has used is free, and the save remembers it.
#[test]
fn a_save_onto_a_fresh_name_writes_and_remembers() {
    let path = scratch("fresh-name.toml");
    let _ = std::fs::remove_file(&path);
    let mut s = Settings::default();
    s.set("host", "\"127.0.0.1\"").unwrap();
    s.save(Some(&path)).unwrap();
    assert_eq!(s.path(), Some(path.as_path()));
    assert!(!s.dirty());
}

/// The one way to walk without sneaking, and the one way to stop resting.
/// Both are bot keys, so both complete under `/set bot.` and rebuild the
/// assist like the rest, and both default on.
#[test]
fn bot_auto_sneak_and_auto_rest_are_keys_that_default_on() {
    assert!(KEYS.contains(&"bot.auto_sneak"));
    assert!(KEYS.contains(&"bot.auto_rest"));
    let s = Settings::default();
    assert_eq!(s.value("bot.auto_sneak").as_deref(), Some("true"));
    assert_eq!(s.value("bot.auto_rest").as_deref(), Some("true"));
    let mut s = Settings::parse(COMMENTED).unwrap();
    s.set("bot.auto_sneak", "false").unwrap();
    assert!(!s.profile().bot.as_ref().unwrap().auto_sneak);
}

