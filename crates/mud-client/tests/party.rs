//! The party state machine, the telepath grammar, the hold set, the
//! wait state, and the party state an assist rebuild carries across.
//! Pure functions, no board.

use std::time::{Duration, Instant};

use mud_client::bank::{BankConfig, Reading};
use mud_client::bot::BotConfig;
use mud_client::correlate::{CmdId, Correlated};
use mud_client::events::{Event, Status};
use mud_client::party::{
    AskState, Change, Health, Holds, Member, PartyConfig, PartyState, Remote, Request, Role,
    Signal, WaitState, bank_names, permitted, remote, say, telepath,
};
use mud_client::sheet::{CastAttempt, HealKind, Inventory, PartyHeal, PartyKind, PartySource};
use mud_client::tui::AssistCasts;
use mud_client::world::RoundClock;
use mud_core::content::Content;

fn state() -> PartyState {
    PartyState::new()
}

#[test]
fn now_following_makes_a_follower() {
    let mut s = state();
    let change = s.observe("You are now following Beef");
    assert_eq!(change, Some(Change::Following("Beef".into())));
    assert_eq!(s.role, Role::Follower);
    assert_eq!(s.leader.as_deref(), Some("Beef"));
    assert!(s.members.is_empty());
}

#[test]
fn started_to_follow_makes_a_leader_and_adds_the_member() {
    let mut s = state();
    assert_eq!(s.observe("Pootwaddle started to follow you."), Some(Change::Joined("Pootwaddle".into())));
    assert_eq!(s.role, Role::Leader);
    assert_eq!(s.members, vec![Member { name: "Pootwaddle".into(), invited: false, class: None, hp: None, pool: None }]);
}

#[test]
fn an_invite_adds_an_invited_member_and_joining_clears_the_flag() {
    let mut s = state();
    assert_eq!(s.observe("You have invited Pootwaddle to follow you."), Some(Change::Invited("Pootwaddle".into())));
    assert_eq!(s.role, Role::Leader);
    assert!(s.members[0].invited);
    s.observe("Pootwaddle started to follow you.");
    assert_eq!(s.members.len(), 1);
    assert!(!s.members[0].invited);
}

#[test]
fn no_longer_following_and_not_in_a_party_reset() {
    let mut s = state();
    s.observe("You are now following Beef");
    assert_eq!(s.observe("You are no longer following Beef."), Some(Change::Ended));
    assert_eq!(s.role, Role::None);
    assert_eq!(s.leader, None);

    s.observe("Pootwaddle started to follow you.");
    assert_eq!(s.observe("You are not in a party at the present time."), Some(Change::Ended));
    assert_eq!(s.role, Role::None);
    assert!(s.members.is_empty());
}

#[test]
fn removed_from_your_followers_drops_the_member() {
    let mut s = state();
    s.observe("Pootwaddle started to follow you.");
    s.observe("Blueberry started to follow you.");
    assert_eq!(s.observe("Pootwaddle has been removed from your followers."), Some(Change::Left("Pootwaddle".into())));
    assert_eq!(s.members, vec![Member { name: "Blueberry".into(), invited: false, class: None, hp: None, pool: None }]);
}

#[test]
fn the_roster_replaces_the_member_list_in_either_row_format() {
    let mut s = state();
    s.observe("Pootwaddle started to follow you.");
    s.observe("Stale started to follow you.");
    assert_eq!(s.observe("The following people are in your travel party:"), None);
    assert_eq!(s.observe("  Pootwaddle                     Mystic"), None);
    assert_eq!(s.observe("  Blueberry (Ninja) [H: 90%] - front"), None);
    assert_eq!(s.observe("  Newguy                         Warrior      [Invited]"), None);
    assert_eq!(s.observe(""), Some(Change::Roster));
    assert_eq!(
        s.members,
        vec![
            Member { name: "Pootwaddle".into(), invited: false, class: None, hp: None, pool: None },
            Member { name: "Blueberry".into(), invited: false, class: Some("Ninja".into()), hp: Some(90), pool: None },
            Member { name: "Newguy".into(), invited: true, class: None, hp: None, pool: None },
        ]
    );
    assert_eq!(s.role, Role::Leader);
}

/// The live board's row, captured 2026-09-13: class in parentheses,
/// the pool tagged K or M when there is one, then the health.
#[test]
fn a_live_roster_row_carries_class_pool_and_health() {
    let mut s = state();
    s.observe("Beef started to follow you.");
    s.observe("Carrot started to follow you.");
    s.observe("Salad started to follow you.");
    s.observe("The following people are in your travel party:");
    s.observe("  Blueberry                      (Mystic)     [K:100%] [H:100%]   - Frontrank");
    s.observe("  Beef                           (Ninja)               [H:100%]   - Midrank");
    s.observe("  Salad                          (Ranger)     [M:100%] [H: 86%]   - Midrank");
    s.observe("");
    let by = |name: &str| s.members.iter().find(|m| m.name == name).cloned().unwrap();
    assert_eq!(by("Blueberry").class.as_deref(), Some("Mystic"));
    assert_eq!(by("Blueberry").pool, Some(100));
    assert_eq!(by("Beef").pool, None);
    assert_eq!(by("Beef").hp, Some(100));
    assert_eq!(by("Salad").hp, Some(86));
    assert_eq!(by("Salad").pool, Some(100));
}

/// A poll that changed only the numbers is not a change to the party.
/// The window prints nothing for it, so a roster every twenty seconds
/// is silent.
#[test]
fn a_roster_that_changed_only_numbers_is_vitals_not_roster() {
    let mut s = state();
    s.observe("Beef started to follow you.");
    s.observe("The following people are in your travel party:");
    s.observe("  Beef                           (Ninja)               [H:100%]   - Midrank");
    assert_eq!(s.observe(""), Some(Change::Vitals));
    s.observe("The following people are in your travel party:");
    s.observe("  Beef                           (Ninja)               [H: 40%]   - Midrank");
    assert_eq!(s.observe(""), Some(Change::Vitals));
    assert_eq!(s.members[0].hp, Some(40));
    s.observe("The following people are in your travel party:");
    s.observe("  Beef                           (Ninja)               [H: 40%]   - Midrank");
    s.observe("  Carrot                         (Paladin)    [M: 90%] [H: 70%]   - Midrank");
    assert_eq!(s.observe(""), Some(Change::Roster), "a new name is a change to the party");
}

#[test]
fn a_prompt_ends_the_roster_block_too() {
    let mut s = state();
    s.observe("Pootwaddle started to follow you.");
    s.observe("The following people are in your travel party:");
    s.observe("  Blueberry                      Ninja");
    assert_eq!(s.observe_prompt(), Some(Change::Roster));
    assert_eq!(s.members.len(), 1);
    assert_eq!(s.members[0].name, "Blueberry");
}

#[test]
fn an_empty_roster_while_leading_ends_the_party() {
    let mut s = state();
    s.observe("Pootwaddle started to follow you.");
    s.observe("The following people are in your travel party:");
    assert_eq!(s.observe(""), Some(Change::Ended));
    assert_eq!(s.role, Role::None);
}

#[test]
fn a_non_indented_line_ends_the_roster_and_is_then_read_itself() {
    let mut s = state();
    s.observe("Pootwaddle started to follow you.");
    s.observe("Blueberry started to follow you.");
    s.observe("The following people are in your travel party:");
    s.observe("  Pootwaddle                     Mystic");
    assert_eq!(
        s.observe("Blueberry has been removed from your followers."),
        Some(Change::Left("Blueberry".into()))
    );
    assert_eq!(s.members, vec![Member { name: "Pootwaddle".into(), invited: false, class: None, hp: None, pool: None }]);
}

#[test]
fn a_roster_ended_by_chatter_still_reads_as_the_roster() {
    let mut s = state();
    s.observe("Pootwaddle started to follow you.");
    s.observe("The following people are in your travel party:");
    s.observe("  Pootwaddle                     Mystic");
    assert_eq!(s.observe("Beef says \"hi\""), Some(Change::Vitals));
}

#[test]
fn chatter_changes_nothing() {
    let mut s = state();
    assert_eq!(s.observe("Beef says \"You are now following me, ha\""), None);
    assert_eq!(s.role, Role::None);
    assert_eq!(s.observe("A kobold thief started to follow you around the room."), None);
}

#[test]
fn followers_excludes_invited_rows() {
    let mut s = state();
    s.observe("Pootwaddle started to follow you.");
    s.observe("You have invited Newguy to follow you.");
    assert_eq!(s.followers(), vec!["Pootwaddle".to_string()]);
}

#[test]
fn a_telepath_with_a_known_word_is_a_request() {
    assert_eq!(
        remote("Pootwaddle telepaths: @bank"),
        Some(Request { from: "Pootwaddle".into(), command: Remote::Bank })
    );
    assert_eq!(remote("Beef telepaths: @WAIT").map(|r| r.command), Some(Remote::Wait));
    assert_eq!(remote("Beef telepaths: @Ok now").map(|r| r.command), Some(Remote::Ok));
}

#[test]
fn chat_unknown_words_and_the_send_echo_are_not_requests() {
    assert_eq!(remote("Pootwaddle telepaths: hello there"), None);
    assert_eq!(remote("Pootwaddle telepaths: @reroll"), None);
    assert_eq!(remote("--- Telepath Sent to Pootwaddle ---"), None);
    assert_eq!(remote("Pootwaddle gangpaths: @bank"), None);
}

#[test]
fn only_the_leader_and_joined_members_are_permitted() {
    let mut s = PartyState::new();
    s.observe("You are now following Beef");
    assert!(permitted(&s, "beef"));
    assert!(!permitted(&s, "Stranger"));

    let mut l = PartyState::new();
    l.observe("Pootwaddle started to follow you.");
    l.observe("You have invited Newguy to follow you.");
    assert!(permitted(&l, "POOTWADDLE"));
    assert!(!permitted(&l, "Newguy"));
    assert!(!permitted(&l, "Stranger"));
}

#[test]
fn the_wire_form_is_a_slash_and_the_name() {
    assert_eq!(telepath("Beef", "@ok"), "/Beef @ok");
}

#[test]
fn holds_expire_release_and_follow_the_roster() {
    let t0 = Instant::now();
    let mut h = Holds::new();
    assert!(h.is_empty());
    h.hold("Pootwaddle", t0 + Duration::from_secs(10));
    h.hold("Blueberry", t0 + Duration::from_secs(20));
    assert_eq!(h.names(), vec!["Blueberry".to_string(), "Pootwaddle".to_string()]);
    h.release("pootwaddle");
    assert_eq!(h.names(), vec!["Blueberry".to_string()]);
    assert_eq!(h.expire(t0 + Duration::from_secs(15)), Vec::<String>::new());
    assert_eq!(h.expire(t0 + Duration::from_secs(21)), vec!["Blueberry".to_string()]);
    assert!(h.is_empty());

    h.hold("Gone", t0 + Duration::from_secs(60));
    let mut s = PartyState::new();
    s.observe("Pootwaddle started to follow you.");
    h.retain_members(&s);
    assert!(h.is_empty());
}

#[test]
fn a_hold_is_extended_not_shortened() {
    let t0 = Instant::now();
    let mut h = Holds::new();
    h.hold("Pootwaddle", t0 + Duration::from_secs(60));
    h.hold("Pootwaddle", t0 + Duration::from_secs(10));
    assert!(h.expire(t0 + Duration::from_secs(30)).is_empty());
}

#[test]
fn bank_names_are_the_shop_type_seven_rooms() {
    use mud_core::content::{Room, RoomId, Shop, ShopId, ShopStock};
    let mut c = Content::default();
    c.add_shop(Shop { id: ShopId(8), name: "Bank of Godfrey".into(), shop_type: 7, min_level: 0, max_level: 0, markup: 0, class_limit: 0, stock: [ShopStock::default(); 20] });
    c.add_shop(Shop { id: ShopId(9), name: "Weapons".into(), shop_type: 1, min_level: 0, max_level: 0, markup: 0, class_limit: 0, stock: [ShopStock::default(); 20] });
    c.add_room(Room { id: RoomId { map: 1, room: 297 }, name: "Bank of Godfrey".into(), room_type: 1, shop: Some(ShopId(8)), ..Default::default() });
    c.add_room(Room { id: RoomId { map: 1, room: 298 }, name: "Armoury".into(), room_type: 1, shop: Some(ShopId(9)), ..Default::default() });
    let names = bank_names(&c);
    assert!(names.contains("Bank of Godfrey"));
    assert!(!names.contains("Armoury"));
}

#[test]
fn a_bank_room_is_found_by_the_name_the_block_prints() {
    use mud_client::party::bank_room;
    use mud_core::content::{Room, RoomId, Shop, ShopId, ShopStock};
    let mut c = Content::default();
    // The shop's name is not the room's for four of the five shipped
    // banks, and the block prints the room's.
    c.add_shop(Shop { id: ShopId(8), name: "Rhudaur Bank".into(), shop_type: 7, min_level: 0, max_level: 0, markup: 0, class_limit: 0, stock: [ShopStock::default(); 20] });
    c.add_room(Room { id: RoomId { map: 2, room: 2568 }, name: "Bank of Rhudaur".into(), room_type: 1, shop: Some(ShopId(8)), ..Default::default() });
    c.add_room(Room { id: RoomId { map: 2, room: 2569 }, name: "Rhudaur Bank".into(), ..Default::default() });
    assert_eq!(bank_room(&c, "Bank of Rhudaur"), Some(RoomId { map: 2, room: 2568 }));
    assert_eq!(bank_room(&c, "Rhudaur Bank"), None, "the shop's name names no room");
    assert_eq!(bank_room(&c, "Armoury"), None);
}

#[test]
fn wait_state_signals_once_each_way() {
    let mut w = WaitState::new();
    assert_eq!(w.on_rest_sent(), Some(Signal::Wait));
    assert_eq!(w.on_rest_sent(), None);
    assert_eq!(w.on_prompt(Some(&Status::Resting)), None);
    assert_eq!(w.on_prompt(Some(&Status::Resting)), None);
    assert_eq!(w.on_prompt(None), Some(Signal::Ok));
    assert_eq!(w.on_prompt(None), None);
}

/// The leader's step was already on the wire when `@wait` landed, so
/// the follower is dragged one room and the board drops its Resting
/// status. The bot sits down again in the new room, and that rest
/// rides under the same hold: no second warning, and the release waits
/// for it to finish. Live 2026-09-14: every rest ended in a drag, and
/// the `@ok` it produced released the leader before the hold was ever
/// judged.
#[test]
fn a_rest_after_a_drag_rides_under_the_same_hold() {
    let mut w = WaitState::new();
    assert_eq!(w.on_rest_sent(), Some(Signal::Wait));
    assert_eq!(w.on_prompt(Some(&Status::Resting)), None);
    assert_eq!(w.on_rest_sent(), None);
    assert_eq!(w.on_prompt(None), None, "the echo's prompt is not yet resting");
    assert_eq!(w.on_prompt(Some(&Status::Resting)), None);
    assert_eq!(w.on_prompt(None), Some(Signal::Ok));
}

#[test]
fn a_refused_rest_releases_at_once() {
    let mut w = WaitState::new();
    w.on_rest_sent();
    assert_eq!(w.on_refused(), Some(Signal::Ok));
    assert_eq!(w.on_refused(), None);
    assert_eq!(w.on_prompt(None), None);
}

#[test]
fn party_config_defaults_and_refuses_zero_waits() {
    let cfg = PartyConfig::default();
    assert_eq!(cfg.wait_secs, 90);
    assert_eq!(cfg.bank_wait_secs, 15);
    assert!(cfg.follow_normal);
    assert_eq!(cfg.poll_secs, 20);
    assert!(cfg.heal);
    assert!(cfg.validate().is_ok());
    assert!(PartyConfig { wait_secs: 0, ..Default::default() }.validate().is_err());
    assert!(PartyConfig { bank_wait_secs: 0, ..Default::default() }.validate().is_err());
    assert!(PartyConfig { poll_secs: 0, ..Default::default() }.validate().is_ok(), "zero turns the poll off");
}

/// The bar says which party the character is in. A follower reads the
/// leader's name, a leader reads how many have joined, and a character
/// on their own reads neither.
#[test]
fn the_bar_names_the_party() {
    use mud_client::lost::Fix;
    use mud_client::session::GameState;
    use mud_client::tui::render_status;

    let mut alone = state();
    let text =
        render_status(&GameState::default(), Instant::now(), None, Fix::Unknown, None, None, None, false, &alone, 120);
    assert!(!text.contains("following"), "{text}");
    assert!(!text.contains("leading"), "{text}");

    alone.observe("You are now following Beef");
    let text =
        render_status(&GameState::default(), Instant::now(), None, Fix::Unknown, None, None, None, false, &alone, 120);
    assert!(text.contains("following Beef"), "{text}");

    let mut leader = state();
    leader.observe("A started to follow you.");
    leader.observe("B started to follow you.");
    let text =
        render_status(&GameState::default(), Instant::now(), None, Fix::Unknown, None, None, None, false, &leader, 120);
    assert!(text.contains("leading 2"), "{text}");
}

/// The assist is rebuilt when a job hands the character back and when a
/// `bot.*` setting changes under it. Both go through `carry_party`, so
/// neither drops an `@ok` the leader is standing for, and neither lets
/// the follower ask for a bank again inside the five minutes.
#[test]
fn a_rebuild_carries_the_wait_and_the_ask_throttle() {
    let cfg = BankConfig { deposit_at_coins: 10, ..BankConfig::default() };
    let heavy = Reading::of(&Inventory::parse(
        "You are carrying 50 gold crowns\nEncumbrance: 16/2400 - None [0%]\n",
    ))
    .expect("a reply with an encumbrance line");
    let now = Instant::now();
    let minute = now + Duration::from_secs(60);

    let mut old = AssistCasts::default();
    old.follower.on_pickup();
    assert!(old.follower.on_reading(&cfg, heavy, now), "the leader is asked once");
    assert_eq!(old.wait.on_rest_sent(), Some(Signal::Wait), "the rest owes an @ok");

    let mut rebuilt = AssistCasts::default();
    rebuilt.carry_party(old);
    assert!(rebuilt.wait.is_waiting(), "the owed @ok survives the rebuild");
    rebuilt.follower.on_pickup();
    assert!(
        !rebuilt.follower.on_reading(&cfg, heavy, minute),
        "the leader was asked a minute ago, rebuild or not"
    );

    // The control: a gate that carried nothing has nobody to wait on
    // and asks straight away, which is what the rebuilt one must not do.
    let mut blank = AssistCasts::default();
    blank.follower.on_pickup();
    assert!(blank.follower.on_reading(&cfg, heavy, minute));
    assert!(!blank.wait.is_waiting());
}

/// The room hears a said word. Captured 2026-09-13 as
/// `Blueberry says "toot"`. The `@` passing through unchanged is
/// UNVERIFIED.
#[test]
fn the_new_words_read_in_both_shapes() {
    assert_eq!(remote("Celery telepaths: @heal 35").map(|r| r.command), Some(Remote::Heal(35)));
    assert_eq!(remote("Celery says \"@heal 35\"").map(|r| r.command), Some(Remote::Heal(35)));
    assert_eq!(remote("Celery says \"@heal\"").map(|r| r.command), Some(Remote::Heal(0)));
    assert_eq!(remote("Celery says \"@heal lots\"").map(|r| r.command), Some(Remote::Heal(0)));
    assert_eq!(remote("Celery says \"@cure\"").map(|r| r.command), Some(Remote::Cure));
    assert_eq!(
        remote("Celery says \"@iam Human Witchunter\"").map(|r| r.command),
        Some(Remote::Iam { race: "Human".into(), class: "Witchunter".into() })
    );
    assert_eq!(
        remote("Celery says \"@iam Half-Elf Cleric\"").map(|r| r.command),
        Some(Remote::Iam { race: "Half-Elf".into(), class: "Cleric".into() })
    );
    assert_eq!(remote("Celery says \"@iam Human\""), None, "a class is required");
    assert_eq!(remote("Beef says \"@wait\"").map(|r| r.command), Some(Remote::Wait));
    assert_eq!(remote("You say \"@heal 35\""), None, "the own echo is not a request");
    assert_eq!(remote("Celery says \"heal me\""), None);
}

#[test]
fn say_is_the_wire_form_for_a_said_word() {
    assert_eq!(say("@heal 35"), "say @heal 35");
}

fn member(name: &str, class: &str, hp: u8) -> Member {
    Member { name: name.into(), invited: false, class: Some(class.into()), hp: Some(hp), pool: None }
}

#[test]
fn a_roster_fills_the_table_and_drops_the_departed() {
    let t0 = Instant::now();
    let mut h = Health::new();
    h.on_roster(&[member("Beef", "Ninja", 100), member("Salad", "Ranger", 86)], t0);
    assert_eq!(h.get("salad").map(|v| v.hp), Some(Some(86)));
    assert_eq!(h.get("Beef").map(|v| v.seen), Some(t0));
    let t1 = t0 + Duration::from_secs(20);
    h.on_roster(&[member("Salad", "Ranger", 90)], t1);
    assert!(h.get("Beef").is_none(), "the departed are dropped");
    assert_eq!(h.get("Salad").map(|v| (v.hp, v.seen)), Some((Some(90), t1)));
}

#[test]
fn a_request_is_a_fresher_row() {
    let t0 = Instant::now();
    let mut h = Health::new();
    h.on_roster(&[member("Celery", "Mage", 80)], t0);
    let t1 = t0 + Duration::from_secs(3);
    h.on_heal("celery", 35, t1);
    let v = h.get("Celery").unwrap();
    assert_eq!((v.hp, v.seen), (Some(35), t1));
    assert_eq!(v.class.as_deref(), Some("Mage"), "a request keeps what the roster said");
    h.on_heal("Newcomer", 20, t1);
    assert_eq!(h.get("newcomer").map(|v| v.hp), Some(Some(20)), "a request from a name the roster has not shown yet still counts");
}

/// The stock roster is two columns and carries no numbers at all. A
/// row without them says nothing about the member's health, so it must
/// not erase what the member has just said out loud, and it must not
/// bump `seen` either, or a healer would heal an already healed member
/// again.
#[test]
fn a_roster_row_without_numbers_erases_nothing() {
    let t0 = Instant::now();
    let mut h = Health::new();
    h.on_heal("Beef", 35, t0);
    let t1 = t0 + Duration::from_secs(3);
    let plain = Member { name: "Beef".into(), invited: false, class: None, hp: None, pool: None };
    h.on_roster(&[plain], t1);
    let v = h.get("Beef").unwrap();
    assert_eq!(v.hp, Some(35), "the said number stands");
    assert_eq!(v.seen, t0, "and it was not seen again");
    assert_eq!(v.class, None);

    // A row that does carry them writes them and is a fresh sighting.
    h.on_roster(&[member("Beef", "Ninja", 90)], t1);
    let v = h.get("Beef").unwrap();
    assert_eq!(v.hp, Some(90));
    assert_eq!(v.seen, t1);
}

#[test]
fn a_cure_request_holds_until_the_next_roster() {
    let t0 = Instant::now();
    let mut h = Health::new();
    h.on_roster(&[member("Celery", "Mage", 80)], t0);
    let t1 = t0 + Duration::from_secs(3);
    h.on_cure("Celery", t1);
    assert_eq!(h.get("Celery").unwrap().poisoned, Some(t1));
    h.on_roster(&[member("Celery", "Mage", 80)], t1 + Duration::from_secs(20));
    assert_eq!(h.get("Celery").unwrap().poisoned, None, "the member says it again on the next tick if still poisoned");
}

#[test]
fn an_introduction_sets_race_and_class_and_a_witchunter_resists() {
    let t0 = Instant::now();
    let mut h = Health::new();
    h.on_roster(&[member("Beef", "Ninja", 100)], t0);
    h.on_iam("Beef", "Human", "Witchunter", t0);
    let v = h.get("Beef").unwrap();
    assert_eq!(v.race.as_deref(), Some("Human"));
    assert!(v.resists_magic());
    h.on_iam("Carrot", "Dwarf", "Paladin", t0);
    assert!(!h.get("Carrot").unwrap().resists_magic());
    let mut from_roster = Health::new();
    from_roster.on_roster(&[member("Beef", "Witchunter", 100)], t0);
    assert!(from_roster.get("Beef").unwrap().resists_magic(), "the roster's class word marks it too");
    h.clear();
    assert!(h.is_empty());
}

#[test]
fn the_ask_goes_out_once_per_round() {
    let clock = RoundClock::new();
    let t0 = Instant::now();
    let mut a = AskState::new();
    assert!(a.due(t0, &clock));
    a.on_sent(t0);
    assert!(!a.due(t0 + Duration::from_millis(500), &clock));
    assert!(a.due(t0 + Duration::from_secs(10), &clock));
}

/// The board prints a `*Combat Off*` ahead of any cast made mid-fight,
/// a cast on another member included. The bot reads `in_flight` to
/// tell that from the target walking off, so a party cast out has to
/// count as ours.
#[test]
fn a_party_cast_in_flight_is_our_own_combat_off() {
    let now = Instant::now();
    let clock = RoundClock::new();
    let mut casts = AssistCasts::default();
    assert!(!casts.in_flight(), "nothing of ours is out to begin with");
    casts.party = party_cast_out(now, &clock);
    assert!(casts.in_flight(), "the cast is ours, so the Combat Off it draws is ours");
}

/// `carry_party` hands the party cast state to the assist replacing
/// the old one. A rebuild that dropped it would forget the cast it
/// has out and would heal everyone it had just healed again.
#[test]
fn a_rebuild_carries_the_party_cast_state() {
    let now = Instant::now();
    let clock = RoundClock::new();
    let mut old = AssistCasts::default();
    old.party = party_cast_out(now, &clock);

    let mut rebuilt = AssistCasts::default();
    rebuilt.carry_party(old);
    assert!(rebuilt.party.in_flight(), "the cast out crosses the rebuild");

    // The control: an assist built fresh has nothing out at all.
    let blank = AssistCasts::default();
    assert!(!blank.party.in_flight());
}

/// A member that left keeps its last number until the next poll
/// otherwise, and a healer goes on casting at a name the room no
/// longer holds.
#[test]
fn a_departed_member_leaves_the_health_table() {
    let now = Instant::now();
    let mut s = state();
    s.observe("Beef started to follow you.");
    s.observe("Carrot started to follow you.");
    let mut h = Health::new();
    h.on_heal("Beef", 40, now);
    h.on_heal("Carrot", 30, now);
    h.retain_members(&s);
    assert!(h.get("Beef").is_some());
    assert!(h.get("Carrot").is_some());

    s.observe("Carrot has been removed from your followers.");
    h.retain_members(&s);
    assert!(h.get("Beef").is_some(), "the member still in the party keeps its row");
    assert!(h.get("Carrot").is_none(), "the member that left does not");
}

/// A healer with one minor heal, its pool seen, and a cast at a hurt
/// member already out on the board.
fn party_cast_out(now: Instant, clock: &RoundClock) -> PartyHeal {
    let mut p = PartyHeal::new(vec![PartySource {
        name: "minor healing".into(),
        cmd: "cast mihe".into(),
        mana_cost: 2,
        kind: PartyKind::Single(HealKind::Minor),
    }]);
    let pool = Correlated {
        event: Event::Prompt { hp: 100, mana: Some(20), status: None },
        answers: None,
        elsewhere: false,
    };
    p.on_event(&pool, now, clock);
    let mut health = Health::new();
    health.on_heal("Celery", 30, now);
    let cfg = BotConfig { minor_heal_at_percent: 70, major_heal_at_percent: 40, ..BotConfig::default() };
    let CastAttempt::Send(cmd) = p.attempt(now, clock, &health, Some(100), &cfg) else {
        panic!("a member at 30 percent is worth a heal");
    };
    assert_eq!(cmd, "cast mihe celery");
    p.on_sent(&cmd, CmdId(7));
    p
}

