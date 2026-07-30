//! The gang membership state machine (gangs.md §1) — CREATE through
//! DISBAND, the roster, and the gangpath channel. Every string is the
//! DLL literal from the slice-7 verification pass unless tagged.

use mud_core::content::{Class, ClassId, Content, Race, RaceId, Room, RoomId, StatBlock};
use mud_core::game::{Core, CoreConfig, Event, Gender, Player, SessionId};

const HALL: RoomId = RoomId { map: 1, room: 1 };

fn world() -> Content {
    let mut content = Content::default();
    content.add_room(Room {
        id: HALL,
        name: "Hall".into(),
        ..Default::default()
    });
    content.add_race(Race {
        id: RaceId(1),
        name: "Human".into(),
        abilities: vec![],
        base_stats: StatBlock::default(),
        max_stats: StatBlock::default(),
        cp: 100,
        hp_per_level: 0,
        exp_chart: 30,
    });
    content.add_class(Class {
        id: ClassId(1),
        name: "Warrior".into(),
        abilities: vec![],
        hp_per_level: 5,
        hp_seed: 4,
        caster_group: 0,
        casting_factor: 0,
        exp_base: 0,
        combat_factor: 4,
        weapon_code: 8,
        armour_code: 9,
    });
    content
}

fn person(name: &str) -> Player {
    Player {
        name: name.into(),
        gender: Gender::Male,
        race: RaceId(1),
        class: ClassId(1),
        level: 10,
        experience: 150_000,
        current_hp: 30,
        hunger: 1000,
        thirst: 1000,
        lives: 9,
        location: HALL,
        ..Default::default()
    }
}

fn texts(events: &[Event], who: SessionId) -> String {
    events
        .iter()
        .filter_map(|e| match e {
            Event::Output { session, text } if *session == who => Some(text.as_str()),
            _ => None,
        })
        .collect()
}

fn core_with(players: &[&Player]) -> (Core, Vec<SessionId>) {
    let mut core = Core::new(world(), CoreConfig::default());
    let ids = players.iter().map(|p| core.attach_player((*p).clone())).collect();
    core.drain_events();
    (core, ids)
}

/// A core with "Iron Fist" (leader Salad) restored, plus the named
/// members attached: (name, gang?, flags). Returns sessions in order.
fn gang_world(members: &[(&str, bool, u16)]) -> (Core, Vec<SessionId>) {
    use mud_core::gang::Gang;
    let config = CoreConfig {
        restored_gangs: vec![Gang::new("Iron Fist", "Salad", 0)],
        restored_gang_members: members
            .iter()
            .filter(|(_, in_gang, _)| *in_gang)
            .map(|(n, _, f)| (n.to_string(), "Iron Fist".to_string(), *f))
            .collect(),
        ..CoreConfig::default()
    };
    let mut core = Core::new(world(), config);
    let ids = members
        .iter()
        .map(|(name, in_gang, flags)| {
            let mut p = person(name);
            if *in_gang {
                p.gang = "Iron Fist".into();
                p.gang_flags = *flags;
            }
            core.attach_player(p)
        })
        .collect();
    core.drain_events();
    (core, ids)
}

// --- §1.1 CREATE ---

#[test]
fn create_gang_success() {
    let (mut core, s) = core_with(&[&person("Salad")]);
    core.input(s[0], "create gang Iron Fist");
    let events = core.drain_events();
    let out = texts(&events, s[0]);
    assert!(out.contains("Gang created."), "{out:?}");

    let gang = core.gang("Iron Fist").expect("gang exists");
    assert_eq!(gang.leader, "Salad");
    assert_eq!(gang.member_count, 1);
    assert_eq!(gang.display, "Iron Fist");
    assert_eq!(gang.name_key, "IRON FIST");
    assert!(
        events.iter().any(|e| matches!(e, Event::PersistGang(g) if g.display == "Iron Fist")),
        "gang persisted"
    );
    assert!(
        events.iter().any(|e| matches!(e, Event::Persist(p) if p.gang == "Iron Fist")),
        "member row persisted"
    );
}

#[test]
fn create_gate_order_exp_before_membership() {
    // gangs.md §1.1 / cmd_create 53520-53534: the exp gate fires first.
    let mut poor = person("Scrub");
    poor.experience = 99_999;
    poor.gang = "Somewhere".into();
    let (mut core, s) = core_with(&[&poor]);
    core.input(s[0], "create gang Nope");
    let out = texts(&core.drain_events(), s[0]);
    assert!(
        out.contains("You are not experienced enough to start your own gang!"),
        "{out:?}"
    );
}

#[test]
fn create_refuses_second_gang() {
    let (mut core, s) = core_with(&[&person("Salad")]);
    core.input(s[0], "create gang First");
    core.drain_events();
    core.input(s[0], "create gang Second");
    let out = texts(&core.drain_events(), s[0]);
    assert!(
        out.contains("You are already in one gang.  You cannot create another one."),
        "{out:?}"
    );
    assert!(core.gang("Second").is_none());
}

#[test]
fn create_name_validation_chain() {
    // Order per cmd_create: too-long → 'None' → invalid character.
    let (mut core, s) = core_with(&[&person("Salad")]);

    core.input(s[0], "create gang This Name Is Way Too Long For A Gang");
    let out = texts(&core.drain_events(), s[0]);
    assert!(
        out.contains("The name you have chosen is too LONG: This Name Is Way Too Long For A Gang"),
        "{out:?}"
    );

    core.input(s[0], "create gang none");
    let out = texts(&core.drain_events(), s[0]);
    assert!(out.contains("You may not use 'None' as a gang name."), "{out:?}");

    core.input(s[0], "create gang Caf\u{e9} Crew");
    let out = texts(&core.drain_events(), s[0]);
    assert!(
        out.contains("You have specified an invalid character in your gang name."),
        "{out:?}"
    );
    assert!(core.gang("none").is_none());
}

#[test]
fn create_duplicate_names_refused_with_leader_line() {
    let (mut core, s) = core_with(&[&person("Salad"), &person("Torgo")]);
    core.input(s[0], "create gang Iron Fist");
    core.drain_events();
    // Case-insensitive collision via the uppercase key.
    core.input(s[1], "create gang IRON fist");
    let out = texts(&core.drain_events(), s[1]);
    assert!(out.contains("The name you have chosen is already being used!"), "{out:?}");
    assert!(out.contains("Salad is the leader of Iron Fist."), "{out:?}");
}

#[test]
fn create_without_name_falls_to_say() {
    // margc < 3 → return 0 → the SAY fall-through (cmd_create 53476).
    let (mut core, s) = core_with(&[&person("Salad")]);
    core.input(s[0], "create gang");
    let out = texts(&core.drain_events(), s[0]);
    assert!(out.contains("You say"), "falls through to say: {out:?}");
    assert!(!out.contains("Gang created"), "{out:?}");
}

#[test]
fn create_room_prints_the_lease_stub_and_others_consume_silently() {
    let (mut core, s) = core_with(&[&person("Salad")]);
    core.input(s[0], "create room north");
    let out = texts(&core.drain_events(), s[0]);
    assert!(
        out.contains("If you are a gang leader you may lease a Gang House."),
        "{out:?}"
    );
    // Non-keyword forms with args are consumed with no message (the
    // decompile's silent fall-off arm) — only the prompt comes back:
    // neither the say fall-through nor the lease stub fires.
    core.input(s[0], "create castle now");
    let out = texts(&core.drain_events(), s[0]);
    assert!(!out.contains("You say"), "not say: {out:?}");
    assert!(!out.contains("gang leader"), "not the lease stub: {out:?}");
}

// --- §1.2 INVITE MEMBER ---

#[test]
fn invite_member_notifies_both_sides() {
    let (mut core, s) = gang_world(&[("Salad", true, 0), ("Torgo", false, 0)]);
    core.input(s[0], "invite member Torgo");
    let events = core.drain_events();
    let inviter = texts(&events, s[0]);
    let target = texts(&events, s[1]);
    assert!(
        inviter.contains("You have invited Torgo to join your gang."),
        "{inviter:?}"
    );
    assert!(
        target.contains("Gang leader Salad has invited you to join Iron Fist."),
        "{target:?}"
    );
}

#[test]
fn lieutenant_invites_with_the_lieutenant_prefix() {
    use mud_core::gang::GF_LIEUTENANT;
    let (mut core, s) = gang_world(&[
        ("Vex", true, GF_LIEUTENANT),
        ("Torgo", false, 0),
    ]);
    core.input(s[0], "invite member Torgo");
    let target = texts(&core.drain_events(), s[1]);
    assert!(
        target.contains("Lieutenant Vex has invited you to join Iron Fist."),
        "{target:?}"
    );
}

#[test]
fn plain_member_may_not_invite() {
    let (mut core, s) = gang_world(&[
        ("Salad", true, 0),
        ("Grunt", true, 0),
        ("Torgo", false, 0),
    ]);
    core.input(s[1], "invite member Torgo");
    let out = texts(&core.drain_events(), s[1]);
    assert!(
        out.contains("You must be the leader or a lieutenant of your gang to invite new members!"),
        "{out:?}"
    );
}

#[test]
fn invite_edge_cases() {
    let (mut core, s) = gang_world(&[("Salad", true, 0), ("Torgo", false, 0)]);

    // Unknown target: the DLL prints margv[1] — the subword as typed —
    // so the line really reads "You don't see member here!" (cmd_invite
    // 52753, a faithful oddity).
    core.input(s[0], "invite member Nobody");
    let out = texts(&core.drain_events(), s[0]);
    assert!(out.contains("You don't see member here!"), "{out:?}");

    // Self-invite.
    core.input(s[0], "invite member Salad");
    let out = texts(&core.drain_events(), s[0]);
    assert!(out.contains("Why would you invite yourself?"), "{out:?}");

    // Bare invite: the syntax line.
    core.input(s[0], "invite");
    let out = texts(&core.drain_events(), s[0]);
    assert!(out.contains("Syntax: INVITE {user name}"), "{out:?}");

    // Duplicate invite: consumed, inviter confirmation repeats, target
    // is NOT re-notified (invite_to_gang dedupes silently).
    core.input(s[0], "invite member Torgo");
    core.drain_events();
    core.input(s[0], "invite member Torgo");
    let events = core.drain_events();
    assert!(
        texts(&events, s[0]).contains("You have invited Torgo to join your gang."),
        "inviter line repeats"
    );
    assert!(
        !texts(&events, s[1]).contains("has invited you"),
        "no duplicate notification"
    );
}

#[test]
fn invite_without_member_subword_is_the_unported_party_arm() {
    // Plain `INVITE <name>` is the party-follow invite (M8) — it falls
    // through to say like every unported system.
    let (mut core, s) = gang_world(&[("Salad", true, 0), ("Torgo", false, 0)]);
    core.input(s[0], "invite Torgo");
    let out = texts(&core.drain_events(), s[0]);
    assert!(out.contains("You say"), "{out:?}");
}

// --- §1.3 JOIN GANG ---

#[test]
fn join_with_invite_announces_to_the_gang() {
    let (mut core, s) = gang_world(&[("Salad", true, 0), ("Torgo", false, 0)]);
    core.input(s[0], "invite member Torgo");
    core.drain_events();
    core.input(s[1], "join gang iron fist");
    let events = core.drain_events();
    let joiner = texts(&events, s[1]);
    assert!(joiner.contains("You have joined the gang Iron Fist."), "{joiner:?}");
    // tell_gang has no sender exclusion — the joiner hears the
    // broadcast too, as does the leader.
    assert!(joiner.contains("Torgo just joined your gang."), "{joiner:?}");
    assert!(
        texts(&events, s[0]).contains("Torgo just joined your gang."),
        "leader hears it"
    );
    assert_eq!(core.gang("Iron Fist").unwrap().member_count, 2);
    assert!(
        events.iter().any(|e| matches!(e, Event::Persist(p) if p.name == "Torgo" && p.gang == "Iron Fist")),
        "joiner persisted"
    );
    assert!(
        events.iter().any(|e| matches!(e, Event::PersistGang(g) if g.member_count == 2)),
        "count persisted"
    );
}

#[test]
fn join_gates_and_the_clear_all_invites_rule() {
    use mud_core::gang::Gang;
    // Two gangs: invited to Iron Fist, but tries Rivals first.
    let config = CoreConfig {
        restored_gangs: vec![
            Gang::new("Iron Fist", "Salad", 0),
            Gang::new("Rivals", "Ghost", 0),
        ],
        restored_gang_members: vec![("Salad".into(), "Iron Fist".into(), 0)],
        ..CoreConfig::default()
    };
    let mut core = Core::new(world(), config);
    let mut salad = person("Salad");
    salad.gang = "Iron Fist".into();
    let s0 = core.attach_player(salad);
    let s1 = core.attach_player(person("Torgo"));
    core.drain_events();
    core.input(s0, "invite member Torgo");
    core.drain_events();

    // Nonexistent gang: refused, invites untouched.
    core.input(s1, "join gang Nobodies");
    let out = texts(&core.drain_events(), s1);
    assert!(out.contains("That gang doesn't exist!"), "{out:?}");

    // Existing-but-uninvited gang: refused — and clear_gang_invitations
    // (user, NULL) wipes EVERY pending invite for the user.
    core.input(s1, "join gang Rivals");
    let out = texts(&core.drain_events(), s1);
    assert!(out.contains("You have not been invited to join that gang!"), "{out:?}");
    core.input(s1, "join gang Iron Fist");
    let out = texts(&core.drain_events(), s1);
    assert!(
        out.contains("You have not been invited to join that gang!"),
        "the failed Rivals attempt cleared the Iron Fist invite: {out:?}"
    );

    // Already in a gang.
    core.input(s0, "join gang Rivals");
    let out = texts(&core.drain_events(), s0);
    assert!(
        out.contains("You may not join another gang!  You are already a member of one."),
        "{out:?}"
    );
}

#[test]
fn join_malformed_forms_fall_through() {
    // JOIN GANG with no name is cmd_follow (margc < 3), JOIN 3 is the
    // channel system, JOIN GUILD is NOT an alias (cmd_join matches only
    // "gang") — all unported (M8), all say.
    let (mut core, s) = gang_world(&[("Salad", true, 0), ("Torgo", false, 0)]);
    for form in ["join gang", "join 3", "join guild Iron Fist"] {
        core.input(s[1], form);
        let out = texts(&core.drain_events(), s[1]);
        assert!(out.contains("You say"), "{form}: {out:?}");
    }
}
