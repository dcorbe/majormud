//! Room-graph tests against the real WG3-NT database. Ground truths
//! computed with re/room_graph_wg.py (the decode reference).

use mud_client::graph::{ExitRequirement, RoomGraph, DIRECTIONS};
use mud_core::content::{Direction, RoomId};
use mud_client::graph::{Capabilities, Cost, exit_cost_for};
use mud_client::purse::Purse;

fn db_path() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../re/mmud_wgnt.sqlite")
}

fn graph() -> &'static RoomGraph {
    use std::sync::OnceLock;
    static G: OnceLock<RoomGraph> = OnceLock::new();
    G.get_or_init(|| RoomGraph::load(&db_path()).expect("load room graph"))
}

#[test]
fn loads_all_rooms() {
    assert_eq!(graph().len(), 26720);
}

#[test]
fn town_gates_decodes_exactly() {
    let g = graph();
    let gates = RoomId { map: 1, room: 1 };
    let room = g.room(gates).expect("room (1,1)");
    assert_eq!(room.name, "Town Gates");
    let n = room.exits[Direction::North as usize].as_ref().unwrap();
    assert_eq!(n.dest, RoomId { map: 1, room: 3 });
    assert_eq!(n.exit_type, 19);
    let s = room.exits[Direction::South as usize].as_ref().unwrap();
    assert_eq!(s.dest, RoomId { map: 1, room: 100 });
    assert_eq!(s.exit_type, 19);
    let e = room.exits[Direction::East as usize].as_ref().unwrap();
    assert_eq!(e.dest, RoomId { map: 1, room: 1381 });
    assert_eq!(e.exit_type, 11);
    let w = room.exits[Direction::West as usize].as_ref().unwrap();
    assert_eq!(w.dest, RoomId { map: 1, room: 101 });
    assert_eq!(w.exit_type, 0);
    assert_eq!(
        room.exits.iter().filter(|e| e.is_some()).count(),
        4,
        "exactly four exits"
    );
}

#[test]
fn route_to_adjacent_room() {
    let g = graph();
    let route = g
        .route(RoomId { map: 1, room: 1 }, RoomId { map: 1, room: 100 })
        .expect("route exists");
    assert_eq!(route, vec![Direction::South]);
}

#[test]
fn route_crosses_map_change_portal() {
    // (1,1) -> (15,982) is 3 steps; the last hop is a type-8 map-change
    // exit N from (1,1382) (ground truth: room_graph_wg.py BFS).
    let g = graph();
    let route = g
        .route(RoomId { map: 1, room: 1 }, RoomId { map: 15, room: 982 })
        .expect("route exists");
    assert_eq!(route.len(), 3);
    assert_eq!(*route.last().unwrap(), Direction::North);
}

#[test]
fn route_to_self_is_empty() {
    let g = graph();
    let route = g
        .route(RoomId { map: 1, room: 1 }, RoomId { map: 1, room: 1 })
        .expect("trivial route");
    assert!(route.is_empty());
}

#[test]
fn route_to_unknown_room_is_none() {
    let g = graph();
    assert!(
        g.route(RoomId { map: 1, room: 1 }, RoomId { map: 999, room: 9999 })
            .is_none()
    );
}

/// Threat ranking comes from the shipped data, not from a guess, and it
/// has to rank the Newhaven dungeon the way a player would: the cave bear
/// above the wanderers that drift into its lair.
#[test]
fn threat_ranks_the_dungeon_the_way_a_player_would() {
    let db = std::path::Path::new("../../re/mmud_wgnt.sqlite");
    if !db.exists() {
        eprintln!("skipping: {} not present", db.display());
        return;
    }
    let threat = RoomGraph::load_threat(db).expect("threat table");
    let get = |n: &str| *threat.get(n).unwrap_or_else(|| panic!("missing {n}"));

    assert!(get("cave bear") > get("acid slime"), "the bear is the boss here");
    assert!(get("acid slime") > get("kobold thief"));
    assert!(get("kobold thief") > get("filthbug"));
    assert!(get("filthbug") > get("giant rat"));
}

/// `distances` duplicates `route`'s BFS skeleton on purpose — one
/// early-exits and rebuilds a path, the other does neither — so the
/// invariant that keeps them honest has to be asserted rather than
/// assumed.
#[test]
fn distances_agree_with_route_lengths() {
    let g = graph();
    let from = RoomId { map: 1, room: 1 };
    let d = g.distances(from);
    assert_eq!(d.get(&from), Some(&0), "standing still costs nothing");
    for to in [
        RoomId { map: 1, room: 3 },
        RoomId { map: 1, room: 1072 },
        RoomId { map: 1, room: 2324 },
        RoomId { map: 1, room: 2146 },
    ] {
        let steps = g.route(from, to).expect("reachable").len();
        assert_eq!(d.get(&to), Some(&steps), "{to:?}");
    }
}

/// The Silvermere Small Alleyway (1/405) has a type-6 HIDDEN exit south
/// into the secret passage that runs under the Adventurer's Guild. A
/// hop-count router takes it — 24 steps against 27 by the street — and
/// the walk then stands in the alley sending `s` at a brick wall until it
/// desyncs (live, 2026-08-02, walking a ranger to its trainer).
///
/// Three steps is a cheap price for not gambling on three search rolls,
/// so the route has to leave by the only exit the board will show.
#[test]
fn a_route_prefers_three_more_streets_to_a_hidden_exit() {
    let g = graph();
    let alley = RoomId { map: 1, room: 405 };
    let trainer = RoomId { map: 1, room: 502 };
    let route = g.route(alley, trainer).expect("the guild is reachable");
    assert_eq!(
        route.first(),
        Some(&Direction::North),
        "left by the hidden exit instead of the street: {route:?}"
    );
    assert_eq!(route.len(), 27, "the street route, not the secret passage");
}

/// Costed, not forbidden. 1,383 shipped exits are hidden and whole areas
/// sit behind them, so a router that refused type 6 outright would answer
/// "no route" to rooms that are perfectly reachable — worse than the bug
/// it fixes.
#[test]
fn a_hidden_exit_is_still_taken_when_it_is_the_only_way() {
    let g = graph();
    let alley = RoomId { map: 1, room: 405 };
    let passage = RoomId { map: 1, room: 452 };
    assert_eq!(
        g.route(alley, passage),
        Some(vec![Direction::South]),
        "the secret passage is only reachable through its hidden exit"
    );
}

#[test]
fn distances_omit_unreachable_rooms() {
    let g = graph();
    let d = g.distances(RoomId { map: 1, room: 1 });
    assert!(!d.contains_key(&RoomId { map: 999, room: 9999 }));
    assert!(d.len() <= g.len());
}

/// A room that is not in the graph reaches nothing, rather than
/// reporting itself at distance zero.
#[test]
fn distances_from_an_unknown_room_are_empty() {
    assert!(
        graph()
            .distances(RoomId {
                map: 999,
                room: 9999
            })
            .is_empty()
    );
}

// --- constrained routing ----------------------------------------------

/// The router can be fenced. This is a PROHIBITION, unlike `exit_cost`,
/// which prices awkward exits rather than refusing them — and the
/// difference is deliberate: a cost is the client's opinion about a type
/// of exit, a fence is the operator's decision about a specific room, and
/// "no route" is the right answer to the second.
#[test]
fn a_forbidden_room_is_routed_around() {
    let g = graph();
    // Three Slum Street rooms in a row; the middle one is on the direct
    // route between its neighbours.
    let from = RoomId { map: 1, room: 1072 };
    let to = RoomId { map: 1, room: 1076 };
    let direct = g.route(from, to).expect("the slums are connected");

    let midway = {
        let mut at = from;
        let mut seen = Vec::new();
        for d in &direct {
            at = g.room(at).unwrap().exits[*d as usize].as_ref().unwrap().dest;
            seen.push(at);
        }
        seen[0]
    };

    let around = g
        .route_within(from, to, &|_, e| e.dest != midway)
        .expect("the slums have more than one way through");
    assert!(
        around.len() > direct.len(),
        "a detour is longer than the direct route: {} vs {}",
        around.len(),
        direct.len()
    );

    // And walking it really does miss the fenced room.
    let mut at = from;
    for d in &around {
        at = g.room(at).unwrap().exits[*d as usize].as_ref().unwrap().dest;
        assert_ne!(at, midway, "the detour walked through the fence");
    }
    assert_eq!(at, to);
}

/// Fencing the only way through answers "no route", and that is the
/// correct answer rather than a defect: the caller asked for a wall.
#[test]
fn fencing_the_only_way_through_answers_none() {
    let g = graph();
    let cavern = RoomId { map: 1, room: 2156 };
    let entrance = RoomId { map: 1, room: 2152 };
    assert!(
        g.route(entrance, cavern).is_some(),
        "the Small Cavern is a dead end off the Dungeon Entrance"
    );
    assert_eq!(
        g.route_within(entrance, cavern, &|_, e| e.dest != cavern),
        None,
        "fencing the destination itself leaves nowhere to arrive"
    );
}

/// An unconstrained `route_within` is `route`. Pinned because the two
/// share one search and the whole point of the refactor was that the
/// existing callers could not drift.
#[test]
fn an_open_fence_routes_exactly_as_before() {
    let g = graph();
    let from = RoomId { map: 1, room: 1 };
    let to = RoomId { map: 1, room: 2156 };
    assert_eq!(
        g.route_within(from, to, &|_, _| true),
        g.route(from, to),
        "no fence must mean no difference"
    );
    assert_eq!(
        g.distances_within(from, &|_, _| true).len(),
        g.distances(from).len()
    );
}

// --- exit requirements --------------------------------------------------

fn requirement(g: &RoomGraph, room: RoomId, dir: Direction) -> ExitRequirement {
    let i = DIRECTIONS.iter().position(|d| *d == dir).expect("compass");
    g.room(room)
        .and_then(|r| r.exits[i].as_ref())
        .map(|e| e.requirement.clone())
        .unwrap_or(ExitRequirement::None)
}

/// The Silvermere gates charge to pass. Both directions carry the same
/// `roomtype = 4, para1 = 5` in the data; whether both actually CHARGE is
/// an engine question this plan does not answer.
#[test]
fn the_silvermere_gate_is_a_toll() {
    let g = graph();
    let inner = RoomId { map: 1, room: 1381 }; // Town Gates, Inner Bailey
    let road = RoomId { map: 1, room: 1382 };  // Main Road, Silvermere Gates
    assert_eq!(
        requirement(g, inner, Direction::East),
        ExitRequirement::Toll { gold: 5 }
    );
    assert_eq!(
        requirement(g, road, Direction::West),
        ExitRequirement::Toll { gold: 5 }
    );
}

/// Every type-4 exit in the shipped world is a toll and no other type is.
/// 57 of them, and their amounts are a currency distribution -- which is
/// the evidence type 4 was identified from in the first place.
#[test]
fn every_toll_in_the_world_is_a_type_four_exit() {
    let g = graph();
    let mut amounts: Vec<u32> = Vec::new();
    for (_, room) in g.iter() {
        for edge in room.exits.iter().flatten() {
            match (&edge.requirement, edge.exit_type) {
                (ExitRequirement::Toll { gold }, 4) => amounts.push(*gold),
                (ExitRequirement::Toll { .. }, t) => {
                    panic!("a toll on exit type {t}, which is not 4")
                }
                (_, 4) => panic!("a type-4 exit that is not a toll"),
                _ => {}
            }
        }
    }
    amounts.sort_unstable();
    assert_eq!(amounts.len(), 57, "57 shipped type-4 exits");
    let distinct: std::collections::BTreeSet<u32> = amounts.iter().copied().collect();
    assert_eq!(
        distinct,
        [0, 5, 5000, 10000].into_iter().collect(),
        "a currency distribution, not room or message ids"
    );
}

/// The Marble Chamber candle is NOT a puzzle: it is one of 250 plain
/// command exits, and the walker already speaks those. Guards against
/// over-classifying narrative dressing as mechanism.
#[test]
fn the_crypt_candle_is_an_ordinary_command_exit() {
    let g = graph();
    let chamber = RoomId { map: 1, room: 2252 };
    assert_eq!(
        requirement(g, chamber, Direction::North),
        ExitRequirement::Command
    );
    let i = DIRECTIONS
        .iter()
        .position(|d| *d == Direction::North)
        .unwrap();
    let edge = g.room(chamber).and_then(|r| r.exits[i].as_ref()).unwrap();
    assert_eq!(edge.command.as_deref(), Some("turn right candle left"));
}

/// 17/3042's north passage is concealed by a bit-word that four
/// `remoteaction` scripts clear -- one per gem, in four other rooms. No
/// number of SEARCH rolls can ever reveal it, so the walker must not try.
#[test]
fn the_gem_passage_is_concealed_by_a_puzzle_not_by_search() {
    let g = graph();
    let shadowy = RoomId { map: 17, room: 3042 };
    match requirement(g, shadowy, Direction::North) {
        ExitRequirement::Hidden { searchable } => {
            assert!(!searchable, "no search roll can clear a bit-word")
        }
        other => panic!("expected a hidden exit, got {other:?}"),
    }
}

/// The ordinary hidden exit is still searchable. Without this the fix
/// would be "never search anything", which strands the 1,383 shipped
/// hidden exits that SEARCH genuinely does reveal.
///
/// The bounds are exact, not a range: the shipped world has exactly
/// 1,383 type-6 exits, full stop, and this loader's partition of them is
/// exhaustive by construction (every `Hidden` edge is either `searchable`
/// or not, there is no third bucket) -- so 1368 + 15 must equal all of
/// them. A range that merely says ">1000" and "<50" passes just as
/// happily for a loader that produced 3 or 40 puzzle-locked exits
/// instead of the real 15; only equality can catch that the partition
/// itself is wrong, and the fixture is the shipped game data, which does
/// not drift.
#[test]
fn an_ordinary_hidden_exit_stays_searchable() {
    let g = graph();
    let mut searchable = 0usize;
    let mut puzzle_locked = 0usize;
    for (_, room) in g.iter() {
        for edge in room.exits.iter().flatten() {
            if let ExitRequirement::Hidden { searchable: s } = edge.requirement {
                if s {
                    searchable += 1
                } else {
                    puzzle_locked += 1
                }
            }
        }
    }
    assert_eq!(
        searchable, 1368,
        "the shipped world has exactly 1368 genuinely searchable hidden exits"
    );
    assert_eq!(
        puzzle_locked, 15,
        "the shipped world has exactly 15 remoteaction-concealed hidden exits"
    );
    assert_eq!(
        searchable + puzzle_locked,
        1383,
        "every type-6 exit in the world must land in exactly one bucket"
    );
}

/// 1/1104's warehouse door is an ordinary Door (type 7) that a
/// `remoteaction` script -- a crowbar on its chains -- also opens. The
/// classifier's `_` arm replaces `Door` with `Puzzle` outright on real
/// data; until now nothing exercised that arm at all.
#[test]
fn a_crowbarred_door_becomes_a_puzzle_not_a_door() {
    let g = graph();
    let warehouse = RoomId { map: 1, room: 1104 };
    match requirement(g, warehouse, Direction::North) {
        ExitRequirement::Puzzle { actions } => {
            assert_eq!(actions.len(), 1, "one actor room: itself");
            assert_eq!(actions[0].room, warehouse);
            assert!(
                actions[0]
                    .commands
                    .iter()
                    .any(|c| c == "use crowbar"),
                "expected 'use crowbar' among {:?}",
                actions[0].commands
            );
        }
        other => panic!("expected a puzzle-locked door, got {other:?}"),
    }

    // Pinned the same way as the Hidden partition above: the shipped
    // world has exactly 10 remoteaction targets whose own type was
    // Door/Gate (7 or 0xb) rather than Hidden, and every one of them is
    // now classified Puzzle.
    let puzzle_count = g
        .iter()
        .flat_map(|(_, room)| room.exits.iter().flatten())
        .filter(|edge| matches!(edge.requirement, ExitRequirement::Puzzle { .. }))
        .count();
    assert_eq!(
        puzzle_count, 10,
        "the shipped world has exactly 10 Puzzle-classified (non-Hidden) exits"
    );
}

/// `Toll` and the `Hidden` partition are pinned exactly elsewhere; this
/// pins the six remaining classifications the same way, so a typo in a
/// type number (`0x15` for `0x14`, say) cannot hide silently.
///
/// The expected counts are measurements, not derived by calling
/// `from_exit_type` -- that would only prove the loader agrees with
/// itself. They come from grouping `roomtype_1..10` directly against
/// `re/mmud_wgnt.sqlite`, over exit slots the loader actually keeps
/// (`roomexit_d > 0`, room's own `mapnumber` in 1..=999 and `roomnumber`
/// >= 1), then mapping raw types to variants the same way
/// `from_exit_type` does:
///
/// ```sql
/// WITH exits AS (
///   SELECT roomtype_1 AS rt, roomexit_1 AS rx FROM room
///     WHERE mapnumber BETWEEN 1 AND 999 AND roomnumber >= 1
///   UNION ALL SELECT roomtype_2, roomexit_2 FROM room ... -- through _10
/// )
/// SELECT rt, COUNT(*) FROM exits WHERE rx > 0 GROUP BY rt ORDER BY rt;
/// ```
///
/// which measured (raw type -> populated-slot count): 0=57045, 2=73,
/// 3=194, 4=57, 5=222, 6=1383, 7=1604, 8=296, 9=273, 10=250, 11=164,
/// 12=284, 13=50, 14=2, 15=28, 16=2, 17=1, 19=94, 20=13, 22=290, 23=5,
/// 24=22 -- summing to all 62,352 populated exit slots, confirming
/// nothing was dropped by the grouping.
///
/// Door = types 2, 7, 0xb(11): 73 + 1604 + 164 = 1841 raw. But the
/// Puzzle pass above overwrites 10 of those (7 from type 7, 3 from type
/// 0xb -- confirmed by grouping the loaded graph's Puzzle-classified
/// edges by their retained `exit_type`) with `Puzzle`, so the classified
/// world has 1841 - 10 = 1831 `Door` edges, not the raw count. Trap =
/// types 9, 0x18(24): 273 + 22 = 295, untouched by any remoteaction
/// (they never target a Trap exit). Gate = types 0x14(20), 0x16(22),
/// 0x17(23): 13 + 290 + 5 = 308, likewise untouched. Timed = type
/// 0x10(16): 2. Command = type 10: 250. None = every other raw type (0,
/// 3, 5, 8, 12, 13, 14, 15, 17, 19): 57045 + 194 + 222 + 296 + 284 + 50
/// + 2 + 28 + 1 + 94 = 58216.
#[test]
fn the_remaining_classifications_have_exact_counts() {
    let g = graph();
    let mut door = 0usize;
    let mut trap = 0usize;
    let mut gate = 0usize;
    let mut timed = 0usize;
    let mut command = 0usize;
    let mut none = 0usize;
    for (_, room) in g.iter() {
        for edge in room.exits.iter().flatten() {
            match edge.requirement {
                ExitRequirement::Door => door += 1,
                ExitRequirement::Trap => trap += 1,
                ExitRequirement::Gate => gate += 1,
                ExitRequirement::Timed => timed += 1,
                ExitRequirement::Command => command += 1,
                ExitRequirement::None => none += 1,
                _ => {}
            }
        }
    }
    assert_eq!(door, 1831, "1841 raw type-{{2,7,0xb}} exits minus 10 taken by Puzzle");
    assert_eq!(trap, 295, "273 type-9 + 22 type-0x18");
    assert_eq!(gate, 308, "13 type-0x14 + 290 type-0x16 + 5 type-0x17");
    assert_eq!(timed, 2, "2 type-0x10 exits");
    assert_eq!(command, 250, "250 type-10 command exits");
    assert_eq!(none, 58216, "every unclassified raw type, summed");
}

/// A toll you can pay is a step. A toll you cannot pay is a wall.
#[test]
fn a_toll_costs_a_step_when_affordable_and_is_impassable_otherwise() {
    let toll = ExitRequirement::Toll { gold: 5 };
    let rich = Capabilities {
        purse: Purse::from_farthings(500),
    };
    let broke = Capabilities {
        purse: Purse::from_farthings(499),
    };
    assert_eq!(exit_cost_for(&toll, 4, &rich), Cost::Steps(1));
    assert_eq!(exit_cost_for(&toll, 4, &broke), Cost::Impassable);
}

/// A puzzle-concealed exit is not merely expensive: no amount of walking
/// opens it, so pricing it high would still route through it.
#[test]
fn a_puzzle_concealed_exit_is_impassable_not_expensive() {
    let caps = Capabilities::unrestricted();
    assert_eq!(
        exit_cost_for(&ExitRequirement::Hidden { searchable: false }, 6, &caps),
        Cost::Impassable
    );
    assert_eq!(
        exit_cost_for(&ExitRequirement::Hidden { searchable: true }, 6, &caps),
        Cost::Steps(40),
        "a searchable hidden exit keeps its old price"
    );
}

/// Everything exit_cost priced before must still cost the same, or this
/// change silently reroutes the whole world. The old function is the
/// oracle for the new one on every requirement that does not consult
/// state.
#[test]
fn state_free_requirements_keep_their_old_prices() {
    let caps = Capabilities::unrestricted();
    for exit_type in [0, 2, 7, 0xb, 9, 0x18, 0x10, 0x14, 0x16, 0x17, 10, 0x13, 3, 5] {
        let req = ExitRequirement::from_exit_type(exit_type, 0);
        let want = mud_client::graph::exit_cost(exit_type);
        assert_eq!(
            exit_cost_for(&req, exit_type, &caps),
            Cost::Steps(want),
            "type {exit_type:#x} changed price"
        );
    }
}
