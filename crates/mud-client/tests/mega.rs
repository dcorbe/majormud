//! Reading MegaMud path files, against the mirrored megamud.net corpus
//! under `docs/mirrors/`.

use mud_client::mega::{name_hash, parse_mp, slug};

fn paths_dir() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../docs/mirrors/megamud.net/paths")
}

fn read(name: &str) -> String {
    std::fs::read_to_string(paths_dir().join(name)).expect("corpus file")
}

// --- the codec --------------------------------------------------------

#[test]
fn the_name_hash_is_the_last_three_hex_digits_of_the_weighted_sum() {
    // 'A' is 65, times position 1.
    assert_eq!(name_hash("A"), "041");
    // Short sums are zero-padded to three.
    assert_eq!(name_hash(""), "000");
    assert_eq!(name_hash("Slum Street, Crossroads"), "ABC");
}

// --- the parser -------------------------------------------------------

#[test]
fn a_path_with_both_header_lines_parses() {
    let mp = parse_mp(&read("rocsloop.mp")).expect("parse");
    assert_eq!(mp.name, "Narrow Plateau");
    assert_eq!(mp.author.as_deref(), Some("Connor Macleod"));
    assert_eq!(mp.start, "F0601040");
    assert_eq!(mp.steps.len(), 72);
    assert_eq!(mp.steps[0].command, "w");
}

/// `slm2loop.mp` has one bracket group and no author, and its goto-tree
/// lines are missing entirely. A bare title still has to be read as one.
#[test]
fn a_path_with_only_a_title_parses() {
    let mp = parse_mp(&read("slm2loop.mp")).expect("parse");
    assert_eq!(mp.name, "Slum loop with HO's in it");
    assert_eq!(mp.author, None);
    assert_eq!(mp.steps.len(), 82);
}

/// `delfcity.mp` is LF-only with an unbalanced bracket in its title.
#[test]
fn a_path_with_neither_crlf_nor_a_tidy_title_parses() {
    let mp = parse_mp(&read("delfcity.mp")).expect("parse");
    assert_eq!(mp.steps.len(), 71);
}

#[test]
fn something_that_is_not_a_path_is_refused() {
    assert!(parse_mp("hello\nworld\n").is_err());
    assert!(parse_mp("").is_err());
}

#[test]
fn a_title_becomes_a_usable_file_name() {
    assert_eq!(slug("Narrow Plateau"), "narrow-plateau");
    assert_eq!(slug("Dark Elf City)"), "dark-elf-city");
    assert_eq!(slug("Slum loop with HO's in it"), "slum-loop-with-ho-s-in-it");
    assert_eq!(slug("!!!"), "imported");
}
