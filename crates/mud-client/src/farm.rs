//! Autonomous farming: the `[farm]` configuration and the validated
//! patrol plan built from it.
//!
//! A patrol circuit is a list of rooms the character walks between,
//! farming each with the [`crate::bot`] policy core. Every room id and
//! every leg between them is checked against the room graph up front —
//! a typo in `[farm].circuit` must fail before the client connects, not
//! halfway around the lap with a live character standing in a spawn.
//!
//! `[farm].start` is where the character *stands at login*, not where the
//! farming happens. The runner verifies it by room name and then walks
//! the first leg onto the circuit itself, so the two are often different
//! rooms — a town room to log in at, a lair to farm.
//!
//! `[farm].finish_at` is where to leave the character when the run stops.
//! Without it a run ends wherever it happened to be, which for a lair
//! circuit means standing among the monsters with nobody driving. See
//! [`go_to_finish`], which the caller runs on every exit route rather
//! than [`run_farm`] running it internally — Ctrl-C cancels `run_farm`,
//! and that is the commonest way a farm ends.
//!
//! **There is no dry run.** Building the plan validates the configuration,
//! but `mmc farm` connects and starts farming as soon as the start room
//! checks out; there is no way to ask it only to check the config.

use std::collections::VecDeque;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use mud_core::content::RoomId;

use crate::events::Event;
use crate::graph::RoomGraph;

/// How long to wait for the prompt that acknowledges a sent command
/// before assuming the board swallowed it and moving on. Waiting forever
/// would wedge the runner on any line the board answers silently.
pub const ACK_TIMEOUT: Duration = Duration::from_secs(5);

/// `"1/860"` -> map 1, room 860. The `mmc path` argument syntax.
pub fn parse_room_id(s: &str) -> Option<RoomId> {
    let (map, room) = s.split_once('/')?;
    Some(RoomId {
        map: map.parse().ok()?,
        room: room.parse().ok()?,
    })
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct FarmConfig {
    /// Room graph database (the decoded WG3-NT dump).
    pub content: PathBuf,
    /// Where the character is standing at login, as `map/room`. Verified
    /// by room name before the first step; the runner refuses to walk
    /// from a position it cannot confirm.
    pub start: String,
    /// Rooms to farm, in patrol order. The lap wraps from the last back
    /// to the first.
    pub circuit: Vec<String>,
    /// Where to leave the character when the run ends, as `map/room`.
    ///
    /// Without this the runner stops wherever it happens to be — which
    /// for a lair circuit means standing among the monsters, linkdead,
    /// until somebody walks it out. Somewhere safe (a town room) is the
    /// point. Absent means stay put, which is the old behaviour.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub finish_at: Option<String>,
    /// Laps to walk; 0 = until stopped.
    pub loops: u32,
    /// Wall-clock cap; 0 = unlimited.
    pub max_seconds: u64,
    /// Leave a stop after this many consecutive prompts with nothing to
    /// fight and nothing to do.
    pub dwell_idle_prompts: u32,
    /// Never start a leg below this hp% (needs a known max HP); 0
    /// disables the gate. This is the first of the two travel defences:
    /// it keeps a wounded character from setting off at all, while
    /// `interrupt_at_percent` stops one that gets hurt on the way.
    pub depart_at_percent: u32,
    /// How long to hold off sending after the board says it dropped our
    /// input ("Why don't you slow down for a few seconds?").
    pub slowdown_backoff_ms: u64,
    /// How long a stop may go quiet before the runner pokes it with a
    /// `look`.
    ///
    /// An idle board sends **nothing** — not a prompt, not a tick, for
    /// minutes at a stretch (`tests/board_cadence.rs` measures silences
    /// past an hour, one of them while the character sat wounded at
    /// 15/51). So the poke is not a fallback for a quiet test server: it
    /// is the only thing that produces prompts at an empty stop on any
    /// board, and a dwell rule that waited for them unaided would wait
    /// forever everywhere. The poke doubles as a respawn check.
    ///
    /// Keep it slow. Flood control is measured at eight sends 1.3s
    /// apart, barely under the MbbsEmu pacer's 1500ms floor, and the
    /// poke is pure overhead on top of everything else the runner sends.
    pub idle_poke_ms: u64,
    /// Prompts to wait for a heal to show progress before concluding it
    /// never landed and re-arming the policy.
    pub heal_retry_prompts: u32,
    /// Lines that mean the heal was refused outright, matched as
    /// substrings. Empty by default: no refusal wording has been
    /// captured off the board yet, and inventing one would be a fixture
    /// that is tidier than reality.
    pub heal_refused: Vec<String>,
    /// Stop walking a leg when hp drops below this percent, defend where
    /// the character stands, and resume once it is fit to travel again.
    /// 0 disables the hp trip; dying still stops the walk.
    ///
    /// Must not exceed `depart_at_percent`, and [`FarmPlan::build`]
    /// refuses the pair when it does: the defend pump would end, the
    /// departure gate would release at the lower number, and the very
    /// next prompt would trip the guard again — a run that burns its
    /// whole interrupt budget without walking a step.
    pub interrupt_at_percent: u32,
    /// Interruptions tolerated on a single leg before the run gives up.
    /// A character that keeps being stopped is not going to walk this
    /// leg, and walking it anyway is how a run ends in a corpse.
    pub travel_interrupts: u32,
    /// Cap on defending one interruption, in seconds.
    ///
    /// Not optional polish: a stop only ends on its dwell rule or the
    /// run's own max_seconds, which defaults to unlimited — and a heal
    /// that never lands re-arms the policy on every retry, resetting the
    /// dwell counter forever. Defending is entered *because* hp is low,
    /// which is exactly when that happens, so a protected travel without
    /// this could wedge a live character in a corridor. That would be
    /// strictly worse than the unprotected travel it replaces.
    pub defend_seconds: u64,
    /// Navigation limits, as `[farm.nav]`. The runner is the only thing
    /// in the client that builds a [`crate::nav::Navigator`] — `mmc path`
    /// asks the graph directly and `mmc play` never navigates — so the
    /// limits live under the table that owns them rather than at the top
    /// level, where they would read as global and honoured by nothing.
    pub nav: crate::nav::NavConfig,
}

impl Default for FarmConfig {
    fn default() -> Self {
        FarmConfig {
            content: PathBuf::from("re/mmud_wgnt.sqlite"),
            start: String::new(),
            circuit: Vec::new(),
            finish_at: None,
            loops: 0,
            max_seconds: 0,
            dwell_idle_prompts: 3,
            depart_at_percent: 80,
            slowdown_backoff_ms: 5000,
            // 5s x the 3-prompt dwell leaves an empty stop after about
            // fifteen seconds, while keeping idle traffic well clear of
            // the flood-control rate.
            idle_poke_ms: 5000,
            heal_retry_prompts: 3,
            heal_refused: Vec::new(),
            // Below the 80% departure gate, and at the point the bot
            // policy would itself want to stop and heal.
            interrupt_at_percent: 50,
            travel_interrupts: 3,
            defend_seconds: 60,
            nav: crate::nav::NavConfig::default(),
        }
    }
}

/// A circuit whose rooms all exist and whose every leg is walkable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FarmPlan {
    pub start: RoomId,
    pub circuit: Vec<RoomId>,
    /// Validated `finish_at`. Checked at build time with everything else,
    /// because finding out the way home is unwalkable at the moment the
    /// run ends is exactly too late to do anything about it.
    pub finish: Option<RoomId>,
}

impl FarmPlan {
    pub fn build(cfg: &FarmConfig, graph: &RoomGraph) -> Result<FarmPlan, String> {
        if cfg.circuit.is_empty() {
            return Err("circuit is empty; [farm].circuit needs at least one room".into());
        }
        if cfg.depart_at_percent != 0 && cfg.interrupt_at_percent > cfg.depart_at_percent {
            return Err(format!(
                "interrupt_at_percent ({}) is above depart_at_percent ({}): \
                 the patrol would set off at {}% and be interrupted immediately, \
                 burning its interrupt budget without walking a step",
                cfg.interrupt_at_percent, cfg.depart_at_percent, cfg.depart_at_percent
            ));
        }
        let resolve = |s: &String| -> Result<RoomId, String> {
            let id = parse_room_id(s)
                .ok_or_else(|| format!("bad room id {s:?}; want map/room, e.g. 1/1"))?;
            if graph.room(id).is_none() {
                return Err(format!("room {}/{} is not in the graph", id.map, id.room));
            }
            Ok(id)
        };

        let start = resolve(&cfg.start)?;
        let circuit = cfg
            .circuit
            .iter()
            .map(resolve)
            .collect::<Result<Vec<_>, _>>()?;

        // Every leg the runner will ever walk: onto the circuit, around
        // it, and the wrap that starts the next lap.
        let mut legs = vec![(start, circuit[0])];
        for pair in circuit.windows(2) {
            legs.push((pair[0], pair[1]));
        }
        legs.push((circuit[circuit.len() - 1], circuit[0]));
        for (from, to) in legs {
            if graph.route(from, to).is_none() {
                return Err(format!(
                    "no route from {}/{} to {}/{}",
                    from.map, from.room, to.map, to.room
                ));
            }
        }

        // The way home is validated from every stop the run can end at,
        // not just from the circuit's start: a run stops wherever it
        // stopped, and a finish room reachable from only one of them
        // would strand the character from all the others.
        let finish = cfg.finish_at.as_ref().map(&resolve).transpose()?;
        if let Some(finish) = finish {
            for &from in std::iter::once(&start).chain(circuit.iter()) {
                if graph.route(from, finish).is_none() {
                    return Err(format!(
                        "no route home from {}/{} to finish_at {}/{}",
                        from.map, from.room, finish.map, finish.room
                    ));
                }
            }
        }

        Ok(FarmPlan {
            start,
            circuit,
            finish,
        })
    }
}

/// The runner's only outbound path.
///
/// [`crate::session::Session::send`] is an unbounded, unacknowledged
/// queue, and the live board paces at 1500ms, so firing every bot
/// decision straight at it buries the character under minutes of stale
/// commands with no way to cancel. The gate keeps exactly one command in
/// flight until the board's prompt acknowledges it.
///
/// It is also the only thing that acts on [`Event::SlowDown`], which
/// means the board *dropped* our input. The dropped command goes back to
/// the front of the queue and sending pauses until the board calms down.
/// The bot's own latches stay truthful through all of this: it decided to
/// swing once, and the swing does eventually happen, so nothing has to
/// re-arm.
pub struct Gate {
    queue: VecDeque<String>,
    /// The command awaiting its prompt, and when it went out.
    in_flight: Option<(String, Instant)>,
    /// Nothing may be sent before this instant (flood control).
    blocked_until: Option<Instant>,
    backoff: Duration,
}

impl Gate {
    pub fn new(backoff: Duration) -> Self {
        Gate {
            queue: VecDeque::new(),
            in_flight: None,
            blocked_until: None,
            backoff,
        }
    }

    /// Queue a command. Order is preserved; a resend jumps ahead of it.
    pub fn push(&mut self, line: String) {
        self.queue.push_back(line);
    }

    /// The command awaiting acknowledgement, if any.
    pub fn in_flight(&self) -> Option<&str> {
        self.in_flight.as_ref().map(|(line, _)| line.as_str())
    }

    pub fn on_event(&mut self, ev: &Event, now: Instant) {
        match ev {
            // The board answered, so whatever we sent landed.
            Event::Prompt { .. } => self.in_flight = None,
            Event::SlowDown => {
                // Flood control ate the in-flight command. Taking it here
                // is what keeps a burst of scoldings from queueing a
                // resend apiece: the second SlowDown finds nothing left.
                if let Some((line, _)) = self.in_flight.take() {
                    self.queue.push_front(line);
                }
                self.blocked_until = Some(now + self.backoff);
            }
            _ => {}
        }
    }

    /// The next command clear to send, or `None` while the gate is
    /// waiting on an acknowledgement or a backoff.
    pub fn poll(&mut self, now: Instant) -> Option<String> {
        if let Some(until) = self.blocked_until {
            if now < until {
                return None;
            }
            self.blocked_until = None;
        }
        if let Some((_, sent_at)) = &self.in_flight {
            if now.duration_since(*sent_at) < ACK_TIMEOUT {
                return None;
            }
            self.in_flight = None;
        }
        let line = self.queue.pop_front()?;
        self.in_flight = Some((line.clone(), now));
        Some(line)
    }

    /// When the gate could next release a command without any new event
    /// arriving — a backoff expiry or an unacknowledged send timing out.
    /// The runner sleeps until this rather than polling.
    pub fn next_deadline(&self) -> Option<Instant> {
        let backoff = self.blocked_until;
        let ack = self.in_flight.as_ref().map(|(_, at)| *at + ACK_TIMEOUT);
        match (backoff, ack) {
            // Both must pass before anything can go out.
            (Some(a), Some(b)) => Some(a.max(b)),
            (a, b) => a.or(b),
        }
    }
}

/// Decides when a heal has plainly failed, so the runner can call
/// [`crate::bot::Bot::rearm`].
///
/// The bot's heal latch clears only when HP climbs back over the
/// threshold, so a heal that never lands latches it forever: the
/// character sits wounded and never rests again. `bot.rs` documents that
/// the runner rearms it "when it sees the heal was refused" — but there
/// is nothing to see. No refusal wording survives in any of the 51
/// captured transcripts, and the Rust server has no rest command at all,
/// so a string predicate would be a fixture tidier than the board.
///
/// This watches for *progress* instead, clocked by the board's own
/// prompts: a heal that has not moved HP after `heal_retry_prompts`
/// prompts never landed, whatever the reason. `[farm].heal_refused`
/// remains as the escape hatch for the day a real refusal line is
/// finally captured off the live board.
///
/// Note that [`crate::bot::Bot::rearm`] releases the flee latch along
/// with the heal, so a rearm may also let a wounded character run again.
/// That is the right outcome: if resting is not working, leaving is the
/// other option.
pub struct HealWatch {
    heal_command: String,
    retry_prompts: u32,
    refused: Vec<String>,
    /// HP at the first prompt after the heal went out; `None` until then.
    baseline: Option<i32>,
    /// Prompts seen since the heal went out. `None` = not watching.
    prompts: Option<u32>,
}

impl HealWatch {
    pub fn new(bot: &crate::bot::BotConfig, farm: &FarmConfig) -> Self {
        HealWatch {
            heal_command: bot.heal_command.clone(),
            retry_prompts: farm.heal_retry_prompts,
            refused: farm.heal_refused.clone(),
            baseline: None,
            prompts: None,
        }
    }

    /// Called for every command the gate actually releases. A fresh heal
    /// restarts the watch: new baseline, new patience.
    pub fn on_sent(&mut self, line: &str) {
        if line == self.heal_command {
            self.baseline = None;
            self.prompts = Some(0);
        }
    }

    /// Returns true exactly once per heal, when it is time to rearm.
    pub fn on_event(&mut self, ev: &Event) -> bool {
        if self.prompts.is_none() {
            return false;
        }
        match ev {
            Event::Line(line) => {
                if self.refused.iter().any(|r| line.contains(r.as_str())) {
                    self.stop();
                    return true;
                }
                false
            }
            Event::Prompt { hp, .. } => {
                let seen = self.prompts.unwrap_or(0) + 1;
                self.prompts = Some(seen);
                match self.baseline {
                    // First prompt after the heal: this is the mark to beat.
                    None => {
                        self.baseline = Some(*hp);
                        false
                    }
                    // Any climb at all is the heal doing its job.
                    Some(base) if *hp > base => {
                        self.stop();
                        false
                    }
                    Some(_) => {
                        if seen >= self.retry_prompts {
                            self.stop();
                            return true;
                        }
                        false
                    }
                }
            }
            _ => false,
        }
    }

    fn stop(&mut self) {
        self.baseline = None;
        self.prompts = None;
    }
}

/// `Health:    27/35    [77%]` -> `(27, 35)`, searching anywhere in the
/// given text.
///
/// The board prints this for the `health` command ("he"), with a
/// `Mana:`/`Kai:` pool appended for casters. Only the health half is
/// read; the layout is `mud_core::text::health_line`, and the captures
/// in `re/oracle` agree with it down to the spacing.
pub fn parse_health(text: &str) -> Option<(i32, i32)> {
    static HEALTH_RE: std::sync::LazyLock<regex::Regex> =
        std::sync::LazyLock::new(|| regex::Regex::new(r"Health:\s*(\d+)/(\d+)").unwrap());
    let c = HEALTH_RE.captures(text)?;
    Some((c[1].parse().ok()?, c[2].parse().ok()?))
}

/// Ask the board for the character's maximum HP.
///
/// [`crate::bot::BotConfig::max_hp`] scales every percent policy the bot
/// has, and a wrong value mis-scales them silently while `0` disables
/// them outright. Nothing else in the client parses a max HP — the
/// prompt only carries the current value — so the runner asks rather
/// than trusting a number typed into a profile.
pub async fn discover_max_hp(session: &crate::session::Session) -> Option<i32> {
    let mark = session.mark();
    session.send("health");
    session
        .expect("Health:", std::time::Duration::from_secs(15))
        .await
        .ok()?;
    parse_health(&session.since(mark)).map(|(_, max)| max)
}

/// Why the run stopped.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FarmEnd {
    /// The configured number of laps was walked.
    LoopsDone,
    /// `[farm].max_seconds` elapsed.
    TimeUp,
    /// The character died. Nothing else matters after this.
    Died,
    /// A leg was interrupted more often than
    /// [`FarmConfig::travel_interrupts`] allows. An expected outcome
    /// rather than an error: the character is standing somewhere known
    /// and is simply too hurt to keep patrolling.
    TooHurt,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FarmStats {
    pub kills: u32,
    pub loops: u32,
    pub flees: u32,
    pub slowdowns: u32,
    /// Legs stopped part-way by the travel guard.
    pub interrupts: u32,
}

#[derive(Debug)]
pub enum FarmError {
    /// The character is not standing where `[farm].start` says. Every
    /// room id in the circuit is relative to that, so the runner refuses
    /// rather than walking a live character blind.
    NotAtStart {
        expected: String,
        saw: Option<String>,
    },
    /// A flee (or anything else) left the character somewhere that is
    /// not the stop or one of its neighbors, so there is no honest way
    /// to work out where "back" is.
    Lost {
        saw: String,
    },
    Nav(crate::nav::NavError),
    /// The session ended under us.
    Disconnected,
}

impl std::fmt::Display for FarmError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FarmError::NotAtStart { expected, saw } => write!(
                f,
                "not at the configured start: expected {expected:?}, saw {saw:?}"
            ),
            FarmError::Lost { saw } => {
                write!(f, "lost: {saw:?} is not the stop or any neighbor of it")
            }
            FarmError::Nav(e) => write!(f, "{e}"),
            FarmError::Disconnected => write!(f, "disconnected"),
        }
    }
}

impl std::error::Error for FarmError {}

/// The player's own death line. The board prints `<name> is dead.` for a
/// player and `The <template> is dead.` (or the template's own wording)
/// for a monster, so matching the capitalised username cannot collide
/// with a kill.
pub fn is_player_death(line: &str, username: &str) -> bool {
    line.trim() == format!("{username} is dead.")
}

/// The runner's travel guard: what makes walking the wrong thing to be
/// doing right now.
///
/// Pure and stateless — events in, an interrupt or nothing out, no
/// latches. The runner hands the same guard to a resumed leg, so a
/// character that is still wounded has to be able to stop it again.
pub struct FarmGuard {
    max_hp: i32,
    hurt_at_percent: u32,
    username: String,
}

impl FarmGuard {
    pub fn new(max_hp: i32, hurt_at_percent: u32, username: &str) -> Self {
        FarmGuard {
            max_hp,
            hurt_at_percent,
            username: username.to_string(),
        }
    }

    /// A guard that stops only for a death.
    ///
    /// This is what the recovery walk uses. `recover` runs immediately
    /// after AutoFlee bolted at `flee_at_percent`, which is *below*
    /// `interrupt_at_percent` by construction — so a guard watching hp%
    /// would trip on the first prompt of every walk back, and recovery
    /// would become impossible exactly when it is needed. The character
    /// has already decided to run and the walk back is one hop; the only
    /// thing left worth stopping for is dying.
    pub fn death_only(username: &str) -> Self {
        FarmGuard::new(0, 0, username)
    }
}

impl crate::nav::TravelGuard for FarmGuard {
    fn on_event(&mut self, ev: &Event) -> Option<crate::nav::Interrupt> {
        use crate::nav::Interrupt;
        match ev {
            Event::Line(line) if is_player_death(line, &self.username) => Some(Interrupt::Died),
            // HP reads negative while downed, and nothing lands until a
            // revive.
            Event::Prompt { hp, .. } if *hp <= 0 => Some(Interrupt::Died),
            // 0 max HP means the profile never said and the probe found
            // nothing. Guessing would mis-scale the one decision keeping
            // the character alive, so say nothing rather than something
            // wrong.
            Event::Prompt { hp, .. }
                if self.max_hp > 0 && *hp * 100 / self.max_hp < self.hurt_at_percent as i32 =>
            {
                Some(Interrupt::Hurt { hp: *hp })
            }
            _ => None,
        }
    }
}

/// Run the patrol.
///
/// One sender at a time, by construction. The navigator and the bot both
/// send movement and both read `RoomSeen`, so letting them run at once
/// would have them fighting over the same events: instead the runner is
/// a sequence of phases, and each phase owns the connection outright.
///
/// - **Travel** is [`crate::nav::Navigator::goto`] under a
///   [`FarmGuard`]. The bot still does not drive it — feeding it events
///   while suppressing its commands would leave it believing it had
///   swung — but the walk is no longer blind: the guard watches every
///   event goto goes past, including the ones its between-step drain
///   throws away, and hands the connection back when the character is
///   hurt or dead. The runner then defends where it stands and picks
///   the leg back up. `[farm].depart_at_percent` still keeps a wounded
///   character from setting off; what is new is that aggro in transit
///   is fought rather than merely survived.
/// - **Farm** is a fresh [`crate::bot::Bot`] per stop — latches start clean, so no
///   reset API is needed — driven by the pump below.
/// - **Recover** is what happens when a flee moves the character with no
///   navigator involved: work out where it landed, walk back, and after
///   a few round trips give up on the stop rather than flee-loop.
pub async fn run_farm(
    session: &crate::session::Session,
    graph: std::sync::Arc<RoomGraph>,
    plan: &FarmPlan,
    bot_config: &crate::bot::BotConfig,
    cfg: &FarmConfig,
) -> Result<(FarmEnd, FarmStats), FarmError> {
    let started = Instant::now();
    let mut stats = FarmStats::default();
    let nav = crate::nav::Navigator::new(graph.clone(), cfg.nav.clone());
    // Danger ranking from the shipped data. A missing or unreadable
    // database is not fatal: an empty table simply means "no opinion",
    // and the bot falls back to the board's own listing order.
    let threat = std::sync::Arc::new(
        RoomGraph::load_threat(&cfg.content).unwrap_or_else(|e| {
            eprintln!("threat ranking unavailable ({e}); using board order");
            crate::bot::ThreatTable::new()
        }),
    );

    // Every percent policy divides by this, and a wrong value mis-scales
    // heal and flee silently. 0 means the profile did not say, so ask.
    let mut bot_config = bot_config.clone();
    if bot_config.max_hp == 0
        && let Some(max) = discover_max_hp(session).await
    {
        bot_config.max_hp = max;
    }

    verify_start(session, &graph, plan.start).await?;

    let mut current = plan.start;
    loop {
        for &stop in &plan.circuit {
            if let Some(end) = time_up(started, cfg) {
                return Ok((end, stats));
            }
            if current != stop {
                match travel(
                    session,
                    &nav,
                    &graph,
                    &mut current,
                    stop,
                    cfg,
                    &bot_config,
                    &threat,
                    started,
                    &mut stats,
                )
                .await?
                {
                    LegEnd::Arrived => {}
                    LegEnd::Died => return Ok((FarmEnd::Died, stats)),
                    LegEnd::TimeUp => return Ok((FarmEnd::TimeUp, stats)),
                    LegEnd::TooHurt => return Ok((FarmEnd::TooHurt, stats)),
                }
            }
            match farm_stop(
                session,
                &nav,
                &graph,
                stop,
                &bot_config,
                &threat,
                cfg,
                started,
                None,
                &mut stats,
            )
            .await?
            {
                StopEnd::Dwelt => {}
                StopEnd::Died => return Ok((FarmEnd::Died, stats)),
                StopEnd::TimeUp => return Ok((FarmEnd::TimeUp, stats)),
            }
        }
        stats.loops += 1;
        if cfg.loops != 0 && stats.loops >= cfg.loops {
            return Ok((FarmEnd::LoopsDone, stats));
        }
    }
}

fn time_up(started: Instant, cfg: &FarmConfig) -> Option<FarmEnd> {
    (cfg.max_seconds != 0 && started.elapsed().as_secs() >= cfg.max_seconds)
        .then_some(FarmEnd::TimeUp)
}

/// Confirm by room name that the character is where the profile claims.
/// Walk the character to the plan's finish room, if it has one.
///
/// Deliberately NOT part of [`run_farm`]. The commonest way a farm ends
/// is Ctrl-C, which cancels `run_farm` outright — anything inside it
/// would never run on the path that matters most. So this is a separate
/// step the caller takes on every exit route, normal or interrupted.
///
/// It asks the board where the character is rather than trusting a
/// position carried out of the run: an interrupted run may have stopped
/// anywhere, including mid-leg. That answer goes through
/// [`crate::nav::Navigator::localize_view`], so being several rooms from
/// the circuit is recoverable rather than fatal.
///
/// A dead character cannot walk, so callers should skip this on
/// [`FarmEnd::Died`].
pub async fn go_to_finish(
    session: &crate::session::Session,
    graph: std::sync::Arc<RoomGraph>,
    plan: &FarmPlan,
    cfg: &FarmConfig,
) -> Result<(), FarmError> {
    let Some(finish) = plan.finish else {
        return Ok(());
    };
    let nav = crate::nav::Navigator::new(graph.clone(), cfg.nav.clone());
    let mut events = session.events();
    crate::session::drain(&mut events, |_| {});
    session.send("look");
    let seen = next_room_view(&mut events, Duration::from_secs(15))
        .await
        .ok_or(FarmError::NotAtStart {
            expected: "a room block answering the finish walk's look".into(),
            saw: None,
        })?;
    if let Some(here) = graph.room(finish)
        && here.name == seen.name
    {
        return Ok(());
    }
    let hint = plan.circuit.last().copied().unwrap_or(plan.start);
    let at = nav
        .localize_view(hint, &seen)
        .ok_or(FarmError::Lost { saw: seen.name })?;
    let mut guard = FarmGuard::death_only(&session.profile().username);
    nav.goto(session, at, finish, &mut guard)
        .await
        .map(|_| ())
        .map_err(FarmError::Nav)
}

/// The next room block in full, not just its name.
async fn next_room_view(
    events: &mut tokio::sync::broadcast::Receiver<Event>,
    within: Duration,
) -> Option<crate::events::RoomView> {
    let deadline = tokio::time::Instant::now() + within;
    loop {
        match tokio::time::timeout_at(deadline, events.recv()).await {
            Ok(Ok(Event::RoomSeen(room))) => return Some(room),
            Ok(Ok(_)) => continue,
            Ok(Err(tokio::sync::broadcast::error::RecvError::Lagged(_))) => continue,
            Ok(Err(_)) | Err(_) => return None,
        }
    }
}

async fn verify_start(
    session: &crate::session::Session,
    graph: &RoomGraph,
    start: RoomId,
) -> Result<(), FarmError> {
    let expected = graph
        .room(start)
        .map(|r| r.name.clone())
        .unwrap_or_default();
    let mut events = session.events();
    crate::session::drain(&mut events, |_| {});
    session.send("look");
    let saw = next_room(session, &mut events, Duration::from_secs(15)).await;
    match saw {
        Some(name) if name == expected => Ok(()),
        saw => Err(FarmError::NotAtStart { expected, saw }),
    }
}

/// The next room block, or `None` if the board did not print one in time.
async fn next_room(
    _session: &crate::session::Session,
    events: &mut tokio::sync::broadcast::Receiver<Event>,
    within: Duration,
) -> Option<String> {
    let deadline = tokio::time::Instant::now() + within;
    loop {
        match tokio::time::timeout_at(deadline, events.recv()).await {
            Ok(Ok(Event::RoomSeen(room))) => return Some(room.name),
            Ok(Ok(_)) => continue,
            Ok(Err(tokio::sync::broadcast::error::RecvError::Lagged(_))) => continue,
            Ok(Err(_)) | Err(_) => return None,
        }
    }
}

/// How a leg ended.
enum LegEnd {
    Arrived,
    Died,
    TimeUp,
    TooHurt,
}

/// Walk one leg, defending it where necessary. Nothing else sends while
/// this runs: the navigator holds the connection for the walk, the
/// defend pump holds it while defending, never both at once.
///
/// Worst case is bounded, and worth stating because it is long:
/// `travel_interrupts` interruptions, each costing up to
/// `defend_seconds` of defending — inside which the stop pump may itself
/// spend three flee round trips — plus up to two minutes waiting on the
/// departure gate before each attempt.
#[allow(clippy::too_many_arguments)]
async fn travel(
    session: &crate::session::Session,
    nav: &crate::nav::Navigator,
    graph: &RoomGraph,
    current: &mut RoomId,
    stop: RoomId,
    cfg: &FarmConfig,
    bot_config: &crate::bot::BotConfig,
    threat: &std::sync::Arc<crate::bot::ThreatTable>,
    started: Instant,
    stats: &mut FarmStats,
) -> Result<LegEnd, FarmError> {
    use crate::nav::{Interrupt, NavErrorKind};

    let mut guard = FarmGuard::new(
        bot_config.max_hp,
        cfg.interrupt_at_percent,
        &session.profile().username,
    );
    let mut budget = cfg.travel_interrupts;

    loop {
        if time_up(started, cfg).is_some() {
            return Ok(LegEnd::TimeUp);
        }
        wait_for_departure_health(session, cfg, bot_config).await;

        let err = match nav.goto(session, *current, stop, &mut guard).await {
            Ok(at) => {
                *current = at;
                return Ok(LegEnd::Arrived);
            }
            Err(e) => e,
        };
        // Every exit from here writes the position first. A resumed leg
        // that started from a stale `current` would be walking a route
        // computed from a lie, which is the failure verified navigation
        // exists to prevent.
        *current = err.at;

        match err.kind {
            NavErrorKind::Interrupted(Interrupt::Died) => return Ok(LegEnd::Died),
            NavErrorKind::Interrupted(Interrupt::Hurt { .. }) => {
                stats.interrupts += 1;
                if budget == 0 {
                    return Ok(LegEnd::TooHurt);
                }
                budget -= 1;

                // Defend where we stand. The stop pump already knows how
                // to fight, heal, flee and walk back, and it takes the
                // room as an argument — there is no second pump to
                // write, and a corridor is farmed exactly like a stop.
                let until = Instant::now() + Duration::from_secs(cfg.defend_seconds);
                match farm_stop(
                    session,
                    nav,
                    graph,
                    err.at,
                    bot_config,
                    threat,
                    cfg,
                    started,
                    Some(until),
                    stats,
                )
                .await?
                {
                    StopEnd::Dwelt => continue,
                    StopEnd::Died => return Ok(LegEnd::Died),
                    StopEnd::TimeUp => return Ok(LegEnd::TimeUp),
                }
            }
            _ => Err(FarmError::Nav(err))?,
        }
    }
}

/// Hold at the stop until HP is fit to travel. The bot is not driving
/// while the navigator walks, so setting off wounded means relying on
/// the travel guard to stop the leg part-way — cheaper to leave fit.
async fn wait_for_departure_health(
    session: &crate::session::Session,
    cfg: &FarmConfig,
    bot_config: &crate::bot::BotConfig,
) {
    if cfg.depart_at_percent == 0 || bot_config.max_hp <= 0 {
        return;
    }
    let target = bot_config.max_hp * cfg.depart_at_percent as i32 / 100;
    let mut state = session.state();
    // Bounded: a character that cannot reach the threshold (poisoned, no
    // heal configured) must not wedge the patrol forever.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(120);
    while state.borrow().hp < target {
        if tokio::time::timeout_at(deadline, state.changed())
            .await
            .is_err()
        {
            return;
        }
    }
}

enum StopEnd {
    Dwelt,
    Died,
    TimeUp,
}

/// Farm one stop until it goes quiet.
///
/// A fresh [`crate::bot::Bot`] per stop: its latches (engaged, fled, the
/// heal debounce) all start clean, which is also how a lagged broadcast
/// is recovered — throw the bot away and re-`look` rather than carry on
/// with state that silently missed events.
#[allow(clippy::too_many_arguments)]
async fn farm_stop(
    session: &crate::session::Session,
    nav: &crate::nav::Navigator,
    graph: &RoomGraph,
    stop: RoomId,
    bot_config: &crate::bot::BotConfig,
    threat: &std::sync::Arc<crate::bot::ThreatTable>,
    cfg: &FarmConfig,
    started: Instant,
    // Hard cap on this stop, or None to stay until it goes quiet.
    until: Option<Instant>,
    stats: &mut FarmStats,
) -> Result<StopEnd, FarmError> {
    let stop_name = graph.room(stop).map(|r| r.name.clone()).unwrap_or_default();
    let username = session.profile().username.clone();
    let backoff = Duration::from_millis(cfg.slowdown_backoff_ms);
    let poke_after = Duration::from_millis(cfg.idle_poke_ms);

    // Flee round trips tolerated before the stop is written off. Without
    // this a character that flees on every prompt never leaves the room
    // pair it is bouncing between.
    let mut recoveries_left = 3u32;

    let mut events = session.events();
    crate::session::drain(&mut events, |_| {});

    let mut bot = crate::bot::Bot::with_threat(bot_config.clone(), threat.clone());
    let mut gate = Gate::new(backoff);
    let mut heal = HealWatch::new(bot_config, cfg);
    let mut idle_prompts = 0u32;
    let mut acted_since_prompt = false;

    // Seed the bot: exits to flee through, and anything already here.
    gate.push("look".into());

    loop {
        if time_up(started, cfg).is_some() {
            return Ok(StopEnd::TimeUp);
        }
        // Defending is capped: see FarmConfig::defend_seconds for why a
        // stop that cannot go quiet must still end.
        if until.is_some_and(|d| Instant::now() >= d) {
            return Ok(StopEnd::Dwelt);
        }

        // Release whatever the gate is willing to send.
        let now = Instant::now();
        while let Some(cmd) = gate.poll(now) {
            heal.on_sent(&cmd);
            session.send(&cmd);
        }

        // Sleep until the next event, the gate's own deadline, or the
        // idle poke — whichever comes first.
        let wake = gate
            .next_deadline()
            .map(|d| d.min(now + poke_after))
            .unwrap_or(now + poke_after);
        let wake = tokio::time::Instant::from_std(wake);

        let ev = match tokio::time::timeout_at(wake, events.recv()).await {
            Ok(Ok(ev)) => ev,
            // Dropped events desync a stateful bot: it can miss the death
            // that ends a fight and sit latched on a corpse. Start over
            // rather than carry on with a bot that quietly lost track.
            Ok(Err(tokio::sync::broadcast::error::RecvError::Lagged(_))) => {
                bot = crate::bot::Bot::with_threat(bot_config.clone(), threat.clone());
                gate = Gate::new(backoff);
                gate.push("look".into());
                continue;
            }
            Ok(Err(_)) => return Err(FarmError::Disconnected),
            // Nothing happening. Ask the room whether that is still true;
            // the answer is a prompt, which is what dwell counts.
            Err(_) => {
                if gate.in_flight().is_none() && bot.engaged().is_none() {
                    gate.push("look".into());
                }
                continue;
            }
        };

        if let Event::SlowDown = ev {
            stats.slowdowns += 1;
        }
        if let Event::Line(line) = &ev {
            if is_player_death(line, &username) {
                return Ok(StopEnd::Died);
            }
            if line.contains("falls to the ground") {
                stats.kills += 1;
            }
        }
        if let Event::Prompt { hp, .. } = ev
            && hp <= 0
        {
            return Ok(StopEnd::Died);
        }

        // A room block naming somewhere else means the character moved
        // without the navigator — the bot fled.
        if let Event::RoomSeen(room) = &ev
            && room.name != stop_name
        {
            stats.flees += 1;
            if recoveries_left == 0 {
                // Give up on this stop, but walk back to it so the next
                // leg starts from where the plan believes we are.
                return recover(session, nav, graph, stop, room)
                    .await
                    .map(|end| match end {
                        RecoverEnd::Back => StopEnd::Dwelt,
                        RecoverEnd::Died => StopEnd::Died,
                    });
            }
            recoveries_left -= 1;
            if let RecoverEnd::Died = recover(session, nav, graph, stop, room).await? {
                return Ok(StopEnd::Died);
            }
            // Back at the stop with a clean slate.
            events = session.events();
            bot = crate::bot::Bot::with_threat(bot_config.clone(), threat.clone());
            gate = Gate::new(backoff);
            heal = HealWatch::new(bot_config, cfg);
            gate.push("look".into());
            idle_prompts = 0;
            acted_since_prompt = false;
            continue;
        }

        gate.on_event(&ev, Instant::now());
        if heal.on_event(&ev) {
            bot.rearm();
        }

        let actions = bot.on_event(&ev);
        if !actions.is_empty() {
            acted_since_prompt = true;
        }
        for crate::bot::BotAction::Send(cmd) in actions {
            gate.push(cmd);
        }

        // The stop is done when the board has answered repeatedly with
        // nothing for the bot to do and no fight outstanding. Only the
        // bot's own decisions count as activity: the idle `look` is the
        // runner asking whether anything is happening, and counting it
        // would answer its own question and dwell forever.
        if let Event::Prompt { .. } = ev {
            if acted_since_prompt || bot.engaged().is_some() {
                idle_prompts = 0;
            } else {
                idle_prompts += 1;
                if idle_prompts >= cfg.dwell_idle_prompts {
                    return Ok(StopEnd::Dwelt);
                }
            }
            acted_since_prompt = false;
        }
    }
}

/// How a walk back ended.
enum RecoverEnd {
    Back,
    Died,
}

/// Find out where a flee left us and walk back to the stop.
///
/// The walk back is guarded, but only against dying. Guarding it on hp%
/// like an ordinary leg would break it outright: `recover` is called
/// immediately after AutoFlee bolted at `flee_at_percent`, which sits
/// *below* `interrupt_at_percent` by construction, so the guard would
/// trip on the first prompt of every recovery and make walking back
/// impossible exactly when it is needed. It would also want to defend
/// where it stood, and the defending pump is what called `recover` —
/// mutually recursive `async fn`s do not compile.
///
/// The character has already decided to run and the walk back is one
/// hop. The only thing left worth stopping for is a death, which used
/// to pass unnoticed here.
async fn recover(
    session: &crate::session::Session,
    nav: &crate::nav::Navigator,
    graph: &RoomGraph,
    stop: RoomId,
    saw: &crate::events::RoomView,
) -> Result<RecoverEnd, FarmError> {
    let _ = graph;
    // The whole room block, not just its name: a flee can chain further
    // than one hop, and the exits are what make a wider search safe.
    let at = nav.localize_view(stop, saw).ok_or_else(|| FarmError::Lost {
        saw: saw.name.clone(),
    })?;
    let mut guard = FarmGuard::death_only(&session.profile().username);
    match nav.goto(session, at, stop, &mut guard).await {
        Ok(_) => Ok(RecoverEnd::Back),
        Err(e) if matches!(e.kind, crate::nav::NavErrorKind::Interrupted(_)) => {
            Ok(RecoverEnd::Died)
        }
        Err(e) => Err(FarmError::Nav(e)),
    }
}
