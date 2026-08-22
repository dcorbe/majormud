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

/// The meter takes its value from OUR inventory reply.
#[test]
fn an_inventory_reply_sets_the_purse() {
    let mut m = PurseMeter::default();
    assert_eq!(m.current(), Purse::ZERO);
    m.expect_reply(); // we just sent `i`
    assert!(m.observe("2 gold crowns, 8 copper farthings"));
    assert_eq!(m.current().farthings(), 208);
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
    assert!(!m.observe("You are carrying Nothing!"));
    assert_eq!(m.current(), Purse::ZERO);
    assert!(m.settled(), "we asked and got an answer");
}
