//! Drain tests: throwing away the events queued before a command goes
//! out. A drain that leaves anything behind is a correctness bug, not a
//! tidiness one — `Navigator::goto` drains before every step precisely
//! so that a stale room block cannot satisfy the next step's arrival
//! check, and a false arrival is silent position drift.

use tokio::sync::broadcast;

use mud_client::events::Event;
use mud_client::session::drain;

fn prompt(hp: i32) -> Event {
    Event::Prompt { hp, mana: None }
}

#[test]
fn drain_empties_a_channel() {
    let (tx, mut rx) = broadcast::channel(8);
    for hp in 1..=3 {
        tx.send(prompt(hp)).unwrap();
    }

    drain(&mut rx);

    assert!(matches!(
        rx.try_recv(),
        Err(broadcast::error::TryRecvError::Empty)
    ));
}

/// A lagged receiver is *not* an empty one. Tokio drops the oldest
/// messages and reports `Lagged`, but the next receive then returns the
/// oldest message still retained — so a drain that stops at the first
/// `Err` walks away with stale events still queued, which is the exact
/// thing it exists to prevent.
#[test]
fn drain_empties_a_channel_that_lagged() {
    let (tx, mut rx) = broadcast::channel(2);
    for hp in 1..=5 {
        tx.send(prompt(hp)).unwrap();
    }

    drain(&mut rx);

    assert!(
        matches!(rx.try_recv(), Err(broadcast::error::TryRecvError::Empty)),
        "the drain stopped at the lag and left stale events behind"
    );
}
