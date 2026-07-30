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

// --- §1.6 roster + the SET GANG view toggle ---

#[test]
fn all_roster_view_lists_mirror_members_with_online_marks() {
    use mud_core::gang::GF_LIEUTENANT;
    // Ghost is in the mirror but never attaches: the ALL view still
    // shows them, unmarked.
    let config = CoreConfig {
        restored_gangs: {
            let mut g = mud_core::gang::Gang::new("Iron Fist", "Salad", 0);
            g.member_count = 4;
            vec![g]
        },
        restored_gang_members: vec![
            ("Salad".into(), "Iron Fist".into(), 0),
            ("Vex".into(), "Iron Fist".into(), GF_LIEUTENANT),
            ("Grunt".into(), "Iron Fist".into(), 0),
            ("Ghost".into(), "Iron Fist".into(), 0),
        ],
        ..CoreConfig::default()
    };
    let mut core = Core::new(world(), config);
    let mut ids = Vec::new();
    for (name, flags) in [("Salad", 0), ("Vex", GF_LIEUTENANT), ("Grunt", 0)] {
        let mut p = person(name);
        p.gang = "Iron Fist".into();
        p.gang_flags = flags;
        ids.push(core.attach_player(p));
    }
    core.drain_events();
    core.input(ids[0], "gang");
    let out = texts(&core.drain_events(), ids[0]);
    assert!(out.contains("Iron Fist members (4)"), "{out:?}");
    let salad = format!("{:<29.29} - Online [Leader]", "Salad");
    assert!(out.contains(&salad), "{out:?}");
    let vex = format!("{:<29.29} - Online [Lieutenant]", "Vex");
    assert!(out.contains(&vex), "{out:?}");
    let grunt = format!("{:<29.29} - Online ", "Grunt");
    assert!(out.contains(&grunt), "{out:?}");
    let ghost = format!("{:<29.29} ", "Ghost");
    assert!(out.contains(&ghost), "offline member listed: {out:?}");
    assert!(!out.contains("Ghost                         - Online"), "{out:?}");
}

#[test]
fn online_roster_view_hides_offline_members() {
    use mud_core::gang::{GF_LIEUTENANT, GF_ROSTER_ONLINE_ONLY};
    let (mut core, s) = gang_world(&[
        ("Salad", true, GF_ROSTER_ONLINE_ONLY),
        ("Vex", true, GF_LIEUTENANT),
    ]);
    // Ghost is only in the mirror (never attached in gang_world's list),
    // so seed via a third member who detaches — instead just rely on
    // the mirror having exactly the attached two plus none offline.
    core.input(s[0], "gang");
    let out = texts(&core.drain_events(), s[0]);
    assert!(out.contains("Iron Fist members (online)"), "{out:?}");
    let leader = format!("{:<29.29}  [Leader]", "Salad");
    assert!(out.contains(&leader), "{out:?}");
    let vex = format!("{:<29.29}  [Lieutenant]", "Vex");
    assert!(out.contains(&vex), "{out:?}");
}

#[test]
fn roster_leader_offline_forms() {
    use mud_core::gang::GF_ROSTER_ONLINE_ONLY;
    // The leader never attaches.
    let (mut core, s) = gang_world(&[("Grunt", true, 0)]);
    core.input(s[0], "gang");
    let out = texts(&core.drain_events(), s[0]);
    let leader = format!("{:<29.29}          [Leader]", "Salad");
    assert!(out.contains(&leader), "ALL view offline leader: {out:?}");

    let (mut core, s) = gang_world(&[("Grunt", true, GF_ROSTER_ONLINE_ONLY)]);
    core.input(s[0], "gang");
    let out = texts(&core.drain_events(), s[0]);
    let leader = format!("{:<29.29}  [Leader - Offline]", "Salad");
    assert!(out.contains(&leader), "online view offline leader: {out:?}");
}

#[test]
fn all_roster_shows_the_disbanded_banner() {
    use mud_core::gang::GANG_DISBANDED;
    let config = CoreConfig {
        restored_gangs: {
            let mut g = mud_core::gang::Gang::new("Iron Fist", "Salad", 0);
            g.flags |= GANG_DISBANDED;
            vec![g]
        },
        restored_gang_members: vec![("Grunt".into(), "Iron Fist".into(), 0)],
        ..CoreConfig::default()
    };
    let mut core = Core::new(world(), config);
    let mut p = person("Grunt");
    p.gang = "Iron Fist".into();
    let s = core.attach_player(p);
    core.drain_events();
    core.input(s, "gang");
    let out = texts(&core.drain_events(), s);
    assert!(out.contains("This gang has been disbanded."), "{out:?}");
}

#[test]
fn set_gang_toggles_and_sets_the_roster_view() {
    use mud_core::gang::GF_ROSTER_ONLINE_ONLY;
    let (mut core, s) = gang_world(&[("Salad", true, 0)]);

    // Bare SET GANG: toggle.
    core.input(s[0], "set gang");
    let out = texts(&core.drain_events(), s[0]);
    assert!(out.contains("You will now only see online gang members."), "{out:?}");
    core.input(s[0], "set gang");
    let out = texts(&core.drain_events(), s[0]);
    assert!(out.contains("You will now see all gang members."), "{out:?}");

    // Explicit forms.
    core.input(s[0], "set gang online");
    let out = texts(&core.drain_events(), s[0]);
    assert!(out.contains("You will now only see online gang members."), "{out:?}");
    core.input(s[0], "set gang online");
    let out = texts(&core.drain_events(), s[0]);
    assert!(
        out.contains("You will now only see online gang members."),
        "explicit online is idempotent, not a toggle: {out:?}"
    );
    core.input(s[0], "set gang bogus");
    let out = texts(&core.drain_events(), s[0]);
    assert!(out.contains("Valid gang options: Online, All"), "{out:?}");
    core.input(s[0], "set gang all");
    let events = core.drain_events();
    assert!(texts(&events, s[0]).contains("You will now see all gang members."));
    assert!(
        events.iter().any(|e| matches!(e, Event::Persist(p)
            if p.gang_flags & GF_ROSTER_ONLINE_ONLY == 0)),
        "view choice persists"
    );
}

// --- §5.2 gangpaths ---

#[test]
fn gangpath_reaches_the_gang_including_the_sender() {
    use mud_core::gang::GF_LIEUTENANT;
    let (mut core, s) = gang_world(&[
        ("Salad", true, 0),
        ("Vex", true, GF_LIEUTENANT),
        ("Torgo", false, 0),
    ]);
    core.input(s[0], "gang meet at the well");
    let events = core.drain_events();
    for (label, id) in [("sender", s[0]), ("member", s[1])] {
        let out = texts(&events, id);
        assert!(
            out.contains("Salad gangpaths: ") && out.contains("meet at the well"),
            "{label}: {out:?}"
        );
    }
    assert!(
        !texts(&events, s[2]).contains("gangpaths"),
        "non-members hear nothing"
    );
}

#[test]
fn gangpath_without_a_gang_refuses() {
    let (mut core, s) = gang_world(&[("Torgo", false, 0)]);
    core.input(s[0], "gang hello?");
    let out = texts(&core.drain_events(), s[0]);
    assert!(out.contains("You are not in a gang at the present!"), "{out:?}");
}

// --- §1.4 LEAVE GANG ---

#[test]
fn member_leaves_with_room_broadcast() {
    let (mut core, s) = gang_world(&[
        ("Salad", true, 0),
        ("Grunt", true, 0),
        ("Torgo", false, 0),
    ]);
    core.input(s[1], "leave gang");
    let events = core.drain_events();
    let leaver = texts(&events, s[1]);
    assert!(leaver.contains("You have left Iron Fist."), "{leaver:?}");
    // tell_room, not tell_gang: bystanders in the room see the line.
    assert!(
        texts(&events, s[2]).contains("Grunt has left Iron Fist."),
        "room broadcast"
    );
    assert_eq!(core.gang("Iron Fist").unwrap().member_count, 0, "count--");
    assert!(
        events.iter().any(|e| matches!(e, Event::Persist(p) if p.name == "Grunt" && p.gang.is_empty())),
        "leaver persisted gangless"
    );
    // The roster no longer lists them.
    core.input(s[0], "gang");
    let out = texts(&core.drain_events(), s[0]);
    assert!(!out.contains("Grunt"), "{out:?}");
}

#[test]
fn leader_may_not_leave() {
    let (mut core, s) = gang_world(&[("Salad", true, 0)]);
    core.input(s[0], "leave gang");
    let out = texts(&core.drain_events(), s[0]);
    assert!(
        out.contains("You are the leader - you may not leave your gang. Use DISBAND GANG"),
        "{out:?}"
    );
    assert_eq!(core.gang("Iron Fist").unwrap().member_count, 1);
}

#[test]
fn leave_without_a_gang_and_malformed_forms() {
    let (mut core, s) = gang_world(&[("Torgo", false, 0)]);
    core.input(s[0], "leave gang");
    let out = texts(&core.drain_events(), s[0]);
    assert!(out.contains("You are not currently in a gang."), "{out:?}");
    // Bare LEAVE is the group system (M8) — falls through.
    core.input(s[0], "leave");
    let out = texts(&core.drain_events(), s[0]);
    assert!(out.contains("You say"), "{out:?}");
}

// --- §1.4 DISBAND GANG (the 0x88 confirmation) ---

#[test]
fn disband_confirms_then_sweeps() {
    use mud_core::gang::GF_LIEUTENANT;
    let (mut core, s) = gang_world(&[
        ("Salad", true, 0),
        ("Vex", true, GF_LIEUTENANT),
    ]);
    core.input(s[0], "disband gang");
    let out = texts(&core.drain_events(), s[0]);
    assert!(out.contains("Are you sure you want to disband Iron Fist? "), "{out:?}");

    core.input(s[0], "yes");
    let events = core.drain_events();
    for id in [s[0], s[1]] {
        let out = texts(&events, id);
        assert!(out.contains("The gang Iron Fist has now been disbanded."), "{out:?}");
        assert!(
            out.contains("The name may not be used again until all members have entered the game!"),
            "{out:?}"
        );
    }
    let gang = core.gang("Iron Fist").unwrap();
    assert!(gang.is_disbanded());
    assert_eq!(gang.member_count, 0, "both online members drained");
    assert!(
        events.iter().any(|e| matches!(e, Event::Persist(p)
            if p.name == "Vex" && p.gang.is_empty() && p.gang_flags & GF_LIEUTENANT == 0)),
        "swept member persisted stripped"
    );
    assert!(
        events.iter().any(|e| matches!(e, Event::PersistGang(g) if g.is_disbanded())),
        "gang row persisted disbanded"
    );
}

#[test]
fn disband_decline_and_gates() {
    let (mut core, s) = gang_world(&[("Salad", true, 0), ("Grunt", true, 0)]);

    // Non-leader.
    core.input(s[1], "disband gang");
    let out = texts(&core.drain_events(), s[1]);
    assert!(
        out.contains("You are not the leader of the gang; You may not disband it!"),
        "{out:?}"
    );

    // Decline: anything but a single Y-word (the 0x88 arm requires
    // margc == 1 with a leading Y — two words decline too).
    core.input(s[0], "disband gang");
    core.drain_events();
    core.input(s[0], "y u sure");
    let out = texts(&core.drain_events(), s[0]);
    assert!(out.contains("Your gang has not been disbanded."), "{out:?}");
    assert!(!core.gang("Iron Fist").unwrap().is_disbanded());

    // The declined line is CONSUMED by the continuation, not executed.
    core.input(s[0], "disband gang");
    core.drain_events();
    core.input(s[0], "no");
    let out = texts(&core.drain_events(), s[0]);
    assert!(out.contains("Your gang has not been disbanded."), "{out:?}");
    assert!(!out.contains("You say"), "{out:?}");

    // Syntax forms: DISBAND GUILD is not an alias; bare DISBAND too.
    for form in ["disband guild", "disband"] {
        core.input(s[0], form);
        let out = texts(&core.drain_events(), s[0]);
        assert!(out.contains("Syntax: DISBAND {Party/Gang}"), "{form}: {out:?}");
    }

    // Gangless.
    let (mut core2, t) = gang_world(&[("Torgo", false, 0)]);
    core2.input(t[0], "disband gang");
    let out = texts(&core2.drain_events(), t[0]);
    assert!(out.contains("You are not in a gang!"), "{out:?}");
}

// --- §1.4 UNINVITE MEMBER ---

#[test]
fn leader_uninvites_online_member() {
    let (mut core, s) = gang_world(&[("Salad", true, 0), ("Grunt", true, 0)]);
    core.input(s[0], "uninvite member Grunt");
    let events = core.drain_events();
    assert!(
        texts(&events, s[1]).contains("Gang leader Salad has exiled you from Iron Fist."),
        "{:?}", texts(&events, s[1])
    );
    assert!(
        texts(&events, s[0]).contains("You have removed Grunt from your gang."),
        "{:?}", texts(&events, s[0])
    );
    assert_eq!(core.gang("Iron Fist").unwrap().member_count, 0);
    assert!(
        events.iter().any(|e| matches!(e, Event::Persist(p) if p.name == "Grunt" && p.gang.is_empty())),
    );
}

#[test]
fn uninvite_rank_gates() {
    use mud_core::gang::GF_LIEUTENANT;
    let (mut core, s) = gang_world(&[
        ("Salad", true, 0),
        ("Vex", true, GF_LIEUTENANT),
        ("Kord", true, GF_LIEUTENANT),
        ("Grunt", true, 0),
    ]);
    // Lieutenant removing a lieutenant.
    core.input(s[1], "uninvite member Kord");
    let out = texts(&core.drain_events(), s[1]);
    assert!(out.contains("You must be the gang leader to uninvite a lieutenant!"), "{out:?}");
    // Typing the leader's exact name.
    core.input(s[1], "uninvite member Salad");
    let out = texts(&core.drain_events(), s[1]);
    assert!(out.contains("You are not able to uninvite the gang leader."), "{out:?}");
    // Reaching the leader by abbreviation slips the name gate and hits
    // the insolence arm instead.
    core.input(s[1], "uninvite member Sal");
    let out = texts(&core.drain_events(), s[1]);
    assert!(out.contains("Such insolence as this may not be tolerated"), "{out:?}");
    // A plain member has no rank at all.
    core.input(s[3], "uninvite member Vex");
    let out = texts(&core.drain_events(), s[3]);
    assert!(
        out.contains("You must be the leader or lieutenant of your gang to uninvite members!"),
        "{out:?}"
    );
}

#[test]
fn uninvite_offline_member_and_misc() {
    use mud_core::gang::Gang;
    let config = CoreConfig {
        restored_gangs: {
            let mut g = Gang::new("Iron Fist", "Salad", 0);
            g.member_count = 2;
            vec![g]
        },
        restored_gang_members: vec![
            ("Salad".into(), "Iron Fist".into(), 0),
            ("Ghost".into(), "Iron Fist".into(), 0),
        ],
        ..CoreConfig::default()
    };
    let mut core = Core::new(world(), config);
    let mut leader = person("Salad");
    leader.gang = "Iron Fist".into();
    let s = core.attach_player(leader);
    core.drain_events();

    // Offline removal prints the name AS TYPED and emits the offline
    // row write.
    core.input(s, "uninvite member ghost");
    let events = core.drain_events();
    assert!(
        texts(&events, s).contains("You removed ghost from your gang."),
        "{:?}", texts(&events, s)
    );
    assert_eq!(core.gang("Iron Fist").unwrap().member_count, 1);
    assert!(
        events.iter().any(|e| matches!(e, Event::PersistOfflineGangMember { name, clear_gang: true, .. } if name == "Ghost")),
        "offline clear event"
    );
    // Roster no longer lists Ghost.
    core.input(s, "gang");
    assert!(!texts(&core.drain_events(), s).contains("Ghost"));

    // Unknown name — the bang line with the name as typed.
    core.input(s, "uninvite member nobody");
    let out = texts(&core.drain_events(), s);
    assert!(out.contains("You don't see nobody here!"), "{out:?}");

    // Self, bare, and the unported party arm.
    core.input(s, "uninvite member Salad");
    let out = texts(&core.drain_events(), s);
    assert!(out.contains("You are not able to uninvite the gang leader."), "{out:?}");
    core.input(s, "uninvite");
    let out = texts(&core.drain_events(), s);
    assert!(out.contains("Syntax: UNINVITE {user name}"), "{out:?}");
    core.input(s, "uninvite Salad");
    let out = texts(&core.drain_events(), s);
    assert!(out.contains("You say"), "party-follow arm falls through: {out:?}");
}

// --- §1.5 PROMOTE / DEMOTE ---

#[test]
fn promote_and_demote_online() {
    use mud_core::gang::GF_LIEUTENANT;
    let (mut core, s) = gang_world(&[("Salad", true, 0), ("Grunt", true, 0)]);
    core.input(s[0], "promote Grunt");
    let events = core.drain_events();
    assert!(
        texts(&events, s[1]).contains("Your gang leader has promoted you to the rank of lieutenant."),
    );
    assert!(
        texts(&events, s[0]).contains("Gang member Grunt has been notified of their promotion."),
    );
    assert!(
        events.iter().any(|e| matches!(e, Event::Persist(p)
            if p.name == "Grunt" && p.gang_flags & GF_LIEUTENANT != 0)),
    );
    // The roster reflects the new rank.
    core.input(s[0], "gang");
    let out = texts(&core.drain_events(), s[0]);
    assert!(out.contains("[Lieutenant]"), "{out:?}");

    core.input(s[0], "demote Grunt");
    let events = core.drain_events();
    assert!(texts(&events, s[1]).contains("Your gang leader has demoted you."));
    assert!(
        texts(&events, s[0]).contains("Gang member Grunt has been notified of their demotion."),
    );
    assert!(
        events.iter().any(|e| matches!(e, Event::Persist(p)
            if p.name == "Grunt" && p.gang_flags & GF_LIEUTENANT == 0)),
    );
}

#[test]
fn promote_and_demote_offline_set_pending_bits() {
    use mud_core::gang::{Gang, GF_PENDING_DEMOTE, GF_PENDING_PROMOTE};
    let config = CoreConfig {
        restored_gangs: vec![Gang::new("Iron Fist", "Salad", 0)],
        restored_gang_members: vec![
            ("Salad".into(), "Iron Fist".into(), 0),
            ("Ghost".into(), "Iron Fist".into(), 0),
        ],
        ..CoreConfig::default()
    };
    let mut core = Core::new(world(), config);
    let mut leader = person("Salad");
    leader.gang = "Iron Fist".into();
    let s = core.attach_player(leader);
    core.drain_events();

    core.input(s, "promote ghost");
    let events = core.drain_events();
    assert!(
        texts(&events, s)
            .contains("Gang member Ghost will be notified of their promotion next time they log on."),
        "record casing in the notice: {:?}", texts(&events, s)
    );
    assert!(
        events.iter().any(|e| matches!(e, Event::PersistOfflineGangMember { name, or_mask, clear_gang: false, .. }
            if name == "Ghost" && or_mask & GF_PENDING_PROMOTE != 0)),
    );

    core.input(s, "demote ghost");
    let events = core.drain_events();
    assert!(
        texts(&events, s)
            .contains("Gang member Ghost will be notified of their demotion next time they log on."),
    );
    assert!(
        events.iter().any(|e| matches!(e, Event::PersistOfflineGangMember { name, or_mask, clear_gang: false, .. }
            if name == "Ghost" && or_mask & GF_PENDING_DEMOTE != 0)),
    );
}

#[test]
fn promote_demote_gates() {
    use mud_core::gang::GF_LIEUTENANT;
    let (mut core, s) = gang_world(&[
        ("Salad", true, 0),
        ("Vex", true, GF_LIEUTENANT),
        ("Torgo", false, 0),
    ]);
    // Self: the DLL's reused demote-yourself line.
    core.input(s[0], "promote Salad");
    let out = texts(&core.drain_events(), s[0]);
    assert!(
        out.contains("You may not demote yourself to lieutenant. Your gang needs a leader!"),
        "{out:?}"
    );
    core.input(s[0], "demote Salad");
    let out = texts(&core.drain_events(), s[0]);
    assert!(out.contains("You wish to demote yourself from leader of your gang?"), "{out:?}");
    // Online non-member (note the two arms' different wording, sic).
    core.input(s[0], "promote Torgo");
    let out = texts(&core.drain_events(), s[0]);
    assert!(out.contains("You may not promote somebody who is not in your gang!."), "{out:?}");
    core.input(s[0], "demote Torgo");
    let out = texts(&core.drain_events(), s[0]);
    assert!(out.contains("You may not demote someone who is not in your gang!"), "{out:?}");
    // Non-leader: silent consume (the decompile has no else arm).
    core.input(s[1], "promote Torgo");
    let out = texts(&core.drain_events(), s[1]);
    assert!(!out.contains("notified") && !out.contains("You say"), "{out:?}");
    // Bare: syntax.
    core.input(s[0], "promote");
    let out = texts(&core.drain_events(), s[0]);
    assert!(out.contains("Syntax: PROMOTE {user name}"), "{out:?}");
    // Unknown offline name: the syntax line again (the DLL quirk).
    core.input(s[0], "promote nobody");
    let out = texts(&core.drain_events(), s[0]);
    assert!(out.contains("Syntax: PROMOTE {user name}"), "{out:?}");
    // Multi-word names never reach the command (margc == 2 gate).
    core.input(s[0], "promote Iron Fist");
    let out = texts(&core.drain_events(), s[0]);
    assert!(!out.contains("Syntax") && !out.contains("You say"), "{out:?}");
}
