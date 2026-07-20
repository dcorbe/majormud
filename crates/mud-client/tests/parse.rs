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
    assert_eq!(files.len(), 51, "corpus size changed");
    let mut prompts = 0;
    for f in &files {
        let ev = corpus_events(f);
        prompts += count(&ev, |e| matches!(e, Event::Prompt { .. }));
    }
    // Ground truth computed with the Python reference pipeline
    // (mudlib.py semantics + backspace resolution) over the same files.
    assert_eq!(prompts, 5347);
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
