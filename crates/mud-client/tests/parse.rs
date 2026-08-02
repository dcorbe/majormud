//! Event-parser tests. Line patterns are anchored to `mud_core::text`
//! builders (the canonical output spec); corpus assertions come from the
//! 51 captured live-board transcripts under `re/oracle/`.

use mud_client::events::{Actor, Event};
use mud_client::parse::Parser;
use mud_client::wire::{TelnetFilter, cp437_to_string};
use mud_core::content::Direction;
use mud_core::text::{self, color};

fn parse_all(input: &str) -> Vec<Event> {
    let mut p = Parser::new();
    let mut ev = p.push(input);
    ev.extend(p.finish());
    ev
}

// --- prompt ---

#[test]
fn prompt_plain() {
    let ev = parse_all("\x1b[0;37m[HP=35\x1b[0;37m]:\x1b[0m");
    assert_eq!(ev, vec![Event::Prompt { hp: 35, mana: None }]);
}

#[test]
fn prompt_negative_hp_while_downed() {
    let ev = parse_all("[HP=-12]:");
    assert_eq!(
        ev,
        vec![Event::Prompt {
            hp: -12,
            mana: None
        }]
    );
}

#[test]
fn prompt_with_mana_and_kai() {
    assert_eq!(
        parse_all("[HP=26/MA=12]:"),
        vec![Event::Prompt {
            hp: 26,
            mana: Some(12)
        }]
    );
    assert_eq!(
        parse_all("[HP=20/KAI=4]:"),
        vec![Event::Prompt {
            hp: 20,
            mana: Some(4)
        }]
    );
}

#[test]
fn prompt_emitted_midstream_without_newline() {
    // The prompt arrives with no trailing newline; the parser must emit
    // it immediately (expect/bot logic keys on it), not on finish().
    let mut p = Parser::new();
    let ev = p.push("\r\n[HP=35]:");
    assert_eq!(ev, vec![Event::Prompt { hp: 35, mana: None }]);
    assert_eq!(p.finish(), vec![]);
}

#[test]
fn prompt_followed_by_async_line() {
    // Captured shape (oracle_arena2.raw): the prompt and an incoming
    // combat line share one physical line.
    let ev = parse_all("[HP=35]:The kobold thief stabs you for 4 damage!\r\n");
    assert_eq!(
        ev,
        vec![
            Event::Prompt { hp: 35, mana: None },
            Event::CombatHit {
                attacker: Actor::Other("The kobold thief".into()),
                target: Actor::You,
                damage: 4
            },
        ]
    );
}

#[test]
fn prompt_redrawn_midline() {
    // Captured shapes: rest-tick dots around a prompt redraw
    // (oracle_monster_attacks3.raw) and typing echo split by an async
    // redraw (oracle_ptargets_zinvar.raw).
    let ev = parse_all(".......[HP=19/MA=18]:.........\r\n");
    assert_eq!(
        ev,
        vec![
            Event::Line(".......".into()),
            Event::Prompt {
                hp: 19,
                mana: Some(18)
            },
            Event::Line(".........".into()),
        ]
    );
    let ev = parse_all("[HP=42/MA=43]:l[HP=43/MA=46]:ook\r\n");
    assert_eq!(
        ev,
        vec![
            Event::Prompt {
                hp: 42,
                mana: Some(43)
            },
            Event::Line("l".into()),
            Event::Prompt {
                hp: 43,
                mana: Some(46)
            },
            Event::Line("ook".into()),
        ]
    );
}

// --- room blocks ---

fn arena_block() -> String {
    format!(
        "{name}Rocky Trail, Cliff Face{reset}\r\n\
         {plain}A pit of sand and gravel, walls all around.{reset}\r\n\
         You notice 7 silver nobles, 43 copper farthings here.\r\n\
         Also here: kobold thief.\r\n\
         Obvious exits: closed door north, uF\x08p\r\n\
         {plain}[HP=35{plain}]:{reset}",
        name = color::ROOM_NAME,
        plain = color::PLAIN,
        reset = color::RESET,
    )
}

#[test]
fn room_block_full() {
    let ev = parse_all(&arena_block());
    assert_eq!(ev.len(), 2, "RoomSeen + Prompt, got {ev:?}");
    match &ev[0] {
        Event::RoomSeen(r) => {
            assert_eq!(r.name, "Rocky Trail, Cliff Face");
            assert_eq!(r.exits, vec!["closed door north", "up"]);
            assert_eq!(r.also_here, vec!["kobold thief"]);
            assert_eq!(r.items, vec!["7 silver nobles", "43 copper farthings"]);
        }
        other => panic!("expected RoomSeen, got {other:?}"),
    }
    assert_eq!(ev[1], Event::Prompt { hp: 35, mana: None });
}

#[test]
fn room_block_no_exits_marker() {
    let block = format!(
        "{}Solid Rock Cell{}\r\nObvious exits: {}\r\n",
        color::ROOM_NAME,
        color::RESET,
        text::NO_EXITS
    );
    let ev = parse_all(&block);
    match &ev[0] {
        Event::RoomSeen(r) => {
            assert_eq!(r.name, "Solid Rock Cell");
            assert!(r.exits.is_empty());
        }
        other => panic!("expected RoomSeen, got {other:?}"),
    }
}

#[test]
fn banner_titles_are_superseded_by_real_room_name() {
    // 1;36 also paints banner art; a later 1;36 line before the exits
    // line must win.
    let block = format!(
        "{n}M A J O R  M U D banner art{r}\r\n\
         {n}Newhaven, Village Entrance{r}\r\n\
         {p}Welcome to Newhaven!{r}\r\n\
         Obvious exits: nP\x08orth, sM\x08outh\r\n",
        n = color::ROOM_NAME,
        p = color::PLAIN,
        r = color::RESET,
    );
    let ev = parse_all(&block);
    let room = ev
        .iter()
        .find_map(|e| match e {
            Event::RoomSeen(r) => Some(r),
            _ => None,
        })
        .expect("RoomSeen");
    assert_eq!(room.name, "Newhaven, Village Entrance");
    assert_eq!(room.exits, vec!["north", "south"]);
}

#[test]
fn chunked_input_equals_single_push() {
    let block = arena_block();
    let whole = parse_all(&block);
    let mut p = Parser::new();
    let mut chunked = Vec::new();
    let mut buf = String::new();
    for c in block.chars() {
        buf.push(c);
        if buf.len() >= 3 {
            chunked.extend(p.push(&buf));
            buf.clear();
        }
    }
    chunked.extend(p.push(&buf));
    chunked.extend(p.finish());
    assert_eq!(whole, chunked);
}

// --- combat ---

#[test]
fn player_hit_line() {
    let line = text::player_hit("punch", "kobold thief", 1);
    assert_eq!(
        parse_all(&format!("{line}\r\n")),
        vec![Event::CombatHit {
            attacker: Actor::You,
            target: Actor::Other("kobold thief".into()),
            damage: 1
        }]
    );
}

#[test]
fn player_crit_line() {
    let line = text::player_crit("slash", "orc warrior", 12);
    assert_eq!(
        parse_all(&format!("{line}\r\n")),
        vec![Event::CombatHit {
            attacker: Actor::You,
            target: Actor::Other("orc warrior".into()),
            damage: 12
        }]
    );
}

#[test]
fn player_miss_and_glance_lines() {
    let miss = text::player_miss("swing at", "kobold thief");
    let glance = text::player_glance("swing at", "kobold thief");
    for line in [miss, glance] {
        let ev = parse_all(&format!("{line}\r\n"));
        assert!(
            matches!(ev.as_slice(), [Event::CombatMiss { .. }]),
            "expected CombatMiss for {line:?}, got {ev:?}"
        );
    }
}

#[test]
fn a_cyan_line_that_is_not_a_swing_is_not_a_combat_miss() {
    // MEASURED 2026-07-26: the board paints the player's plain miss and the
    // defender's parry `0;36`, NOT the `0;31` this parser assumed — the red
    // family is only the glance. (The old colour-only rule therefore never
    // matched a real plain miss; it matched the glance, which also starts
    // with "You" and which the `glances off` clause catches anyway.)
    //
    // Cyan is a crowded colour: `You notice ... here.` outside a room block
    // and `You attempt to cast ..., but fail.` share it, and 40 distinct such
    // lines appear across the corpus. What separates a swing from all of them
    // is the terminal `!` — across all 57 transcripts, every cyan line that
    // starts with "You" and ends with "!" is a miss or a parry (84 of them,
    // no exceptions), so the shape carries the rule and the colour alone
    // does not.
    for line in [
        "You notice pewter tankard here.",
        "You attempt to cast magic missile, but fail.",
    ] {
        let painted = format!("{}{line}{}", text::color::YOUR_MISS, text::color::RESET);
        let ev = parse_all(&format!("{painted}\r\n"));
        assert!(
            !matches!(ev.as_slice(), [Event::CombatMiss { .. }]),
            "{line:?} is not a swing, got {ev:?}"
        );
    }
    // The real thing still classifies.
    let miss = text::player_miss("swing at", "giant bat");
    assert!(
        matches!(
            parse_all(&format!("{miss}\r\n")).as_slice(),
            [Event::CombatMiss { .. }]
        ),
        "a plain miss is still a miss"
    );
    let parry = text::player_dodge("swing at", "giant bat");
    assert!(
        matches!(
            parse_all(&format!("{parry}\r\n")).as_slice(),
            [Event::CombatMiss { .. }]
        ),
        "a parry reads as a non-damaging swing to the bot"
    );
}

#[test]
fn monster_hit_and_dodge_lines() {
    let hit = text::fill_message(text::MONSTER_HIT_TPL, &["The zombie", "smashes", "9"]);
    assert_eq!(
        parse_all(&format!("{hit}\r\n")),
        vec![Event::CombatHit {
            attacker: Actor::Other("The zombie".into()),
            target: Actor::You,
            damage: 9
        }]
    );
    let dodge = text::fill_message(
        text::MONSTER_DODGE_TPL,
        &["The zombie", "swings at", "a rusty sword"],
    );
    let ev = parse_all(&format!("{dodge}\r\n"));
    assert!(
        matches!(ev.as_slice(), [Event::CombatMiss { .. }]),
        "expected CombatMiss, got {ev:?}"
    );
}

// --- movement ---

#[test]
fn actor_left_lines() {
    let line = text::left_via("Kaimon", Direction::East);
    assert_eq!(
        parse_all(&format!("{line}\r\n")),
        vec![Event::ActorLeft {
            name: "Kaimon".into(),
            to: Some("east".into())
        }]
    );
    let up = text::left_via("Kaimon", Direction::Up);
    assert_eq!(
        parse_all(&format!("{up}\r\n")),
        vec![Event::ActorLeft {
            name: "Kaimon".into(),
            to: Some("up".into())
        }]
    );
}

#[test]
fn actor_entered_lines() {
    let walk = text::walks_in_from("Oracle", Direction::West);
    assert_eq!(
        parse_all(&format!("{walk}\r\n")),
        vec![Event::ActorEntered {
            name: "Oracle".into(),
            from: Some("west".into())
        }]
    );
    let mmove = text::monster_moves_in_from("happy guardsman", Direction::Down);
    assert_eq!(
        parse_all(&format!("{mmove}\r\n")),
        vec![Event::ActorEntered {
            name: "happy guardsman".into(),
            from: Some("down".into())
        }]
    );
    let spawn = text::spawn_arrived("kobold thief", None);
    assert_eq!(
        parse_all(&format!("{spawn}\r\n")),
        vec![Event::ActorEntered {
            name: "kobold thief".into(),
            from: None
        }]
    );
}

/// Monster movement wordings are DATA, not grammar: each monster's
/// `movemsg` record holds free-form enter/leave/follow templates
/// (`re/mmud_wgnt.sqlite`, `message` table), so the verb is per-monster,
/// the article is part of the template, and spawns say "from nowhere".
/// Every line here is either a live capture (2026-07-31 arena session,
/// `re/docs/spellcasting.md` §spawn-incidentals) or a shipped template
/// rendered with a real monster name.
#[test]
fn movemsg_arrivals_cover_the_template_families() {
    for (line, name, from) in [
        // The one that shipped the bug: farm sat blind to a spawned rat.
        (
            "A thin giant rat creeps into the room from nowhere.",
            "thin giant rat",
            None,
        ),
        ("A acid slime oozes into the room from nowhere.", "acid slime", None),
        (
            "A angry kobold thief sneaks into the room from nowhere.",
            "angry kobold thief",
            None,
        ),
        (
            "A black cat slinks into the room from the west.",
            "black cat",
            Some("west"),
        ),
        (
            "An small orc rogue walks into the room from the west.",
            "small orc rogue",
            Some("west"),
        ),
        // "<verb> in from <origin>" family.
        ("A cave bear lumbers in from the east!", "cave bear", Some("east")),
        ("The giant snake slithers in from the north.", "giant snake", Some("north")),
        // Templates hardcoding "the" double it when %s fills "the west".
        ("A scorpion crawls in from the the west!", "scorpion", Some("west")),
        // "enters" with and without "the room".
        ("A walking chest enters from the south.", "walking chest", Some("south")),
        (
            "A giant war dog enters the room from the east.",
            "giant war dog",
            Some("east"),
        ),
        // Vertical and spawn origins on the default monster wording.
        ("short kobold moves into the room from below.", "short kobold", Some("down")),
        ("kobold moves into the room from nowhere.", "kobold", None),
        (
            "A tasloi warrior drops into the room from above.",
            "tasloi warrior",
            Some("up"),
        ),
        ("A demonling flaps down from above!", "demonling", Some("up")),
        // Origin-less room entries.
        ("A hill giant stomps into the room!", "hill giant", None),
        ("A snake slithers into the area!", "snake", None),
        ("A giant crab scurries into the room.", "giant crab", None),
        // Follow: the monster chased us through the exit.
        ("The giant rat creeps in after you!", "giant rat", None),
        (
            "The kobold thief creeps into the room after you!",
            "kobold thief",
            None,
        ),
    ] {
        assert_eq!(
            parse_all(&format!("{line}\r\n")),
            vec![Event::ActorEntered {
                name: name.into(),
                from: from.map(str::to_string),
            }],
            "line: {line}"
        );
    }
}

#[test]
fn movemsg_departures_cover_the_template_families() {
    for (line, name, to) in [
        (
            "The thin giant rat creeps out of the room to the north.",
            "thin giant rat",
            Some("north"),
        ),
        ("The acid slime oozes out of the room to the east.", "acid slime", Some("east")),
        ("The giant snake slithers out to the west!", "giant snake", Some("west")),
        ("A black cat slinks off to the south.", "black cat", Some("south")),
        ("The barmaid walks off to the the west.", "barmaid", Some("west")),
        ("The moss zombie leaves to the north.", "moss zombie", Some("north")),
    ] {
        assert_eq!(
            parse_all(&format!("{line}\r\n")),
            vec![Event::ActorLeft {
                name: name.into(),
                to: to.map(str::to_string),
            }],
            "line: {line}"
        );
    }
}

/// Live monster whiffs: the attack text is per-monster data too
/// (`attackmissmsg`/`attackdodgemsg` templates), so the fixed mud-core
/// tails are not enough. All four lines are live captures or shipped
/// templates rendered with their real monster.
#[test]
fn live_monster_whiffs_are_combat_misses() {
    for line in [
        "The thin giant rat lunges at you!",
        "The thin giant rat lunges at you, but you dodge out of the way!",
        "The giant snake snaps at you, but you dodge out of its way!",
        "The kobold thief strikes you, but your armour deflects the blow!",
    ] {
        let ev = parse_all(&format!("{line}\r\n"));
        assert!(
            matches!(ev.as_slice(), [Event::CombatMiss { .. }]),
            "expected CombatMiss for {line:?}, got {ev:?}"
        );
    }
    // The long tail has no shared wording at all ("reaches out for
    // you!", "slashes you with their scimitar!") — there the whiff cyan
    // plus "you" is the signature. All four painted lines are corpus
    // captures.
    for line in [
        "The carrion beast snaps at you with its teeth!",
        "The angry orc trainee swings at you with their longsword!",
        "The angry dark cleric attempted to cast spiritual hammer at you, but failed.",
        "The wraith reaches out for you!",
    ] {
        let ev = parse_all(&format!("\x1b[0;36m{line}\x1b[0m\r\n"));
        assert!(
            matches!(ev.as_slice(), [Event::CombatMiss { .. }]),
            "expected CombatMiss for {line:?}, got {ev:?}"
        );
    }
    // A bystander's whiff names no "you": same cyan, stays a plain line.
    let ev = parse_all("\x1b[0;36mPoop swipes at kobold thief!\x1b[0m\r\n");
    assert!(
        matches!(ev.as_slice(), [Event::Line(_)]),
        "bystander whiff misread: {ev:?}"
    );
}

/// The template dump also holds pattern-less lines ("A dark storm
/// approaches!") and near-misses; those must stay plain [`Event::Line`]s
/// for the combat backstops rather than fabricate an actor.
#[test]
fn arrival_lookalikes_stay_plain_lines() {
    for line in [
        "A massive wave appears from the east, announcing a massive sea creature!",
        "A dark storm approaches!",
        "You hear movement to the north.",
    ] {
        assert_eq!(
            parse_all(&format!("{line}\r\n")),
            vec![Event::Line(line.into())],
            "line: {line}"
        );
    }
}

// --- flood control / fallback ---

#[test]
fn slow_down_line() {
    let ev = parse_all("Why don't you slow down for a few seconds?\r\n");
    assert_eq!(ev, vec![Event::SlowDown]);
}

#[test]
fn unknown_lines_are_never_dropped() {
    let ev = parse_all("something the classifier has never seen\r\n");
    assert_eq!(
        ev,
        vec![Event::Line(
            "something the classifier has never seen".into()
        )]
    );
}

// --- corpus ---

fn corpus_events(path: &std::path::Path) -> Vec<Event> {
    let raw = std::fs::read(path).unwrap();
    let mut f = TelnetFilter::new();
    let data = f.push(&raw).data;
    let mut p = Parser::new();
    let mut ev = p.push(&cp437_to_string(&data));
    ev.extend(p.finish());
    ev
}

fn count<F: Fn(&Event) -> bool>(ev: &[Event], f: F) -> usize {
    ev.iter().filter(|e| f(e)).count()
}

#[test]
fn corpus_all_files_parse_and_prompt_totals_match() {
    let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/../../re/oracle");
    let mut files: Vec<_> = std::fs::read_dir(dir)
        .unwrap()
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "raw"))
        .collect();
    files.sort();
    // The corpus GROWS: `re/oracle/` is shared with the server track, and
    // every oracle expedition adds transcripts. Both numbers below therefore
    // move by design — when they do, recompute the ground truth rather than
    // taking the Rust parser's word for it (see below) and update them.
    assert_eq!(files.len(), 120, "corpus size changed");
    let mut prompts = 0;
    for f in &files {
        let ev = corpus_events(f);
        prompts += count(&ev, |e| matches!(e, Event::Prompt { .. }));
    }
    // Ground truth computed with the Python reference pipeline (mudlib.py
    // semantics: CP437 decode, ANSI strip, anti-bot backspace resolution)
    // over the same files, NOT with this parser — the point is to
    // cross-validate the Rust wire layer against the reference, so deriving
    // the number from the thing under test would make it circular.
    //
    // 5347 over the 51 files that predate the 2026-07-26 dodge-parry
    // expedition; that expedition's five captures contribute 598 more
    // (death_revive 46, training 111, parry acc-mid 141, acc-mid2 47,
    // acc-high 253), and the to-hit scale run that followed it the same day
    // (`oracle_dodge_parry_control.raw`, grey spider) adds 182.
    //
    // Recomputed with the reference pipeline, which reproduced the previous
    // 5945 over the previous 56 files before the new one was added — note
    // that the count needs the FULL prompt shape including the `/MA=` and
    // `/KAI=` variants; an HP-only regex undercounts these transcripts
    // roughly threefold.
    //
    // 2026-07-30: the slice-8 close-out campaign added 45 raws (dodge-parry
    // blocks a1-b3, engage-lock attempts, the charm lifecycle takes, the
    // swarm probes, recreations and training runs) — 13,911 more prompts.
    // Recomputed with the same reference pipeline over all 102 files.
    //
    // 2026-07-31: two expeditions landed the same day and merged. The
    // slice-8 field expedition added 13 raws (hand session, gang program
    // + restart-persistence passes, the min-abbrev sweeps, seedy fame
    // walks, the slime lair camp — 607 prompts, the first `/KAI=`
    // captures), and the correlation rework's five acceptance runs added
    // accept-run1..5 (1,124 prompts). 20,038 + 607 + 1,124 over 120.
    assert_eq!(prompts, 21769);
}

#[test]
fn corpus_oracle_m1_events() {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../re/oracle/oracle_m1.raw");
    let ev = corpus_events(std::path::Path::new(path));
    assert_eq!(count(&ev, |e| matches!(e, Event::Prompt { .. })), 7);
    let rooms: Vec<_> = ev
        .iter()
        .filter_map(|e| match e {
            Event::RoomSeen(r) => Some(r),
            _ => None,
        })
        .collect();
    assert_eq!(rooms.len(), 4);
    assert_eq!(rooms[0].name, "Newhaven, Village Entrance");
    assert_eq!(rooms[0].exits, vec!["north", "south", "west", "southeast"]);
}

#[test]
fn corpus_oracle_arena2_events() {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../re/oracle/oracle_arena2.raw"
    );
    let ev = corpus_events(std::path::Path::new(path));
    assert_eq!(count(&ev, |e| matches!(e, Event::Prompt { .. })), 57);
    let rooms: Vec<_> = ev
        .iter()
        .filter_map(|e| match e {
            Event::RoomSeen(r) => Some(r),
            _ => None,
        })
        .collect();
    assert_eq!(rooms.len(), 6);
    let pit = rooms.last().unwrap();
    assert_eq!(pit.exits, vec!["closed door north", "up"]);
    assert_eq!(pit.also_here, vec!["kobold thief"]);
    let incoming = count(
        &ev,
        |e| matches!(e, Event::CombatHit { target: Actor::You, .. }),
    );
    assert_eq!(incoming, 45);
}

// --- prompt-glued room render (stopstate-run6.raw, live board) ---

#[test]
fn a_room_name_glued_to_a_redrawn_prompt_still_opens_the_block() {
    // A dangling prompt disturbed by async output gets redrawn, and the
    // room render lands on the SAME physical line: the board separates
    // them with cursor-back + erase-line, not a newline. Byte-exact from
    // stopstate-run6.raw — this is how every room block renders in a busy
    // room, which is exactly where position tracking matters most.
    let glued = "\x1b[79D\x1b[K\x1b[0;37m[HP=45\x1b[0;37m/MA=8\x1b[0;37m]:\
                 \x1b[0;37;40m\x1b[79D\x1b[K\x1b[1;36mDungeon, Entrance\r\n\
                 \x1b[79D\x1b[K\x1b[0;37;40m    You stand in a barely torchlit entryway.\r\n\
                 Obvious exits: north, south\r\n";
    let ev = parse_all(glued);
    assert!(
        ev.iter().any(|e| matches!(
            e,
            Event::RoomSeen(r) if r.name == "Dungeon, Entrance"
                && r.exits == ["north", "south"]
        )),
        "the glued room block dissolved: {ev:?}"
    );
}

/// A foreign board's redraw (cwrun2.raw, mud.cwgaming.com 2026-08-01):
/// the dangling prompt is overwritten with a bare `\r` + erase-line
/// instead of stock's newline, so the redrawn text reaches the
/// classifier with a leading carriage return — and every ^-anchored
/// rule missed it. Live cost: NO ActorEntered ever fired on that board;
/// mob entries during stops went unseen (the at-you whiff rule survived
/// only because it anchors on the tail).
#[test]
fn a_line_redrawn_over_the_prompt_with_a_bare_cr_still_classifies() {
    let mut p = Parser::new();
    let mut ev = p.push(
        "\x1b[0;37m[HP=\x1b[0;37m38\x1b[0;37m/MA=\x1b[0;37m8\x1b[0;37m]:\r\x1b[2K\x1b[79D\x1b[K\x1b[0;36mA small carrion beast creeps in the room from nowhere.\x1b[0m\r\n",
    );
    ev.extend(p.finish());
    assert!(
        ev.iter().any(|e| matches!(
            e,
            Event::ActorEntered { name, .. } if name == "small carrion beast"
        )),
        "{ev:?}"
    );
}

// --- occupant colour ---------------------------------------------------
//
// The board paints occupants by what they ARE, and it is the only signal
// that says so. `strip_ansi` runs before classification, so the colour
// has to be read off the raw line or it is gone.

/// Bytes copied verbatim from cwrun3.raw / cwrun6.raw.
fn also_here_line(entries: &[(&str, &str)]) -> String {
    let mut s = String::from("\x1b[0;35mAlso here: \x1b[0m");
    for (i, (sgr, name)) in entries.iter().enumerate() {
        if i > 0 {
            s.push_str("\x1b[0;35m, \x1b[0m");
        }
        s.push_str(&format!("\x1b[{sgr}m{name}\x1b[0m\x1b[0m"));
    }
    s.push_str("\x1b[0;35m.\x1b[0m\r\n");
    s
}

fn room_with(entries: &[(&str, &str)]) -> mud_client::events::RoomView {
    let mut p = Parser::new();
    let mut text = String::from("\r\n\x1b[1;36mNewhaven, Village Center\r\n");
    text.push_str(&also_here_line(entries));
    text.push_str("\x1b[0;32mObvious exits: north\r\n");
    let mut evs = p.push(&text);
    evs.extend(p.finish());
    evs.into_iter()
        .find_map(|e| match e {
            Event::RoomSeen(r) => Some(r),
            _ => None,
        })
        .expect("a room block")
}

#[test]
fn occupant_colours_survive_parsing() {
    let r = room_with(&[
        ("1;35", "big kobold thief"),
        ("0;36", "big drunken brawler"),
        ("0;37", "fierce guardsman"),
    ]);
    assert_eq!(
        r.also_here,
        vec!["big kobold thief", "big drunken brawler", "fierce guardsman"]
    );
    assert_eq!(
        r.also_here_sgr,
        vec![
            Some("1;35".to_string()),
            Some("0;36".to_string()),
            Some("0;37".to_string())
        ]
    );
}

/// An unpainted board must read as "no opinion", never as "nothing here
/// is aggressive" — that is the difference between falling back to the
/// case rule and refusing to fight at all.
#[test]
fn an_unpainted_block_carries_no_colour_opinion() {
    let mut p = Parser::new();
    let mut evs = p.push(
        "\r\n\x1b[1;36mNewhaven, Village Center\r\nAlso here: giant rat.\r\n\x1b[0;32mObvious exits: north\r\n",
    );
    evs.extend(p.finish());
    let r = evs
        .into_iter()
        .find_map(|e| match e {
            Event::RoomSeen(r) => Some(r),
            _ => None,
        })
        .expect("a room block");
    assert_eq!(r.also_here, vec!["giant rat"]);
    assert!(r.also_here_sgr.iter().all(Option::is_none), "{:?}", r.also_here_sgr);
}

/// Players are bright magenta too, so colour alone cannot tell them from
/// an aggressive monster — capitalisation does.
#[test]
fn players_are_painted_like_aggressive_monsters() {
    let r = room_with(&[("1;35", "Habuji"), ("1;35", "large filthbug")]);
    assert_eq!(
        r.also_here_sgr,
        vec![Some("1;35".to_string()), Some("1;35".to_string())]
    );
}
