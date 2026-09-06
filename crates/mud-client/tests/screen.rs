//! A window's in-memory terminal.

use mud_client::screen::Screen;

#[test]
fn board_bytes_land_on_the_screen_with_colours_kept_in_the_snapshot() {
    let mut s = Screen::new(5, 20, 100);
    s.feed(b"Welcome\r\n\x1b[31mred\x1b[0m line\r\n");
    let text = s.text();
    assert!(text.starts_with("Welcome\n"), "{text:?}");
    assert!(text.contains("red line"), "{text:?}");
    let snap = s.snapshot();
    assert_eq!(snap.cell(1, 0).map(|c| c.fgcolor()), Some(vt100::Color::Idx(1)));
}

#[test]
fn a_note_is_its_own_line_and_newlines_are_translated() {
    let mut s = Screen::new(5, 40, 100);
    s.feed(b"prompt> ");
    s.note("-- one --\nsecond");
    let text = s.text();
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(lines[0].trim_end(), "prompt>");
    assert_eq!(lines[1], "-- one --");
    assert_eq!(lines[2], "second");
}

#[test]
fn the_scrollback_is_capped_and_pages() {
    let mut s = Screen::new(3, 10, 4);
    for i in 0..20 {
        s.feed(format!("line{i}\r\n").as_bytes());
    }
    assert!(!s.scrolled());
    s.page_up();
    assert!(s.scrolled());
    assert!(s.text().contains("line16"), "one page up shows the rows just above the screen: {:?}", s.text());
    let after_one = s.text();
    s.page_up();
    s.page_up();
    assert_ne!(s.text(), after_one, "page_up keeps moving up to the cap");
    assert!(!s.text().contains("line1\n"), "the cap of 4 rows keeps line1 out of reach: {:?}", s.text());
    s.to_bottom();
    assert!(!s.scrolled());
    assert!(s.text().contains("line19"));
    s.page_up();
    s.page_down();
    assert!(!s.scrolled());
}

#[test]
fn resize_keeps_the_content() {
    let mut s = Screen::new(4, 20, 10);
    s.feed(b"keep me\r\n");
    s.resize(6, 30);
    assert_eq!(s.size(), (6, 30));
    assert!(s.text().contains("keep me"));
}

