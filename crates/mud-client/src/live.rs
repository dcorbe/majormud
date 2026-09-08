//! A job's live settings: the configs it runs under and the rule that
//! derives them from the session's profile.
//!
//! Split out of [`crate::farm`] because every job kind uses it, not
//! only the farm runner.

/// What a job derives from a profile: its bot table and its farm table
/// with the job's own forced fields applied. A go keeps combat off and
/// its walk mode, a bank keeps looting off, whatever the profile says.
pub type Derive = std::sync::Arc<
    dyn Fn(&crate::profile::Profile) -> (crate::bot::BotConfig, crate::farm::FarmConfig) + Send + Sync,
>;

/// A job's settings, kept current while it runs.
///
/// The window replaces the session's profile on every `/set`, `/unset`
/// and `/load`. This holds the receiver, the configs the job runs
/// under, and the rule that derives one from the other. A job asks
/// [`Live::refresh`] at every decision that reads a config and selects
/// on [`Live::changed`] where it waits on the board, so a change lands
/// at the next step, pass or pickup and never part way through one.
///
/// The generation counter is for the guards, bots and watches a job
/// builds from the configs: each remembers the generation it was built
/// at and rebuilds when the counter moves, whichever caller did the
/// refresh.
pub struct Live {
    rx: tokio::sync::watch::Receiver<crate::profile::Profile>,
    derive: Derive,
    pub bot: crate::bot::BotConfig,
    pub farm: crate::farm::FarmConfig,
    /// What `discover_vitals` found, reapplied to every rebuild whose
    /// profile does not say.
    vitals: Option<(i32, i32)>,
    generation: u64,
    what: &'static str,
    notices: crate::farm::Notices,
    /// Set by `changed` when a wait resolves, cleared by `refresh` once
    /// it has rebuilt for it. Tracked here instead of by rewinding the
    /// channel's own version, so a wait's wake can never be mistaken
    /// for a second send.
    pending: bool,
    /// `fixed` keeps its own sender so the receiver never reports a
    /// closed channel.
    _pinned: Option<tokio::sync::watch::Sender<crate::profile::Profile>>,
}

impl Live {
    /// The live settings for a job on this session. `bot` and `farm`
    /// are what the job starts with, which for a named loop is not what
    /// `derive` would make of the profile.
    pub fn new(
        session: &crate::session::Session,
        what: &'static str,
        notices: crate::farm::Notices,
        bot: crate::bot::BotConfig,
        farm: crate::farm::FarmConfig,
        derive: Derive,
    ) -> Live {
        Live::over(session.profile_changes(), what, notices, bot, farm, derive)
    }

    /// As [`Live::new`], over a receiver the caller holds the sender
    /// of. What a test uses to change the settings under a job.
    pub fn over(
        rx: tokio::sync::watch::Receiver<crate::profile::Profile>,
        what: &'static str,
        notices: crate::farm::Notices,
        bot: crate::bot::BotConfig,
        farm: crate::farm::FarmConfig,
        derive: Derive,
    ) -> Live {
        Live {
            rx,
            derive,
            bot,
            farm,
            vitals: None,
            generation: 0,
            what,
            notices,
            pending: false,
            _pinned: None,
        }
    }

    /// Settings that never change: the headless commands, and every
    /// test that is not about reloading.
    pub fn fixed(bot: crate::bot::BotConfig, farm: crate::farm::FarmConfig) -> Live {
        let (tx, rx) = tokio::sync::watch::channel(crate::profile::Profile::default());
        let (b, f) = (bot.clone(), farm.clone());
        let mut live = Live::over(
            rx,
            "job",
            std::sync::Arc::new(|_: &str| {}),
            bot,
            farm,
            std::sync::Arc::new(move |_: &crate::profile::Profile| (b.clone(), f.clone())),
        );
        live._pinned = Some(tx);
        live
    }

    pub fn generation(&self) -> u64 {
        self.generation
    }

    /// The profile as it stands, for the tables `derive` does not
    /// cover, like `[bank]`.
    pub fn profile(&self) -> crate::profile::Profile {
        self.rx.borrow().clone()
    }

    /// What the board said the pools are. Applied now and to every
    /// rebuild whose profile leaves `max_hp` at zero.
    pub fn learned_vitals(&mut self, max_hp: i32, max_mana: i32) {
        self.vitals = Some((max_hp, max_mana));
        self.bot.max_hp = max_hp;
        self.bot.max_mana = max_mana;
    }

    /// Rebuild the configs if the profile has changed since the last
    /// look, or a `changed` wait landed one since then. True when it
    /// did. The pending flag is false on the way out whichever path
    /// ran, so a wait that already forced this rebuild does not force a
    /// second one right after.
    ///
    /// Reads `Ref::has_changed` off the borrow rather than
    /// `Receiver::has_changed`, which reports the channel closed the
    /// moment the last sender drops, even with an unread change still
    /// sitting in it. A run whose window closes must still pick up
    /// whatever it last set.
    pub fn refresh(&mut self) -> bool {
        let now = self.rx.borrow_and_update();
        // The early return leaves `pending` alone on purpose: it can
        // only be false here, since a true one would have made this
        // changed. Nothing to clear, and nothing to clone either.
        if !(self.pending || now.has_changed()) {
            return false;
        }
        let profile = now.clone();
        drop(now);
        self.pending = false;
        let (bot, farm) = (self.derive)(&profile);
        self.bot = bot;
        self.farm = farm;
        if let Some((hp, mana)) = self.vitals
            && self.bot.max_hp == 0
        {
            self.bot.max_hp = hp;
            self.bot.max_mana = mana;
        }
        self.generation += 1;
        (self.notices)(&format!("-- {}: settings reloaded --", self.what));
        true
    }

    /// [`Live::refresh`], plus the caller's own build generation. True
    /// when this call rebuilt the configs, and true when `built_at`
    /// lags because some other refresh point rebuilt them first. Stores
    /// the current generation whenever it returns true, so the caller
    /// asks again without tracking any of that itself.
    pub fn took(&mut self, built_at: &mut u64) -> bool {
        let rebuilt = self.refresh();
        if !rebuilt && *built_at == self.generation {
            return false;
        }
        *built_at = self.generation;
        true
    }

    /// Resolves when the profile changes. Never, once the sender is
    /// gone. A select arm that fired forever would spin the pump.
    ///
    /// Marks the wake pending on `Live` itself rather than asking
    /// `watch::Receiver::mark_changed` to fake one. That call rewinds
    /// the receiver's own version, so a second wait with no
    /// intervening `refresh` would see its own rewind as a fresh
    /// change and resolve at once with nothing sent, a silent zero
    /// backoff spin. The flag holds the fact a change arrived until
    /// `refresh` consumes it, so a second wait genuinely waits for a
    /// second send.
    pub async fn changed(&mut self) {
        if self.rx.changed().await.is_ok() {
            self.pending = true;
        } else {
            std::future::pending::<()>().await;
        }
    }
}

