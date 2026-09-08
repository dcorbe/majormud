//! `/recover`: sneak to the room the character died in, search once,
//! pick up everything the search lists, and run back to where the job
//! started. It never attacks, and it turns for home at the first broken
//! sneak.
//!
//! The run home needs no sneak. Aggressive monsters acquire a target
//! inside the combat round and skip a player who moved this round, and
//! pursuit refuses the same player, so a character that keeps moving is
//! neither acquired nor followed. What it needs is no pauses, which is
//! why the walk home is unsneaked: arming costs a round standing still.
//!
//! The pure parts, what the sweep asks for and how an ending reads, sit
//! above the job so they are tested without a board.

use std::sync::Arc;
use std::time::Duration;

use mud_core::content::RoomId;

use crate::bot::BotConfig;
use crate::events::Event;
use crate::farm::{FarmError, Live, Notices, Phase, PhaseSink, set_phase};
use crate::graph::RoomGraph;
use crate::nav::{Interrupt, NavErrorKind, Navigator};
use crate::session::Session;

/// One thing the sweep will ask for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Take {
    /// The entry as the board listed it, for the report.
    pub label: String,
    /// The `get` that asks for it.
    pub cmd: String,
    pub kind: TakeKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TakeKind {
    Gear,
    Coins,
}

/// The "You notice ... here." entries as `get` commands, gear first and
/// coins last, so a forced exit leaves coins behind rather than
/// equipment.
///
/// A coin pile is taken by denomination, as `get` takes it. Everything
/// else is an item, taken by its printed name with the article dropped:
/// the board matches by word prefix and the printed name is what it
/// printed, so nothing is gained by resolving it first.
pub fn sweep_list(items: &[String]) -> Vec<Take> {
    let mut gear = Vec::new();
    let mut coins = Vec::new();
    for entry in items {
        let entry = entry.trim();
        if entry.is_empty() {
            continue;
        }
        match crate::bot::coin_pile(entry) {
            Some((_, denom)) => coins.push(Take {
                label: entry.to_string(),
                cmd: format!("get {denom}"),
                kind: TakeKind::Coins,
            }),
            None => gear.push(Take {
                label: entry.to_string(),
                cmd: format!("get {}", strip_article(entry)),
                kind: TakeKind::Gear,
            }),
        }
    }
    gear.extend(coins);
    gear
}

fn strip_article(entry: &str) -> &str {
    for article in ["a ", "an ", "the "] {
        if let Some(rest) = entry.strip_prefix(article) {
            return rest.trim();
        }
    }
    entry
}

/// What one line of a `get` reply said.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GetReply {
    /// The board confirmed the pickup, coins or item.
    Taken,
    /// Somebody else got there first: "You don't see <name> here." or
    /// "You don't see any <plural>". Excludes `rob`'s missing-target
    /// refusal, "You don't see that anywhere!", which shares no `get`.
    Gone,
    Other,
}

pub fn read_get_reply(line: &str) -> GetReply {
    if crate::bot::picked_up(line).is_some() || crate::bot::picked_up_item(line).is_some() {
        return GetReply::Taken;
    }
    let lower = line.to_lowercase();
    if (lower.contains("you don't see") && lower.contains(" here"))
        || lower.contains("you don't see any ")
    {
        return GetReply::Gone;
    }
    GetReply::Other
}

/// What the sweep asked for and what it got.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Haul {
    /// The entries taken, as the board listed them, in the order taken.
    pub taken: Vec<String>,
    pub items_wanted: usize,
    pub items_taken: usize,
    pub coins_wanted: usize,
    pub coins_taken: usize,
}

impl Haul {
    pub fn wanted(list: &[Take]) -> Haul {
        Haul {
            items_wanted: list.iter().filter(|t| t.kind == TakeKind::Gear).count(),
            coins_wanted: list.iter().filter(|t| t.kind == TakeKind::Coins).count(),
            ..Haul::default()
        }
    }

    pub fn took(&mut self, take: &Take) {
        match take.kind {
            TakeKind::Gear => self.items_taken += 1,
            TakeKind::Coins => self.coins_taken += 1,
        }
        self.taken.push(take.label.clone());
    }

    /// `3 of 7 items`, with ` and 2 coin piles` when any coins were
    /// listed.
    pub fn summary(&self) -> String {
        let mut s = format!("{} of {} items", self.items_taken, self.items_wanted);
        if self.coins_wanted > 0 {
            s.push_str(&format!(" and {} coin piles", self.coins_taken));
        }
        s
    }
}

/// Why the character came home.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HomeWhy {
    Swept,
    /// The search listed nothing.
    Nothing,
    /// A hop on the way in arrived without `Sneaking...`.
    Broke { at: RoomId, name: String },
    /// The board refused a move on the way in because something had
    /// the character in combat.
    Attacked { at: RoomId, name: String },
    /// Hitpoints fell under the minor heal mark during the sweep.
    Hurt { mark: u32 },
}

/// How the job ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RecoverEnd {
    /// Back in the start room.
    Home { at: RoomId, why: HomeWhy, haul: Haul },
    /// The walk home did not complete. Where the character stands.
    Stopped { at: RoomId, name: String, haul: Haul },
    Died { haul: Haul },
}

impl RecoverEnd {
    pub fn haul(&self) -> &Haul {
        match self {
            RecoverEnd::Home { haul, .. }
            | RecoverEnd::Stopped { haul, .. }
            | RecoverEnd::Died { haul } => haul,
        }
    }

    /// The ending as the bar and the lobby read it.
    pub fn phase(&self) -> Phase {
        match self {
            RecoverEnd::Home { at, why, haul } => Phase::Done {
                why: match why {
                    HomeWhy::Swept => format!("recovered {}", haul.summary()),
                    HomeWhy::Nothing => "nothing there".into(),
                    HomeWhy::Broke { at, name } => {
                        format!("sneak broke at {}/{} {name}, nothing taken", at.map, at.room)
                    }
                    HomeWhy::Attacked { at, name } => {
                        format!("attacked at {}/{} {name}, nothing taken", at.map, at.room)
                    }
                    HomeWhy::Hurt { mark } => {
                        format!("hurt under {mark}%, back with {}", haul.summary())
                    }
                },
                at: Some(*at),
            },
            RecoverEnd::Stopped { at, name, haul } => Phase::Done {
                why: format!(
                    "stopped at {}/{} {name} with {}",
                    at.map,
                    at.room,
                    haul.summary()
                ),
                at: Some(*at),
            },
            RecoverEnd::Died { .. } => Phase::Done {
                why: crate::farm::DIED.into(),
                at: None,
            },
        }
    }
}

/// How long the buffs may take before the sneak. Several rounds of
/// fizzles behind a paced board, and nothing longer.
const BUFF_WAIT: Duration = Duration::from_secs(20);
/// Moves the walk home retries when the board refuses one for combat.
/// Each retry waits out a prompt, and the round passes with it.
const COMBAT_RETRIES: u32 = 12;

/// Why the job will not start. Checked before anything is sent, by the
/// window before it spawns the job and by the job itself, so neither can
/// forget.
pub fn refusal(
    session: &Session,
    graph: &RoomGraph,
    here: Option<RoomId>,
    target: RoomId,
) -> Result<RoomId, String> {
    let from = here.ok_or(
        "nobody knows with any confidence where you are standing; /where first, then recover",
    )?;
    if session.capabilities().stealth == 0 {
        return Err("cannot sneak: the sheet says Stealth is 0".into());
    }
    let sneaks = session.profile().bot.as_ref().is_none_or(|b| b.auto_sneak);
    if !sneaks {
        return Err(
            "bot.auto_sneak is off, and a recovery is a sneak: /set bot.auto_sneak true".into(),
        );
    }
    if graph.room(target).is_none() {
        return Err(format!("no such room {}/{}", target.map, target.room));
    }
    if graph.route(from, target).is_none() {
        return Err(format!(
            "no route from {}/{} to {}/{}",
            from.map, from.room, target.map, target.room
        ));
    }
    Ok(from)
}

/// What a recovery runs under, derived from the profile.
///
/// A recovery is a walk with the fighting taken out of it. Combat is off
/// at both switches, the flee is off because the job turns for home
/// itself, and the loot assist is off because the sweep does the taking
/// and a second hand in the room would race it. The travel interrupt is
/// spent on nothing, so the walk in never stops to swing, and doors stay
/// unbashed because a corpse run is quiet.
///
/// `bot.auto_sneak` is left as the profile has it. [`refusal`] reads the
/// same key off the same profile, and a job that forced it on would
/// start on a promise the refusal never made.
///
/// One function so the window's initial pair and the reload that follows
/// a `/set` cannot drift apart.
pub fn recover_config(p: &crate::profile::Profile) -> (BotConfig, crate::farm::FarmConfig) {
    let base = p.farm.clone().unwrap_or_else(|| crate::farm::FarmConfig {
        content: crate::tui::content_path(p),
        ..Default::default()
    });
    let mut farm = crate::go::go_config(&base, false);
    farm.interrupt_at_percent = 0;
    farm.nav.bash_doors = false;
    let mut bot = crate::tui::assist_config_for(p);
    bot.auto_combat = false;
    bot.auto_flee = false;
    bot.auto_get = false;
    (bot, farm)
}

/// The two navigators a recovery walks with, built together because
/// both read the same tables and a settings change moves both.
struct Navs {
    /// The walk in. It arms a sneak and casts the stealth buff for it.
    /// One navigator serves every hop, so its buff keeps the cast time
    /// it measured on the hop before.
    sneaker: Navigator,
    /// The walk home. It never arms, because arming costs a round
    /// standing still and the run home lives on not stopping.
    runner: Navigator,
}

impl Navs {
    fn build(
        session: &Session,
        graph: &Arc<RoomGraph>,
        live: &Live,
        clock: &crate::world::RoundClock,
    ) -> Navs {
        let caps = session.capabilities();
        let cfg = crate::farm::nav_config(&live.bot, &live.farm);
        let sneaker = Navigator::new(graph.clone(), cfg.clone())
            .with_capabilities(caps.clone())
            .with_stealth(crate::farm::stealth_buffs(session), clock.clone());
        let runner = Navigator::new(
            graph.clone(),
            crate::nav::NavConfig {
                sneak: false,
                ..cfg
            },
        )
        .with_capabilities(caps);
        Navs { sneaker, runner }
    }
}

/// What the job reads once and never again. `from`, the target and the
/// route are fixed at the start for the same reason: a settings change
/// may move how the character walks, never where it is going.
struct Fixed {
    graph: Arc<RoomGraph>,
    clock: crate::world::RoundClock,
    /// The room the job started in, which is also the safe room.
    home: RoomId,
    /// The character's own name, so a guard can see its death line.
    name: String,
}

/// Pick up a settings change, if one arrived, and rebuild the two
/// navigators for the configs it produced. `Live::refresh` prints the
/// reload notice itself, so the job says nothing here.
///
/// A rebuild costs the walk in its stealth buff's cast time, which is
/// the same bargain `farm::casts_need_rebuild` describes: a fresh
/// config and a stale timing cannot both be believed.
fn refresh_navs(
    navs: &mut Navs,
    live: &mut Live,
    built_at: &mut u64,
    session: &Session,
    fixed: &Fixed,
) {
    if live.took(built_at) {
        *navs = Navs::build(session, &fixed.graph, live, &fixed.clock);
    }
}

fn room_name(graph: &RoomGraph, id: RoomId) -> String {
    graph.room(id).map(|r| r.name.clone()).unwrap_or_default()
}

/// Sneak to `target` from `from`, search once, take what is listed, and
/// run back to `from`. `from` is where the character stands when the job
/// starts, and it is also the safe room.
///
/// `live` is the job's settings as the window keeps them. A change is
/// picked up at the next hop, at the next pickup, or in the wait a
/// pickup is already sitting in. The search's own wait has no arm for
/// one, so a change that lands during it waits for the first pickup.
pub async fn run_recover(
    session: &Session,
    graph: Arc<RoomGraph>,
    from: RoomId,
    target: RoomId,
    live: Live,
    phase: PhaseSink<'_>,
    notices: &Notices,
) -> Result<RecoverEnd, FarmError> {
    refusal(session, &graph, Some(from), target).map_err(FarmError::Config)?;
    // Both fight switches off for the job's life. The guard reads this
    // one live, and the job's own bot config has combat off, so no code
    // path in the runner can swing.
    let fought = session.travel_fights().get();
    session.travel_fights().set(false);
    let mut live = live;
    let out = recover(session, graph, from, target, &mut live, phase, notices).await;
    session.travel_fights().set(fought);
    out
}

async fn recover(
    session: &Session,
    graph: Arc<RoomGraph>,
    from: RoomId,
    target: RoomId,
    live: &mut Live,
    phase: PhaseSink<'_>,
    notices: &Notices,
) -> Result<RecoverEnd, FarmError> {
    set_phase(phase, Phase::Preparing);
    let mut built_at = live.generation();
    // The item table goes to the session before the capabilities are
    // read, so the walk routes with the pack and the stealth spell has a
    // table to be found in. Best effort, as in `run_go`.
    crate::farm::content_for(session, &live.farm, notices);
    let durations = RoomGraph::load_spell_durations(&live.farm.content).unwrap_or_default();
    let clock = crate::world::RoundClock::new();
    let mut navs = Navs::build(session, &graph, live, &clock);

    // Where the character really stands. The caller's room is a hint the
    // locator checks against the board's own block.
    let seen = crate::farm::look_around(session, "the recovery's opening look").await?;
    let home = crate::lost::place(session, &graph, &navs.sneaker, from, &seen)
        .await
        .map_err(FarmError::Lost)?
        .at;
    let fixed = Fixed {
        graph,
        clock,
        home,
        name: session.character_name().unwrap_or_default(),
    };

    // Every percent mark divides by the maximum, and 0 means nobody
    // said. The answer is kept on the settings, so a reload cannot lose
    // it again.
    if live.bot.max_hp == 0
        && let Some(v) = crate::farm::discover_vitals(session).await
    {
        live.learned_vitals(v.max_hp, v.max_mana);
    }

    // Light and buffs, standing still, before the sneak. Either breaks
    // one, which is why both come first.
    let sheet = crate::farm::sheet_from(session, &live.bot, &durations);
    let mut light = crate::sheet::LightState::new(sheet.light);
    let mut buff = crate::sheet::BuffState::new(sheet.buffs.0);
    for why in sheet.buffs.1 {
        notices(&why);
    }
    if fixed.graph.dark(target) || crate::farm::leg_needs_light(&fixed.graph, home, target) {
        crate::farm::ensure_lit(session, &mut light, &fixed.clock).await;
    }
    // Standing still, on the same cycle the light runs on. A fizzle is
    // not retried past the deadline: a buff is a bonus, and the sneak
    // goes without it.
    let buffs_by = tokio::time::Instant::now() + BUFF_WAIT;
    crate::farm::drive_cast(session, &mut buff, &fixed.clock, buffs_by).await;

    // The route, as rooms, so every hop is its own walk and every
    // arrival's sneak state is read. One walk to the target would hide
    // every break before the last.
    let route = fixed
        .graph
        .route(home, target)
        .ok_or_else(|| FarmError::Config("no route to the target".into()))?;
    let mut hops = Vec::with_capacity(route.len());
    let mut at = home;
    for dir in route {
        let next = fixed
            .graph
            .room(at)
            .and_then(|r| r.exits[dir as usize].as_ref())
            .map(|e| e.dest)
            .ok_or_else(|| FarmError::Config("the route left the graph".into()))?;
        hops.push(next);
        at = next;
    }

    set_phase(phase, Phase::SneakingIn { to: target });
    let mut haul = Haul::default();
    let mut here = home;
    let mut sneaking = false;
    let mut guard = crate::farm::FarmGuard::death_only(&fixed.name);
    for next in hops {
        refresh_navs(&mut navs, live, &mut built_at, session, &fixed);
        match navs
            .sneaker
            .goto(session, here, next, &mut guard, sneaking)
            .await
        {
            Ok(arrived) => {
                here = arrived.at;
                sneaking = arrived.sneaking;
                if here != next || !sneaking {
                    let why = HomeWhy::Broke {
                        at: here,
                        name: room_name(&fixed.graph, here),
                    };
                    return go_home(
                        session,
                        &fixed,
                        &mut navs,
                        live,
                        &mut built_at,
                        phase,
                        here,
                        why,
                        haul,
                    )
                    .await;
                }
            }
            Err(e) => {
                here = e.at;
                return match e.kind {
                    NavErrorKind::Interrupted(Interrupt::Died) => Ok(RecoverEnd::Died { haul }),
                    // Not a blow. `FarmGuard::death_only` never trips on
                    // one, so the only thing that reaches here is a move
                    // the board refused because something already has
                    // the character in combat, and a refused move is a
                    // hop that will not happen however long it waits.
                    NavErrorKind::Interrupted(Interrupt::Attacked { .. }) => {
                        let why = HomeWhy::Attacked {
                            at: here,
                            name: room_name(&fixed.graph, here),
                        };
                        go_home(
                            session,
                            &fixed,
                            &mut navs,
                            live,
                            &mut built_at,
                            phase,
                            here,
                            why,
                            haul,
                        )
                        .await
                    }
                    _ => Err(FarmError::Nav(e)),
                };
            }
        }
    }

    set_phase(phase, Phase::Sweeping { at: here });
    let why = match sweep(session, live, &mut haul, &fixed.name).await? {
        SweepEnd::Died => return Ok(RecoverEnd::Died { haul }),
        SweepEnd::Swept => HomeWhy::Swept,
        SweepEnd::Nothing => HomeWhy::Nothing,
        SweepEnd::Hurt { mark } => HomeWhy::Hurt { mark },
        SweepEnd::Blocked => HomeWhy::Attacked {
            at: here,
            name: room_name(&fixed.graph, here),
        },
    };
    go_home(
        session,
        &fixed,
        &mut navs,
        live,
        &mut built_at,
        phase,
        here,
        why,
        haul,
    )
    .await
}

/// How the sweep ended.
enum SweepEnd {
    Swept,
    Nothing,
    Hurt { mark: u32 },
    /// The board refused the search for combat.
    Blocked,
    Died,
}

/// A death or the hitpoint mark, read off any event during the sweep.
fn vitals_end(ev: &Event, bot: &BotConfig, name: &str) -> Option<SweepEnd> {
    match ev {
        Event::Line(line) if crate::farm::is_player_death(line, name) => Some(SweepEnd::Died),
        Event::Prompt { hp, .. } if *hp <= 0 => Some(SweepEnd::Died),
        Event::Prompt { hp, .. }
            if bot.max_hp > 0 && *hp * 100 / bot.max_hp < bot.minor_heal_at_percent as i32 =>
        {
            Some(SweepEnd::Hurt {
                mark: bot.minor_heal_at_percent,
            })
        }
        _ => None,
    }
}

/// When a command sent inside the sweep has waited long enough.
///
/// `farm.nav.step_timeout_ms` is the crate's per-command answer
/// deadline, and it bounds every wait the sweep makes: the search, and
/// each of the three attempts an entry is worth. So an operator who
/// shortens it shortens the sweep, and an entry the board never answers
/// costs three of it. Read off the settings every time, so a change
/// moves it mid sweep.
fn answer_by(live: &Live) -> tokio::time::Instant {
    tokio::time::Instant::now() + Duration::from_millis(live.farm.nav.step_timeout_ms)
}

/// A death or the hitpoint mark, read off whatever the board has
/// already said.
///
/// Only what has already arrived: [`crate::session::drain`] is
/// `try_recv` and never waits. The prompt that follows a reply is
/// normally in the receiver by the time this runs, because the board
/// writes a reply and its prompt together, but one still on the wire is
/// left for the next wait to read.
fn drained_end(
    events: &mut tokio::sync::broadcast::Receiver<crate::correlate::Correlated>,
    bot: &BotConfig,
    name: &str,
) -> Option<SweepEnd> {
    let mut end = None;
    crate::session::drain(events, |cor| {
        if end.is_none() {
            end = vitals_end(&cor.event, bot, name);
        }
    });
    end
}

/// One bare `search`, then one `get` per listed entry, gear first and
/// coins last.
///
/// One `get` is in flight at a time. Each is retired by the board's own
/// word, and an entry somebody else already took is dropped where it
/// stands rather than asked for again. Silence is the third answer: an
/// item too heavy to lift draws no line at all, so the attempt is spent
/// and the entry is asked for [`crate::farm::LOOT_TRIES`] times before
/// the sweep moves on.
///
/// Every reply is followed by a prompt, and every prompt is read: the
/// sweep ends the moment hitpoints fall under the minor heal mark, with
/// what it has taken so far.
async fn sweep(
    session: &Session,
    live: &mut Live,
    haul: &mut Haul,
    name: &str,
) -> Result<SweepEnd, FarmError> {
    let mut events = session.events();
    crate::session::drain(&mut events, |_| {});
    let ask = session.send("search");
    let deadline = answer_by(live);
    let items: Vec<String> = loop {
        match tokio::time::timeout_at(deadline, events.recv()).await {
            Ok(Ok(cor)) => {
                if let Some(end) = vitals_end(&cor.event, &live.bot, name) {
                    return Ok(end);
                }
                if cor.answers != Some(ask) {
                    continue;
                }
                match &cor.event {
                    Event::RoomSeen(room) => break room.items.clone(),
                    Event::Line(l) if l.to_lowercase().contains("search revealed nothing") => {
                        return Ok(SweepEnd::Nothing);
                    }
                    Event::Line(l)
                        if l.to_lowercase().contains("may not search while attacking") =>
                    {
                        return Ok(SweepEnd::Blocked);
                    }
                    _ => continue,
                }
            }
            Ok(Err(tokio::sync::broadcast::error::RecvError::Lagged(_))) => continue,
            Ok(Err(_)) => return Err(FarmError::Disconnected),
            // A search the board never answered and a search that found
            // nothing say the same thing to the operator: there is
            // nothing here to take. Erroring out instead left the
            // character standing in the room it died in, which is the
            // one place the job exists to leave.
            Err(_) => return Ok(SweepEnd::Nothing),
        }
    };
    let list = sweep_list(&items);
    if list.is_empty() {
        return Ok(SweepEnd::Nothing);
    }
    *haul = Haul::wanted(&list);
    // The prompt the search's own block ended on. Read before the first
    // `get` goes out, so a character who arrived already under the mark
    // asks for nothing.
    if let Some(end) = drained_end(&mut events, &live.bot, name) {
        return Ok(end);
    }
    for take in &list {
        let mut tries = 0u32;
        'entry: while tries < crate::farm::LOOT_TRIES {
            tries += 1;
            // A change that landed between two asks, which no wait
            // below was sitting in to hear.
            live.refresh();
            let sent = session.send(&take.cmd);
            let deadline = answer_by(live);
            loop {
                // The receiver was subscribed before the search, so it
                // is already listening when each `get` goes out and no
                // reply can land unheard. Whatever the reply before
                // left in it is skipped here by its send id, and read
                // for vitals on the way past.
                let ev = tokio::select! {
                    _ = live.changed() => None,
                    ev = tokio::time::timeout_at(deadline, events.recv()) => Some(ev),
                };
                let Some(ev) = ev else {
                    // The wake, and the pass after it takes the change.
                    live.refresh();
                    continue;
                };
                match ev {
                    Ok(Ok(cor)) => {
                        if let Some(end) = vitals_end(&cor.event, &live.bot, name) {
                            return Ok(end);
                        }
                        if cor.answers != Some(sent) {
                            continue;
                        }
                        if let Event::Line(line) = &cor.event {
                            match read_get_reply(line) {
                                GetReply::Taken => {
                                    haul.took(take);
                                    break 'entry;
                                }
                                GetReply::Gone => break 'entry,
                                GetReply::Other => {}
                            }
                        }
                    }
                    Ok(Err(tokio::sync::broadcast::error::RecvError::Lagged(_))) => {}
                    Ok(Err(_)) => return Err(FarmError::Disconnected),
                    // Silence: the attempt is spent.
                    Err(_) => break,
                }
            }
        }
        // The prompt that followed the reply may already say the
        // character is too hurt to stand here for the next one.
        if let Some(end) = drained_end(&mut events, &live.bot, name) {
            return Ok(end);
        }
    }
    Ok(SweepEnd::Swept)
}

/// Wait for the next prompt, so a move refused for combat is retried
/// after the round rather than at once. Bounded, because a board that
/// prints no prompt must not wedge the walk home.
///
/// `events` is the caller's receiver, opened before the refused move went
/// out and emptied right before this call. Opening one here instead
/// would start listening after the board had already answered, and the
/// wait would sit out its whole deadline for a prompt it had missed.
/// Not emptying it first would end the wait on the refusal's own
/// prompt, which is this round's, and the retry would go out inside the
/// round that just refused it.
async fn wait_a_prompt(
    events: &mut tokio::sync::broadcast::Receiver<crate::correlate::Correlated>,
) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(6);
    loop {
        match tokio::time::timeout_at(deadline, events.recv()).await {
            Ok(Ok(cor)) if matches!(cor.event, Event::Prompt { .. }) => return,
            Ok(Ok(_)) => continue,
            Ok(Err(tokio::sync::broadcast::error::RecvError::Lagged(_))) => continue,
            Ok(Err(_)) | Err(_) => return,
        }
    }
}

/// The unsneaked run back. Only a death stops it: blows do not, and a
/// move the board refuses for combat is sent again once the round has
/// passed.
#[allow(clippy::too_many_arguments)]
async fn go_home(
    session: &Session,
    fixed: &Fixed,
    navs: &mut Navs,
    live: &mut Live,
    built_at: &mut u64,
    phase: PhaseSink<'_>,
    here: RoomId,
    why: HomeWhy,
    haul: Haul,
) -> Result<RecoverEnd, FarmError> {
    set_phase(phase, Phase::GoingHome { to: fixed.home });
    let mut here = here;
    let mut guard = crate::farm::FarmGuard::death_only(&fixed.name);
    let mut refused = 0u32;
    // Opened before the first step, so nothing the board says about one
    // can be missed.
    let mut events = session.events();
    loop {
        refresh_navs(navs, live, built_at, session, fixed);
        match navs
            .runner
            .goto(session, here, fixed.home, &mut guard, false)
            .await
        {
            Ok(arrived) => {
                return Ok(RecoverEnd::Home {
                    at: arrived.at,
                    why,
                    haul,
                });
            }
            Err(e) => {
                here = e.at;
                match e.kind {
                    NavErrorKind::Interrupted(Interrupt::Died) => {
                        return Ok(RecoverEnd::Died { haul });
                    }
                    NavErrorKind::Interrupted(Interrupt::Attacked { .. })
                        if refused < COMBAT_RETRIES =>
                    {
                        refused += 1;
                        // Everything the refused step printed, the
                        // round's own prompt with it. What the wait
                        // ends on is then the next round's.
                        crate::session::drain(&mut events, |_| {});
                        wait_a_prompt(&mut events).await;
                    }
                    _ => {
                        return Ok(RecoverEnd::Stopped {
                            at: here,
                            name: room_name(&fixed.graph, here),
                            haul,
                        });
                    }
                }
            }
        }
    }
}

