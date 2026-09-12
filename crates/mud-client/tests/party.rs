//! The party state machine, the telepath grammar, the hold set and the
//! wait state. Pure functions, no board.

use mud_client::party::{Change, Member, PartyState, Role};

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
