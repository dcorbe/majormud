//! `/go` target resolution and config, pure: no socket, no board.
//!
//! The interesting behaviour here is all refusal. Walking somewhere is
//! the navigator's job and is tested against a real board elsewhere;
//! what this file pins is that `/go` never walks a live character
//! somewhere the operator did not unambiguously ask for.

use mud_client::farm::FarmConfig;
use mud_client::go::{GoRefusal, go_config, resolve};
use mud_client::graph::{ExitEdge, GraphRoom, RoomGraph};
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
            };
            if (n as usize) < names.len() {
                room.exits[Direction::East as usize] = Some(ExitEdge {
                    dest: id(n + 1),
                    exit_type: 0,
                });
            }
            if n > 1 {
                room.exits[Direction::West as usize] = Some(ExitEdge {
                    dest: id(n - 1),
                    exit_type: 0,
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
    let lines = refusal.lines();
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
    assert_eq!(refusal.lines(), vec!["go: no room matches \"Atlantis\""]);
}

// --- config ---------------------------------------------------------

#[test]
fn walk_mode_fights_on_the_way_and_run_mode_does_not() {
    let base = FarmConfig::default();
    assert!(go_config(&base, true).fight_while_travelling);
    assert!(!go_config(&base, false).fight_while_travelling);
}

/// Inheriting the farm's 80% departure gate would have a post-death
/// `/go` send `rest` and sit silently for up to `max_rest_seconds`
/// before its first step — which is exactly when somebody types this.
#[test]
fn go_never_waits_at_the_departure_gate() {
    let base = FarmConfig::default();
    assert_eq!(base.depart_at_percent, 80, "the farm's gate, for contrast");
    assert_eq!(go_config(&base, true).depart_at_percent, 0);
}

/// `open` is still tried and still free; this only stops a weaponless
/// character grinding minutes of failed bashes at a locked door.
#[test]
fn go_fails_loudly_at_a_locked_door() {
    let base = FarmConfig::default();
    assert!(base.nav.bash_doors, "the farm bashes, for contrast");
    assert!(!go_config(&base, true).nav.bash_doors);
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
    let cfg = go_config(&base, true);
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
    assert_eq!(go_config(&base, true).max_seconds, 0);
}

// --- against the shipped world ---------------------------------------

/// Anchors the ambiguity path to reality rather than to a fixture: the
/// slums really are 150-odd rooms sharing one name, which is why `/go`
/// refuses instead of guessing.
#[test]
fn slum_street_is_ambiguous_in_the_shipped_world() {
    let db = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../re/mmud_wgnt.sqlite");
    if !db.exists() {
        eprintln!("skipping: {} not present", db.display());
        return;
    }
    let g = RoomGraph::load(&db).expect("load room graph");
    // The Grungy Shop is unique, so it resolves by name alone.
    assert_eq!(resolve(&g, None, "Grungy Shop"), Ok(id(2324)));
    // Its street is not.
    let Err(GoRefusal::Ambiguous { total, nearest, .. }) =
        resolve(&g, Some(id(1072)), "Slum Street")
    else {
        panic!("Slum Street must be ambiguous");
    };
    assert!(total > 100, "expected the whole slum maze, got {total}");
    assert_eq!(nearest.len(), 5, "listing is capped");
    let steps: Vec<_> = nearest.iter().map(|c| c.steps).collect();
    assert!(
        steps.windows(2).all(|w| w[0] <= w[1]),
        "nearest first: {steps:?}"
    );
}
