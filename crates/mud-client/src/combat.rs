//! The bot's combat latches, collapsed into one value.
//!
//! `Bot` used to carry `engaged`, `cooling`, `switch_watch`, and
//! `quiet_prompts` as four separate fields, mutated from about ten sites
//! spread across `decide`, `engage`, `on_line`, `on_vitals`, and the
//! prompt arm of `on_event`. Every one of those mutations is a
//! transition on the same underlying state — which target (if any) is
//! under attack, whether its recent disappearance is still in question,
//! and whether the very next event might undo an apparent ending. This
//! module names those transitions instead of leaving them as four
//! fields a caller could poke independently and inconsistently.
//!
//! This is a pure move: every method here is the exact logic `bot.rs`
//! used to inline, with the same conditions and the same ordering.
//! `backstab_open` is not part of this — it drives the second-round
//! backstab re-send, a separate concern, and stays a `Bot` field.

use crate::bot::target_word;

/// Combat-latch state for one bot: the target under attack, the
/// wander-out cooldown, the target-switch window, and the silence
/// counter that backstops a fight the board never announced the end of.
#[derive(Debug, Default)]
pub struct CombatState {
    /// Name currently under attack; cleared once it is gone.
    engaged: Option<String>,
    /// Prompts seen since the last blow involving the engaged target.
    quiet_prompts: u32,
    /// A noun we must not re-engage yet, and how many blocks have listed
    /// it since. See `Bot`'s old `cooling` field doc for the full
    /// motivation: this is the wander-out cooldown, armed by a
    /// TARGETLESS `*Combat Off*` and settled by absence, by a leave or
    /// arrival event, or by surviving two listed blocks.
    cooling: Option<(String, u32)>,
    /// A `*Combat Off*` just un-latched a fight, and the very next event
    /// decides whether it was real. See `Bot`'s old `switch_watch` field
    /// doc: `Some(None)` is the window with nothing to restore, `None`
    /// is no window open.
    switch_watch: Option<Option<String>>,
}

impl CombatState {
    pub fn new() -> Self {
        Self::default()
    }

    /// The name currently under attack, or `None`.
    pub fn engaged(&self) -> Option<&str> {
        self.engaged.as_deref()
    }

    /// Enter the fight with `target`. Resets the silence counter: a
    /// fresh fight has traded no blows yet.
    pub fn engage(&mut self, target: &str) {
        self.engaged = Some(target.to_string());
        self.quiet_prompts = 0;
    }

    /// The board un-latched: clear the target, arm the wander-out
    /// cooldown against it, and open the one-event window a following
    /// `*Combat Engaged*` can still undo this through.
    pub fn on_combat_off(&mut self) {
        self.switch_watch = Some(self.engaged.clone());
        if let Some(target) = &self.engaged {
            self.cooling = Some((target_word(target).to_string(), 0));
        }
        self.engaged = None;
        self.quiet_prompts = 0;
    }

    /// A `*Combat Engaged*` arrived. If the switch window is open,
    /// restore the target the matching `on_combat_off` cleared and
    /// cancel the cooldown it armed against it.
    pub fn on_combat_engaged(&mut self) {
        if let Some(restore) = self.switch_watch.take()
            && let Some(target) = restore
        {
            if self
                .cooling
                .as_ref()
                .is_some_and(|(noun, _)| *noun == target_word(&target))
            {
                self.cooling = None;
            }
            self.engaged = Some(target);
        }
    }

    /// Any event other than a Combat Engaged closes the switch window:
    /// the un-latch `on_combat_off` applied stands.
    pub fn close_switch_window(&mut self) {
        self.switch_watch = None;
    }

    /// A kill or exp-award: clear the target with no cooldown. A
    /// wander-out is a question; a kill is not.
    pub fn on_kill(&mut self) {
        self.engaged = None;
        self.quiet_prompts = 0;
    }

    /// A prompt passed with no blow traded. Increments the silence
    /// counter while a target is engaged; once it reaches `idle_after`
    /// the fight is presumed over (the board's own ending went unseen)
    /// and the target clears. Returns whether that just happened.
    pub fn note_quiet_prompt(&mut self, idle_after: u32) -> bool {
        if self.engaged.is_some() {
            self.quiet_prompts += 1;
            if self.quiet_prompts >= idle_after {
                self.engaged = None;
                self.quiet_prompts = 0;
                return true;
            }
        }
        false
    }

    /// A blow landed involving the engaged target: the silence counter
    /// starts over.
    pub fn note_blow(&mut self) {
        self.quiet_prompts = 0;
    }

    /// The noun currently cooling (wander-out in question), or `None`.
    pub fn cooling_noun(&self) -> Option<&str> {
        self.cooling.as_ref().map(|(noun, _)| noun.as_str())
    }

    /// The per-block settle of the wander-out cooldown: absence means
    /// the leave completed, and a monster still present after two
    /// looks is not leaving, it is standing there.
    pub fn settle_cooling(&mut self, present: bool) {
        if let Some((_, listed)) = &mut self.cooling {
            *listed += 1;
            if !present || *listed >= 2 {
                self.cooling = None;
            }
        }
    }

    /// Force to idle: clear the target with no cooldown of its own.
    /// Used where a block shows the target absent, and anywhere else an
    /// engagement ends with nothing left to track — the silence counter
    /// left standing is inert, since it is read only while a target is
    /// engaged and `engage` resets it on the next fight regardless.
    pub fn clear(&mut self) {
        self.engaged = None;
        self.quiet_prompts = 0;
    }
}
