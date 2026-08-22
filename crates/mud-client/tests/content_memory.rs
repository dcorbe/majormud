//! Measures the resident cost of a fully loaded `Content` — the number
//! Daniel asked Task 2 of `2026-08-22-one-path-to-content` to report.
//! Not acted on: no narrowing, no optimisation, see the plan's Task 2.
//!
//! Run with `--nocapture` to see the reported figure:
//! `CARGO_BUILD_JOBS=3 cargo test -p mud-client --test content_memory -- --nocapture`

fn db_path() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../re/mmud_wgnt.sqlite")
}

/// `VmRSS` from `/proc/self/status`, in kilobytes. Linux-only, which is
/// fine: this is a one-off measurement, not a portability requirement.
fn vm_rss_kb() -> u64 {
    let status = std::fs::read_to_string("/proc/self/status").expect("read /proc/self/status");
    for line in status.lines() {
        if let Some(rest) = line.strip_prefix("VmRSS:") {
            return rest.trim().trim_end_matches(" kB").trim().parse().expect("VmRSS kB");
        }
    }
    panic!("no VmRSS line in /proc/self/status");
}

#[test]
fn a_fully_loaded_content_s_resident_cost() {
    let before = vm_rss_kb();
    let content = mud_core::content_db::load(&db_path()).expect("load content");

    // Sanity: these are the counts Task 2's report is measured against
    // (spec `2026-08-22-one-path-to-content-design.md`).
    assert_eq!(content.rooms.len(), 26720);
    assert_eq!(content.items.len(), 1950);
    assert_eq!(content.monsters.len(), 1101);
    assert_eq!(content.spells.len(), 1379);
    assert_eq!(content.messages.len(), 3867);
    assert_eq!(content.textblocks.len(), 3267);

    let after = vm_rss_kb();
    eprintln!(
        "Content resident cost: {} kB -> {} kB (delta {} kB, {:.1} MB)",
        before,
        after,
        after.saturating_sub(before),
        after.saturating_sub(before) as f64 / 1024.0,
    );

    // Held past the measurement point on purpose — a `Content` dropped
    // immediately after `load` would not answer "what does the client
    // pay to hold this", which is the question Task 2 asks.
    std::hint::black_box(&content);
}
