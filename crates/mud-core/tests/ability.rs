//! Tests for the generated `Ability` enum (from `re/docs/ability_ids.tsv`).

use mud_core::ability::Ability;

#[test]
fn count_is_188() {
    assert_eq!(Ability::COUNT, 188);
}

#[test]
fn known_ids_map_to_expected_variants() {
    assert_eq!(Ability::from_id(0), Some(Ability::Empty));
    assert_eq!(Ability::from_id(1), Some(Ability::Damage));
    assert_eq!(Ability::from_id(187), Some(Ability::Meditate));
}

#[test]
fn out_of_range_id_is_none() {
    assert_eq!(Ability::from_id(188), None);
    assert_eq!(Ability::from_id(u16::MAX), None);
}

#[test]
fn id_round_trips_for_every_ability() {
    for id in 0..188u16 {
        let ability = Ability::from_id(id).expect("ids 0..188 are contiguous");
        assert_eq!(ability.id(), id);
    }
}

#[test]
fn name_returns_original_tsv_spelling() {
    assert_eq!(Ability::Empty.name(), "empty");
    assert_eq!(Ability::from_id(10).unwrap().name(), "AC(Blur)");
    assert_eq!(Ability::from_id(17).unwrap().name(), "Damage(-MR)");
    assert_eq!(Ability::from_id(16).unwrap().name(), "Alter thirst");
}

#[test]
fn embedded_newline_row_survives_parsing() {
    // Row 42's description contains a raw newline in the tsv; the row after
    // it must still resolve correctly.
    assert_eq!(Ability::from_id(42).unwrap().name(), "LearnSp");
    assert_eq!(Ability::from_id(43).unwrap().name(), "CastsSp");
}
