//! `/go` target resolution and config, pure: no socket, no board.
//!
//! The interesting behaviour here is all refusal. Walking somewhere is
//! the navigator's job and is tested against a real board elsewhere;
//! what this file pins is that `/go` never walks a live character
//! somewhere the operator did not unambiguously ask for.

use mud_client::farm::{FarmConfig, Phase};
use mud_client::go::{GoRefusal, go_config, resolve};
use mud_client::graph::{ExitEdge, ExitRequirement, GraphRoom, RoomGraph};
use mud_core::content::{Direction, RoomId};

fn id(room: u16) -> RoomId {
    RoomId { map: 1, room }
}

/// A line of rooms, each linked east to the next, named as given.
fn line(names: &[&str]) -> RoomGraph {
    let rooms: Vec<(RoomId, GraphRoom)> = names
        .iter()
        .enumerate()
        .map(|(i, name)| {
            let n = i as u16 + 1;
            let mut room = GraphRoom {
                name: (*name).into(),
                exits: Default::default(),
                light: 0,
                ..Default::default()
            };
            if (n as usize) < names.len() {
                room.exits[Direction::East as usize] = Some(ExitEdge {
                    dest: id(n + 1),
                    exit_type: 0,
                    command: None,
                    requirement: ExitRequirement::None,
                });
            }
            if n > 1 {
                room.exits[Direction::West as usize] = Some(ExitEdge {
                    dest: id(n - 1),
                    exit_type: 0,
                    command: None,
                    requirement: ExitRequirement::None,
                });
            }
            (id(n), room)
        })
        .collect();
    RoomGraph::from_rooms(rooms)
}

#[test]
fn a_numeric_target_is_a_room_id() {
    let g = line(&["Town Gates", "Town Square"]);
    assert_eq!(resolve(&g, None, "1/2"), Ok(id(2)));
}

/// A number naming no room is an error, not the start of a name search:
/// nobody types a slash in a room name.
#[test]
fn a_numeric_target_not_in_the_graph_says_so() {
    let g = line(&["Town Gates"]);
    assert_eq!(
        resolve(&g, None, "1/9999"),
        Err(GoRefusal::Unknown("1/9999".into()))
    );
}

#[test]
fn name_matching_ignores_case() {
    let g = line(&["Town Gates", "Grungy Shop"]);
    assert_eq!(resolve(&g, None, "gRuNgY sHoP"), Ok(id(2)));
}

/// Exact before substring. Without this rule a name copied straight off
/// the status bar would drown in every longer name containing it.
#[test]
fn an_exact_name_wins_over_a_longer_one_containing_it() {
    let g = line(&["Lower Slum Street", "Slum Street", "Upper Slum Street"]);
    assert_eq!(resolve(&g, None, "slum street"), Ok(id(2)));
}

/// Substring rather than prefix, because MajorMUD names are
/// area-prefixed and the fragment somebody remembers is the tail.
#[test]
fn a_partial_name_falls_back_to_a_substring_search() {
    let g = line(&["Town Gates", "Newhaven, Narrow Road"]);
    assert_eq!(resolve(&g, None, "narrow road"), Ok(id(2)));
}

#[test]
fn an_unknown_name_is_refused_rather_than_walked() {
    let g = line(&["Town Gates"]);
    assert_eq!(
        resolve(&g, None, "Atlantis"),
        Err(GoRefusal::Unknown("Atlantis".into()))
    );
}

#[test]
fn an_ambiguous_name_refuses_and_ranks_by_route_length() {
    // Standing at room 1; the three Slum Streets are 1, 3 and 5 steps east.
    let g = line(&[
        "Town Gates",
        "Slum Street",
        "Alley",
        "Slum Street",
        "Alley",
        "Slum Street",
    ]);
    let Err(GoRefusal::Ambiguous {
        typed,
        nearest,
        total,
    }) = resolve(&g, Some(id(1)), "Slum Street")
    else {
        panic!("three matches must refuse");
    };
    assert_eq!(typed, "Slum Street");
    assert_eq!(total, 3);
    assert_eq!(
        nearest.iter().map(|c| (c.id, c.steps)).collect::<Vec<_>>(),
        vec![(id(2), Some(1)), (id(4), Some(3)), (id(6), Some(5))],
        "nearest first"
    );
}

/// Not knowing where the character stands is no reason to withhold the
/// candidate list — the ids are the whole point of the refusal.
#[test]
fn an_ambiguous_name_without_a_position_still_lists_candidates() {
    let g = line(&["Slum Street", "Alley", "Slum Street"]);
    let Err(GoRefusal::Ambiguous { nearest, total, .. }) = resolve(&g, None, "Slum Street") else {
        panic!("two matches must refuse");
    };
    assert_eq!(total, 2);
    assert_eq!(nearest.len(), 2);
    assert!(nearest.iter().all(|c| c.steps.is_none()));
}

/// The refusal text is the feature: an operator who cannot re-issue the
/// command from what it printed has been told nothing useful.
#[test]
fn the_refusal_names_every_candidate_with_its_id() {
    let g = line(&["Town Gates", "Slum Street", "Alley", "Slum Street"]);
    let Err(refusal) = resolve(&g, Some(id(1)), "Slum Street") else {
        panic!("must refuse");
    };
    let lines = refusal.lines("go");
    assert!(lines[0].contains("2 rooms match"), "{lines:?}");
    assert!(lines.iter().any(|l| l.contains("1/2") && l.contains("1 step")), "{lines:?}");
    assert!(lines.iter().any(|l| l.contains("1/4") && l.contains("3 steps")), "{lines:?}");
    assert!(
        lines.last().unwrap().contains("/go 1/2"),
        "must show how to re-issue: {lines:?}"
    );
}

#[test]
fn an_unknown_target_says_what_it_could_not_find() {
    let refusal = GoRefusal::Unknown("Atlantis".into());
    assert_eq!(refusal.lines("go"), vec!["go: no room matches \"Atlantis\""]);
}

/// The refusal answers the verb that asked. An operator who typed
/// `/recover Slum` and is told to re-issue `/go 1/2` walks the character
/// unsneaked into the room they died in, which is the one move a
/// recovery exists to avoid.
#[test]
fn the_refusal_names_the_verb_that_asked() {
    let g = line(&["Town Gates", "Slum Street", "Alley", "Slum Street"]);
    let Err(refusal) = resolve(&g, Some(id(1)), "Slum Street") else {
        panic!("must refuse");
    };
    let lines = refusal.lines("recover");
    assert_eq!(lines[0], "recover: 2 rooms match \"Slum Street\". Nearest:");
    assert_eq!(
        lines.last().unwrap(),
        "recover: re-issue with the id, e.g. /recover 1/2"
    );
    assert!(
        lines.iter().all(|l| !l.contains("/go") && !l.starts_with("go:")),
        "a recovery must never be told to go: {lines:?}"
    );
    assert_eq!(
        GoRefusal::Unknown("Atlantis".into()).lines("recover"),
        vec!["recover: no room matches \"Atlantis\""]
    );
    assert_eq!(refusal.lines("room")[0], "room: 2 rooms match \"Slum Street\". Nearest:");
    assert_eq!(refusal.lines("map")[0], "map: 2 rooms match \"Slum Street\". Nearest:");
    // The `/go` wording is unchanged, to the byte.
    assert_eq!(
        refusal.lines("go").last().unwrap(),
        "go: re-issue with the id, e.g. /go 1/2"
    );
}

// --- config ---------------------------------------------------------

/// Whether the walk takes fights on the way is the profile's own
/// `fight_while_travelling`, the same key a farm's leg reads. `/bot`
/// off walks past them through the live settings, not through this.
#[test]
fn go_takes_the_profiles_fight_while_travelling() {
    let fights = FarmConfig { fight_while_travelling: true, ..FarmConfig::default() };
    let runs = FarmConfig { fight_while_travelling: false, ..FarmConfig::default() };
    assert!(go_config(&fights).fight_while_travelling);
    assert!(!go_config(&runs).fight_while_travelling);
}

/// The walk rests to the bot's own mark, the same as a farm would.
/// `go_config` always clears a farm's own mark rather than inheriting
/// it. Inheriting a stricter one would have a post-death `/go` send
/// `rest` and sit silently for up to `max_rest_seconds` before its
/// first step, which is exactly when somebody types this.
#[test]
fn go_rests_to_the_bots_mark_not_the_farms() {
    let base = FarmConfig {
        depart_at_percent: Some(80),
        ..FarmConfig::default()
    };
    assert_eq!(go_config(&base).depart_at_percent, None);
}

/// `open` is still tried and still free; this only stops a weaponless
/// character grinding minutes of failed bashes at a locked door.
#[test]
fn go_fails_loudly_at_a_locked_door() {
    let base = FarmConfig::default();
    assert!(base.nav.bash_doors, "the farm bashes, for contrast");
    assert!(!go_config(&base).nav.bash_doors);
}

#[test]
fn go_keeps_the_profiles_other_nav_limits() {
    let base = FarmConfig {
        interrupt_at_percent: 33,
        nav: mud_client::nav::NavConfig {
            step_timeout_ms: 4321,
            ..Default::default()
        },
        ..Default::default()
    };
    let cfg = go_config(&base);
    assert_eq!(cfg.nav.step_timeout_ms, 4321);
    assert_eq!(cfg.interrupt_at_percent, 33);
}

/// A walk ends when it arrives, dies or gives up — never on a lap clock.
#[test]
fn go_runs_without_a_time_budget() {
    let base = FarmConfig {
        max_seconds: 600,
        ..Default::default()
    };
    assert_eq!(go_config(&base).max_seconds, 0);
}

// --- Phase::Done carries where it ended -------------------------------

/// A walk that finished knowing where it stands is the best position
/// evidence there is. Reporting only a sentence threw it away, and the
/// client fell back to whatever the last room block happened to localize
/// to.
#[test]
fn a_finished_walk_reports_the_room_it_reached() {
    let at = RoomId { map: 1, room: 2324 };
    let done = Phase::Done { why: "arrived".into(), at: Some(at) };
    assert_eq!(done.room(), Some(at));
}

#[test]
fn a_walk_that_ended_without_arriving_reports_no_room() {
    let done = Phase::Done { why: "gave up".into(), at: None };
    assert_eq!(done.room(), None);
}

