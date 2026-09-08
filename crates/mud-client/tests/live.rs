//! Live settings tests: what a job rebuilds when the session's profile
//! changes under it, and when it does not.

use std::time::Duration;

use mud_client::bot::BotConfig;
use mud_client::farm::FarmConfig;
use mud_client::live::Live;
use mud_client::profile::Profile;

fn quiet() -> mud_client::farm::Notices {
    std::sync::Arc::new(|_: &str| {})
}

fn collected() -> (mud_client::farm::Notices, std::sync::Arc<std::sync::Mutex<Vec<String>>>) {
    let said = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let into = std::sync::Arc::clone(&said);
    (
        std::sync::Arc::new(move |line: &str| into.lock().unwrap().push(line.to_string())),
        said,
    )
}

/// A go keeps combat off whatever the profile says. The derivation is
/// the job's, applied to every profile that arrives.
fn derive_like_a_go() -> mud_client::live::Derive {
    std::sync::Arc::new(|p: &Profile| {
        let bot = BotConfig {
            auto_combat: false,
            ..p.bot.clone().unwrap_or_default()
        };
        (bot, p.farm.clone().unwrap_or_default())
    })
}

#[test]
fn refresh_is_false_until_the_profile_changes_and_true_once_after() {
    let (tx, rx) = tokio::sync::watch::channel(Profile::default());
    let mut live = Live::over(rx, "farm", quiet(), BotConfig::default(), FarmConfig::default(), derive_like_a_go());
    assert!(!live.refresh());
    assert_eq!(live.generation(), 0);
    tx.send(Profile { bot: Some(BotConfig { ignore_coins: vec!["copper".into()], ..Default::default() }), ..Default::default() }).unwrap();
    assert!(live.refresh());
    assert_eq!(live.generation(), 1);
    assert_eq!(live.bot.ignore_coins, vec!["copper".to_string()]);
    assert!(!live.refresh(), "one change, one rebuild");
    assert_eq!(live.generation(), 1);
}

/// The three refresh points share one generation. Whichever of them
/// runs the rebuild, the other two must still notice their own build is
/// behind and redo it.
#[test]
fn took_fires_once_for_the_caller_that_refreshed_and_once_for_the_one_that_lagged() {
    let (tx, rx) = tokio::sync::watch::channel(Profile::default());
    let mut live = Live::over(rx, "farm", quiet(), BotConfig::default(), FarmConfig::default(), derive_like_a_go());
    let mut built_at = live.generation();
    assert!(!live.took(&mut built_at), "nothing was sent");
    tx.send(Profile { bot: Some(BotConfig { ignore_coins: vec!["copper".into()], ..Default::default() }), ..Default::default() }).unwrap();
    assert!(live.took(&mut built_at));
    assert!(!live.took(&mut built_at), "one change, one rebuild");
    assert_eq!(built_at, live.generation());

    // The other caller: its own build predates the refresh above, and
    // `refresh` has nothing left to report.
    let mut lagging = 0;
    assert!(live.took(&mut lagging), "a build behind the generation rebuilds");
    assert_eq!(lagging, live.generation());
    assert!(!live.took(&mut lagging));
}

#[test]
fn a_rebuild_applies_the_jobs_forced_fields() {
    let (tx, rx) = tokio::sync::watch::channel(Profile::default());
    let mut live = Live::over(rx, "go", quiet(), BotConfig::default(), FarmConfig::default(), derive_like_a_go());
    tx.send(Profile { bot: Some(BotConfig { auto_combat: true, ..Default::default() }), ..Default::default() }).unwrap();
    assert!(live.refresh());
    assert!(!live.bot.auto_combat, "a go never fights, whatever the profile says");
}

#[test]
fn a_rebuild_keeps_discovered_vitals_when_the_profile_does_not_say() {
    let (tx, rx) = tokio::sync::watch::channel(Profile::default());
    let mut live = Live::over(rx, "farm", quiet(), BotConfig::default(), FarmConfig::default(), derive_like_a_go());
    live.learned_vitals(120, 40);
    assert_eq!((live.bot.max_hp, live.bot.max_mana), (120, 40));
    tx.send(Profile { bot: Some(BotConfig::default()), ..Default::default() }).unwrap();
    assert!(live.refresh());
    assert_eq!((live.bot.max_hp, live.bot.max_mana), (120, 40), "the board's answer outlives a rebuild");
    tx.send(Profile { bot: Some(BotConfig { max_hp: 200, max_mana: 50, ..Default::default() }), ..Default::default() }).unwrap();
    assert!(live.refresh());
    assert_eq!((live.bot.max_hp, live.bot.max_mana), (200, 50), "a profile that says wins");
}

#[test]
fn the_reloaded_line_prints_once_per_change() {
    let (tx, rx) = tokio::sync::watch::channel(Profile::default());
    let (notices, said) = collected();
    let mut live = Live::over(rx, "farm", notices, BotConfig::default(), FarmConfig::default(), derive_like_a_go());
    tx.send(Profile::default()).unwrap();
    live.refresh();
    live.refresh();
    assert_eq!(said.lock().unwrap().as_slice(), ["-- farm: settings reloaded --"]);
}

#[tokio::test]
async fn changed_resolves_after_a_send_and_a_fixed_live_never_fires() {
    let (tx, rx) = tokio::sync::watch::channel(Profile::default());
    let mut live = Live::over(rx, "farm", quiet(), BotConfig::default(), FarmConfig::default(), derive_like_a_go());
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(50)).await;
        tx.send(Profile::default()).unwrap();
    });
    tokio::time::timeout(Duration::from_secs(2), live.changed()).await.expect("a send wakes changed");
    assert!(live.refresh());

    let mut fixed = Live::fixed(BotConfig::default(), FarmConfig::default());
    assert!(
        tokio::time::timeout(Duration::from_millis(100), fixed.changed()).await.is_err(),
        "a fixed live never wakes"
    );
    assert!(!fixed.refresh());
}

/// A wait that already woke must not spend a second wake on its own
/// leftovers. Only a genuine second send may resolve it again.
#[tokio::test]
async fn a_second_changed_without_a_refresh_waits_for_a_second_send() {
    let (tx, rx) = tokio::sync::watch::channel(Profile::default());
    let mut live = Live::over(rx, "farm", quiet(), BotConfig::default(), FarmConfig::default(), derive_like_a_go());
    tx.send(Profile::default()).unwrap();
    tokio::time::timeout(Duration::from_secs(2), live.changed())
        .await
        .expect("the first send wakes changed");
    assert!(
        tokio::time::timeout(Duration::from_millis(200), live.changed())
            .await
            .is_err(),
        "no second send means no second wake"
    );
    tx.send(Profile::default()).unwrap();
    tokio::time::timeout(Duration::from_secs(2), live.changed())
        .await
        .expect("the second send wakes changed");
    assert!(live.refresh(), "the change landed even though two sends went by");
}

/// The wake from `changed` is not itself the rebuild. `refresh` is
/// still the one thing that consumes it.
#[tokio::test]
async fn a_refresh_after_a_changed_wake_sees_the_change() {
    let (tx, rx) = tokio::sync::watch::channel(Profile::default());
    let mut live = Live::over(rx, "farm", quiet(), BotConfig::default(), FarmConfig::default(), derive_like_a_go());
    tx.send(Profile::default()).unwrap();
    tokio::time::timeout(Duration::from_secs(2), live.changed())
        .await
        .expect("a send wakes changed");
    assert!(live.refresh());
    assert!(!live.refresh(), "nothing changed since the last refresh");
}


// --- the bot switch --------------------------------------------------

/// `/bot` is the master switch over every automatic policy. Off turns
/// them all off whatever the profile says, on restores the profile's
/// own choices. The marks and the pools stay, so the policies come
/// back as they were.
#[test]
fn the_bot_switch_turns_every_automatic_policy_off_and_back_on() {
    let (tx, rx) = tokio::sync::watch::channel(Profile::default());
    let (switch, switch_rx) = tokio::sync::watch::channel(true);
    let mut live = Live::over(rx, "farm", quiet(), BotConfig::default(), FarmConfig::default(), derive_like_a_go())
        .switched(switch_rx);
    assert!(live.bot.auto_rest);
    switch.send(false).unwrap();
    assert!(live.refresh(), "a flip is a change");
    assert!(!live.bot.auto_rest && !live.bot.auto_heal && !live.bot.auto_get, "{:?}", live.bot);
    assert!(!live.bot.auto_flee && !live.bot.auto_sneak && !live.bot.take_keys, "{:?}", live.bot);
    assert_eq!(live.bot.rest_until_percent, BotConfig::default().rest_until_percent, "the marks stay");
    assert!(!live.refresh(), "one flip, one rebuild");
    // A profile change while the switch is off lands, still switched off.
    tx.send(Profile { bot: Some(BotConfig { buffs: vec!["bless".into()], ..Default::default() }), ..Default::default() }).unwrap();
    assert!(live.refresh());
    assert!(live.bot.buffs.is_empty(), "off means no buffs either: {:?}", live.bot);
    assert!(!live.bot.auto_rest);
    switch.send(true).unwrap();
    assert!(live.refresh());
    assert!(live.bot.auto_rest && live.bot.auto_heal, "{:?}", live.bot);
    assert_eq!(live.bot.buffs, vec!["bless".to_string()], "back on, the profile's buffs return");
}

/// A job started while the switch is off starts with nothing automatic,
/// and the switch it read at the start is not a change to report.
#[test]
fn a_job_started_with_the_bot_off_starts_with_nothing_automatic() {
    let (_tx, rx) = tokio::sync::watch::channel(Profile::default());
    let (_switch, switch_rx) = tokio::sync::watch::channel(false);
    let mut live = Live::over(rx, "go", quiet(), BotConfig { max_hp: 52, ..Default::default() }, FarmConfig::default(), derive_like_a_go())
        .switched(switch_rx);
    assert!(!live.bot.auto_rest && !live.bot.auto_heal, "{:?}", live.bot);
    assert_eq!(live.bot.max_hp, 52, "the pool is not a policy");
    assert!(!live.refresh(), "nothing changed since the start");
}

/// Off keeps the pools the board taught the job, the same as a profile
/// change does.
#[test]
fn a_switch_flip_keeps_the_learned_vitals() {
    let (_tx, rx) = tokio::sync::watch::channel(Profile::default());
    let (switch, switch_rx) = tokio::sync::watch::channel(true);
    let mut live = Live::over(rx, "go", quiet(), BotConfig::default(), FarmConfig::default(), derive_like_a_go())
        .switched(switch_rx);
    live.learned_vitals(52, 10);
    switch.send(false).unwrap();
    assert!(live.refresh());
    assert_eq!((live.bot.max_hp, live.bot.max_mana), (52, 10));
}

/// A job waiting on the board wakes for a `/bot` press the same as for
/// a `/set`, so the flip lands at the next step and not at the end of
/// the leg.
#[tokio::test]
async fn changed_wakes_for_the_bot_switch() {
    let (_tx, rx) = tokio::sync::watch::channel(Profile::default());
    let (switch, switch_rx) = tokio::sync::watch::channel(true);
    let mut live = Live::over(rx, "go", quiet(), BotConfig::default(), FarmConfig::default(), derive_like_a_go())
        .switched(switch_rx);
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(20)).await;
        switch.send(false).unwrap();
        // Held so the receiver never reads a closed channel.
        tokio::time::sleep(Duration::from_secs(5)).await;
    });
    tokio::time::timeout(Duration::from_secs(2), live.changed())
        .await
        .expect("a flip wakes changed");
    assert!(live.refresh());
    assert!(!live.bot.auto_combat);
}
