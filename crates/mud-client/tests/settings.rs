//! Live settings: the profile as an editable document.

use mud_client::settings::{KEYS, Settings};

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

#[test]
fn a_wrong_type_is_refused_and_nothing_changes() {
    let mut s = Settings::parse(COMMENTED).unwrap();
    let before = s.text();
    let err = s.set("bot.rest_at_percent", "soon").unwrap_err();
    assert!(err.contains("rest_at_percent"), "{err}");
    assert_eq!(s.text(), before);
    assert_eq!(s.profile().bot.as_ref().unwrap().rest_at_percent, 60);
    assert!(!s.dirty());
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

#[test]
fn save_without_a_path_needs_one_once() {
    let mut s = Settings::default();
    s.set("host", "\"127.0.0.1\"").unwrap();
    let err = s.save(None).unwrap_err();
    assert!(err.contains("/save <file>"), "{err}");
    let path = scratch("fresh.toml");
    s.save(Some(&path)).unwrap();
    assert_eq!(s.path(), Some(path.as_path()));
    s.set("port", "2327").unwrap();
    s.save(None).unwrap();
    assert!(std::fs::read_to_string(&path).unwrap().contains("port = 2327"));
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

