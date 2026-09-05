//! Working out where a lost character is, by walking until the answer is
//! forced.
//!
//! [`crate::nav::Navigator::localize_view`] answers from one room block,
//! and that is as far as a single look can go: 26,720 rooms carry 1,754
//! distinct names, and only 5,002 of them — **18.7%** — are unique by
//! name and the exits the board lists. Log back in inside a generic room
//! and there is nothing to route from. Live, 2026-08-02: a farm run
//! resumed in a "Dark Tunnel" and ended with `lost: "Dark Tunnel" is not
//! the stop or any neighbor of it`.
//!
//! **Not solved by hunting for a unique room.** The rooms most likely to
//! strand a character are the ones with the fewest unique neighbours: the
//! Library is 212 identical rooms, the Negative Power Plane 138, Crystal
//! Lake 433. A search for something recognisable would walk a very long
//! way and might never arrive.
//!
//! What works is carrying the whole CANDIDATE SET through the moves. Look
//! once and the graph names every room that could be this one; step, look
//! again, and keep only the candidates whose neighbour in that direction
//! matches what the board just showed. Each step multiplies the evidence
//! instead of restarting the search, and the direction is chosen to split
//! the surviving set as far as it can be split.
//!
//! Simulated over every room in the world (`re/mmud_wgnt.sqlite`, exits
//! as the board lists them):
//!
//! | steps | resolved |
//! |---|---|
//! | 0 | 18.7% |
//! | 1 | 52.0% |
//! | 2 | 71.4% |
//! | 3 | 82.4% |
//! | 12 | 93.3% |
//!
//! The remaining 6.7% are true mazes — rooms no walk can tell apart by
//! name and exits — so the budget stops and reports rather than
//! wandering. That is [`Lost::Maze`], and it is a real answer: it says
//! how many rooms the character is standing in one of.
//!
//! **What this does not do: fight.** The walk sends bare directions and
//! reads the reply. A step refused because something is swinging comes
//! back as [`Lost::NoAnswer`] and the caller — which is holding a defend
//! pump this module is not — deals with it and asks again.

use std::collections::BTreeSet;

use mud_core::content::{Direction, RoomId};

use crate::events::RoomView;
use crate::graph::RoomGraph;
use crate::nav::{Navigator, direction_of, dir_word};
use crate::session::Session;

/// Steps the walk may take before it calls the room a maze.
///
/// Twelve resolves 93.3% of the world and the curve is flat past it —
/// steps 8 through 12 buy 1.5% between them, and everything still
/// standing at that point is a room genuinely indistinguishable from its
/// neighbours. Walking further would be walking for its own sake, and
/// every step is a room the character did not choose to be in.
pub const BUDGET: usize = 12;

/// How long to wait for the room block a step owes.
const STEP_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(15);

/// Where a character turned out to be, and what it cost to find out.
///
/// The step count is not decoration: it is how far the character was
/// MOVED to answer the question, which the operator who asked is entitled
/// to know and a caller may want to undo.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Placed {
    pub at: RoomId,
    /// Steps walked. Zero means the look alone settled it.
    pub steps: usize,
}

/// Why a character could not be placed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Lost {
    /// No room in the graph matches the block at all. Walking cannot fix
    /// this — the world the client loaded is not the world the character
    /// is standing in — so it is refused before a single command goes
    /// out.
    Unknown { saw: String },
    /// Several rooms still match after the budget, and they are
    /// indistinguishable by name and exits. Carries the count, because
    /// "one of 212" and "one of 2" are different situations.
    Maze { saw: String, candidates: usize },
    /// A step produced no room block inside the deadline: the board
    /// refused the move (combat), the room is unlit and nothing lit it,
    /// or the character is dead. All three are the caller's to handle.
    NoAnswer { saw: String, sent: &'static str },
}

impl std::fmt::Display for Lost {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Lost::Unknown { saw } => write!(f, "no room in the graph looks like {saw:?}"),
            Lost::Maze { saw, candidates } => {
                write!(f, "{saw:?} is one of {candidates} rooms that walking cannot tell apart")
            }
            Lost::NoAnswer { saw, sent } => {
                write!(f, "no room block answered {sent:?} while placing {saw:?}")
            }
        }
    }
}

impl std::error::Error for Lost {}

/// Every room the graph says this block could describe.
///
/// Two readings of the exits line, tried in that order:
///
/// 1. **Exactly**, against the exits the board WOULD list — every exit
///    the room has except the hidden (type 6) and command (type 10) ones,
///    which are never printed. This is the discriminating test: it is
///    what makes 18.7% of the world unique on sight rather than the 13.4%
///    a looser rule manages, and it carries all the way through the walk
///    (93.3% resolved within twelve steps against 81.1%).
/// 2. **As a subset**, if the exact reading admits nothing. Then our
///    model of what the board prints is incomplete, and the honest
///    response is to widen rather than to answer "nowhere" about a
///    character that is plainly somewhere.
///
/// Deterministic order (rooms are held sorted), so a caller that prints
/// the candidates prints the same list twice.
pub fn candidates(graph: &RoomGraph, seen: &RoomView) -> Vec<RoomId> {
    consistent(graph, graph.iter().map(|(id, _)| id), seen)
}

/// [`candidates`], restricted to a pool — the destinations that survived
/// the last step.
fn consistent(
    graph: &RoomGraph,
    pool: impl Iterator<Item = RoomId>,
    seen: &RoomView,
) -> Vec<RoomId> {
    let observed: BTreeSet<Direction> = seen.exits.iter().filter_map(|e| direction_of(e)).collect();
    let named: Vec<RoomId> = pool
        .filter(|id| graph.room(*id).is_some_and(|r| r.name == seen.name))
        .collect();
    let exact: Vec<RoomId> = named
        .iter()
        .copied()
        .filter(|id| graph.room(*id).is_some_and(|r| listed(r) == observed))
        .collect();
    if !exact.is_empty() {
        return exact;
    }
    named
        .into_iter()
        .filter(|id| {
            graph
                .room(*id)
                .is_some_and(|r| observed.iter().all(|d| r.exits[*d as usize].is_some()))
        })
        .collect()
}

/// The directions this room's "Obvious exits" line names.
///
/// Everything the room has except hidden (6) and command (10) exits: the
/// board prints neither, which is the whole reason the alleyway at 1/405
/// looks like a dead end and the Newhaven ferry looks like a dock.
fn listed(room: &crate::graph::GraphRoom) -> BTreeSet<Direction> {
    crate::graph::DIRECTIONS
        .iter()
        .enumerate()
        .filter(|(i, _)| {
            room.exits[*i]
                .as_ref()
                .is_some_and(|e| {
                    !crate::nav::is_hidden(e.exit_type)
                        && !crate::nav::is_remote_action(e.exit_type)
                        && e.command.is_none()
                })
        })
        .map(|(_, d)| *d)
        .collect()
}

/// Walk until the graph can name the room the character is standing in,
/// and return that room — where the walk ENDED, not where it began.
///
/// Reporting the end is the point. The character has moved, and a caller
/// handed the starting room would route from somewhere nobody is; every
/// caller here re-routes from wherever it is told, so the walk does not
/// retrace its steps to pretend it never happened.
///
/// Sends nothing at all when the first look already settles it.
pub async fn relocalize(
    session: &Session,
    graph: &RoomGraph,
    seen: &RoomView,
    budget: usize,
) -> Result<Placed, Lost> {
    let mut here = candidates(graph, seen);
    let mut saw = seen.clone();
    let mut events = session.events();
    let mut steps = 0usize;
    for _ in 0..budget {
        match here.as_slice() {
            [] => return Err(Lost::Unknown { saw: saw.name }),
            [only] => return Ok(Placed { at: *only, steps }),
            _ => {}
        }
        let Some(dir) = splitting_step(graph, &here, &saw) else {
            break;
        };
        let word = dir_word(dir);
        let sent = session.send(word);
        let Some(block) = crate::farm::next_room_view(&mut events, sent, STEP_TIMEOUT).await
        else {
            return Err(Lost::NoAnswer { saw: saw.name, sent: word });
        };
        saw = block;
        let moved: BTreeSet<RoomId> = here
            .iter()
            .filter_map(|c| graph.room(*c)?.exits[dir as usize].as_ref().map(|e| e.dest))
            .collect();
        here = consistent(graph, moved.into_iter(), &saw);
        steps += 1;
    }
    match here.as_slice() {
        [] => Err(Lost::Unknown { saw: saw.name }),
        [only] => Ok(Placed { at: *only, steps }),
        rest => Err(Lost::Maze {
            saw: saw.name,
            candidates: rest.len(),
        }),
    }
}

/// The listed exit that splits the candidate set furthest.
///
/// Every candidate must have the exit and the graph must know where it
/// goes — otherwise taking it is a guess about which candidate we are,
/// which is the question being asked. Among those, the best is whichever
/// leads to the most DIFFERENT-looking rooms, since only a difference the
/// board can show is evidence. Ties go to the first direction in compass
/// order, so a walk is reproducible.
///
/// A direction whose destinations all look alike is still worth taking,
/// and this is measured rather than assumed. Refusing to move unless the
/// step splits the set immediately looks tidier — it wanders less when
/// the case is hopeless — but a step that teaches nothing today still
/// carries the whole set forward to a room where the neighbours differ.
/// Over the shipped world: patient resolves **93.3%**, impatient
/// **87.7%**. That is 1,490 rooms where the difference is "found it"
/// against "gave up", bought with about eight extra steps in the mazes
/// that were never going to resolve anyway — and in a maze those steps
/// are all through rooms nobody can tell apart.
///
/// `None` when no listed exit is usable at all, which ends the walk:
/// there is nothing left to learn by moving.
fn splitting_step(graph: &RoomGraph, here: &[RoomId], saw: &RoomView) -> Option<Direction> {
    let observed: Vec<Direction> = saw.exits.iter().filter_map(|e| direction_of(e)).collect();
    let mut best: Option<(usize, Direction)> = None;
    for d in observed {
        let mut shapes: BTreeSet<(String, BTreeSet<Direction>)> = BTreeSet::new();
        let mut usable = true;
        for c in here {
            let dest = graph
                .room(*c)
                .and_then(|r| r.exits[d as usize].as_ref())
                .map(|e| e.dest)
                .and_then(|id| graph.room(id));
            let Some(dest) = dest else {
                usable = false;
                break;
            };
            shapes.insert((dest.name.clone(), listed(dest)));
        }
        if usable && best.is_none_or(|(n, _)| shapes.len() > n) {
            best = Some((shapes.len(), d));
        }
    }
    best.map(|(_, d)| d)
}

/// Place a character from one room block, walking only if it has to.
///
/// The whole answer to "where am I", in the order that costs least:
///
/// 1. [`Navigator::localize_view`] — free, and right for the 18.7% of
///    rooms that are unique on sight plus every room reachable in one hop
///    from `hint`, which is the ordinary case for a walk that merely
///    stumbled.
/// 2. [`relocalize`] — walks, for the rest.
///
/// `hint` is where the caller thought the character was. A wrong hint
/// costs nothing but the search that would have run anyway; callers with
/// no idea at all pass an impossible id so the one-hop shortcut misses.
pub async fn place(
    session: &Session,
    graph: &RoomGraph,
    nav: &Navigator,
    hint: RoomId,
    seen: &RoomView,
) -> Result<Placed, Lost> {
    if let Some(at) = nav.localize_view(hint, seen) {
        return Ok(Placed { at, steps: 0 });
    }
    relocalize(session, graph, seen, BUDGET).await
}

/// What the client believes about where the character is standing, and
/// how much that belief is worth.
///
/// `Option<RoomId>` could not tell "a room block resolved to this id"
/// apart from "no block has resolved since, so this is the last id that
/// did". Both were handed to [`place`] as hints of equal standing, and
/// its name-only one-hop shortcut then CONFIRMED the stale one — a wrong
/// origin that routes an entire walk from a room the character was never
/// in. Making the difference a type means every consumer has to say
/// which of the two it can live with.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Fix {
    /// Nothing has resolved yet: before the first block after login, or
    /// after a demotion chain with no history behind it.
    #[default]
    Unknown,
    /// A room block resolved to this id. Safe as a routing origin and as
    /// a trusted hint.
    Confirmed(RoomId),
    /// The last id that DID resolve, kept only to say where the
    /// character last was. Never a routing origin, never a hint.
    Stale(RoomId),
}

impl Fix {
    /// The id a caller may route from, or hand to [`place`] as a trusted
    /// hint. `None` unless the belief is current.
    pub fn confirmed(self) -> Option<RoomId> {
        match self {
            Fix::Confirmed(at) => Some(at),
            _ => None,
        }
    }

    /// The best id to SHOW, current or not. Display only — never feed
    /// this to a router or a localizer.
    pub fn last_known(self) -> Option<RoomId> {
        match self {
            Fix::Confirmed(at) | Fix::Stale(at) => Some(at),
            Fix::Unknown => None,
        }
    }

    /// Demote on an unresolvable block: the character is somewhere the
    /// graph could not name, so whatever was believed is now only
    /// history. Idempotent — a stale fix does not decay further, and an
    /// unknown one cannot invent a room to be stale about.
    pub fn demote(self) -> Fix {
        match self {
            Fix::Confirmed(at) | Fix::Stale(at) => Fix::Stale(at),
            Fix::Unknown => Fix::Unknown,
        }
    }
}

/// Fold one room block into a positional fix.
///
/// THE place a block becomes a position, so the client and the map
/// cannot disagree about what a block meant. Callers must have already
/// dropped blocks that describe somewhere else (`Correlated::elsewhere`).
pub fn refix(nav: &Navigator, fix: Fix, seen: &RoomView) -> Fix {
    // Only a CONFIRMED fix may seed the one-hop shortcut. Seeding it with
    // a stale id is exactly how a stale id used to confirm itself; the
    // impossible id forces the global search instead.
    let hint = fix.confirmed().unwrap_or(RoomId { map: 0, room: 0 });
    match nav.localize_view(hint, seen) {
        Some(at) => Fix::Confirmed(at),
        None => fix.demote(),
    }
}
