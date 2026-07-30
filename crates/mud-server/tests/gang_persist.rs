//! M7 slice 7: gangs survive a board restart (gangs.md §0 — the WCCGANG2
//! record + the player-row membership truth, rebuilt through StateDb).

use mud_core::gang::{Gang, GANG_DISBANDED, GF_LIEUTENANT};
use mud_core::game::{Core, CoreConfig};
use mud_server::state_db::StateDb;

#[test]
fn gangs_survive_a_restart() {
    let db = StateDb::open_in_memory().expect("state db");

    // Session 1: two gangs exist — one live with a lieutenant on the
    // roster, one disbanded but not yet drained (§0 step 5: the row
    // stays until every offline member logs in).
    let mut live = Gang::new("Iron Fist", "Salad", 1_753_900_000);
    live.exp_pool = 42_000;
    live.member_count = 2;
    db.save_gang(&live).expect("save live");
    let mut dead = Gang::new("Old Guard", "Ghost", 1_700_000_000);
    dead.flags |= GANG_DISBANDED;
    db.save_gang(&dead).expect("save disbanded");

    let mut member = test_player("Torgo");
    member.gang = "Iron Fist".into();
    member.gang_flags = GF_LIEUTENANT;
    db.save_player(&member).expect("save member");

    // Restart: the server boot path — load gangs and the membership
    // scan, hand both to the Core.
    let config = CoreConfig {
        restored_gangs: db.load_gangs().expect("load gangs"),
        restored_gang_members: db.load_gang_members().expect("scan members"),
        ..CoreConfig::default()
    };
    let core = Core::new(mud_core::content::Content::default(), config);

    let restored = core.gang("Iron Fist").expect("live gang restored");
    assert_eq!(restored.exp_pool, 42_000);
    assert_eq!(restored.member_count, 2);
    assert_eq!(restored.leader, "Salad");

    let dead = core.gang("OLD GUARD").expect("disbanded row survives");
    assert!(dead.is_disbanded(), "flags ride through the restart");
}

fn test_player(name: &str) -> mud_core::game::Player {
    mud_core::game::Player {
        name: name.into(),
        current_hp: 10,
        hunger: 1000,
        thirst: 1000,
        lives: 9,
        ..Default::default()
    }
}
