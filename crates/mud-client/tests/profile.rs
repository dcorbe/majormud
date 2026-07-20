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
