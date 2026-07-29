//! Tests for single-attack resolution (`re/docs/combat.md`, WG3-NT column).

use mud_core::combat::{calculate_attack, AttackType, Fighter, Outcome};

fn fighter(accuracy: i32, evasion: (i32, i32), armor: i32) -> Fighter {
    Fighter {
        accuracy,
        evasion_a: evasion.0,
        evasion_b: evasion.1,
        armor,
        min_damage: 2,
        max_damage: 10,
        parry: 0,
        crit_rating: 1,
    }
}

/// Scripted roll source: pops from the front; panics if exhausted.
fn rolls(values: &[i32]) -> impl FnMut(i32, i32) -> i32 + '_ {
    let mut it = values.iter().copied();
    move |lo, hi| {
        let v = it.next().expect("script exhausted");
        assert!(v >= lo && v <= hi, "scripted roll {v} outside [{lo},{hi}]");
        v
    }
}

#[test]
fn to_hit_threshold_is_quadratic() {
    // acc 200 vs defense 100: 100 - 140*10000/40000 = 65 (spec sanity check).
    // Roll 65 hits; roll 66 misses.
    let att = fighter(200, (0, 0), 0);
    let def = fighter(0, (50, 50), 0);

    // hit path: to-hit 65, crit roll 100 (no crit), damage roll, parry skipped (parry 0)
    let r = calculate_attack(&att, &def, AttackType::Normal, &mut rolls(&[65, 100, 5]));
    assert_eq!(r.outcome, Outcome::Hit);

    let r = calculate_attack(&att, &def, AttackType::Normal, &mut rolls(&[66]));
    assert_eq!(r.outcome, Outcome::Dodged);
}

#[test]
fn evenly_matched_clamps_to_ten_percent() {
    // acc == defense -> 100-140 = -40 -> clamp 10.
    let att = fighter(100, (0, 0), 0);
    let def = fighter(0, (50, 50), 0);
    let r = calculate_attack(&att, &def, AttackType::Normal, &mut rolls(&[10, 100, 5]));
    assert_eq!(r.outcome, Outcome::Hit, "roll 10 <= clamped threshold 10");
    let r = calculate_attack(&att, &def, AttackType::Normal, &mut rolls(&[11]));
    assert_eq!(r.outcome, Outcome::Dodged);
}

#[test]
fn tiny_accuracy_threshold_clamps_up_to_ten() {
    // accuracy 11: den = 11*11/14/10 = 121/14=8, 8/10=0 -> raw threshold 5,
    // but the [10,99] clamp sits OUTSIDE the den==0 arm (decompile 25315;
    // 16-bit _CALCULATE_ATTACK.asm 1f5b falls through to 1f60), so 5 -> 10.
    let att = fighter(11, (0, 0), 0);
    let def = fighter(0, (10, 10), 0);
    let r = calculate_attack(&att, &def, AttackType::Normal, &mut rolls(&[10, 100, 5]));
    assert_eq!(r.outcome, Outcome::Hit);
    let r = calculate_attack(&att, &def, AttackType::Normal, &mut rolls(&[11]));
    assert_eq!(r.outcome, Outcome::Dodged);
}

#[test]
fn backstab_threshold_is_clamped() {
    // The [10,99] clamp covers the backstab arm too (decompile 25315 is
    // outside the whole if/else).
    // Raw 80 - 75 = 5 -> clamps up to 10: roll 10 hits.
    let att = fighter(80, (0, 0), 0);
    let def = fighter(0, (75, 0), 0);
    let r = calculate_attack(&att, &def, AttackType::Backstab, &mut rolls(&[10, 11]));
    assert_eq!(r.outcome, Outcome::Hit);
    let r = calculate_attack(&att, &def, AttackType::Backstab, &mut rolls(&[11]));
    assert_eq!(r.outcome, Outcome::Dodged);

    // Raw 200 - 0 = 200 -> clamps down to 99: roll 100 misses.
    let att = fighter(200, (0, 0), 0);
    let def = fighter(0, (0, 0), 0);
    let r = calculate_attack(&att, &def, AttackType::Backstab, &mut rolls(&[100]));
    assert_eq!(r.outcome, Outcome::Dodged);
}

#[test]
fn helpless_defender_is_nearly_auto_hit() {
    // Defender parry < 0 (HP<1): roll(0,100) > parry+100 -> threshold 99.
    let att = fighter(50, (0, 0), 0);
    let mut def = fighter(0, (200, 200), 0);
    def.parry = -1;
    let r = calculate_attack(&att, &def, AttackType::Normal, &mut rolls(&[100, 42, 100, 5]));
    assert_eq!(r.outcome, Outcome::Hit, "roll 100 > 99 gate, then 42 <= 99 hits");
}

#[test]
fn damage_is_range_roll_minus_tenth_armor() {
    let att = fighter(200, (0, 0), 0);
    let mut def = fighter(0, (10, 10), 0);
    def.armor = 30; // -3
    // to-hit 10 (hits), crit roll 100 (no), damage roll 10 -> 10 - 3 = 7.
    let r = calculate_attack(&att, &def, AttackType::Normal, &mut rolls(&[10, 100, 10]));
    assert_eq!(r.outcome, Outcome::Hit);
    assert_eq!(r.damage, 7);
}

#[test]
fn absorbed_hit_is_no_damage() {
    let att = fighter(200, (0, 0), 0);
    let mut def = fighter(0, (10, 10), 0);
    def.armor = 200; // -20 swallows the 2-10 roll
    let r = calculate_attack(&att, &def, AttackType::Normal, &mut rolls(&[10, 100, 10]));
    assert_eq!(r.outcome, Outcome::NoDamage);
    assert_eq!(r.damage, 0);
}

#[test]
fn critical_raises_floor_and_quadruples_ceiling() {
    // WG3-NT: min = 2*max_old, max = 4*max_old (min 2..max 10 -> 20..40).
    let mut att = fighter(200, (0, 0), 0);
    att.crit_rating = 50; // capped: 40 + (50-40)/3 = 43
    let def = fighter(0, (10, 10), 0);
    // to-hit 10, crit roll 42 (< 43 -> crit), damage roll 40.
    let r = calculate_attack(&att, &def, AttackType::Normal, &mut rolls(&[10, 42, 40]));
    assert_eq!(r.outcome, Outcome::Critical);
    assert_eq!(r.damage, 40);
}

#[test]
fn crit_rating_diminishes_past_forty() {
    let mut att = fighter(200, (0, 0), 0);
    att.crit_rating = 50; // effective 43
    let def = fighter(0, (10, 10), 0);
    let r = calculate_attack(&att, &def, AttackType::Normal, &mut rolls(&[10, 43, 5]));
    assert_eq!(r.outcome, Outcome::Hit, "roll 43 is not < 43");
}

#[test]
fn parry_cancels_the_hit() {
    let att = fighter(160, (0, 0), 0); // acc/8 = 20
    let mut def = fighter(0, (10, 10), 0);
    def.parry = 100; // p = 100*10/20 = 50
    // to-hit 10, crit 100, damage 10, parry roll 49 < 50 -> parried.
    let r = calculate_attack(&att, &def, AttackType::Normal, &mut rolls(&[10, 100, 10, 49]));
    assert_eq!(r.outcome, Outcome::Parried);
    assert_eq!(r.damage, 0);

    let r = calculate_attack(&att, &def, AttackType::Normal, &mut rolls(&[10, 100, 10, 50]));
    assert_eq!(r.outcome, Outcome::Hit, "roll 50 is not < 50");
}

#[test]
fn bash_triples_and_cannot_crit() {
    // Type 6 (WG3-NT): seed 20 (+20% min/max), acc mod -15, dmg *3, no crit.
    let att = fighter(200, (0, 0), 0); // effective acc 185
    let def = fighter(0, (10, 10), 0);
    // min 2 -> 2*120/100 = 2; max 10 -> 12. to-hit, damage roll 12 -> 12*3 = 36.
    let r = calculate_attack(&att, &def, AttackType::Bash, &mut rolls(&[10, 12]));
    assert_eq!(r.outcome, Outcome::Hit);
    assert_eq!(r.damage, 36);
}

#[test]
fn smash_quintuples_with_bonus_seed() {
    // Type 7 (WG3-NT): seed 125 (+125%), acc mod -25, dmg *5, no crit.
    let att = fighter(200, (0, 0), 0);
    let def = fighter(0, (10, 10), 0);
    // min 2 -> 2*225/100 = 4; max 10 -> 22. damage roll 22 -> 110.
    let r = calculate_attack(&att, &def, AttackType::Smash, &mut rolls(&[10, 22]));
    assert_eq!(r.outcome, Outcome::Hit);
    assert_eq!(r.damage, 110);
}

#[test]
fn backstab_uses_direct_threshold_and_weak_parry() {
    // Type 4: threshold = att.accuracy - def.evasion_a (bs compare);
    // seed 10 (+10%); parry / 5; no crit.
    let att = fighter(80, (0, 0), 0);
    let mut def = fighter(0, (30, 0), 0);
    def.parry = 100; // p = 100*10/(80/8) = 100 -> clamp 95 -> /5 = 19
    // threshold = 80 - 30 = 50. Roll 50 hits; dmg roll 11 (max 10*110/100=11);
    // parry roll 19 not < 19 -> hit stands.
    let r = calculate_attack(&att, &def, AttackType::Backstab, &mut rolls(&[50, 11, 19]));
    assert_eq!(r.outcome, Outcome::Hit);
    assert_eq!(r.damage, 11);

    let r = calculate_attack(&att, &def, AttackType::Backstab, &mut rolls(&[50, 11, 18]));
    assert_eq!(r.outcome, Outcome::Parried);
}

/// A roll source that records how many draws were taken, so a test can
/// pin the RNG stream and not just the outcome.
fn counted<'a>(
    values: &'a [i32],
    taken: &'a mut usize,
) -> impl FnMut(i32, i32) -> i32 + use<'a> {
    let mut it = values.iter().copied();
    move |lo, hi| {
        let v = it.next().expect("script exhausted");
        assert!(v >= lo && v <= hi, "scripted roll {v} outside [{lo},{hi}]");
        *taken += 1;
        v
    }
}

#[test]
fn a_parrying_defender_always_costs_a_draw() {
    // 25357: `if ((0 < parry) && (genrdn(0,100) < chance))` — the DLL
    // draws whenever the defender has ANY parry rating, even when the
    // computed chance is 0. Skipping the draw would drift the shared RNG
    // stream for everything that follows.
    //
    // Accuracy 5 is below the 9-point floor at 25344, so the chance IS 0
    // here — and the draw still happens.
    let att = fighter(5, (0, 0), 0);
    let mut def = fighter(0, (10, 10), 0);
    def.parry = 30;
    let mut taken = 0;
    // to-hit 5 (under the den==0 threshold, clamped up to 10), crit 100
    // (none), damage 10,
    // parry draw 0 (0 < 0 is false, so the hit stands).
    let r = calculate_attack(
        &att,
        &def,
        AttackType::Normal,
        &mut counted(&[5, 100, 10, 0], &mut taken),
    );
    assert_eq!(r.outcome, Outcome::Hit);
    assert_eq!(taken, 4, "the parry draw is taken even at chance 0");
}

#[test]
fn accuracy_eight_forces_the_parry_chance_to_zero() {
    // 25344: `if (*param_1 < 9) chance = 0;` — the guard is on the
    // ACCURACY, not on the `accuracy >> 3` denominator, so accuracy 8
    // means no parry at all rather than a denominator of 1.
    let att = fighter(8, (0, 0), 0);
    let mut def = fighter(0, (10, 10), 0);
    def.parry = 30;
    let r = calculate_attack(&att, &def, AttackType::Normal, &mut rolls(&[5, 100, 10, 0]));
    assert_eq!(r.outcome, Outcome::Hit, "accuracy 8 cannot be parried");
    assert_eq!(r.damage, 10);

    // Accuracy 9 is the first that can: denominator 9>>3 = 1, so the
    // chance is `30*10` clamped to the 0x5f cap.
    let att = fighter(9, (0, 0), 0);
    let r = calculate_attack(&att, &def, AttackType::Normal, &mut rolls(&[5, 100, 10, 94]));
    assert_eq!(r.outcome, Outcome::Parried, "94 < the 95 cap");
    let r = calculate_attack(&att, &def, AttackType::Normal, &mut rolls(&[5, 100, 10, 95]));
    assert_eq!(r.outcome, Outcome::Hit, "95 is not < 95");
}
