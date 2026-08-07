//! A tracked position is worth exactly what its provenance is worth.
//! `Option<RoomId>` could not say "unsure", so an unresolvable block left
//! a stale id that read as confirmed — and `lost::place`'s name-only
//! shortcut then confirmed it right back. `Fix` makes the difference a
//! type so no consumer can forget to ask.

use mud_client::lost::Fix;
use mud_core::content::RoomId;

const A: RoomId = RoomId { map: 1, room: 2324 };

#[test]
fn only_a_confirmed_fix_may_be_routed_from() {
    assert_eq!(Fix::Confirmed(A).confirmed(), Some(A));
    assert_eq!(Fix::Stale(A).confirmed(), None);
    assert_eq!(Fix::Unknown.confirmed(), None);
}

#[test]
fn a_stale_fix_still_says_where_we_last_were() {
    assert_eq!(Fix::Confirmed(A).last_known(), Some(A));
    assert_eq!(Fix::Stale(A).last_known(), Some(A));
    assert_eq!(Fix::Unknown.last_known(), None);
}

#[test]
fn demotion_keeps_the_room_but_drops_the_claim() {
    assert_eq!(Fix::Confirmed(A).demote(), Fix::Stale(A));
}

#[test]
fn demotion_is_idempotent_and_cannot_invent_history() {
    assert_eq!(Fix::Stale(A).demote(), Fix::Stale(A));
    assert_eq!(Fix::Unknown.demote(), Fix::Unknown);
}

#[test]
fn a_fresh_client_knows_nothing() {
    assert_eq!(Fix::default(), Fix::Unknown);
}
