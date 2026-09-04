//! Generates the `Ability` enum from the reverse-engineered ability table.
//!
//! Source of truth: `ability_ids.tsv` beside this file (id, name, description),
//! a copy of the table recovered in `re/docs/`. One description (id 42)
//! contains a raw newline, so rows are stitched back together before parsing:
//! a line that does not start with `<digits>\t` is a continuation of the
//! previous row's description.

use std::env;
use std::fmt::Write as _;
use std::fs;
use std::path::Path;

const TSV_PATH: &str = "ability_ids.tsv";

struct Row {
    id: u16,
    name: String,
    description: String,
}

fn main() {
    println!("cargo::rerun-if-changed={TSV_PATH}");
    let raw = fs::read_to_string(TSV_PATH).expect("read ability_ids.tsv");
    let rows = parse_rows(&raw);

    for (expected_id, row) in rows.iter().enumerate() {
        assert_eq!(
            usize::from(row.id),
            expected_id,
            "ability ids must be contiguous from 0"
        );
    }

    let variants: Vec<String> = rows.iter().map(|r| variant_ident(&r.name)).collect();
    {
        let mut sorted = variants.clone();
        sorted.sort();
        sorted.dedup();
        assert_eq!(sorted.len(), variants.len(), "variant names must be unique");
    }

    let code = generate(&rows, &variants);
    let out = Path::new(&env::var("OUT_DIR").unwrap()).join("ability_generated.rs");
    fs::write(out, code).expect("write ability_generated.rs");
}

fn parse_rows(raw: &str) -> Vec<Row> {
    let mut rows: Vec<Row> = Vec::new();
    for line in raw.lines().skip(1) {
        if is_row_start(line) {
            let mut fields = line.splitn(3, '\t');
            let id = fields.next().unwrap().parse().unwrap();
            let name = fields.next().expect("name field").to_owned();
            let description = fields.next().unwrap_or("").to_owned();
            rows.push(Row {
                id,
                name,
                description,
            });
        } else {
            let prev = rows.last_mut().expect("continuation before first row");
            prev.description.push(' ');
            prev.description.push_str(line);
        }
    }
    rows
}

fn is_row_start(line: &str) -> bool {
    match line.split_once('\t') {
        Some((first, _)) => !first.is_empty() && first.bytes().all(|b| b.is_ascii_digit()),
        None => false,
    }
}

/// `"AC(Blur)"` → `ACBlur`, `"Alter thirst"` → `AlterThirst`, `"empty"` → `Empty`.
/// Alphanumerics are kept (original capitalization preserved); every other
/// character is dropped and capitalizes the next kept character.
fn variant_ident(name: &str) -> String {
    let mut ident = String::new();
    let mut upper_next = true;
    for c in name.chars() {
        if c.is_ascii_alphanumeric() {
            ident.push(if upper_next { c.to_ascii_uppercase() } else { c });
            upper_next = false;
        } else {
            upper_next = true;
        }
    }
    assert!(
        ident.starts_with(|c: char| c.is_ascii_alphabetic()),
        "variant for {name:?} must start with a letter, got {ident:?}"
    );
    ident
}

fn generate(rows: &[Row], variants: &[String]) -> String {
    let n = rows.len();
    let mut code = String::new();
    code.push_str(
        "/// One entry of the game's shared ability table (`re/docs/abilities.md`).\n\
         ///\n\
         /// Every effect in the game — spells, items, race/class bonuses, monster\n\
         /// abilities — is an `(Ability, value)` pair dispatched through one code path.\n\
         #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]\n\
         #[repr(u16)]\n\
         pub enum Ability {\n",
    );
    for (row, variant) in rows.iter().zip(variants) {
        writeln!(code, "    #[doc = {:?}]", row.description).unwrap();
        writeln!(code, "    {variant} = {},", row.id).unwrap();
    }
    code.push_str("}\n\nimpl Ability {\n");
    writeln!(code, "    pub const COUNT: usize = {n};\n").unwrap();
    writeln!(code, "    const ALL: [Ability; {n}] = [").unwrap();
    for variant in variants {
        writeln!(code, "        Ability::{variant},").unwrap();
    }
    code.push_str("    ];\n\n");
    writeln!(code, "    const NAMES: [&'static str; {n}] = [").unwrap();
    for row in rows {
        writeln!(code, "        {:?},", row.name).unwrap();
    }
    code.push_str(
        "    ];\n\n\
         \x20   /// Looks up an ability by its id as stored in game data.\n\
         \x20   pub fn from_id(id: u16) -> Option<Ability> {\n\
         \x20       Self::ALL.get(usize::from(id)).copied()\n\
         \x20   }\n\n\
         \x20   /// The id as stored in game data.\n\
         \x20   pub fn id(self) -> u16 {\n\
         \x20       self as u16\n\
         \x20   }\n\n\
         \x20   /// The name exactly as it appears in the ability table.\n\
         \x20   pub fn name(self) -> &'static str {\n\
         \x20       Self::NAMES[self as usize]\n\
         \x20   }\n\
         }\n",
    );
    code
}
