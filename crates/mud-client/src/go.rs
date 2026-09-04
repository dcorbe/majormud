//! Point-to-point travel on demand: the `/go` command.
//!
//! One sender at a time, exactly as the farm is (see
//! [`crate::farm::run_farm`]): a `/go` owns the connection for its whole
//! walk. It owns it by reusing the farm's own leg — [`crate::farm::travel`]
//! — rather than by growing a second interrupt ladder beside it. That
//! ladder's five branches were each taught by a live failure, and a
//! second one written fresh would omit them, look correct, and
//! rediscover them on a live character.
//!
//! **Walk versus run** is the `/bot` toggle. Its state when `/go` is
//! typed seeds [`crate::farm::FarmConfig::fight_while_travelling`], and
//! every later press moves the session's live switch
//! ([`crate::session::Session::travel_fights`]) under the walk in
//! progress. The two switches read as contradictory until you separate
//! them: that flag decides whether the walk *stops*, while
//! [`crate::bot::BotConfig::auto_combat`] decides what the defence
//! *does once stopped*. Run mode wants the first off and the second on,
//! because the board refuses movement outright while in combat — a walk
//! that would not fight is a walk that stays stuck wherever something
//! picked a fight.

use std::sync::Arc;
use std::time::Instant;

use mud_core::content::RoomId;

use crate::farm::{FarmConfig, FarmError, FarmStats, LegEnd};
use crate::graph::RoomGraph;

/// How many candidates an ambiguous name lists before giving up on
/// being helpful. Five fits a notice without scrolling the board away.
const NEAREST: usize = 5;

/// A room the operator might have meant.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Candidate {
    pub id: RoomId,
    pub name: String,
    /// Steps from where the operator is standing. `None` when that is
    /// unknown, or when the room cannot be reached from there.
    pub steps: Option<usize>,
}

/// Why `/go` will not walk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GoRefusal {
    /// Nothing in the graph answers to this.
    Unknown(String),
    /// Too many rooms do.
    Ambiguous {
        typed: String,
        nearest: Vec<Candidate>,
        total: usize,
    },
}

impl GoRefusal {
    /// One line each, for the caller to print.
    ///
    /// Deliberately not [`std::fmt::Display`]: the caller paints into a
    /// raw-mode terminal and needs the lines separately.
    pub fn lines(&self) -> Vec<String> {
        match self {
            GoRefusal::Unknown(typed) => {
                vec![format!("go: no room matches {typed:?}")]
            }
            GoRefusal::Ambiguous {
                typed,
                nearest,
                total,
            } => {
                let mut out = vec![format!("go: {total} rooms match {typed:?}. Nearest:")];
                for c in nearest {
                    let where_ = match c.steps {
                        Some(0) => "  (you are here)".to_string(),
                        Some(1) => "  (1 step)".to_string(),
                        Some(n) => format!("  ({n} steps)"),
                        None => String::new(),
                    };
                    out.push(format!(
                        "  {}/{}  {}{}",
                        c.id.map, c.id.room, c.name, where_
                    ));
                }
                out.push(format!(
                    "go: re-issue with the id, e.g. /go {}/{}",
                    nearest.first().map(|c| c.id.map).unwrap_or(1),
                    nearest.first().map(|c| c.id.room).unwrap_or(1),
                ));
                out
            }
        }
    }
}

/// Work out which room the operator meant.
///
/// Numeric first: `1/2324` is a room id, and a number that names no room
/// is an error rather than the start of a name search — nobody types a
/// slash in a room name.
///
/// Otherwise a name, matched exactly (ignoring case) before falling back
/// to a substring search. Exact first so that a name copied off the
/// status bar always resolves, rather than drowning in every room whose
/// name contains it. Substring rather than prefix for the fallback,
/// because MajorMUD names are area-prefixed — "Newhaven, Narrow Road",
/// "Dungeon, Old Mineshaft" — so the fragment somebody remembers is the
/// tail, which is exactly what a prefix match cannot find.
pub fn resolve(graph: &RoomGraph, from: Option<RoomId>, typed: &str) -> Result<RoomId, GoRefusal> {
    let typed = typed.trim();
    if let Some(id) = crate::farm::parse_room_id(typed) {
        return match graph.room(id) {
            Some(_) => Ok(id),
            None => Err(GoRefusal::Unknown(typed.to_string())),
        };
    }

    let wanted = typed.to_lowercase();
    let mut hits: Vec<RoomId> = graph
        .iter()
        .filter(|(_, r)| r.name.to_lowercase() == wanted)
        .map(|(id, _)| id)
        .collect();
    if hits.is_empty() {
        hits = graph
            .iter()
            .filter(|(_, r)| r.name.to_lowercase().contains(&wanted))
            .map(|(id, _)| id)
            .collect();
    }

    match hits.len() {
        0 => Err(GoRefusal::Unknown(typed.to_string())),
        1 => Ok(hits[0]),
        total => {
            // One traversal for the whole candidate set, not one per
            // candidate: see `RoomGraph::distances`.
            let steps = from.map(|f| graph.distances(f));
            let mut ranked: Vec<Candidate> = hits
                .into_iter()
                .map(|id| Candidate {
                    id,
                    name: graph.room(id).map(|r| r.name.clone()).unwrap_or_default(),
                    steps: steps.as_ref().and_then(|d| d.get(&id).copied()),
                })
                .collect();
            // Unreachable and unknown-distance candidates sort last, but
            // are still listed: "I cannot get there from here" is worth
            // seeing when the alternative is an empty list.
            ranked.sort_by_key(|c| (c.steps.is_none(), c.steps.unwrap_or(0), c.id));
            ranked.truncate(NEAREST);
            Err(GoRefusal::Ambiguous {
                typed: typed.to_string(),
                nearest: ranked,
                total,
            })
        }
    }
}

/// The config a `/go` runs under, from the profile's `[farm]` table (or
/// its defaults when the profile has no farm at all).
///
/// `walking` is the `/bot` toggle as it stood when `/go` was typed: on
/// means take the fights on the way, off means walk past them. Later
/// presses reach the walk through the session's switch, not this.
///
/// Three fields deliberately diverge from what a farm would use. Each is
/// the difference between a command that answers a keystroke and one
/// that appears to have hung.
pub fn go_config(base: &FarmConfig, walking: bool) -> FarmConfig {
    FarmConfig {
        fight_while_travelling: walking,
        // The walk rests to the bot's mark like a farm does. A profile
        // that wants the old instant start sets `rest_until_percent`
        // to 0.
        depart_at_percent: None,
        // `open` is tried first and costs nothing; `picklock` follows and
        // costs a command and no health, so nothing here needs to force
        // it off for an interactive walk. Whether it is ever attempted
        // at all is not a config knob any more: `Navigator` reads it
        // straight off the character's own Picklocks
        // (`crate::graph::Capabilities::picklocks`).
        //
        // BASHING is the one that is forced off. It is minutes of
        // silence — up to 60 failed rolls plus four times that in
        // cooldown scolds — it charges HP per swing, and a freshly-dead
        // character has no weapon to do it with. A locked door that
        // resists picking now stops with `DoorLocked`, which names the
        // door and the direction, so the operator can decide to `bash`
        // by hand. That is the "fail loudly" this always meant: it used
        // to report a TIMEOUT for an instant, known refusal, which read
        // as though the client had hung (live, beef.raw 2026-08-22).
        nav: crate::nav::NavConfig {
            bash_doors: false,
            ..base.nav.clone()
        },
        // No lap budget: a walk ends when it arrives, dies, or gives up.
        max_seconds: 0,
        ..base.clone()
    }
}

/// How a walk ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GoEnd {
    Arrived(RoomId),
    /// Gave up short of the target, and where it stands now. Reporting
    /// the room matters: a bare "stopped" strands the operator worse
    /// than never having tried.
    Stopped(RoomId),
    Died,
}

/// Walk to `to` from wherever the character is currently standing.
///
/// `hint` is the caller's best guess at the current room — the TUI keeps
/// one. It is passed to [`crate::nav::Navigator::localize_view`] as a
/// hint rather than being trusted: the neighbour shortcut it enables
/// resolves same-named twins that the global search cannot, and a wrong
/// hint costs only the global search that would otherwise have run.
#[allow(clippy::too_many_arguments)]
pub async fn run_go(
    session: &crate::session::Session,
    graph: Arc<RoomGraph>,
    hint: Option<RoomId>,
    to: RoomId,
    bot_config: &crate::bot::BotConfig,
    cfg: &FarmConfig,
    phase: crate::farm::PhaseSink<'_>,
) -> Result<GoEnd, FarmError> {
    crate::farm::check_departure_mark(cfg, bot_config)?;
    // How the walk starts; `/bot` moves the switch from here on. See
    // `run_farm`.
    session.travel_fights().set(cfg.fight_while_travelling);
    // The board's own per-monster death wordings, so the room model can
    // see a kill somebody else landed. Best effort, as in `run_farm`.
    if let Err(e) = crate::deaths::init(&cfg.content) {
        eprintln!("death wordings unavailable ({e}); shared-room kills will be missed");
    }
    let nav = crate::nav::Navigator::new(graph.clone(), cfg.nav.clone())
        .with_capabilities(session.capabilities());
    // Item identity for the backstab opener -- best effort, same
    // "reload the path again" pattern as the threat/duration tables
    // `run_farm` already loads. `session.wielded()`/`.contents()` are
    // themselves best-effort (whatever this session has read so far),
    // exactly as `session.capabilities()`'s purse already is above.
    let nav = match RoomGraph::load_content(&cfg.content) {
        Ok(content) => {
            nav.with_backstab(Arc::new(content), session.wielded(), session.contents().items)
        }
        Err(e) => {
            eprintln!("item identity unavailable ({e}); backstab opener disabled");
            nav
        }
    };

    let seen = crate::farm::look_around(session, "the go walk's look").await?;
    // An impossible id when there is no hint, so the neighbour shortcut
    // necessarily misses and the global search runs.
    let hint = hint.unwrap_or(RoomId { map: 0, room: 0 });
    let from = crate::lost::place(session, &graph, &nav, hint, &seen)
        .await
        .map_err(FarmError::Lost)?
        .at;
    if from == to {
        return Ok(GoEnd::Arrived(to));
    }

    // Every percent policy divides by these, and a wrong value mis-scales
    // the travel guard silently. 0 max_hp means the profile did not say,
    // so ask, and the same answer carries max_mana.
    let mut bot_config = bot_config.clone();
    if bot_config.max_hp == 0
        && let Some(vitals) = crate::farm::discover_vitals(session).await
    {
        bot_config.max_hp = vitals.max_hp;
        bot_config.max_mana = vitals.max_mana;
    }
    let threat = Arc::new(
        RoomGraph::load_threat(&cfg.content).unwrap_or_else(|_| crate::bot::ThreatTable::new()),
    );
    let refusals = crate::bot::Refusals::default();
    // `sheet_from` reads the session's own cached inventory/spellbook
    // (read once, at realm entry — see `crate::tui::on_realm_entry`) and
    // costs nothing on the wire, so there is no reason left to skip it
    // on legs that never pass through the dark; the three-second spell
    // collection this used to pay per walk is gone.
    //
    // Healing is deliberately empty here whatever the book says: `/go`
    // is a walk the operator asked for, and `go_config` already zeroes
    // the departure gate so it never rests either. Recovery on a walk
    // belongs to the person who typed it.
    let sheet = crate::farm::sheet_from(session, &bot_config, &Default::default());
    let mut casts = crate::farm::Casts {
        light: crate::sheet::LightState::new(sheet.light),
        heal: crate::sheet::HealState::new(Vec::new()),
        buff: crate::sheet::BuffState::new(Vec::new()),
    };
    let mut clock = crate::world::RoundClock::new();
    let mut stats = FarmStats::default();
    let mut current = from;

    let leg = crate::farm::travel(
        session,
        &nav,
        &graph,
        &mut current,
        to,
        cfg,
        &bot_config,
        &threat,
        &refusals,
        &mut casts,
        &mut clock,
        Instant::now(),
        &mut stats,
        phase,
    )
    .await?;

    let end = match leg {
        LegEnd::Arrived { .. } => GoEnd::Arrived(current),
        LegEnd::Died => GoEnd::Died,
        // `TimeUp` cannot arise from a `go_config`, which sets
        // `max_seconds = 0`; a caller passing its own config could still
        // reach it, and "stopped where it stands" is the honest reading.
        LegEnd::TooHurt | LegEnd::TimeUp => GoEnd::Stopped(current),
    };
    // A lit source burns a use per tick whether anything needs the light
    // or not. A dead character cannot put it out.
    if !matches!(end, GoEnd::Died)
        && let Some(cmd) = casts.light.extinguish()
    {
        session.send(&cmd);
    }
    Ok(end)
}
