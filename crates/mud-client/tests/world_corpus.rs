//! The room model against real board output.
//!
//! `tests/world.rs` drives `Here` with hand-written events, and
//! hand-written events are always tidier than a board: they carry no
//! second player killing things out from under the model, no free-form
//! per-monster death wording, no mid-render arrivals. This suite replays
//! the captured transcripts through the same wire and parser stack the
//! live client uses and asks the model how often it was wrong.
//!
//! **One approximation, and it is load-bearing.** A `.raw` holds RX only,
//! so there is no send stream to attribute against and no `Correlator`
//! can run. Every room block is therefore fed as ATTRIBUTED. For this
//! measurement that is the honest choice: attribution exists to reject
//! blocks describing another room and answers to forgotten asks, and the
//! first of those is now handled by the model's own room-identity guard.
//! It does mean the replay believes blocks a live client would ignore,
//! so the numbers here are an upper bound on what a live run would see.

use mud_client::correlate::{CmdId, Correlated};
use mud_client::events::Event;
use mud_client::parse::Parser;
use mud_client::wire::{TelnetFilter, cp437_to_string};
use mud_client::world::{DivergenceKind, Here};

const CORPUS: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../re/oracle");

fn events(path: &std::path::Path) -> Vec<Event> {
    let raw = std::fs::read(path).unwrap();
    let mut f = TelnetFilter::new();
    let data = f.push(&raw).data;
    let mut p = Parser::new();
    let mut ev = p.push(&cp437_to_string(&data));
    ev.extend(p.finish());
    ev
}

/// Replay one transcript into a fresh model. Room blocks are attributed
/// (see the module note); everything else rides in unsolicited, which is
/// what the live client does with async truths anyway.
fn with_deaths() {
    let db = concat!(env!("CARGO_MANIFEST_DIR"), "/../../re/mmud_wgnt.sqlite");
    let _ = mud_client::deaths::init(std::path::Path::new(db));
}

fn replay(path: &std::path::Path) -> Here {
    with_deaths();
    let mut here = Here::default();
    let start = std::time::Instant::now();
    for (i, event) in events(path).into_iter().enumerate() {
        let answers = matches!(event, Event::RoomSeen(_)).then_some(CmdId(1));
        here.on_event(
            &Correlated { event, answers },
            start + std::time::Duration::from_millis(i as u64),
        );
    }
    here
}

fn corpus_files() -> Vec<std::path::PathBuf> {
    let mut files: Vec<_> = std::fs::read_dir(CORPUS)
        .unwrap()
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "raw"))
        .collect();
    files.sort();
    files
}

/// Print the tally for every capture, worst first. Not an assertion —
/// this is the measurement the stage-1 gate is read from.
#[test]
#[ignore = "measurement instrument, not an assertion: run with --ignored --nocapture"]
fn measure_the_corpus() {
    let mut rows: Vec<(String, u32, u32)> = corpus_files()
        .iter()
        .map(|p| {
            let here = replay(p);
            (
                p.file_name().unwrap().to_string_lossy().into_owned(),
                here.reconcile.count(DivergenceKind::OccupantExtra),
                here.reconcile.count(DivergenceKind::OccupantMissing),
            )
        })
        .collect();
    rows.sort_by_key(|(_, extra, _)| std::cmp::Reverse(*extra));
    let (te, tm): (u32, u32) = rows
        .iter()
        .fold((0, 0), |(a, b), (_, e, m)| (a + e, b + m));
    println!("\n{:<44} {:>7} {:>9}", "capture", "extra", "missing");
    for (name, e, m) in rows.iter().take(15) {
        println!("{name:<44} {e:>7} {m:>9}");
    }
    println!("{:<44} {te:>7} {tm:>9}  <- TOTAL", format!("({} captures)", rows.len()));
}

/// The same stretch with the death lexicon NOT loaded — the control for
/// the measurement above. Run it alone (`--ignored` with this exact
/// name) so the process-wide lexicon stays empty; running the whole file
/// initialises it and this becomes a duplicate of the other test.
#[test]
#[ignore = "control for measure_a_stationary_shared_room; must run alone"]
fn measure_a_stationary_shared_room_without_the_lexicon() {
    let (blocks, extra, missing) = stationary_arena(false);
    println!("\nNO LEXICON  blocks {blocks}  extra {extra}  missing {missing}");
}

/// The shared-room question, measured where it can be measured.
///
/// The corpus replay above is only sound while the character stays put:
/// `Here` keys room identity on the NAME the board prints, and a maze of
/// same-named rooms (528 blocks called "Sewer Tunnel" in
/// oracle_engage_lock_emptysweep) reads every step as a re-render. The
/// live farm never sees that — it builds a fresh model per stop — so
/// those numbers measure the harness.
///
/// A stationary stretch has no such artifact. This one asks the actual
/// stage-2 blocker: with another player killing things in the same room,
/// how often does the model keep a corpse the board has already removed?
#[test]
#[ignore = "measurement instrument, not an assertion: run with --ignored --nocapture"]
fn measure_a_stationary_shared_room() {
    let (blocks, extra, missing) = stationary_arena(true);
    println!("\nWITH LEXICON  blocks {blocks}  extra {extra}  missing {missing}");
}

fn stationary_arena(lexicon: bool) -> (usize, u32, u32) {
    if lexicon {
        with_deaths();
    }
    let capture = std::env::var("MMC_CAPTURE").unwrap_or_else(|_| {
        concat!(env!("CARGO_MANIFEST_DIR"), "/../../cwrun2.raw").to_string()
    });
    let path = std::path::Path::new(&capture);
    if !path.exists() {
        println!("{} absent (gitignored capture); set MMC_CAPTURE", path.display());
        return (0, 0, 0);
    }
    let mut here = Here::default();
    let start = std::time::Instant::now();
    let mut blocks = 0usize;
    for (i, event) in events(path).into_iter().enumerate() {
        // Only the Arena stretch: one real room, character stationary.
        if let Event::RoomSeen(r) = &event {
            if r.name != "Newhaven, Arena" {
                continue;
            }
            blocks += 1;
        }
        let answers = matches!(event, Event::RoomSeen(_)).then_some(CmdId(1));
        here.on_event(
            &Correlated { event, answers },
            start + std::time::Duration::from_millis(i as u64),
        );
    }
    (
        blocks,
        here.reconcile.count(DivergenceKind::OccupantExtra),
        here.reconcile.count(DivergenceKind::OccupantMissing),
    )
}
