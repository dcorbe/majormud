//! The party state machine, the telepath grammar, the hold set, the
//! wait state, and the party state an assist rebuild carries across.
//! Pure functions, no board.

use std::time::{Duration, Instant};

use mud_client::bank::{BankConfig, Reading};
use mud_client::events::Status;
use mud_client::party::{
    Change, Holds, Member, PartyConfig, PartyState, Remote, Request, Role, Signal, WaitState,
    bank_names, permitted, remote, telepath,
};
use mud_client::sheet::Inventory;
use mud_client::tui::AssistCasts;
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
    assert_eq!(s.members, vec![Member { name: "Pootwaddle".into(), invited: false }]);
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
    assert_eq!(s.members, vec![Member { name: "Blueberry".into(), invited: false }]);
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
            Member { name: "Pootwaddle".into(), invited: false },
            Member { name: "Blueberry".into(), invited: false },
            Member { name: "Newguy".into(), invited: true },
        ]
    );
    assert_eq!(s.role, Role::Leader);
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
    assert_eq!(s.members, vec![Member { name: "Pootwaddle".into(), invited: false }]);
}

#[test]
fn a_roster_ended_by_chatter_still_reads_as_the_roster() {
    let mut s = state();
    s.observe("Pootwaddle started to follow you.");
    s.observe("The following people are in your travel party:");
    s.observe("  Pootwaddle                     Mystic");
    assert_eq!(s.observe("Beef says \"hi\""), Some(Change::Roster));
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
    assert!(cfg.validate().is_ok());
    assert!(PartyConfig { wait_secs: 0, ..Default::default() }.validate().is_err());
    assert!(PartyConfig { bank_wait_secs: 0, ..Default::default() }.validate().is_err());
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
