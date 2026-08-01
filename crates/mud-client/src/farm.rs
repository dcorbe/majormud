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

use crate::correlate::{CmdId, Correlated};
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
    /// Hard cap on one circuit stop, in seconds; 0 = stay until the
    /// room proves quiet (the old behaviour, and the default).
    ///
    /// A stop normally ends on evidence — the room block proves the
    /// room empty. A delay-0 respawn room shared with another player
    /// never proves it: somebody's kill is always mid-respawn, so the
    /// evidence rule holds the stop open forever (live: the Arena
    /// standoff south of the door, 2026-08-01). The cap ends the stop
    /// anyway, mid-fight included, and the lap moves on — the room gets
    /// its next chance when the circuit comes round. Pursuers are the
    /// travel guard's problem, exactly as when a defence deadline
    /// expires.
    pub stop_seconds: u64,
    /// How long to hold a stop open once the room block has PROVEN it
    /// empty, waiting for a respawn. 0 leaves the moment it is proven.
    ///
    /// This is a policy budget, not a state inference, and the difference
    /// is the whole point of [`StopState`]. "Is there anything here to
    /// fight" is a question the board answers outright in every room
    /// block. "How long is it worth waiting for something new to turn up"
    /// is a judgement about this circuit that no message can answer, so it
    /// gets a knob with an honest name.
    ///
    /// Respawns are silent — `generate_monster` places a monster with no
    /// announcement whatsoever — so the wait is spent re-asking, not
    /// listening.
    pub dwell_empty_seconds: u64,
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
    /// Stop and fight when something attacks the character mid-leg,
    /// rather than walking on while it swings.
    ///
    /// On by default. A leg crossing a hostile room otherwise keeps
    /// trying to move while its steps are eaten — measured live at
    /// 1/2150 Newhaven Arena, where a patrol died with `timed out waiting
    /// for "room block after movement"` because three monsters were
    /// beating on it. The HP gate below is not a substitute: a healthy
    /// character can be swarmed a long time without falling past it.
    ///
    /// Turn it off for legs whose point is to get somewhere. The
    /// character then still stops for the HP gate and for dying — it
    /// simply does not turn and swing at everything on the way.
    pub fight_while_travelling: bool,
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
    /// Cap on resting at the departure gate, in seconds.
    ///
    /// Bounded because a character that cannot reach the threshold —
    /// poisoned, no heal configured, out of mana — must not wedge the
    /// patrol forever.
    pub max_rest_seconds: u64,
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
            stop_seconds: 0,
            dwell_empty_seconds: 0,
            depart_at_percent: 80,
            fight_while_travelling: true,
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
            max_rest_seconds: 120,
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

        // Dark stops WARN rather than refuse: build runs before the
        // connection exists, so it cannot know the character's kit
        // (light sources are probed at run start), refusing would brick
        // mixed circuits that farm their lit stops perfectly well, and
        // the threshold is bracketed evidence, not proven. The runtime
        // complement is the graph-aware lighting in travel/farm_stop.
        for &stop in circuit.iter().chain(finish.iter()) {
            if let Some(room) = graph.room(stop)
                && room.light < 0
            {
                eprintln!(
                    "note: {}/{} ({}) is dark (light {}); it needs a working light source",
                    stop.map, stop.room, room.name, room.light
                );
            }
        }

        Ok(FarmPlan {
            start,
            circuit,
            finish,
        })
    }
}

/// What the runner is doing right now.
///
/// Published by [`run_farm`] rather than inferred from board output. A
/// watcher can see combat lines and guess "attacking", but it cannot tell
/// waiting-to-depart from wedged, or travelling from standing still,
/// because both look like silence — and telling those apart is the whole
/// point of showing it.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub enum Phase {
    #[default]
    Starting,
    /// Holding at the start room until fit enough to set off.
    WaitingToDepart,
    Travelling {
        to: RoomId,
    },
    /// At a stop with nothing to do; waiting for a respawn.
    Waiting {
        at: RoomId,
    },
    Fighting {
        at: RoomId,
        target: String,
    },
    Resting {
        at: RoomId,
    },
    /// Walking back to a stop after being moved off it.
    Recovering {
        to: RoomId,
    },
    WalkingHome,
    /// The run ended on its own terms — and says WHICH terms. A bare
    /// "done" hid a TooHurt ending from the operator watching a healthy
    /// character stand idle (cwgaming, 2026-08-01); the reason rides in
    /// the phase for exactly the reason Failed's does.
    Done {
        why: String,
    },
    /// The run ended badly. Carried in the phase rather than logged and
    /// dropped, because the operator watching the status bar is exactly
    /// the person who needs to know WHY it stopped — "done" for a run
    /// that refused to start is worse than saying nothing.
    Failed {
        why: String,
    },
}

impl Phase {
    /// Short label for a status bar.
    pub fn label(&self) -> String {
        match self {
            Phase::Starting => "starting".into(),
            Phase::WaitingToDepart => "waiting to depart".into(),
            Phase::Travelling { to } => format!("travelling to {}/{}", to.map, to.room),
            Phase::Waiting { .. } => "waiting".into(),
            Phase::Fighting { target, .. } => format!("attacking {target}"),
            Phase::Resting { .. } => "resting".into(),
            Phase::Recovering { to } => format!("recovering to {}/{}", to.map, to.room),
            Phase::WalkingHome => "walking home".into(),
            Phase::Done { why } => format!("done: {why}"),
            // First line only. A NavError's Display carries a multi-line
            // `tail:` of raw board output, and the bar is one row.
            Phase::Failed { why } => {
                let head = why.lines().next().unwrap_or("").trim();
                format!("stopped: {head}")
            }
        }
    }

    /// The room the runner believes it is in, when it knows.
    pub fn room(&self) -> Option<RoomId> {
        match self {
            Phase::Waiting { at } | Phase::Fighting { at, .. } | Phase::Resting { at } => Some(*at),
            _ => None,
        }
    }
}

/// Where the runner publishes its [`Phase`]. `None` discards.
pub type PhaseSink<'a> = Option<&'a tokio::sync::watch::Sender<Phase>>;

fn set_phase(sink: PhaseSink<'_>, phase: Phase) {
    if let Some(tx) = sink {
        // A dropped receiver is not an error: nobody is watching.
        let _ = tx.send(phase);
    }
}

/// The runner's only outbound path.
///
/// [`crate::session::Session::send`] is an unbounded, unacknowledged
/// queue, and the live board paces at 1500ms, so firing every bot
/// decision straight at it buries the character under minutes of stale
/// commands with no way to cancel. The gate keeps exactly one command in
/// flight until an event ANSWERING that send arrives — the echo, or any
/// later attributed reply. A prompt is not an acknowledgement: the board
/// sends them in bursts and unsolicited, re-prompting whenever async
/// output disturbs a dangling one, so "next prompt" used to let a
/// stranger's blow clear our in-flight command.
///
/// It is also the only thing that acts on [`Event::SlowDown`], which
/// means the board *dropped* our input. The dropped command goes back to
/// the front of the queue and sending pauses until the board calms down.
/// The bot's own latches stay truthful through all of this: it decided to
/// swing once, and the swing does eventually happen, so nothing has to
/// re-arm.
pub struct Gate {
    queue: VecDeque<String>,
    /// The command awaiting acknowledgement: its text, the send id to
    /// match against `Correlated::answers` (None between `poll` and
    /// `confirm`), and when it went out.
    in_flight: Option<(String, Option<CmdId>, Instant)>,
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
        self.in_flight.as_ref().map(|(line, _, _)| line.as_str())
    }

    /// Attach the session's send id to the command `poll` just released.
    /// The runner calls this right after `session.send`, in the same
    /// synchronous stretch — before any event can be processed — which
    /// is what makes the id-less window unobservable. A second confirm
    /// for one poll would silently swap the id, so it crashes instead.
    pub fn confirm(&mut self, id: CmdId) {
        if let Some((_, slot, _)) = &mut self.in_flight {
            debug_assert!(slot.is_none(), "confirm() twice for one poll");
            *slot = Some(id);
        }
    }

    /// Nothing queued and nothing awaiting acknowledgement: everything the
    /// runner decided has been sent, and the board has answered it.
    ///
    /// The runner will not leave a stop while this is false. A queued
    /// `get copper` from the last kill's loot is the case that matters --
    /// walking out drops it on the floor.
    pub fn is_idle(&self) -> bool {
        self.queue.is_empty() && self.in_flight.is_none()
    }

    pub fn on_event(&mut self, cor: &Correlated, now: Instant) {
        if let Event::SlowDown = cor.event {
            // Flood control ate the in-flight command. Taking it here
            // is what keeps a burst of scoldings from queueing a
            // resend apiece: the second SlowDown finds nothing left.
            // The resend goes out under a NEW send id.
            if let Some((line, _, _)) = self.in_flight.take() {
                self.queue.push_front(line);
            }
            self.blocked_until = Some(now + self.backoff);
            return;
        }
        // The board accepted our send: the echo (or any later reply)
        // arrives attributed to its id. Nothing else acks — least of
        // all a prompt.
        if let (Some(answers), Some((_, Some(id), _))) = (cor.answers, &self.in_flight)
            && answers == *id
        {
            self.in_flight = None;
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
        if let Some((_, _, sent_at)) = &self.in_flight {
            if now.duration_since(*sent_at) < ACK_TIMEOUT {
                return None;
            }
            self.in_flight = None;
        }
        let line = self.queue.pop_front()?;
        self.in_flight = Some((line.clone(), None, now));
        Some(line)
    }

    /// When the gate could next release a command without any new event
    /// arriving — a backoff expiry or an unacknowledged send timing out.
    /// The runner sleeps until this rather than polling.
    pub fn next_deadline(&self) -> Option<Instant> {
        let backoff = self.blocked_until;
        let ack = self.in_flight.as_ref().map(|(_, _, at)| *at + ACK_TIMEOUT);
        match (backoff, ack) {
            // Both must pass before anything can go out.
            (Some(a), Some(b)) => Some(a.max(b)),
            (a, b) => a.or(b),
        }
    }
}

/// What the stop's own evidence says to do next.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
    /// No room block worth trusting. Ask (`look`) and decide when the
    /// board answers.
    Ask,
    /// A fight is outstanding, or the board listed something worth
    /// swinging at. Stay — the bot is driving.
    Busy,
    /// Proven empty, held open for a respawn until this instant.
    Waiting { until: Instant },
    /// Proven empty for the whole respawn budget. Done here.
    Empty,
    /// The board answered a `look` with "you can't see anything". No room
    /// block is coming, so nothing here can be decided from one.
    Blind,
}

/// What the runner has actually SEEN at this stop, and what that implies
/// about leaving it.
///
/// The stop used to end on a count of prompts. A prompt is evidence that
/// the board answered *something*; it is not evidence about who is
/// standing in the room, and four successive patches tuned that threshold
/// without ever making it true. Three live failures came out of it:
/// walking out with three monsters still listed under "Also here:",
/// abandoning a fight that was still going, and hanging forever on an
/// attack the board had refused. The room block states all three
/// outright, and the parser has been delivering it the whole time.
///
/// Pure and clock-injected, like [`Gate`] and [`HealWatch`], so the
/// decision can be unit-tested — which `farm_stop`, an `async fn` over a
/// live [`crate::session::Session`], never could be. That asymmetry is
/// why the four patches never caught each other.
///
/// Two things here are still time-based, and both are honest about it:
/// [`FarmConfig::dwell_empty_seconds`] is a policy budget, and `recheck`
/// bounds how stale an observation may get. The second is unavoidable —
/// **nothing announces a respawn**, the board simply puts a monster in
/// the room, so silence is not proof the room is unchanged. Bounding an
/// observation's shelf life is a different thing from inferring state
/// from a clock: it forces a re-observation, it never decides on its own.
pub struct StopState {
    /// This stop's room name. A block naming anywhere else describes
    /// anywhere else; the runner handles that as a flee.
    stop_name: String,
    /// How long a proven-empty stop is held open for a respawn.
    linger: Duration,
    /// How stale an accepted block may get before it must be re-asked.
    recheck: Duration,
    /// When this stop was last described by a block we believed.
    ///
    /// The block itself is gone. It used to be kept because occupancy
    /// was read off it; [`crate::world::Here`] answers that now, and
    /// what is left is the one question a snapshot could always answer
    /// honestly — how long ago. `None` is "never observed, or observed
    /// and since invalidated", which is what makes `Ask` the floor.
    observed: Option<Instant>,
    /// When the room was FIRST proven empty. The respawn budget runs from
    /// here, so re-asking does not restart it; anything that makes the
    /// room untrue clears it.
    empty_since: Option<Instant>,
    /// The board said the room cannot be seen.
    blind: bool,
    /// The `look` owed an answer: its send id, matched against
    /// [`Correlated::answers`]. Only the block (or dark line) ANSWERING
    /// this send is believed — an unsolicited block is somebody else's
    /// render and clears nothing.
    ///
    /// The stop must not end while this is `Some`. Leaving hands the
    /// connection to the navigator, and the block still owed to our
    /// `look` then arrives mid-step. Attribution keeps it from
    /// SATISFYING the step now, but walking out while owed an answer is
    /// still walking out on evidence in flight.
    ///
    /// Cleared by [`StopState::invalidate`] too: if the room changed
    /// while the look was in flight (a mid-render arrival — parse
    /// classifies async lines inside an accumulating block, so the
    /// block's content can predate an ActorEntered that was EMITTED
    /// first), the answer predates reality and must be re-asked, not
    /// believed.
    pending_look: Option<(crate::correlate::CmdId, Instant)>,
    /// When a death line landed that the board has not finished
    /// narrating.
    ///
    /// A kill is a SEQUENCE, not a line. `check_kill_monster` drops the
    /// corpse's carry, splits the experience and only then breaks
    /// combat, so the coin drop, the award and "*Combat Off*" all
    /// arrive after the wording that announced the death. Leaving on
    /// the first of them walks out on the rest — the live in-process
    /// circuit finished a lap having killed the rat and never seen the
    /// experience for it.
    ///
    /// This used to be covered by accident: a kill invalidated the
    /// observation, the re-`look` that followed left the gate busy, and
    /// `Empty` is guarded on an idle gate. Stage 2 removed the re-look,
    /// so the wait has to be said out loud.
    ///
    /// Cleared by "*Combat Off*" — the board's own full stop — and
    /// bounded by `recheck`, because somebody else's kill never earns
    /// us one.
    resolving: Option<Instant>,
}

impl StopState {
    pub fn new(stop_name: String, cfg: &FarmConfig) -> Self {
        StopState {
            stop_name,
            linger: Duration::from_secs(cfg.dwell_empty_seconds),
            recheck: Duration::from_millis(cfg.idle_poke_ms),
            observed: None,
            resolving: None,
            pending_look: None,
            empty_since: None,
            blind: false,
        }
    }

    /// Called for every command the gate actually releases — the same
    /// hook [`HealWatch::on_sent`] uses — with the send id the session
    /// allocated, which is what the answer will carry.
    pub fn on_sent(&mut self, line: &str, id: crate::correlate::CmdId) {
        let line = line.trim();
        if line.eq_ignore_ascii_case("look") {
            self.pending_look = Some((id, Instant::now()));
        } else if crate::correlate::is_movement(&line.to_lowercase()) {
            // Our own move (a flee) is about to change the room: any
            // in-flight answer predates it — the symmetric hole to the
            // mid-render race. What we saw goes too; the block we may
            // yet believe must postdate our own displacement.
            self.invalidate();
        }
    }

    /// Something happened that could have changed who is standing here.
    /// What we saw is discarded outright, and so is the outstanding ask:
    /// its answer predates this and must not be believed. `blind` goes
    /// too — with the ask dropped, nothing else would ever clear it, and
    /// a stale Blind verdict fires `source_died` against a source that
    /// just lit, silently poisoning the light plan for the run. The cost
    /// of forgetting is one honest re-look.
    fn invalidate(&mut self) {
        self.observed = None;
        self.empty_since = None;
        self.pending_look = None;
        self.blind = false;
        self.resolving = None;
    }

    /// Accept a block the traveller already earned: the answer to the
    /// leg's final step, attributed to OUR command — the same discipline
    /// `pending_look` enforces, settled before the stop opened. Without
    /// this the stop re-asked the board for what the arrival render had
    /// just said, one full round-trip before the first swing.
    ///
    /// Mirrors the `answers_look` acceptance arm field-for-field, and
    /// like [`StopState::on_event`] it is called AFTER the bot has been
    /// shown the same block. A block naming somewhere else seeds
    /// nothing — however it arrived, it describes somewhere else.
    pub fn seed(
        &mut self,
        room: crate::events::RoomView,
        bot: &crate::bot::Bot,
        here: &crate::world::Here,
        now: Instant,
    ) {
        if room.name != self.stop_name {
            return;
        }
        self.blind = false;
        self.observed = Some(now);
        self.note_occupancy(bot, here, now);
    }

    /// Start, hold or clear the respawn budget, from the MAINTAINED
    /// occupant list rather than from the block that happens to be in
    /// hand.
    ///
    /// The distinction is the whole of stage 2. A block is a snapshot
    /// with a shelf life; [`crate::world::Here`] has the kills,
    /// arrivals and departures since it folded in. Reading the snapshot
    /// meant a kill could only be learned from another `look`, so every
    /// kill in a pack cost a round-trip before the next swing.
    ///
    /// Both guards are load-bearing. Without an observation in hand
    /// there is nothing to be empty OF — `verdict` returns `Ask`
    /// anyway, and starting the budget there would have it run while
    /// the room is unknown. And an unseeded model reports an empty
    /// occupant list, which is not the same answer as an empty room.
    fn note_occupancy(&mut self, bot: &crate::bot::Bot, here: &crate::world::Here, now: Instant) {
        if self.observed.is_none() || !here.seeded() {
            return;
        }
        if bot.has_target_among(here.names()) {
            self.empty_since = None;
        } else {
            self.empty_since.get_or_insert(now);
        }
    }

    /// Fold one attributed event. Call AFTER [`crate::bot::Bot::on_event`]
    /// and AFTER [`crate::world::Here::on_event`], so `engaged` and the
    /// occupant list already account for it.
    pub fn on_event(
        &mut self,
        cor: &crate::correlate::Correlated,
        bot: &crate::bot::Bot,
        here: &crate::world::Here,
        now: Instant,
    ) {
        // Does this event answer OUR outstanding look? An unsolicited
        // block — somebody else's render, a stale answer to a forgotten
        // ask — is never believed and never clears the ask.
        let answers_look = cor
            .answers
            .is_some_and(|a| self.pending_look.is_some_and(|(id, _)| a == id));
        match &cor.event {
            // A prompt means the board answered SOMETHING. It says nothing
            // about who is standing here, and that is the whole point.
            Event::Prompt { .. } => {}
            Event::RoomSeen(room) => {
                if answers_look {
                    // Our look was answered — the ask is settled either
                    // way. A block naming somewhere ELSE is not this
                    // stop (the pump reads it as a flee); only the
                    // stop's own block is evidence about the stop.
                    self.pending_look = None;
                    if room.name == self.stop_name {
                        self.blind = false;
                        self.observed = Some(now);
                    }
                }
            }
            // An arrival is the ADDITIVE surprise, and it stays
            // conservative. Parse classifies async lines inside an
            // accumulating block, so a block's content can predate an
            // arrival that was EMITTED first (farm.rs's mid-render
            // race); reseeding from that block would drop the newcomer,
            // and no name matching would help — the parser has been
            // seen gluing a "(Resting)" marker onto an actor name.
            //
            // A DEPARTURE is subtractive, and the model takes the
            // leaver out of the occupant list without asking anyone.
            // Nothing else here is stale because of it.
            Event::ActorEntered { .. } => self.invalidate(),
            // A blow landing on US proves something is here that the block
            // may not have listed. Our own swings prove nothing.
            Event::CombatHit {
                target: crate::events::Actor::You,
                ..
            } => self.invalidate(),
            // A whiff aimed at us proves occupancy exactly like a landed
            // blow: the live rat behind this lunged twenty times without
            // connecting while the runner sat on a proven-empty verdict.
            // Whiff wordings are per-monster data, so no attacker name
            // can be trusted out of them — but the swing itself is
            // enough to re-ask.
            Event::CombatMiss { line } if whiff_at_us(line) => self.invalidate(),
            // A kill and the board's own fight-over announcement are
            // both SUBTRACTIVE, and neither is re-asked any more.
            //
            // `Here` takes the named corpse out of the occupant list —
            // once per kill, without a round-trip, where this used to
            // delete the whole observation and buy a `look` before the
            // next swing. A pack fight is nothing but kills, so that
            // was the round-trip being paid most often.
            //
            // What the model cannot subtract from is the experience
            // award, which names nobody. It is bounded rather than
            // handled: `recheck` is the shelf life on every observation
            // and no model error outlives one poke. That bound was
            // never optional — nothing announces a respawn either.
            //
            // What a death line DOES start is the board narrating the
            // outcome — see `resolving`.
            Event::Line(line)
                if crate::bot::is_kill_line(line) || crate::deaths::killed(line).is_some() =>
            {
                self.resolving.get_or_insert(now);
            }
            // "*Combat Off*" needs no invalidation of its own. It is
            // about the FIGHT, and it unlatches the thing that was
            // stuck: `Bot::engaged`, which outranks every other clause
            // in `verdict` and is what sat Busy on a corpse for 29s
            // live when a prose death and the untrained-XP cap hid both
            // end signals at once. Here it is the board's full stop:
            // the kill is narrated out and the stop may end.
            Event::Line(line) if crate::bot::is_combat_off(line) => self.resolving = None,
            // Our attack echoed back as SPEECH: the target left in the
            // race between the block and the swing, so the block that
            // prompted it describes a room that no longer exists. The
            // bot never speaks during a run, so any say echo is a
            // fallthrough.
            Event::Line(line) if line.starts_with("You say \"") => self.invalidate(),
            Event::Line(line)
                if line.contains(crate::sheet::TOO_DARK) && answers_look =>
            {
                // The answer to OUR look in an unlit room: no block is
                // coming, so nothing is still owed. A stale dark line
                // answering something else proves nothing about now.
                self.blind = true;
                self.pending_look = None;
            }
            _ => {}
        }
        // Every fold, not just the ones carrying a block. A kill that
        // empties the room announces itself in a death line and nothing
        // else — under stage 2 no `look` follows it, so this is the only
        // place the respawn budget can start.
        self.note_occupancy(bot, here, now);
    }

    /// Everything observed is dropped: a lagged broadcast, or a walk back
    /// from a flee. Nothing seen before it can be trusted.
    ///
    /// A lag happens exactly when a lot is going on — that is what
    /// overflows the channel — so it is precisely the busy room where
    /// carrying a stale "empty" across would walk out immediately.
    pub fn reset(&mut self) {
        self.observed = None;
        self.resolving = None;
        // Anything already in flight predates the reset.
        self.pending_look = None;
        self.empty_since = None;
        self.blind = false;
    }

    /// What to do now.
    ///
    /// Asked once per pump iteration, INCLUDING the iterations where no
    /// event arrived — that is where a respawn budget expires, so folding
    /// this into `on_event` would make it unreachable.
    pub fn verdict(
        &self,
        bot: &crate::bot::Bot,
        here: &crate::world::Here,
        now: Instant,
    ) -> Verdict {
        // An unfinished fight outranks everything. A block that raced the
        // blow which started the fight is not evidence it is over.
        if bot.engaged().is_some() {
            return Verdict::Busy;
        }
        // A flee has gone out and nothing has come back to say where it
        // landed, so the last block may describe a room we are no longer
        // standing in. Ending the stop here records a tidy dwell while the
        // character is somewhere else entirely, and the next leg then
        // starts from a lie -- caught by the live flee-recovery test,
        // which the old prompt counter was merely too slow to hit.
        if bot.fled() {
            return Verdict::Ask;
        }
        // An unanswered ask outranks Blind, and the order is load-bearing:
        // the light-recovery flow sends `light` + `look`, and the gate is
        // idle the moment the look's ECHO acks — one event before its
        // answer. Blind-first ended the stop right there, owing the lit
        // room's block, every time lighting worked. Waiting is bounded by
        // `recheck`; a still-dark answer re-enters Blind honestly.
        //
        // Each look supersedes the last in the correlator's eyes, so
        // re-asking while owed would orphan the in-flight answer and
        // loop — and flooding looks at the board was its own measured
        // failure. An OVERDUE ask (answer eaten or expired) falls
        // through to Ask, and the fresh id supersedes honestly.
        if let Some((_, at)) = self.pending_look {
            if now.duration_since(at) < self.recheck {
                return Verdict::Waiting { until: at + self.recheck };
            }
            return Verdict::Ask;
        }
        if self.blind {
            return Verdict::Blind;
        }
        let Some(observed) = self.observed else {
            return Verdict::Ask;
        };
        // Pure shelf life, and it is the one thing the model cannot
        // replace: nothing announces a respawn — `generate_monster`
        // simply puts a monster in the room — so silence is never proof
        // the model is still true, however well it folds what the board
        // does say. An old observation is re-asked, not trusted.
        if now.duration_since(observed) >= self.recheck {
            return Verdict::Ask;
        }
        // Asked of the MODEL, not of the block: `seen` above is only
        // the proof that this stop has been observed at all and how
        // long ago. Who is standing here now is `Here`'s answer, and it
        // has the kills, arrivals and departures since that block in
        // it. Judged against the CURRENT bot too — a target refused
        // since then no longer holds the stop.
        if bot.has_target_among(here.names()) {
            return Verdict::Busy;
        }
        // Nothing left to fight, but the board may still be saying what
        // the last kill dropped. Below `has_target` on purpose: a live
        // monster outranks a finished one's paperwork.
        if let Some(at) = self.resolving
            && now.duration_since(at) < self.recheck
        {
            return Verdict::Waiting { until: at + self.recheck };
        }
        match self.empty_since {
            Some(since) if now.duration_since(since) >= self.linger => Verdict::Empty,
            Some(since) => Verdict::Waiting {
                until: since + self.linger,
            },
            None => Verdict::Ask,
        }
    }
}

/// Is this whiff a monster swinging at US? Our own whiffs start with
/// "You" and prove nothing about occupancy; a bystander's fight ("Poop
/// swipes at kobold thief!") mentions neither "you" nor "your".
fn whiff_at_us(line: &str) -> bool {
    !line.starts_with("You") && crate::events::mentions_you(line)
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
    /// Fights picked mid-leg: the arrival block listed a target, or
    /// something the policy would fight walked in. Not emergencies:
    /// they never touch `travel_interrupts` and cannot end a run
    /// TooHurt.
    pub sightings: u32,
    /// Times the maintained room model ([`crate::world::Here`]) claimed
    /// something the board did not list. **The Stage-1 trust gate**: a
    /// poll is self-correcting and a model is not, so decisions may only
    /// be moved onto the model once a full run leaves this at (or very
    /// near) zero. No respawn can produce it.
    pub model_overclaims: u32,
    /// Times the board listed something the model had never heard of.
    /// Deliberately NOT a defect signal on its own — a silent respawn
    /// produces exactly this, and so does an unparsed movemsg wording.
    /// Only the trend, read against `model_overclaims`, means anything.
    pub model_surprises: u32,
    /// The distinct `kind name` pairs behind those counts, bounded. The
    /// actionable half: on a foreign board an unexplained name here IS a
    /// wording the parser cannot read.
    pub divergent_names: Vec<String>,
}

/// Enough to name the offenders without letting an hour-long run on a
/// badly-diverging board grow this without bound.
const DIVERGENT_NAME_CAP: usize = 32;

impl FarmStats {
    /// What the room model did this run, or `None` if it never once
    /// disagreed with the board.
    ///
    /// Stage 1 is a measurement, and a measurement nothing reports is
    /// not a measurement: the runner counted these all run while the
    /// TUI's end message said only kills and loops, so the `/farm`
    /// workflow produced no gate data at all.
    pub fn divergence_summary(&self) -> Option<String> {
        (self.model_overclaims > 0 || self.model_surprises > 0).then(|| {
            format!(
                "{} overclaims, {} surprises",
                self.model_overclaims, self.model_surprises
            )
        })
    }

    /// Fold every divergence recorded after `since_total` into the run's
    /// tally. The pump calls this per event, so it must count only what
    /// is new — re-counting would multiply each divergence by the number
    /// of events that happened to follow it.
    pub fn note_divergences(&mut self, rec: &crate::world::Reconcile, since_total: u32) {
        use crate::world::DivergenceKind::*;
        let fresh = rec.total().saturating_sub(since_total) as usize;
        if fresh == 0 {
            return;
        }
        let ring = rec.recent();
        for d in ring.iter().skip(ring.len().saturating_sub(fresh)) {
            match d.kind {
                OccupantExtra | PileExtra => self.model_overclaims += 1,
                OccupantMissing | PileMissing => self.model_surprises += 1,
            }
            let entry = format!("{} {}", d.kind.label(), d.name);
            if !self.divergent_names.contains(&entry)
                && self.divergent_names.len() < DIVERGENT_NAME_CAP
            {
                self.divergent_names.push(entry);
            }
        }
    }
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
/// The optional sighting bot is a PREDICATE, not a participant: it is
/// consulted through [`crate::bot::Bot::has_target`] and
/// [`crate::bot::Bot::would_attack`] and never fed `on_event` (feeding
/// it events while suppressing its commands would leave it believing it
/// had swung — see [`run_farm`]'s travel notes).
pub struct FarmGuard {
    max_hp: i32,
    hurt_at_percent: u32,
    username: String,
    /// Stop and fight when something swings at us, rather than walking on
    /// while it does. On unless the caller explicitly wants to run.
    fight_back: bool,
    /// Stop for rooms this bot would fight in. None (recover, the walk
    /// home, `fight_while_travelling = false`) sights nothing.
    sight: Option<crate::bot::Bot>,
}

impl FarmGuard {
    pub fn new(max_hp: i32, hurt_at_percent: u32, username: &str) -> Self {
        FarmGuard {
            max_hp,
            hurt_at_percent,
            username: username.to_string(),
            fight_back: true,
            sight: None,
        }
    }

    /// Stop for rooms this bot would fight in — the live incident was a
    /// leg walking straight through "Also here: angry kobold thief, thin
    /// giant rat, large kobold thief." while they attacked. The bot is
    /// consulted through `has_target` only, so the case rule, the ignore
    /// list, `auto_combat`, and the run's learned refusals all apply.
    pub fn sighting(mut self, bot: crate::bot::Bot) -> Self {
        self.sight = Some(bot);
        self
    }

    /// Walk past sightings for the rest of this leg. The runner calls
    /// this when a room it already defended is still listed — deadline
    /// expired, unkillable, or refused after the first swing — so the
    /// leg moves on instead of looping.
    pub fn stop_sighting(&mut self) {
        self.sight = None;
    }

    /// As [`FarmGuard::new`], but walk past a fight instead of taking it.
    ///
    /// For legs whose point is to get somewhere: the character still
    /// stops for the HP gate and for dying, it simply does not turn and
    /// swing at everything on the way.
    pub fn running(max_hp: i32, hurt_at_percent: u32, username: &str) -> Self {
        FarmGuard {
            fight_back: false,
            ..FarmGuard::new(max_hp, hurt_at_percent, username)
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
        FarmGuard::running(0, 0, username)
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
            // A blow landing on US. Our own swings are not an attack on
            // us, or the guard would trip on the defence it just asked
            // for and the leg would never advance.
            Event::CombatHit {
                attacker: crate::events::Actor::Other(name),
                target: crate::events::Actor::You,
                ..
            } if self.fight_back => Some(Interrupt::Attacked { by: name.clone() }),
            // A whiff aimed at us proves occupancy exactly like a landed
            // blow — the stop pump already lives by that rule (see
            // `StopState::on_event`), and run5 (2026-08-01) showed why
            // the walk must agree: a kobold thief lunged across three
            // rooms without connecting once, and a guard waiting for
            // CombatHit never fired. Whiff wordings are per-monster
            // data, so no attacker name is claimed out of them.
            //
            // NON-emergency, deliberately: no damage has landed, so
            // this is work noticed, not danger — an Entered, never an
            // Attacked. The first cut spent the interrupt budget here
            // and a shared swarm room ended a healthy run TooHurt in
            // minutes (cwgaming, 2026-08-01: every leg out of the
            // Arena was whiffed at three times). Landed damage still
            // spends budget via the CombatHit arm above.
            Event::CombatMiss { line } if self.fight_back && whiff_at_us(line) => {
                Some(Interrupt::Entered {
                    name: "something unseen".into(),
                })
            }
            // Something walked in mid-step. The name is only a
            // pre-filter through the same policy a sighting uses —
            // players are capitalised, refusals are shared run-wide —
            // and the defence decides off its own look, never off entry
            // wording. No sighting bot, no trip: the recovery walk and
            // the walk home keep walking, exactly as they ignore what
            // an arrival block lists.
            Event::ActorEntered { name, .. }
                if self.sight.as_ref().is_some_and(|b| b.would_attack(name)) =>
            {
                Some(Interrupt::Entered { name: name.clone() })
            }
            _ => None,
        }
    }

    fn on_room(&mut self, room: &crate::events::RoomView) -> Option<crate::nav::Interrupt> {
        let bot = self.sight.as_ref()?;
        // A listed coin pile is work exactly like a listed monster: the
        // defence pump this trips sweeps it (fight first if both), and
        // the pile's own listing is what proves it was worth stopping.
        // Walking past left money on the floor for whoever came next.
        (bot.has_target(room) || bot.has_loot(room))
            .then(|| crate::nav::Interrupt::Sighted { room: room.clone() })
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
    phase: PhaseSink<'_>,
) -> Result<(FarmEnd, FarmStats), FarmError> {
    // The board's own per-monster death wordings, so the room model can
    // see a kill somebody ELSE landed. Best effort: without it the model
    // falls back to the award-and-one-phrase test it always had.
    if let Err(e) = crate::deaths::init(&cfg.content) {
        eprintln!("death wordings unavailable ({e}); shared-room kills will be missed");
    }
    let mut light = crate::sheet::LightState::new(read_light_sources(session).await);
    if let Some(cmd) = light.first_command() {
        eprintln!("dark rooms will be handled with `{cmd}`");
    }
    let mut clock = crate::world::RoundClock::new();
    let out = farm_loop(
        session, graph, plan, bot_config, cfg, phase, &mut light, &mut clock,
    )
    .await;
    // A lit source burns one use per 3s medium tick whether anything
    // needs the light or not; walked away from, it spends the run's
    // whole burn budget on idle time. Best effort — a dead character
    // cannot remove anything, and Ctrl-C never reaches here at all.
    if let (Ok((end, _)), Some(cmd)) = (&out, light.extinguish())
        && !matches!(end, FarmEnd::Died)
    {
        session.send(&cmd);
    }
    out
}

#[allow(clippy::too_many_arguments)]
async fn farm_loop(
    session: &crate::session::Session,
    graph: std::sync::Arc<RoomGraph>,
    plan: &FarmPlan,
    bot_config: &crate::bot::BotConfig,
    cfg: &FarmConfig,
    phase: PhaseSink<'_>,
    light: &mut crate::sheet::LightState,
    clock: &mut crate::world::RoundClock,
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

    // Ask once what the character is carrying and what it can cast. The
    // answer decides whether a dark room is a dead end or a command away,
    // and both listings are cheap.
    verify_start(session, &graph, plan.start).await?;

    // One refusal set for the whole run. Learning that the board will not
    // let us hit a template is worth exactly one refused swing, not one
    // per stop per lap.
    let refusals = crate::bot::Refusals::default();

    let mut current = plan.start;
    loop {
        for &stop in &plan.circuit {
            if let Some(end) = time_up(started, cfg) {
                return Ok((end, stats));
            }
            // The leg's final step may have carried the stop's own block
            // out with it; the stop then starts from that evidence
            // instead of re-asking the board.
            let mut arrival = None;
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
                    &refusals,
                    light,
                    clock,
                    started,
                    &mut stats,
                    phase,
                )
                .await?
                {
                    LegEnd::Arrived { seen } => arrival = seen,
                    LegEnd::Died => return Ok((FarmEnd::Died, stats)),
                    LegEnd::TimeUp => return Ok((FarmEnd::TimeUp, stats)),
                    LegEnd::TooHurt => return Ok((FarmEnd::TooHurt, stats)),
                }
            }
            // A circuit stop normally runs on evidence alone; the cap is
            // for rooms that can never prove quiet — see stop_seconds.
            let until = (cfg.stop_seconds != 0)
                .then(|| Instant::now() + Duration::from_secs(cfg.stop_seconds));
            match farm_stop(
                session,
                &nav,
                &graph,
                stop,
                &bot_config,
                &threat,
                &refusals,
                light,
                clock,
                cfg,
                started,
                until,
                false,
                arrival,
                &mut stats,
                phase,
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
    let ask = session.send("look");
    // A dark room answers `look` with "you can't see anything" and no
    // room block at all, so a walk home that insisted on one could never
    // start from the very rooms most likely to strand a character. Light
    // it first if we can, then ask again. No sleep between the light and
    // the second look: the board answers in send order, so the look's
    // attributed answer necessarily postdates the light taking effect.
    let seen = match next_room_view(&mut events, ask, Duration::from_secs(15)).await {
        Some(room) => room,
        None => {
            // Re-derived HERE, not carried in: the commonest way a farm
            // ends is Ctrl-C, which cancels run_farm and drops its
            // LightState outright — a parameter could never cover the
            // exit route that matters most.
            let sources = read_light_sources(session).await;
            let Some(cmd) = sources.first().map(|s| s.command().to_string()) else {
                return Err(FarmError::NotAtStart {
                    expected: "a room block answering the finish walk's look".into(),
                    saw: None,
                });
            };
            session.send(&cmd);
            let again = session.send("look");
            next_room_view(&mut events, again, Duration::from_secs(15))
                .await
                .ok_or(FarmError::NotAtStart {
                    expected: "a room block answering the finish walk's look".into(),
                    saw: None,
                })?
        }
    };
    if let Some(here) = graph.room(finish)
        && here.name == seen.name
    {
        return Ok(());
    }
    let hint = plan.circuit.last().copied().unwrap_or(plan.start);
    let at = nav
        .localize_view(hint, &seen)
        .ok_or(FarmError::Lost { saw: seen.name })?;
    // Fights its way home rather than only stopping for death: the board
    // refuses movement while in combat, so a running guard would simply
    // be stuck wherever something picked a fight.
    let mut guard = FarmGuard::new(0, 0, &session.profile().username);
    nav.goto(session, at, finish, &mut guard)
        .await
        .map(|_| ())
        .map_err(FarmError::Nav)
}

/// The next room block in full, not just its name.
async fn next_room_view(
    events: &mut tokio::sync::broadcast::Receiver<Correlated>,
    answering: crate::correlate::CmdId,
    within: Duration,
) -> Option<crate::events::RoomView> {
    let deadline = tokio::time::Instant::now() + within;
    loop {
        match tokio::time::timeout_at(deadline, events.recv()).await {
            Ok(Ok(Correlated { event: Event::RoomSeen(room), answers }))
                if answers == Some(answering) =>
            {
                return Some(room);
            }
            Ok(Ok(_)) => continue,
            Ok(Err(tokio::sync::broadcast::error::RecvError::Lagged(_))) => continue,
            Ok(Err(_)) | Err(_) => return None,
        }
    }
}

/// Ask the board for the inventory and the spellbook, and work out
/// every way this character could light a dark room.
///
/// Empty means it cannot, which is worth knowing up front rather than
/// discovering at the mouth of an unlit room.
async fn read_light_sources(session: &crate::session::Session) -> Vec<crate::sheet::LightSource> {
    let inventory = ask(session, "inventory", "Encumbrance:").await;
    // No terminal wording is pinned for the spell listing, so the
    // collection is bounded by a short deadline instead of the full 10s
    // — this runs at every farm start and on dark finish walks.
    let spells = ask_for(session, "spells", "", Duration::from_secs(3)).await;
    crate::sheet::light_sources(
        &crate::sheet::Inventory::parse(&inventory),
        &crate::sheet::Spellbook::parse(&spells),
    )
}

/// Send a listing command and collect what comes back.
/// Deliberately NOT attribution-filtered: this collects a multi-line
/// listing from an Opaque-kind command (inventory, spells), and the
/// correlator attributes no line of it — filtering would return nothing.
/// Transcript-style collection bounded by `until` and the deadline is
/// the honest tool here.
async fn ask(session: &crate::session::Session, cmd: &str, until: &str) -> String {
    ask_for(session, cmd, until, Duration::from_secs(10)).await
}

async fn ask_for(
    session: &crate::session::Session,
    cmd: &str,
    until: &str,
    within: Duration,
) -> String {
    let mut events = session.events();
    crate::session::drain(&mut events, |_| {});
    session.send(cmd);
    let deadline = tokio::time::Instant::now() + within;
    let mut out = String::new();
    loop {
        match tokio::time::timeout_at(deadline, events.recv()).await {
            Ok(Ok(Correlated { event: Event::Line(line), .. })) => {
                let done = !until.is_empty() && line.contains(until);
                out.push_str(&line);
                out.push('\n');
                if done {
                    break;
                }
            }
            Ok(Ok(_)) => continue,
            Ok(Err(tokio::sync::broadcast::error::RecvError::Lagged(_))) => continue,
            Ok(Err(_)) | Err(_) => break,
        }
    }
    out
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
    let ask = session.send("look");
    // Attributed: a login-banner render or anybody's stale block can
    // never satisfy start verification.
    let saw = next_room_view(&mut events, ask, Duration::from_secs(15))
        .await
        .map(|r| r.name);
    match saw {
        Some(name) if name == expected => Ok(()),
        saw => Err(FarmError::NotAtStart { expected, saw }),
    }
}

/// How a leg ended.
enum LegEnd {
    /// At the stop. `seen` is the attributed arrival block when the
    /// final step both described the stop and listed something worth
    /// fighting — evidence the stop pump can start from instead of
    /// re-asking the board.
    Arrived { seen: Option<crate::events::RoomView> },
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
    refusals: &crate::bot::Refusals,
    light: &mut crate::sheet::LightState,
    clock: &mut crate::world::RoundClock,
    started: Instant,
    stats: &mut FarmStats,
    phase: PhaseSink<'_>,
) -> Result<LegEnd, FarmError> {
    use crate::nav::{Interrupt, NavErrorKind};
    set_phase(phase, Phase::WaitingToDepart);

    let mut guard = if cfg.fight_while_travelling {
        // Sighting rides the same intent switch: "take fights on the
        // way" covers a monster the arrival block LISTS, not only one
        // that has already drawn blood. The bot is a predicate — never
        // fed events — and shares the run's refusals, so a template the
        // board refused stops tripping legs run-wide.
        FarmGuard::new(
            bot_config.max_hp,
            cfg.interrupt_at_percent,
            &session.profile().username,
        )
        .sighting(crate::bot::Bot::with_refusals(
            bot_config.clone(),
            threat.clone(),
            refusals.clone(),
        ))
    } else {
        FarmGuard::running(
            bot_config.max_hp,
            cfg.interrupt_at_percent,
            &session.profile().username,
        )
    };
    let mut budget = cfg.travel_interrupts;
    // The one room this leg already defended on a sighting or an entry.
    // A second trip there means the defence did not clear it — deadline
    // expired, unkillable, or refused mid-fight — and stopping again
    // would loop, so the leg walks on from then on: `stop_sighting`
    // clears the predicate bot, which mutes entry arming too.
    let mut last_sighted: Option<RoomId> = None;
    // Predicate-only, like the guard's sighting bot: judges whether the
    // departure gate is standing beside work (never fed events).
    let sight = crate::bot::Bot::with_refusals(bot_config.clone(), threat.clone(), refusals.clone());

    loop {
        if time_up(started, cfg).is_some() {
            return Ok(LegEnd::TimeUp);
        }
        // A rest interrupted by an arrival defends where it stands, with
        // the same non-emergency machinery as a mid-step entry: the pump
        // clears the room, and the loop re-enters the wait to rest in
        // the quiet it made. The `last_sighted` memo breaks the cycle
        // when the defence cannot clear it (unkillable, refused): the
        // leg then departs wounded, which is what the travel guard is
        // for. A leg that walks past fights on purpose
        // (`fight_while_travelling = false`) departs wounded directly.
        if let DepartureWait::Contested =
            wait_for_departure_health(session, cfg, bot_config, &sight).await
            && cfg.fight_while_travelling
            && last_sighted != Some(*current)
        {
            last_sighted = Some(*current);
            stats.sightings += 1;
            let until = Instant::now() + Duration::from_secs(cfg.defend_seconds);
            match farm_stop(
                session, nav, graph, *current, bot_config, threat, refusals, light, clock, cfg,
                started, Some(until), true, None, stats, phase,
            )
            .await?
            {
                StopEnd::Dwelt => continue,
                StopEnd::Died => return Ok(LegEnd::Died),
                StopEnd::TimeUp => return Ok(LegEnd::TimeUp),
            }
        }
        // Light up BEFORE stepping into known darkness, standing still
        // where the outcome is verifiable. On failure the walk proceeds
        // blind — today's behavior, now the explicit fallback. A fade
        // mid-leg still walks the remaining dark steps blind and is
        // caught at the stop by the Blind verdict + recast.
        if !light.lit() && leg_needs_light(graph, *current, stop) {
            ensure_lit(session, light, clock).await;
        }
    set_phase(phase, Phase::Travelling { to: stop });

        let err = match nav.goto(session, *current, stop, &mut guard).await {
            Ok(at) => {
                *current = at;
                return Ok(LegEnd::Arrived { seen: None });
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
            // The arrival block listed something worth fighting, or
            // something the policy would fight walked in mid-step. NOT
            // an emergency: it never touches the interrupt budget and
            // can never end a run TooHurt — it is the farm noticing
            // work, where Hurt/Attacked are the farm noticing danger.
            NavErrorKind::Interrupted(
                int @ (Interrupt::Sighted { .. } | Interrupt::Entered { .. }),
            ) => {
                // Sighted carries the attributed block that tripped it;
                // an entry carries none (its wording is per-monster
                // data), so the defence below opens by asking the board
                // where it stands.
                let room = match int {
                    Interrupt::Sighted { room } => Some(room),
                    _ => None,
                };
                // The destination itself: this IS the stop. Hand the
                // evidence up so the stop starts from it.
                if err.at == stop {
                    return Ok(LegEnd::Arrived { seen: room });
                }
                if last_sighted == Some(err.at) {
                    guard.stop_sighting();
                    continue;
                }
                last_sighted = Some(err.at);
                stats.sightings += 1;
                let until = Instant::now() + Duration::from_secs(cfg.defend_seconds);
                match farm_stop(
                    session,
                    nav,
                    graph,
                    err.at,
                    bot_config,
                    threat,
                    refusals,
                    light,
                    clock,
                    cfg,
                    started,
                    Some(until),
                    true,
                    room,
                    stats,
                    phase,
                )
                .await?
                {
                    StopEnd::Dwelt => continue,
                    StopEnd::Died => return Ok(LegEnd::Died),
                    StopEnd::TimeUp => return Ok(LegEnd::TimeUp),
                }
            }
            // Being swung at is handled exactly like being hurt: stop,
            // clear the room with the pump that already knows how to
            // fight, then resume the leg from where we stand.
            NavErrorKind::Interrupted(Interrupt::Attacked { .. })
            | NavErrorKind::Interrupted(Interrupt::Hurt { .. }) => {
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
                    refusals,
                    light,
                    clock,
                    cfg,
                    started,
                    Some(until),
                    true,
                    None,
                    stats,
                    phase,
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

/// The recast coherence rule, pure for testing: a faded light waits
/// while the room holds a monster the bot would fight. Same policy as
/// rest-safety — an occupied room gets the fight first (fighting blind
/// is already the state we are in, and a mid-melee cast would be
/// refused anyway); light comes when the room is cleared.
pub fn recast_waits_for(bot: &crate::bot::Bot, here: &crate::world::Here) -> bool {
    here.occupants
        .iter()
        .any(|o| matches!(o.kind, crate::world::OccupantKind::Monster) && bot.would_attack(&o.name))
}

/// Does this leg cross (or end in) a room the graph marks dark? Decided
/// from the same route goto will compute (BFS is deterministic), so the
/// walk can light up BEFORE stepping into darkness — standing still,
/// where the cast outcome is verifiable — instead of bouncing out of
/// the dark and retrying blind, which was the live lap's shape.
pub fn leg_needs_light(graph: &RoomGraph, from: RoomId, to: RoomId) -> bool {
    let Some(route) = graph.route(from, to) else {
        return false;
    };
    let mut at = from;
    for d in route {
        let Some(next) = graph
            .room(at)
            .and_then(|r| r.exits[d as usize].as_ref())
            .map(|e| e.dest)
        else {
            return false;
        };
        if graph.dark(next) {
            return true;
        }
        at = next;
    }
    false
}

/// Light up before a dark leg, standing still. Runs the attempt/outcome
/// cycle directly (no gate: the walk does not own the connection yet),
/// retrying fizzles on the round and giving up honestly when nothing
/// can work — proceeding blind is then the explicit fallback, exactly
/// today's behavior. Bounded by a hard deadline so a lost outcome can
/// never wedge a leg.
async fn ensure_lit(
    session: &crate::session::Session,
    light: &mut crate::sheet::LightState,
    clock: &crate::world::RoundClock,
) {
    if light.lit() {
        return;
    }
    let mut events = session.events();
    crate::session::drain(&mut events, |_| {});
    let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
    loop {
        if light.lit() || tokio::time::Instant::now() >= deadline {
            return;
        }
        match light.attempt(Instant::now(), clock) {
            crate::sheet::LightAttempt::Send(cmd) => {
                let id = session.send(&cmd);
                light.on_sent(&cmd, id);
            }
            crate::sheet::LightAttempt::Hold(_) => {}
            crate::sheet::LightAttempt::Nothing if !light.in_flight() => return,
            crate::sheet::LightAttempt::Nothing => {}
        }
        match tokio::time::timeout(Duration::from_millis(300), events.recv()).await {
            Ok(Ok(cor)) => light.on_event(&cor),
            Ok(Err(tokio::sync::broadcast::error::RecvError::Lagged(_))) => continue,
            Ok(Err(_)) => return,
            Err(_) => {}
        }
    }
}

/// How the departure wait ended: fit to walk (or past caring — the
/// deadline expired), or standing beside work that has to be dealt
/// with before resting can mean anything.
enum DepartureWait {
    Fit,
    Contested,
}

/// Hold at the stop until HP is fit to travel. The bot is not driving
/// while the navigator walks, so setting off wounded means relying on
/// the travel guard to stop the leg part-way — cheaper to leave fit.
async fn wait_for_departure_health(
    session: &crate::session::Session,
    cfg: &FarmConfig,
    bot_config: &crate::bot::BotConfig,
    // Judges whether the room we are standing in holds work; the same
    // predicate-only role the sighting guard's bot plays.
    sight: &crate::bot::Bot,
) -> DepartureWait {
    if cfg.depart_at_percent == 0 || bot_config.max_hp <= 0 {
        return DepartureWait::Fit;
    }
    let target = bot_config.max_hp * cfg.depart_at_percent as i32 / 100;
    let mut state = session.state();
    // Never rest beside a monster — and keep never doing it for the
    // whole wait, not only at its door. The one-shot version of this
    // check rested a live character at 12 HP beside a giant rat (run6,
    // 2026-08-01): the rat's block landed one beat after the check, and
    // the loop below then watched nothing but the HP number for up to
    // `max_rest_seconds` while the room chewed on it — the more damage
    // landed, the longer it stayed. No phase consumes events here, but
    // none is needed: the HP pokes keep `GameState.room` fresh, so the
    // occupancy answer is already in hand on every pass. Unattributed,
    // so it is a heuristic gate on a best-effort send — the caller's
    // defence, not this check, is what actually clears the room.
    let mut sent_heal = false;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(cfg.max_rest_seconds);
    let poke = Duration::from_millis(cfg.idle_poke_ms.max(1000));
    loop {
        let (hp, contested) = {
            let s = state.borrow();
            (
                s.hp,
                s.room.as_ref().is_some_and(|room| sight.has_target(room)),
            )
        };
        // Fit first: a healthy character standing in a room it just
        // decided to leave (a capped stop that never went quiet) walks
        // out — re-defending there would un-make the cap's decision.
        // The travel guard covers whatever follows it out.
        if hp >= target && hp > 0 {
            return DepartureWait::Fit;
        }
        // A downed character cannot rest its way back over the gate;
        // hand it to the defence pump, whose death handling is the one
        // that knows what a negative HP prompt means.
        if contested || hp <= 0 {
            return DepartureWait::Contested;
        }
        if tokio::time::Instant::now() >= deadline {
            return DepartureWait::Fit;
        }
        // Actually REST, and actually look.
        //
        // This used to watch `hp` and wait. Two things made that
        // useless on a live board: nothing asked the character to heal,
        // and an idle board sends no prompts at all — so GameState
        // never changed and the watch could not observe recovery even
        // if it happened. It was a 120-second sleep that then departed
        // at whatever HP it started with.
        if !sent_heal {
            session.send(&bot_config.heal_command);
            sent_heal = true;
        }
        // A poke is what produces the prompt that carries HP; without one
        // there is nothing to observe.
        match tokio::time::timeout(poke, state.changed()).await {
            Ok(Ok(())) => {}
            Ok(Err(_)) => return DepartureWait::Fit,
            Err(_) => {
                // The poke exists only to provoke a prompt that carries
                // HP into GameState; its own answer is irrelevant, so no
                // attribution is needed (and none is read). That the
                // answer ALSO refreshes the room is what feeds the
                // occupancy check above.
                session.send("look");
            }
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
    // Kept by the RUN, not the stop: a bot is rebuilt per stop and on
    // every lag and recovery, and a forgotten refusal is a refused swing
    // repeated -- a crime-system interaction on the live board.
    refusals: &crate::bot::Refusals,
    light: &mut crate::sheet::LightState,
    clock: &mut crate::world::RoundClock,
    cfg: &FarmConfig,
    started: Instant,
    // Hard cap on this stop, or None to stay until it goes quiet.
    until: Option<Instant>,
    // A defence (travel interrupt) rather than a circuit stop. A blind
    // defence waits out its deadline — walking on would leave whatever
    // is hitting us behind us — where a blind circuit stop leaves at
    // once. Used to key on `until.is_none()`, which stopped meaning
    // "circuit stop" the day circuit stops could carry a cap.
    defending: bool,
    // The attributed block the leg's final step carried in, when it
    // described this stop. Seeds the evidence so the first swing goes
    // out without an opening look.
    arrival: Option<crate::events::RoomView>,
    stats: &mut FarmStats,
    phase: PhaseSink<'_>,
) -> Result<StopEnd, FarmError> {
    let stop_name = graph.room(stop).map(|r| r.name.clone()).unwrap_or_default();
    let mut resting = false;
    // The stop went blind at least once this visit: the room needs
    // light whatever the graph believes, which arms the recast gate for
    // rooms the graph mislabels.
    let mut was_blind_this_visit = false;
    light.new_visit();
    let username = session.profile().username.clone();
    let backoff = Duration::from_millis(cfg.slowdown_backoff_ms);
    let poke_after = Duration::from_millis(cfg.idle_poke_ms);

    // Flee round trips tolerated before the stop is written off. Without
    // this a character that flees on every prompt never leaves the room
    // pair it is bouncing between.
    let mut recoveries_left = 3u32;

    let mut events = session.events();
    crate::session::drain(&mut events, |_| {});

    let mut bot = crate::bot::Bot::with_refusals(bot_config.clone(), threat.clone(), refusals.clone());
    let mut gate = Gate::new(backoff);
    let mut heal = HealWatch::new(bot_config, cfg);
    let mut seen = StopState::new(stop_name.clone(), cfg);
    // The maintained room state — fed the same stream, one fold. Its
    // first consumer is the recast coherence gate; StopState keeps its
    // own hard-won evidence rules untouched until Here earns collapse.
    let mut here = crate::world::Here::default();
    here.room = Some(stop);

    // The traveller's arrival block, believed under the same rule the
    // pump applies at farm.rs's bot_sees: it was ATTRIBUTED — to the
    // leg's final step rather than to a look — so the bot is shown it
    // (engaging anything listed) and the state accepts it as evidence.
    // Bot first, then state, matching on_event's contract. The window
    // between goto accepting the block and the subscribe above can drop
    // an ActorEntered — the same race the opening look always had — and
    // the `recheck` shelf life forces the re-ask that bounds it.
    if let Some(room) = arrival {
        let now = Instant::now();
        for crate::bot::BotAction::Send(cmd) in bot.on_event(&Event::RoomSeen(room.clone())) {
            gate.push(cmd);
        }
        // The model is seeded from the arrival too, and must be before
        // the state reads it. It was ATTRIBUTED (to the leg's final
        // step), which is exactly what `Here` believes; leaving it out
        // left the model unseeded until the first look came back, and
        // an unseeded model answers "no evidence" to every question the
        // arrival block had already settled.
        here.on_event(
            &crate::correlate::Correlated {
                event: Event::RoomSeen(room.clone()),
                answers: Some(crate::correlate::CmdId(0)),
            },
            now,
        );
        seen.seed(room, &bot, &here, now);
    }

    loop {
        if time_up(started, cfg).is_some() {
            return Ok(StopEnd::TimeUp);
        }
        // Defending is capped: see FarmConfig::defend_seconds for why a
        // stop that cannot go quiet must still end.
        if until.is_some_and(|d| Instant::now() >= d) {
            return Ok(StopEnd::Dwelt);
        }

        // What the stop's own evidence says to do, decided BEFORE waiting
        // on anything. It has to be reachable on the iterations where no
        // event arrives, because that is where a respawn budget expires
        // and where an unanswered `look` gets asked again. The opening
        // `look` that seeds the bot is just the first `Ask`.
        let now = Instant::now();
        let verdict = seen.verdict(&bot, &here, now);
        if std::env::var("MMC_TRACE").is_ok() {
            eprintln!("TRACE stop={stop:?} verdict={verdict:?} engaged={:?} model={:?} seeded={}", bot.engaged(), here.names().collect::<Vec<_>>(), here.seeded());
        }
        let mut hold_until = None;
        match &verdict {
            // The bot is driving; nothing for the runner to decide.
            Verdict::Busy => {}
            Verdict::Waiting { until } => hold_until = Some(*until),
            Verdict::Ask => {
                if gate.is_idle() {
                    gate.push("look".into());
                }
            }
            // Never while we still owe the board something: the `get` for
            // the coins the last kill dropped is what would be lost.
            Verdict::Empty => {
                if gate.is_idle() {
                    return Ok(StopEnd::Dwelt);
                }
            }
            Verdict::Blind => {
                // Blind again while a source was believed burning: it
                // burned out (item) or faded unnoticed (spell). The
                // wordings arrive unsolicited and LightState reads them
                // directly; the darkness returning is the backstop for
                // one that was missed.
                light.source_died();
                was_blind_this_visit = true;
                match light.attempt(now, clock) {
                    crate::sheet::LightAttempt::Send(cmd) if gate.is_idle() => {
                        gate.push(cmd);
                        gate.push("look".into());
                    }
                    // An attempt was spent this round; a second cast
                    // inside one is refused anyway. Wait it out — this
                    // is what turns "Blind + no attempt = dead end"
                    // into "Blind + Hold = retry next round", the live
                    // 15-dark-encounters-3-casts bug.
                    crate::sheet::LightAttempt::Hold(next) => {
                        hold_until = Some(hold_until.map_or(next, |h: Instant| h.min(next)));
                    }
                    // A stop we cannot see is a stop we cannot farm, and
                    // fighting in the dark is heavily penalised anyway.
                    // Defending is the exception: there the deadline
                    // governs, or we walk on and leave whatever is
                    // hitting us behind.
                    crate::sheet::LightAttempt::Nothing if !defending && gate.is_idle() => {
                        return Ok(StopEnd::Dwelt);
                    }
                    _ => {}
                }
            }
        }

        // A fade is an indicator to RECAST, not bookkeeping (operator
        // directive): it arrives unsolicited mid-anything, and waiting
        // for the next look to come back "too dark" costs a blind
        // round-trip. Priority: combat > recast > look — a recast never
        // preempts a fight (the board refuses casts mid-round anyway),
        // and it needs no look after it: the fade does not stale the
        // room evidence. A fade in a naturally lit room needs no action.
        if bot.engaged().is_none()
            && gate.is_idle()
            && light.wants_recast()
            && (graph.dark(stop) || was_blind_this_visit)
            && !recast_waits_for(&bot, &here)
            && let crate::sheet::LightAttempt::Send(cmd) = light.attempt(now, clock)
        {
            gate.push(cmd);
        }

        set_phase(
            phase,
            match bot.engaged() {
                Some(target) => {
                    resting = false;
                    Phase::Fighting {
                        at: stop,
                        target: target.to_string(),
                    }
                }
                None if resting => Phase::Resting { at: stop },
                None => Phase::Waiting { at: stop },
            },
        );

        // Release whatever the gate is willing to send.
        while let Some(cmd) = gate.poll(now) {
            heal.on_sent(&cmd);
            let id = session.send(&cmd);
            gate.confirm(id);
            seen.on_sent(&cmd, id);
            light.on_sent(&cmd, id);
        }

        // Sleep until the next event, the gate's own deadline, the idle
        // poke, or a respawn budget expiring — whichever comes first.
        let mut wake = gate
            .next_deadline()
            .map(|d| d.min(now + poke_after))
            .unwrap_or(now + poke_after);
        if let Some(hold) = hold_until {
            wake = wake.min(hold);
        }
        let wake = tokio::time::Instant::from_std(wake);

        let cor = match tokio::time::timeout_at(wake, events.recv()).await {
            Ok(Ok(cor)) => Some(cor),
            // Dropped events desync a stateful bot: it can miss the death
            // that ends a fight and sit latched on a corpse. Start over
            // rather than carry on with a bot that quietly lost track.
            //
            // The stop's evidence goes with it. A lag happens precisely
            // because a lot is going on, which is the busy room where
            // carrying a stale "nothing here" across would walk out on
            // everything still standing in it.
            Ok(Err(tokio::sync::broadcast::error::RecvError::Lagged(_))) => {
                bot = crate::bot::Bot::with_refusals(bot_config.clone(), threat.clone(), refusals.clone());
                gate = Gate::new(backoff);
                seen.reset();
                here.reset();
                here.room = Some(stop);
                None
            }
            Ok(Err(_)) => return Err(FarmError::Disconnected),
            // Nothing arrived. The verdict at the top of the next
            // iteration decides what to make of that.
            Err(_) => None,
        };
        let Some(cor) = cor else { continue };
        let ev = &cor.event;

        if let Event::SlowDown = ev {
            stats.slowdowns += 1;
        }
        // Combat lines arrive in bursts on round boundaries; every one
        // phase-locks the clock the lighting retries pace themselves by.
        if matches!(ev, Event::CombatHit { .. } | Event::CombatMiss { .. }) {
            clock.observe(Instant::now());
        }
        if let Event::Line(line) = &ev {
            if is_player_death(line, &username) {
                return Ok(StopEnd::Died);
            }
            // Counted on the award, NOT on the death phrase, and never on
            // both: one kill prints both lines, and the phrase alone
            // appears in only 67 of the 1085 shipped death records, so
            // this had been undercounting by roughly 94%.
            //
            // The meaning narrows deliberately -- "kills that paid us
            // experience". A monster somebody else finished in the same
            // room no longer counts, which is what the number was always
            // supposed to mean.
            if crate::progress::is_exp_award(line) {
                stats.kills += 1;
            }
        }
        if let Event::Prompt { hp, .. } = ev
            && *hp <= 0
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
            // Back at the stop with a clean slate. Nothing observed
            // before the flee describes the room we are standing in now.
            events = session.events();
            bot = crate::bot::Bot::with_refusals(bot_config.clone(), threat.clone(), refusals.clone());
            gate = Gate::new(backoff);
            heal = HealWatch::new(bot_config, cfg);
            seen = StopState::new(stop_name.clone(), cfg);
            here.reset();
            here.room = Some(stop);
            continue;
        }

        gate.on_event(&cor, Instant::now());
        if heal.on_event(ev) {
            bot.rearm();
        }

        // The bot is attribution-blind by design (a pure Event core),
        // so the pump curates: an UNSOLICITED render — somebody else's
        // block, a stale answer — must not touch its latches. It used
        // to clear `engaged` mid-fight when a pre-arrival block lacked
        // the target, then duplicate the attack on the re-ask. Async
        // truths (combat, arrivals, prompts) pass through untouched.
        let bot_sees = !matches!(ev, Event::RoomSeen(_)) || cor.answers.is_some();
        let actions = if bot_sees { bot.on_event(ev) } else { Vec::new() };
        for crate::bot::BotAction::Send(cmd) in actions {
            // The heal command is the only way to tell resting from
            // simply standing about; the bot's own debounce is private.
            if cmd == bot_config.heal_command {
                resting = true;
            }
            gate.push(cmd);
        }
        light.on_event(&cor);
        // The model folds BEFORE the stop state, because the stop state
        // now decides occupancy by asking it. The tally still runs —
        // the counters that earned `Here` this job are what would catch
        // it losing them.
        let seen_divergences = here.reconcile.total();
        here.on_event(&cor, Instant::now());
        stats.note_divergences(&here.reconcile, seen_divergences);
        // Folded last, so `engaged` and the occupant list already
        // account for this event when the next iteration asks for a
        // verdict.
        seen.on_event(&cor, &bot, &here, Instant::now());
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
