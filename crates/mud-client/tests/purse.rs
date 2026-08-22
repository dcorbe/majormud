use mud_client::purse::{parse_coin_line, Purse};

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
