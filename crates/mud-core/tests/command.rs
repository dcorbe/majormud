//! Tests for the in-game command parser.
//!
//! M1 command set: movement, look, quit. Matching semantics: case-insensitive;
//! single-letter direction aliases are exact; everything else prefix-matches
//! against the verb table in precedence order (first match wins).

use mud_core::command::{parse, Command};
use mud_core::content::Direction;

#[test]
fn single_letter_direction_aliases() {
    assert_eq!(parse("n"), Command::Move(Direction::North));
    assert_eq!(parse("s"), Command::Move(Direction::South));
    assert_eq!(parse("e"), Command::Move(Direction::East));
    assert_eq!(parse("w"), Command::Move(Direction::West));
    assert_eq!(parse("ne"), Command::Move(Direction::NorthEast));
    assert_eq!(parse("nw"), Command::Move(Direction::NorthWest));
    assert_eq!(parse("se"), Command::Move(Direction::SouthEast));
    assert_eq!(parse("sw"), Command::Move(Direction::SouthWest));
    assert_eq!(parse("u"), Command::Move(Direction::Up));
    assert_eq!(parse("d"), Command::Move(Direction::Down));
}

#[test]
fn full_direction_words() {
    assert_eq!(parse("north"), Command::Move(Direction::North));
    assert_eq!(parse("southwest"), Command::Move(Direction::SouthWest));
    assert_eq!(parse("up"), Command::Move(Direction::Up));
    assert_eq!(parse("down"), Command::Move(Direction::Down));
}

#[test]
fn matching_is_case_insensitive() {
    assert_eq!(parse("NORTH"), Command::Move(Direction::North));
    assert_eq!(parse("Look"), Command::Look);
}

#[test]
fn prefix_matches_resolve_in_precedence_order() {
    // Oracle: cardinal words need their full minimums; "lo" is look's.
    assert_eq!(parse("lo"), Command::Look);
    assert_eq!(parse("look"), Command::Look);
}

#[test]
fn look_and_quit() {
    assert_eq!(parse("look"), Command::Look);
    assert_eq!(parse("l"), Command::Look);
    assert_eq!(parse("quit"), Command::Quit);
    assert_eq!(parse("x"), Command::Quit); // the original's exit alias
}

#[test]
fn whitespace_is_tolerated() {
    assert_eq!(parse("  north  "), Command::Move(Direction::North));
    assert_eq!(parse(""), Command::Blank);
    assert_eq!(parse("   "), Command::Blank);
}

#[test]
fn unknown_input_is_reported_verbatim() {
    assert_eq!(parse("xyzzy"), Command::Unknown("xyzzy".into()));
}

#[test]
fn attack_syntax_is_flexible() {
    // Player testimony: "a kobold thief", "a kobold", or just "a" all work.
    assert_eq!(parse("a"), Command::Attack(String::new()));
    assert_eq!(parse("a kobold"), Command::Attack("kobold".into()));
    assert_eq!(parse("a kobold thief"), Command::Attack("kobold thief".into()));
    assert_eq!(parse("at rat"), Command::Attack("rat".into()));
    assert_eq!(parse("att rat"), Command::Attack("rat".into()));
    assert_eq!(parse("attack rat"), Command::Attack("rat".into()));
    assert_eq!(parse("attack"), Command::Attack(String::new()));
    assert_eq!(parse("A Kobold"), Command::Attack("Kobold".into()));
}

#[test]
fn minimum_abbreviations_match_the_oracle() {
    // Oracle (oracle_ambiguity2.raw): each verb has a minimum abbreviation;
    // anything shorter falls to say.
    assert_eq!(parse("q"), Command::Quit);
    assert_eq!(parse("exp"), Command::Experience);
    assert!(matches!(parse("ex"), Command::Unknown(_)));
    assert_eq!(parse("exi"), Command::Exits);
    assert_eq!(parse("st"), Command::Status);
    assert_eq!(parse("sta"), Command::Status);
    assert_eq!(parse("he"), Command::Health);
    assert_eq!(parse("hel"), Command::Help);
    assert_eq!(parse("to"), Command::Top(String::new()));
    assert!(matches!(parse("t"), Command::Unknown(_)));
    assert_eq!(parse("trai"), Command::Train);
    assert!(matches!(parse("tra"), Command::Unknown(_)));
    assert!(matches!(parse("tr"), Command::Unknown(_)));
    assert_eq!(parse("g"), Command::Get(String::new()));
    assert_eq!(parse("get sil"), Command::Get("sil".into()));
    assert_eq!(parse("ai bob"), Command::Aid("bob".into()));
    assert_eq!(parse("a bob"), Command::Attack("bob".into()));
    assert_eq!(parse("lo"), Command::Look);
    assert!(matches!(parse("h"), Command::Unknown(_)));
}

#[test]
fn direction_word_minimums_match_the_oracle() {
    // Oracle (oracle_directions.raw): hand-authored per-direction minimums.
    // north/south/west require the full word; east resolves at 3 (eat blocks
    // 2); down at 3; up at 2; diagonals at 6.
    assert!(matches!(parse("no"), Command::Unknown(_)));
    assert!(matches!(parse("nor"), Command::Unknown(_)));
    assert!(matches!(parse("nort"), Command::Unknown(_)));
    assert_eq!(parse("north"), Command::Move(Direction::North));
    assert!(matches!(parse("sout"), Command::Unknown(_)));
    assert_eq!(parse("south"), Command::Move(Direction::South));
    assert!(matches!(parse("wes"), Command::Unknown(_)));
    assert_eq!(parse("west"), Command::Move(Direction::West));
    assert!(matches!(parse("ea"), Command::Unknown(_)));
    assert_eq!(parse("eas"), Command::Move(Direction::East));
    assert!(matches!(parse("do"), Command::Unknown(_)));
    assert_eq!(parse("dow"), Command::Move(Direction::Down));
    assert_eq!(parse("up"), Command::Move(Direction::Up));
    assert_eq!(parse("northe"), Command::Move(Direction::NorthEast));
    assert_eq!(parse("southw"), Command::Move(Direction::SouthWest));
    // Exact two-letter aliases keep working regardless of minimums.
    assert_eq!(parse("ne"), Command::Move(Direction::NorthEast));
    assert_eq!(parse("sw"), Command::Move(Direction::SouthWest));
}

#[test]
fn use_and_read_verbs() {
    // ORACLE-VERIFY min abbrevs: unmeasured; use = 2 ("u" stays the up
    // alias), read = 2.
    assert_eq!(parse("us scroll"), Command::Use("scroll".into()));
    assert_eq!(parse("use scroll of blur"), Command::Use("scroll of blur".into()));
    assert_eq!(parse("u"), Command::Move(Direction::Up)); // alias intact
    assert_eq!(parse("re scroll"), Command::Read("scroll".into()));
    assert_eq!(parse("read scroll"), Command::Read("scroll".into()));
}

#[test]
fn read_does_not_shadow_remove() {
    // "rem" is not a prefix of "read", so remove's oracle minimum of 3
    // keeps resolving even with read at min 2.
    assert_eq!(parse("rem cap"), Command::Remove("cap".into()));
    assert_eq!(parse("remove cap"), Command::Remove("cap".into()));
}

#[test]
fn equip_verb_minimums_match_the_oracle() {
    // oracle_m4_verify.raw: "ar dagger" arms, "wi quarterstaff" SAYS,
    // "wie dagger" arms, "eq quarterstaff" arms.
    assert_eq!(parse("ar dagger"), Command::Arm("dagger".into()));
    assert_eq!(parse("wie dagger"), Command::Arm("dagger".into()));
    assert_eq!(parse("eq staff"), Command::Arm("staff".into()));
    assert!(!matches!(parse("wi staff"), Command::Arm(_)), "wi is not wield");
}

// --- M7 slice 7: the gang verb surface (gangs.md §1, §5). Min
// abbreviations are ORACLE-VERIFY (parse_command's compiled tree was not
// extracted) — chosen non-colliding against the measured table. ---

#[test]
fn gang_and_guild_parse_with_the_roster_broadgang_split() {
    // Bare = roster, args = broadgang (cmd_broadgang margc split).
    assert_eq!(parse("gang"), Command::Gang(String::new()));
    assert_eq!(parse("guild"), Command::Gang(String::new()));
    assert_eq!(parse("gang hello all"), Command::Gang("hello all".into()));
    assert_eq!(parse("guild hi"), Command::Gang("hi".into()));
    assert_eq!(parse("ga"), Command::Gang(String::new()));
    assert_eq!(parse("gu"), Command::Gang(String::new()));
    // `g` stays get's oracle-measured single-letter match.
    assert_eq!(parse("g"), Command::Get(String::new()));
}

#[test]
fn create_join_leave_disband_parse() {
    assert_eq!(parse("create gang Iron Fist"), Command::Create("gang Iron Fist".into()));
    assert_eq!(parse("cr guild Vex"), Command::Create("guild Vex".into()));
    assert_eq!(parse("join gang Iron Fist"), Command::Join("gang Iron Fist".into()));
    assert_eq!(parse("jo gang X"), Command::Join("gang X".into()));
    // `j` stays jumpkick's single-letter match.
    assert_eq!(parse("j"), Command::JumpKick(String::new()));
    assert_eq!(parse("leave gang"), Command::Leave("gang".into()));
    assert_eq!(parse("le gang"), Command::Leave("gang".into()));
    assert_eq!(parse("disband gang"), Command::Disband("gang".into()));
    assert_eq!(parse("disb gang"), Command::Disband("gang".into()));
    // `dis` stays disarm's measured minimum.
    assert_eq!(parse("dis trap north"), Command::Disarm("trap north".into()));
}

#[test]
fn membership_admin_verbs_parse() {
    assert_eq!(parse("invite Torgo"), Command::Invite("Torgo".into()));
    assert_eq!(parse("invi Torgo"), Command::Invite("Torgo".into()));
    // `in`/`inv` are MEASURED say (§8.12) — invite cannot match below 4.
    assert!(matches!(parse("inv Torgo"), Command::Unknown(_)));
    assert_eq!(parse("uninvite Torgo"), Command::Uninvite("Torgo".into()));
    assert_eq!(parse("un Torgo"), Command::Uninvite("Torgo".into()));
    assert_eq!(parse("promote Torgo"), Command::Promote("Torgo".into()));
    assert_eq!(parse("pr Torgo"), Command::Promote("Torgo".into()));
    assert_eq!(parse("demote Torgo"), Command::Demote("Torgo".into()));
    assert_eq!(parse("de Torgo"), Command::Demote("Torgo".into()));
    // deposit keeps its measured 3.
    assert_eq!(parse("dep 5 gold"), Command::Deposit("5 gold".into()));
}

#[test]
fn gang_shop_verbs_parse() {
    assert_eq!(parse("stock sword"), Command::Stock("sword".into()));
    assert_eq!(parse("sto sword"), Command::Stock("sword".into()));
    // `st` stays status.
    assert_eq!(parse("st"), Command::Status);
    assert_eq!(parse("unstock sword"), Command::Unstock("sword".into()));
    assert_eq!(parse("uns sword"), Command::Unstock("sword".into()));
    assert_eq!(parse("markup 120"), Command::Markup("120".into()));
    assert_eq!(parse("ma 120"), Command::Markup("120".into()));
}

#[test]
fn top_takes_arguments_for_the_gangs_arm() {
    assert_eq!(parse("top"), Command::Top(String::new()));
    assert_eq!(parse("top 5 gangs"), Command::Top("5 gangs".into()));
    assert_eq!(parse("to gangs"), Command::Top("gangs".into()));
}
