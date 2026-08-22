use mud_client::purse::{parse_coin_line, Purse, PurseMeter};

/// The ladder, ORACLE-VERIFIED off the Bank of Godfrey lobby sign:
/// 10 copper = 1 silver, 10 silver = 1 gold, 100 gold = 1 platinum,
/// 100 platinum = 1 runic.
#[test]
fn the_denomination_ladder_is_the_bank_of_godfreys() {
    assert_eq!(Purse::from_gold(1).farthings(), 100);
    assert_eq!(Purse::from_gold(5).farthings(), 500, "the Silvermere toll");
    assert_eq!(
        Purse::from_gold(10_000).farthings(),
        1_000_000,
        "map 17: 10,000 gold is exactly one runic coin"
    );
}

/// The board prints coins high denomination first, comma separated, and
/// singularises at one. Every form it can print must parse.
#[test]
fn a_coin_listing_parses_to_farthings() {
    for (line, want) in [
        ("1 copper farthing", 1u64),
        ("9 copper farthings", 9),
        ("1 silver noble", 10),
        ("1 gold crown, 9 copper farthings", 109),
        ("2 gold crowns, 8 copper farthings", 208),
        ("1 platinum piece", 10_000),
        ("1 runic coin", 1_000_000),
        (
            "1 runic coin, 1 platinum piece, 1 gold crown, 1 silver noble, 1 copper farthing",
            1_010_111,
        ),
    ] {
        assert_eq!(
            parse_coin_line(line).map(|p| p.farthings()),
            Some(want),
            "parsing {line:?}"
        );
    }
}

/// A line that is not a coin listing must not parse as an empty purse —
/// "you have no money" and "this line is about something else" are
/// different facts and the caller acts differently on each.
#[test]
fn a_line_that_is_not_coins_does_not_parse() {
    for line in [
        "You notice a rusty dagger here.",
        "Obvious exits: north, south",
        "",
    ] {
        assert_eq!(parse_coin_line(line), None, "parsing {line:?}");
    }
}

/// The meter takes its value from OUR inventory reply. The real wire
/// shape, per `mud-core`'s `show_inventory`: wrapped in "You are
/// carrying ", not a bare coin line.
#[test]
fn an_inventory_reply_sets_the_purse() {
    let mut m = PurseMeter::default();
    assert_eq!(m.current(), Purse::ZERO);
    m.expect_reply(); // we just sent `i`
    assert!(m.observe("You are carrying 2 gold crowns, 8 copper farthings"));
    assert_eq!(m.current().farthings(), 208);
}

/// The real shape: coins first, then worn gear, then a weapon, then
/// grouped loose items, all comma-joined on ONE line
/// (`crates/mud-core/src/game.rs:12840-12892`). The board never sends a
/// bare coin line for an inventory reply, so the meter must read past
/// the coins to the gear without either choking on it or counting it.
#[test]
fn a_mixed_carry_line_parses_only_the_leading_coins() {
    let mut m = PurseMeter::default();
    m.expect_reply();
    assert!(m.observe(
        "You are carrying 2 gold crowns, 8 copper farthings, a rusty dagger (worn), 3 sickle"
    ));
    assert_eq!(m.current().farthings(), 208, "gear after the coins must not touch the total");
}

/// Carrying gear but no money at all is a real, distinct answer: zero
/// carried, not "we don't know". The board doesn't say "Nothing!" here
/// because there IS something -- just no coins among it. `observe`
/// returns `true`: this line WAS the attributed answer, it just says
/// "zero".
#[test]
fn a_carry_with_no_coins_at_all_reads_as_empty() {
    let mut m = PurseMeter::default();
    m.expect_reply();
    assert!(m.observe("You are carrying a rusty dagger"));
    assert_eq!(m.current(), Purse::ZERO);
    assert!(m.settled(), "we asked and got an answer");
}

/// A LATER empty answer must overwrite a non-zero balance, not leave it
/// stale. Every other fixture in this file starts from
/// `PurseMeter::default()`, which is already zero -- "reset to zero" and
/// "leave unchanged" are indistinguishable there. This one starts from a
/// real balance and spends it, so the two behaviours diverge: the
/// dangerous bug is the router still believing the character has 208
/// farthings after the board just said otherwise.
#[test]
fn a_later_empty_reply_resets_a_nonzero_purse() {
    let mut m = PurseMeter::default();
    m.expect_reply();
    assert!(m.observe("You are carrying 2 gold crowns, 8 copper farthings"));
    assert_eq!(m.current().farthings(), 208);

    m.expect_reply();
    assert!(m.observe("You are carrying Nothing!"));
    assert_eq!(
        m.current(),
        Purse::ZERO,
        "spending it all must clear the purse, not leave it at 208"
    );
}

/// Same divergence, for a later reply that has gear but no coins rather
/// than the literal "Nothing!" wording.
#[test]
fn a_later_gear_only_reply_resets_a_nonzero_purse() {
    let mut m = PurseMeter::default();
    m.expect_reply();
    assert!(m.observe("You are carrying 2 gold crowns, 8 copper farthings"));
    assert_eq!(m.current().farthings(), 208);

    m.expect_reply();
    assert!(m.observe("You are carrying a rusty dagger"));
    assert_eq!(
        m.current(),
        Purse::ZERO,
        "carrying no coins now must not leave the old 208 balance"
    );
}

/// The board always joins coins first, so a real reply never has this
/// shape -- but the parser must not depend on that being true by
/// accident. A coin-shaped segment AFTER a non-coin segment is not
/// LEADING and must not count: `leading_coins` has to `break` at the
/// first non-coin segment, never `continue` past it hunting for one that
/// matches. (Confirmed by mutation: flipping that `break` to `continue`
/// turns this answer into 300 and fails the assertion below.)
#[test]
fn a_non_leading_coin_segment_does_not_count() {
    let mut m = PurseMeter::default();
    m.expect_reply();
    assert!(m.observe("You are carrying a rusty dagger, 3 gold crowns"));
    assert_eq!(
        m.current(),
        Purse::ZERO,
        "a coin-shaped segment after a non-coin one is not leading and must not count"
    );
}

/// The nastiest false positive available: an item whose own name STARTS
/// with a denomination word. `bot.rs`'s `COIN_PILE_RE` documents "silver
/// holy amulet" as exactly this trap (live in oracle_charm_lifecycle) --
/// its leading-count requirement is what keeps a bare denomination word
/// from being misread as one more coin entry.
#[test]
fn an_item_that_looks_like_a_coin_name_is_not_money() {
    let mut m = PurseMeter::default();
    m.expect_reply();
    assert!(m.observe("You are carrying 2 gold crowns, 8 copper farthings, silver holy amulet"));
    assert_eq!(
        m.current().farthings(),
        208,
        "the amulet must not read as an extra denomination"
    );
}

/// Coins lying on the floor are NOT the purse. Walking into a room with
/// money in it must not rewrite the balance -- the router would then
/// refuse a toll the character can actually afford.
#[test]
fn coins_on_the_floor_do_not_touch_the_purse() {
    let mut m = PurseMeter::default();
    m.expect_reply();
    assert!(m.observe("2 gold crowns, 8 copper farthings"));
    let before = m.current();
    // No pending inventory request now; anything coin-shaped is somebody
    // else's money.
    assert!(!m.observe("3 gold crowns"));
    assert!(!m.observe("You notice 11 silver nobles here."));
    assert_eq!(m.current(), before, "the floor is not the purse");
}

/// An inventory with no coins in it means ZERO carried, which is a real
/// answer and different from "we have never asked".
#[test]
fn an_inventory_without_coins_reads_as_empty() {
    let mut m = PurseMeter::default();
    m.expect_reply();
    assert!(m.observe("You are carrying Nothing!"));
    assert_eq!(m.current(), Purse::ZERO);
    assert!(m.settled(), "we asked and got an answer");
}
