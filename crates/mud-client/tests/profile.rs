//! Character profile (TOML) tests — the analog of MegaMud Chars/*.Ini.

use std::time::Duration;

use mud_client::dialect::Target;
use mud_client::profile::Profile;

#[test]
fn minimal_profile_parses_with_target_defaults() {
    let p: Profile = toml::from_str(
        r#"
        target = "mbbs"
        host = "127.0.0.1"
        port = 2327
        username = "Oracle"
        password = "test123"
        "#,
    )
    .unwrap();
    assert_eq!(p.target, Target::MbbsEmu);
    assert_eq!(p.host, "127.0.0.1");
    assert_eq!(p.port, 2327);
    assert_eq!(p.username, "Oracle");
    // Live board default: flood control needs >=1.5s between sends.
    assert_eq!(p.pace(), Duration::from_millis(1500));
}

#[test]
fn rust_target_is_unpaced_by_default() {
    let p: Profile = toml::from_str(
        r#"
        target = "rust"
        host = "127.0.0.1"
        port = 2325
        username = "Alice"
        password = "pw"
        "#,
    )
    .unwrap();
    assert_eq!(p.target, Target::RustServer);
    assert_eq!(p.pace(), Duration::ZERO);
}

#[test]
fn explicit_pace_overrides_target_default() {
    let p: Profile = toml::from_str(
        r#"
        target = "mbbs"
        host = "h"
        port = 1
        username = "u"
        password = "p"
        pace_ms = 200
        "#,
    )
    .unwrap();
    assert_eq!(p.pace(), Duration::from_millis(200));
}

/// Every profile written before C8 has neither table; both must stay
/// optional or `mmc play` breaks for existing characters.
#[test]
fn profile_without_bot_or_farm_tables_still_parses() {
    let p: Profile = toml::from_str(
        r#"
        target = "rust"
        host = "127.0.0.1"
        port = 2325
        username = "Alice"
        password = "pw"
        "#,
    )
    .unwrap();
    assert_eq!(p.bot, None);
    assert_eq!(p.farm, None);
}

#[test]
fn bot_and_farm_tables_parse_and_round_trip() {
    let p: Profile = toml::from_str(
        r#"
        target = "rust"
        host = "127.0.0.1"
        port = 2325
        username = "Alice"
        password = "pw"

        [bot]
        auto_combat = true
        auto_heal = true
        max_hp = 35

        [farm]
        start = "1/1"
        circuit = ["1/2", "1/3"]
        loops = 2
        "#,
    )
    .unwrap();

    let bot = p.bot.clone().expect("[bot] table");
    assert!(bot.auto_combat);
    assert_eq!(bot.max_hp, 35);
    // Unlisted toggles keep BotConfig's defaults rather than erroring.
    assert!(!bot.auto_get);

    let farm = p.farm.clone().expect("[farm] table");
    assert_eq!(farm.start, "1/1");
    assert_eq!(farm.circuit, vec!["1/2".to_string(), "1/3".to_string()]);
    assert_eq!(farm.loops, 2);

    let back: Profile = toml::from_str(&toml::to_string(&p).unwrap()).unwrap();
    assert_eq!(p, back);
}

/// Navigation limits live under the table that owns them. A profile
/// that says nothing about them still gets the shipped values, and a
/// profile that overrides one does not silently zero the rest.
#[test]
fn nav_limits_default_and_override_independently() {
    let bare: Profile = toml::from_str(
        r#"
        target = "rust"
        host = "127.0.0.1"
        port = 2325
        username = "Alice"
        password = "pw"

        [farm]
        start = "1/1"
        circuit = ["1/2"]
        "#,
    )
    .unwrap();
    assert_eq!(
        bare.farm.expect("[farm] table").nav,
        mud_client::nav::NavConfig::default()
    );

    let tuned: Profile = toml::from_str(
        r#"
        target = "rust"
        host = "127.0.0.1"
        port = 2325
        username = "Alice"
        password = "pw"

        [farm]
        start = "1/1"
        circuit = ["1/2"]

        [farm.nav]
        step_timeout_ms = 30000
        "#,
    )
    .unwrap();
    assert_eq!(
        tuned.farm.expect("[farm] table").nav.step_timeout_ms,
        30000
    );
}

#[test]
fn profile_round_trips() {
    let p: Profile = toml::from_str(
        r#"
        target = "rust"
        host = "127.0.0.1"
        port = 2325
        username = "Alice"
        password = "pw"
        pace_ms = 100
        "#,
    )
    .unwrap();
    let s = toml::to_string(&p).unwrap();
    let back: Profile = toml::from_str(&s).unwrap();
    assert_eq!(p, back);
}

/// Evil warnings are a CHARACTER setting, not a bot policy, so the
/// toggle sits at the profile's top level next to `pace_ms` rather than
/// in `[bot]`: it changes persistent state on the board whether or not
/// the bot ever swings at anything.
#[test]
fn evil_warning_toggle_defaults_off_and_parses() {
    let bare: Profile = toml::from_str(
        r#"
        target = "mbbs"
        host = "127.0.0.1"
        port = 2327
        username = "u"
        password = "p"
        "#,
    )
    .unwrap();
    assert!(
        !bare.disable_evil_warnings,
        "must be opt-in: it accrues real fame on the character"
    );

    let opted_in: Profile = toml::from_str(
        r#"
        target = "mbbs"
        host = "127.0.0.1"
        port = 2327
        username = "u"
        password = "p"
        disable_evil_warnings = true
        "#,
    )
    .unwrap();
    assert!(opted_in.disable_evil_warnings);

    let back: Profile = toml::from_str(&toml::to_string(&opted_in).unwrap()).unwrap();
    assert_eq!(opted_in, back);
}

// `Profile::interactive()` is gone: a person typing is their own rate
// limiter, but stripping the pace out of the PROFILE destroyed the value
// `/farm` needed when it put the same session back under automation —
// the unpaced-TUI-farm spin (run4, 2026-08-01). Unpacing is now the
// session's business: `mmc play` calls `Session::set_pace(ZERO)` after
// connecting, and `/farm` restores `profile.pace()`. See tests/pacing.rs
// for the mid-stream retune behaviour.
