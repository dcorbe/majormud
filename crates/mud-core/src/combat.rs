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
    let threshold = if defender.parry < 0 && roll(0, 100) > defender.parry + 100 {
        99
    } else if attack_type == AttackType::Backstab {
        attacker.accuracy - defender.evasion_a
    } else {
        let accuracy = attacker.accuracy + acc_mod;
        let defense = defender.evasion_a + defender.evasion_b;
        let den = accuracy * accuracy / 14 / 10;
        if den == 0 {
            // Sub-formula accuracy: flat 5% (not clamped up to 10).
            5
        } else {
            (100 - defense * defense / den).clamp(10, 99)
        }
    };
    if roll(1, 100) > threshold {
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
    let mut damage = roll(min, max) - defender.armor / 10;
    damage *= multiplier;

    // --- parry/riposte (cancels even a crit) ---
    if defender.parry > 0 {
        let denom = attacker.accuracy / 8;
        if denom > 0 {
            let mut p = (defender.parry * 10 / denom).clamp(0, 95);
            if attack_type == AttackType::Backstab {
                p /= 5;
            }
            if p > 0 && roll(0, 100) < p {
                return AttackResult {
                    outcome: Outcome::Parried,
                    damage: 0,
                };
            }
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
