//! Single-attack resolution — `calculate_attack` (`re/docs/combat.md`).
//!
//! WG3-NT tuning throughout (the project target): percentage damage seeds,
//! re-indexed accuracy penalties, crit floor `2*max_old`, backstab parry /5.
//! The roll source is injected so tests can script exact sequences; the
//! runtime passes the core RNG.

/// The reduced fighter view `move_*_to_fighter` builds: only the fields the
/// resolution math reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Fighter {
    /// `word[0]` — offensive accuracy (also the parry denominator when
    /// attacking); backstab threshold base.
    pub accuracy: i32,
    /// `word[1]` — defensive evasion part A; the backstab compare value.
    pub evasion_a: i32,
    /// `word[2]` — defensive evasion part B (weapon to-hit when attacking).
    pub evasion_b: i32,
    /// `word[3]` — armor; damage -= armor/10.
    pub armor: i32,
    /// `word[8]`/`word[9]` — damage range.
    pub min_damage: i32,
    pub max_damage: i32,
    /// `word[10]` — parry rating; negative = helpless (HP < 1).
    pub parry: i32,
    /// `word[0x117]` — crit rating (players only; monsters hard-zeroed).
    pub crit_rating: i32,
}

/// WG3-NT attack types (the re-indexed table).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AttackType {
    MartialArts1,
    MartialArts2,
    MartialArts3,
    Backstab,
    Normal,
    Bash,
    Smash,
    Type8,
}

impl AttackType {
    /// (damage seed %, accuracy modifier, may crit, on-hit multiplier).
    fn tuning(self) -> (i32, i32, bool, i32) {
        match self {
            AttackType::MartialArts1 => (0, 0, true, 1),
            AttackType::MartialArts2 => (33, 0, true, 1),
            AttackType::MartialArts3 => (66, 0, true, 1),
            AttackType::Backstab => (10, 0, false, 1),
            AttackType::Normal => (0, 0, true, 1),
            AttackType::Bash => (20, -15, false, 3),
            AttackType::Smash => (125, -25, false, 5),
            AttackType::Type8 => (125, -75, false, 1),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    /// Result 1 — connected but the armor absorbed it.
    NoDamage,
    /// Result 2 — a normal hit.
    Hit,
    /// Result 0 — the to-hit roll missed (the DLL leaves the zeroed result
    /// word untouched; renders as the PLAIN miss lines).
    Dodged,
    /// Result 3 — connected but was parried; the DLL renders this with the
    /// ", but you dodge out of the way!" framing (§8.10's "dodge" lines).
    Parried,
    /// Result 4 — critical hit.
    Critical,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AttackResult {
    pub outcome: Outcome,
    pub damage: i32,
}

/// Resolves one swing. `roll(lo, hi)` must return a uniform value in
/// `[lo, hi]` (the engine's `genrdn`).
pub fn calculate_attack(
    attacker: &Fighter,
    defender: &Fighter,
    attack_type: AttackType,
    roll: &mut impl FnMut(i32, i32) -> i32,
) -> AttackResult {
    let (seed, acc_mod, may_crit, multiplier) = attack_type.tuning();

    // Damage seed: a percentage bonus applied before crits and multipliers.
    let mut min = attacker.min_damage;
    let mut max = attacker.max_damage;
    if seed != 0 {
        min = min * (seed + 100) / 100;
        max = max * (seed + 100) / 100;
    }

    // --- to-hit ---
    // The [10,99] clamp sits OUTSIDE the whole if/else (decompile 25315;
    // 16-bit _CALCULATE_ATTACK.asm 1f5b falls through to the clamp at
    // 1f60/1f6c), so the den==0 arm's 5 clamps up to 10 and the backstab
    // difference clamps both ways. The helpless 99 passes through unchanged.
    let threshold = if defender.parry < 0 && roll(0, 100) > defender.parry + 100 {
        99
    } else if attack_type == AttackType::Backstab {
        attacker.accuracy - defender.evasion_a
    } else {
        let accuracy = attacker.accuracy + acc_mod;
        let defense = defender.evasion_a + defender.evasion_b;
        let den = accuracy * accuracy / 14 / 10;
        if den == 0 {
            5
        } else {
            100 - defense * defense / den
        }
    }
    .clamp(10, 99);
    // Hit iff roll < threshold — STRICT (decompile 25324 `iVar3 < iVar2`,
    // genrdn inclusive on both bounds), so a threshold of 99 connects 98%
    // of the time, and the clamp floor 9%. Draw-order note: the DLL draws
    // this roll BEFORE the helpless gate's genrdn(0,100) (25297 vs 25298);
    // we draw the gate first. Same draw count, identical distribution, and
    // the port deliberately does not promise DLL stream parity.
    if roll(1, 100) >= threshold {
        return AttackResult {
            outcome: Outcome::Dodged,
            damage: 0,
        };
    }

    // --- critical (players only via crit_rating; disabled for special types) ---
    let mut crit = false;
    if may_crit && attacker.crit_rating > 0 {
        let mut rating = attacker.crit_rating;
        if rating > 40 {
            rating = 40 + (rating - 40) / 3;
        }
        if roll(0, 100) < rating {
            crit = true;
            let max_old = max;
            min = 2 * max_old;
            max = 4 * max_old;
        }
    }
    if max < min {
        max = min;
    }

    // --- damage ---
    // 25334: `genrdn(0,(max-min)+1) + min`, genrdn inclusive — the range
    // runs one past max. (Same single draw either way; the port targets
    // distributions, not the DLL's literal call shape.)
    let mut damage = roll(min, max + 1) - defender.armor / 10;
    damage *= multiplier;

    // --- parry/riposte (cancels even a crit) ---
    // EXACT (decompile 25344-25360). Two details that are easy to get
    // wrong and both reachable:
    //  - the floor at 25344 is on the ACCURACY (`if (*param_1 < 9)
    //    chance = 0`), not on the `accuracy >> 3` denominator, so
    //    accuracy 8 cannot be parried at all — it does NOT fall through
    //    to a denominator of 1 and a near-certain parry;
    //  - the draw at 25357 is guarded only by `0 < parry`, so a defender
    //    with any parry rating costs a draw even when the chance is 0.
    //    Skipping it would drift the shared RNG stream (`monster_vs_
    //    monster`'s kind-0 forms carry accuracy 5).
    if defender.parry > 0 {
        let mut chance = if attacker.accuracy < 9 {
            0
        } else {
            (defender.parry * 10 / (attacker.accuracy / 8)).min(95)
        };
        if attack_type == AttackType::Backstab {
            chance /= 5;
        }
        if roll(0, 100) < chance {
            return AttackResult {
                outcome: Outcome::Parried,
                damage: 0,
            };
        }
    }

    if damage < 1 {
        return AttackResult {
            outcome: Outcome::NoDamage,
            damage: 0,
        };
    }
    AttackResult {
        outcome: if crit { Outcome::Critical } else { Outcome::Hit },
        damage,
    }
}
