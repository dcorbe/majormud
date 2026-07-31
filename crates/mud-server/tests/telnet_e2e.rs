//! End-to-end test: TCP client connects, creates an account and character,
//! looks around, walks, quits.

use mud_core::content::{
    Class, ClassId, Content, Direction, Exit, Item, ItemId, Race, RaceId, Room, RoomId, Shop,
    ShopId, ShopStock, StatBlock,
};
use mud_core::game::CoreConfig;
use mud_server::server::Server;
use mud_server::state_db::StateDb;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

fn world() -> Content {
    let mut content = Content::default();
    let mut gates = Room {
        id: RoomId { map: 1, room: 1 },
        name: "Town Gates".into(),
        description: vec!["You are before the massive town gates.".into()],
        room_type: 0,
        attributes: 0,
        shop: None,
        placed_items: vec![],
        exits: Default::default(),
        ..Default::default()
    };
    gates.exits[Direction::North as usize] = Some(Exit {
        dest: RoomId { map: 1, room: 2 },
        exit_type: 0,
        trigger_msg: None,
        ..Default::default()
    });
    let mut square = Room {
        id: RoomId { map: 1, room: 2 },
        name: "Town Square".into(),
        description: vec![],
        room_type: 1,
        attributes: 0,
        shop: Some(ShopId(45)),
        placed_items: vec![],
        exits: Default::default(),
        ..Default::default()
    };
    square.exits[Direction::South as usize] = Some(Exit {
        dest: RoomId { map: 1, room: 1 },
        exit_type: 0,
        trigger_msg: None,
        ..Default::default()
    });
    content.add_room(gates);
    content.add_room(square);
    content.add_item(Item {
        id: ItemId(100),
        name: "quarterstaff".into(),
        weight: 100,
        item_type: 1,
        uses: -1,
        min_damage: 2,
        max_damage: 12,
        weapon_type: 1,
        gettable: 1,
        speed: 1200,
        ..Item::default()
    });
    let mut stock = [ShopStock::default(); 20];
    stock[0] = ShopStock {
        item: Some(ItemId(100)),
        max: 5,
        now: 5,
        restock_time: 0,
        restock_amount: 0,
        restock_percent: 0,
    };
    content.add_shop(Shop {
        id: ShopId(45),
        name: "General Store".into(),
        shop_type: 1,
        min_level: 0,
        max_level: 0,
        markup: 50,
        class_limit: 0,
        stock,
    });
    content.add_race(Race {
        id: RaceId(1),
        name: "Human".into(),
        abilities: vec![],
        base_stats: StatBlock {
            intellect: 40,
            wisdom: 40,
            strength: 40,
            health: 40,
            agility: 40,
            charm: 40,
        },
        max_stats: StatBlock {
            intellect: 100,
            wisdom: 100,
            strength: 100,
            health: 100,
            agility: 100,
            charm: 100,
        },
        cp: 100,
        hp_per_level: 0,
        exp_chart: 30,
    });
    content.add_class(Class {
        id: ClassId(1),
        name: "Warrior".into(),
        abilities: vec![],
        hp_per_level: 6,
        hp_seed: 4,
        caster_group: 0,
        casting_factor: 0,
        exp_base: 0,
        combat_factor: 6,
        weapon_code: 8,
        armour_code: 9,
    });
    content
}


fn test_config() -> CoreConfig {
    CoreConfig {
        start_location: RoomId { map: 1, room: 1 },
        exit_meditation_seconds: 1,
        ..CoreConfig::default()
    }
}

/// The shipped binary runs a command round; fixtures normally do not.
fn round_config() -> CoreConfig {
    CoreConfig {
        command_round_seconds: 1,
        ..test_config()
    }
}

/// The shipped binary also scrambles direction words; fixtures do not.
fn noisy_config() -> CoreConfig {
    CoreConfig {
        wire_noise: true,
        ..test_config()
    }
}

/// Reads from the socket until `needle` appears in the accumulated transcript
/// (with a timeout so failures are readable, not hangs).
async fn read_until(stream: &mut TcpStream, transcript: &mut String, needle: &str) {
    let deadline = tokio::time::Instant::now() + tokio::time::Duration::from_secs(5);
    let mut buf = [0u8; 4096];
    while !transcript.contains(needle) {
        let n = tokio::time::timeout_at(deadline, stream.read(&mut buf))
            .await
            .unwrap_or_else(|_| panic!("timed out waiting for {needle:?}; got:\n{transcript}"))
            .expect("read");
        assert!(n > 0, "connection closed waiting for {needle:?}; got:\n{transcript}");
        // Strip telnet IAC sequences crudely for the transcript (test-side).
        transcript.push_str(&String::from_utf8_lossy(&buf[..n]).replace('\u{fffd}', ""));
    }
}

async fn send(stream: &mut TcpStream, line: &str) {
    stream
        .write_all(format!("{line}\r\n").as_bytes())
        .await
        .expect("write");
}

// Telnet bytes, for the negotiation tests below.
const IAC: u8 = 255;
const WILL: u8 = 251;
const WONT: u8 = 252;
const DONT: u8 = 254;
const OPT_ECHO: u8 = 1;
const OPT_SGA: u8 = 3;

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

/// Like `read_until`, but keeps the bytes intact: the lossy transcript
/// above turns every IAC into U+FFFD and then throws it away, so
/// negotiation is invisible to it.
async fn read_raw_until(stream: &mut TcpStream, raw: &mut Vec<u8>, needle: &[u8]) {
    let deadline = tokio::time::Instant::now() + tokio::time::Duration::from_secs(5);
    let mut buf = [0u8; 4096];
    while find(raw, needle).is_none() {
        let n = tokio::time::timeout_at(deadline, stream.read(&mut buf))
            .await
            .unwrap_or_else(|_| {
                panic!(
                    "timed out waiting for {:?}; got:\n{}",
                    String::from_utf8_lossy(needle),
                    String::from_utf8_lossy(raw)
                )
            })
            .expect("read");
        assert!(n > 0, "connection closed waiting for {needle:?}");
        raw.extend_from_slice(&buf[..n]);
    }
}

/// Drives account creation through to the first in-realm prompt.
async fn create_and_enter_raw(stream: &mut TcpStream, raw: &mut Vec<u8>, name: &str) {
    read_raw_until(stream, raw, b"Account: ").await;
    send(stream, name).await;
    read_raw_until(stream, raw, b"Create new account? (y/n)").await;
    send(stream, "y").await;
    read_raw_until(stream, raw, b"Password: ").await;
    send(stream, "pw").await;
    read_raw_until(stream, raw, b"Gender (M/F): ").await;
    send(stream, "M").await;
    read_raw_until(stream, raw, b"race").await;
    send(stream, "1").await;
    read_raw_until(stream, raw, b"class").await;
    send(stream, "1").await;
    read_raw_until(stream, raw, b"Lawful").await;
    send(stream, "No").await;
    read_raw_until(stream, raw, b"[HP=").await;
}

/// The board suppresses the password echo by negotiating telnet ECHO;
/// ours must do the same, or a caller's password appears on their screen.
#[tokio::test]
async fn negotiates_sga_and_suppresses_the_password_echo() {
    let state = StateDb::open_in_memory().expect("state db");
    state
        .create_account("Dora", "right", mud_core::game::Gender::Female)
        .expect("account");
    let server = Server::start(world(), test_config(), state, "127.0.0.1:0")
        .await
        .expect("start server");

    let mut stream = TcpStream::connect(server.local_addr())
        .await
        .expect("connect");
    let mut raw = Vec::new();

    read_raw_until(&mut stream, &mut raw, b"Account: ").await;
    let sga = find(&raw, &[IAC, WILL, OPT_SGA]).expect("IAC WILL SGA at connect");
    let account = find(&raw, b"Account: ").expect("account prompt");
    assert!(sga < account, "negotiation opens the connection");

    send(&mut stream, "Dora").await;
    read_raw_until(&mut stream, &mut raw, b"Password: ").await;
    let will_echo = find(&raw, &[IAC, WILL, OPT_ECHO]).expect("IAC WILL ECHO before the password");
    let password = find(&raw, b"Password: ").expect("password prompt");
    assert!(will_echo < password, "echo is taken over before we ask");

    send(&mut stream, "right").await;
    read_raw_until(&mut stream, &mut raw, &[IAC, WONT, OPT_ECHO]).await;
    let wont_echo = find(&raw, &[IAC, WONT, OPT_ECHO]).expect("IAC WONT ECHO after the password");
    assert!(wont_echo > will_echo, "echo is handed back afterwards");
    assert!(
        !raw.windows(5).any(|w| w == b"right"),
        "the password must never be echoed: {}",
        String::from_utf8_lossy(&raw)
    );
}

/// Same suppression on the account-creation password, which is a
/// separate read in the login dialogue.
#[tokio::test]
async fn negotiates_echo_around_the_creation_password() {
    let state = StateDb::open_in_memory().expect("state db");
    let server = Server::start(world(), test_config(), state, "127.0.0.1:0")
        .await
        .expect("start server");

    let mut stream = TcpStream::connect(server.local_addr())
        .await
        .expect("connect");
    let mut raw = Vec::new();

    read_raw_until(&mut stream, &mut raw, b"Account: ").await;
    send(&mut stream, "Edmund").await;
    read_raw_until(&mut stream, &mut raw, b"Create new account? (y/n)").await;
    send(&mut stream, "y").await;
    read_raw_until(&mut stream, &mut raw, b"Password: ").await;
    assert!(
        find(&raw, &[IAC, WILL, OPT_ECHO]).is_some(),
        "creation asks for a password with echo suppressed"
    );
    send(&mut stream, "s3cret").await;
    read_raw_until(&mut stream, &mut raw, b"Gender (M/F): ").await;
    assert!(
        find(&raw, &[IAC, WONT, OPT_ECHO]).is_some(),
        "echo comes back before the next question"
    );
    assert!(
        find(&raw, b"s3cret").is_none(),
        "the creation password must never be echoed: {}",
        String::from_utf8_lossy(&raw)
    );
}

/// Resolve backspaces the way a period client does, so a test can check
/// what the caller actually sees on their screen.
fn resolve_backspaces(bytes: &[u8]) -> Vec<u8> {
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    for &b in bytes {
        match b {
            0x08 if !matches!(out.last(), None | Some(b'\n') | Some(b'\r')) => {
                out.pop();
            }
            0x08 => {}
            _ => out.push(b),
        }
    }
    out
}

/// The board hides a junk character and a backspace inside every
/// direction word it prints, so a scraper reading "Obvious exits: north"
/// finds "nO\x08orth" instead. MEASURED across the whole oracle corpus:
/// 14515 insertions, every one of them at offset 1 of a direction word,
/// on exits lines and movement lines alike.
#[tokio::test]
async fn direction_words_carry_the_anti_bot_backspace() {
    let state = StateDb::open_in_memory().expect("state db");
    let server = Server::start(world(), noisy_config(), state, "127.0.0.1:0")
        .await
        .expect("start server");

    let mut stream = TcpStream::connect(server.local_addr())
        .await
        .expect("connect");
    let mut raw = Vec::new();
    create_and_enter_raw(&mut stream, &mut raw, "Jonas").await;
    raw.clear();

    send(&mut stream, "look").await;
    read_raw_until(&mut stream, &mut raw, b"Obvious exits").await;
    // Wait for the whole line to land.
    read_raw_until(&mut stream, &mut raw, b"\r\n").await;

    assert!(
        raw.contains(&0x08),
        "the exits line is scrambled: {}",
        String::from_utf8_lossy(&raw)
    );
    assert!(
        find(&raw, b"north").is_none(),
        "a scraper cannot read the direction straight off the wire: {raw:?}"
    );
    // But a client that resolves backspaces sees the real thing.
    let seen = resolve_backspaces(&raw);
    assert!(
        find(&seen, b"Obvious exits: north").is_some(),
        "and a real terminal shows it intact: {}",
        String::from_utf8_lossy(&seen)
    );
}

/// The command echo is the caller's own text coming back; the board
/// leaves it alone (every full-word direction echo in the corpus is
/// clean), and scrambling it would fight the client's echo matching.
#[tokio::test]
async fn the_echo_is_never_scrambled() {
    let state = StateDb::open_in_memory().expect("state db");
    let server = Server::start(world(), noisy_config(), state, "127.0.0.1:0")
        .await
        .expect("start server");

    let mut stream = TcpStream::connect(server.local_addr())
        .await
        .expect("connect");
    let mut raw = Vec::new();
    create_and_enter_raw(&mut stream, &mut raw, "Kara").await;
    raw.clear();

    send(&mut stream, "north").await;
    read_raw_until(&mut stream, &mut raw, b"Town Square").await;
    assert!(
        find(&raw, b"north\r\n").is_some(),
        "the echo comes back verbatim: {}",
        String::from_utf8_lossy(&raw)
    );
}

/// Fixtures get a clean stream by default.
#[tokio::test]
async fn wire_noise_is_off_unless_asked_for() {
    let state = StateDb::open_in_memory().expect("state db");
    let server = Server::start(world(), test_config(), state, "127.0.0.1:0")
        .await
        .expect("start server");

    let mut stream = TcpStream::connect(server.local_addr())
        .await
        .expect("connect");
    let mut raw = Vec::new();
    create_and_enter_raw(&mut stream, &mut raw, "Lena").await;
    raw.clear();
    send(&mut stream, "look").await;
    read_raw_until(&mut stream, &mut raw, b"Obvious exits: north").await;
    assert!(
        !raw.contains(&0x08),
        "no backspaces in a quiet fixture: {raw:?}"
    );
}

/// Pipelining two commands into a board that runs a round produces the
/// live board's double echo: both receipt echoes go out at once, the
/// first command answers, and then the second is echoed AGAIN, right
/// before its own reply. That adjacency is what the client's correlator
/// attributes replies by, and until now only its scripted TCP boards
/// could produce it.
#[tokio::test]
async fn a_queued_command_is_echoed_again_before_its_reply() {
    let state = StateDb::open_in_memory().expect("state db");
    let server = Server::start(world(), round_config(), state, "127.0.0.1:0")
        .await
        .expect("start server");

    let mut stream = TcpStream::connect(server.local_addr())
        .await
        .expect("connect");
    let mut raw = Vec::new();
    create_and_enter_raw(&mut stream, &mut raw, "Ivor").await;
    raw.clear();

    // Both in one write: the second cannot help but land inside the
    // first one's round.
    stream
        .write_all(b"look\r\nn\r\n")
        .await
        .expect("write");

    read_raw_until(&mut stream, &mut raw, b"Town Square").await;
    let first_n = find(&raw, b"n\r\n").expect("the receipt echo of n");
    let gates = find(&raw, b"Town Gates").expect("look's reply");
    let square = find(&raw, b"Town Square").expect("n's reply");
    let second_n = find(&raw[gates..], b"n\r\n")
        .map(|i| i + gates)
        .expect("the execution echo of n");

    assert!(
        first_n < gates,
        "the receipt echo goes out before anything is answered: {}",
        String::from_utf8_lossy(&raw)
    );
    assert!(
        gates < second_n && second_n < square,
        "the second echo sits between the previous reply and its own: {}",
        String::from_utf8_lossy(&raw)
    );
}

/// A caller who mistypes and backspaces sends the correction as bytes;
/// 0x08 used to land in the command text and make the verb unrecognizable.
/// Both DEL and BS erase, and neither eats past the start of the line.
#[tokio::test]
async fn backspace_edits_the_line_before_the_command_is_read() {
    let state = StateDb::open_in_memory().expect("state db");
    let server = Server::start(world(), test_config(), state, "127.0.0.1:0")
        .await
        .expect("start server");

    let mut stream = TcpStream::connect(server.local_addr())
        .await
        .expect("connect");
    let mut raw = Vec::new();
    create_and_enter_raw(&mut stream, &mut raw, "Hilda").await;

    for typed in [
        &b"lokk\x08\x08ok\r\n"[..],  // backspace
        &b"lokk\x7f\x7fok\r\n"[..],  // DEL
        &b"\x08\x08look\r\n"[..],    // nothing to erase yet
    ] {
        raw.clear();
        stream.write_all(typed).await.expect("write");
        read_raw_until(&mut stream, &mut raw, b"Obvious exits: north").await;
        assert!(
            find(&raw, b"look\r\n").is_some(),
            "the corrected command reaches the core: {}",
            String::from_utf8_lossy(&raw)
        );
        assert!(
            find(&raw, b"You say").is_none(),
            "an edited line must not fall through to SAY: {}",
            String::from_utf8_lossy(&raw)
        );
    }
}

/// The board is a DOS program talking to DOS terminals: every byte on
/// the wire is CP437, not UTF-8. Decoding input as UTF-8 turned the high
/// half into replacement characters.
#[tokio::test]
async fn high_bytes_are_cp437_in_both_directions() {
    let state = StateDb::open_in_memory().expect("state db");
    let server = Server::start(world(), test_config(), state, "127.0.0.1:0")
        .await
        .expect("start server");

    let mut stream = TcpStream::connect(server.local_addr())
        .await
        .expect("connect");
    let mut raw = Vec::new();
    create_and_enter_raw(&mut stream, &mut raw, "Gwen").await;
    raw.clear();

    // 0x82 is é in CP437; as UTF-8 it is not a character at all.
    stream
        .write_all(b"say caf\x82\r\n")
        .await
        .expect("write");
    read_raw_until(&mut stream, &mut raw, b"You say").await;
    assert!(
        find(&raw, b"caf\x82").is_some(),
        "the accented byte comes back as itself: {raw:?}"
    );
    assert!(
        find(&raw, "café".as_bytes()).is_none(),
        "and never as UTF-8: {raw:?}"
    );
}

/// Now that we negotiate, clients answer — and their answers can land
/// split across packets, mid-command. A half-arrived IAC sequence must
/// wait for the rest of itself instead of leaking option bytes into the
/// command text.
#[tokio::test]
async fn telnet_replies_split_across_packets_never_reach_the_command() {
    let state = StateDb::open_in_memory().expect("state db");
    let server = Server::start(world(), test_config(), state, "127.0.0.1:0")
        .await
        .expect("start server");

    let mut stream = TcpStream::connect(server.local_addr())
        .await
        .expect("connect");
    let mut raw = Vec::new();
    create_and_enter_raw(&mut stream, &mut raw, "Fitz").await;
    raw.clear();

    // "look", then a refusal dribbled in one byte at a time, then the
    // rest of the line. The board sees the command; we see its reply.
    for chunk in [
        &b"loo"[..],
        &[IAC][..],
        &[DONT][..],
        &[OPT_ECHO][..],
        &b"k\r\n"[..],
    ] {
        stream.write_all(chunk).await.expect("write");
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }

    read_raw_until(&mut stream, &mut raw, b"Obvious exits: north").await;
    assert!(
        find(&raw, b"look\r\n").is_some(),
        "the command reassembles around the negotiation: {}",
        String::from_utf8_lossy(&raw)
    );
}

#[tokio::test]
async fn full_session_create_walk_quit() {
    let state = StateDb::open_in_memory().expect("state db");
    let server = Server::start(world(), test_config(), state, "127.0.0.1:0")
        .await
        .expect("start server");
    let addr = server.local_addr();

    let mut stream = TcpStream::connect(addr).await.expect("connect");
    let mut t = String::new();

    read_until(&mut stream, &mut t, "Account: ").await;
    send(&mut stream, "Alice").await;

    read_until(&mut stream, &mut t, "Create new account? (y/n)").await;
    send(&mut stream, "y").await;

    read_until(&mut stream, &mut t, "Password: ").await;
    send(&mut stream, "hunter2").await;

    read_until(&mut stream, &mut t, "Gender (M/F): ").await;
    send(&mut stream, "F").await;

    read_until(&mut stream, &mut t, "Please choose a race from the following list:").await;
    send(&mut stream, "1").await;

    read_until(&mut stream, &mut t, "Please choose a class from the following list:").await;
    send(&mut stream, "1").await;

    read_until(&mut stream, &mut t, "Do you want to be Lawful?").await;
    send(&mut stream, "No").await;

    // Oracle flow: creation ends on the stat sheet + prompt, not the room.
    read_until(&mut stream, &mut t, "Hits:").await;
    read_until(&mut stream, &mut t, "[HP=").await;
    send(&mut stream, "look").await;
    read_until(&mut stream, &mut t, "Town Gates").await;
    read_until(&mut stream, &mut t, "Obvious exits: north").await;
    // The board echoes every accepted line, and the reply follows the
    // echo — the client's request/response correlation stands on this,
    // so the fixture pins it in its own suite.
    let echo = t.find("look\r\n").expect("the accepted command is echoed");
    let room = t.rfind("Town Gates").expect("room render");
    assert!(echo < room, "the echo must precede the reply");

    send(&mut stream, "n").await;
    read_until(&mut stream, &mut t, "Town Square").await;

    send(&mut stream, "x").await;
    // Server closes the connection after quit.
    let mut buf = [0u8; 256];
    loop {
        match tokio::time::timeout(std::time::Duration::from_secs(5), stream.read(&mut buf))
            .await
            .expect("timely close")
            .expect("read")
        {
            0 => break,
            _ => continue,
        }
    }
}

#[tokio::test]
async fn returning_player_resumes_saved_character() {
    let state = StateDb::open_in_memory().expect("state db");
    let server = Server::start(world(), test_config(), state, "127.0.0.1:0")
        .await
        .expect("start server");
    let addr = server.local_addr();

    // First visit: create and walk north, then quit (position persists).
    let mut stream = TcpStream::connect(addr).await.expect("connect");
    let mut t = String::new();
    read_until(&mut stream, &mut t, "Account: ").await;
    send(&mut stream, "Bob").await;
    read_until(&mut stream, &mut t, "Create new account? (y/n)").await;
    send(&mut stream, "y").await;
    read_until(&mut stream, &mut t, "Password: ").await;
    send(&mut stream, "pw").await;
    read_until(&mut stream, &mut t, "Gender (M/F): ").await;
    send(&mut stream, "M").await;
    read_until(&mut stream, &mut t, "race").await;
    send(&mut stream, "1").await;
    read_until(&mut stream, &mut t, "class").await;
    send(&mut stream, "1").await;
    read_until(&mut stream, &mut t, "Lawful").await;
    send(&mut stream, "No").await;
    read_until(&mut stream, &mut t, "[HP=").await;
    send(&mut stream, "look").await;
    read_until(&mut stream, &mut t, "Town Gates").await;
    send(&mut stream, "n").await;
    read_until(&mut stream, &mut t, "Town Square").await;
    send(&mut stream, "x").await;
    // Wait for the meditation to finish and the server to close (persist
    // completes before we reconnect — avoids racing the save).
    let mut buf = [0u8; 256];
    loop {
        match tokio::time::timeout(std::time::Duration::from_secs(5), stream.read(&mut buf))
            .await
            .expect("timely close")
            .expect("read")
        {
            0 => break,
            _ => continue,
        }
    }

    // Second visit: no creation, resumes in Town Square.
    let mut stream = TcpStream::connect(addr).await.expect("connect");
    let mut t = String::new();
    read_until(&mut stream, &mut t, "Account: ").await;
    send(&mut stream, "Bob").await;
    read_until(&mut stream, &mut t, "Password: ").await;
    send(&mut stream, "pw").await;
    read_until(&mut stream, &mut t, "Town Square").await;
    assert!(
        !t.contains("choose a race"),
        "no creation on return: {t}"
    );
}

#[tokio::test]
async fn wrong_password_is_rejected() {
    let state = StateDb::open_in_memory().expect("state db");
    state
        .create_account("Carol", "right", mud_core::game::Gender::Female)
        .expect("account");
    let server = Server::start(world(), test_config(), state, "127.0.0.1:0")
        .await
        .expect("start server");

    let mut stream = TcpStream::connect(server.local_addr()).await.expect("connect");
    let mut t = String::new();
    read_until(&mut stream, &mut t, "Account: ").await;
    send(&mut stream, "Carol").await;
    read_until(&mut stream, &mut t, "Password: ").await;
    send(&mut stream, "wrong").await;
    read_until(&mut stream, &mut t, "Invalid credentials.").await;
}

/// Full restart cycle: a shelf dented on one server run stays dented when
/// a second server boots from the same state.sqlite (the original's
/// Btrieve shop persistence).
#[tokio::test]
async fn shop_shelves_persist_across_restart() {
    let path = std::env::temp_dir().join(format!(
        "mud_e2e_stock_{}.sqlite",
        std::process::id()
    ));
    let _ = std::fs::remove_file(&path);

    {
        let state = StateDb::open(&path).expect("state db");
        let server = Server::start(world(), test_config(), state, "127.0.0.1:0")
            .await
            .expect("start server");
        let mut stream = TcpStream::connect(server.local_addr()).await.expect("connect");
        let mut t = String::new();
        read_until(&mut stream, &mut t, "Account: ").await;
        send(&mut stream, "Cora").await;
        read_until(&mut stream, &mut t, "Create new account? (y/n)").await;
        send(&mut stream, "y").await;
        read_until(&mut stream, &mut t, "Password: ").await;
        send(&mut stream, "pw").await;
        read_until(&mut stream, &mut t, "Gender (M/F): ").await;
        send(&mut stream, "F").await;
        read_until(&mut stream, &mut t, "race").await;
        send(&mut stream, "1").await;
        read_until(&mut stream, &mut t, "class").await;
        send(&mut stream, "1").await;
        read_until(&mut stream, &mut t, "Lawful").await;
        send(&mut stream, "No").await;
        read_until(&mut stream, &mut t, "[HP=").await;
        send(&mut stream, "n").await;
        read_until(&mut stream, &mut t, "Town Square").await;
        send(&mut stream, "buy quarterstaff").await;
        read_until(&mut stream, &mut t, "You just bought quarterstaff for nothing.").await;
        send(&mut stream, "list").await;
        read_until(&mut stream, &mut t, "quarterstaff                  4").await;
    }

    {
        let state = StateDb::open(&path).expect("reopen state db");
        let server = Server::start(world(), test_config(), state, "127.0.0.1:0")
            .await
            .expect("restart server");
        let mut stream = TcpStream::connect(server.local_addr()).await.expect("connect");
        let mut t = String::new();
        read_until(&mut stream, &mut t, "Account: ").await;
        send(&mut stream, "Bram").await;
        read_until(&mut stream, &mut t, "Create new account? (y/n)").await;
        send(&mut stream, "y").await;
        read_until(&mut stream, &mut t, "Password: ").await;
        send(&mut stream, "pw").await;
        read_until(&mut stream, &mut t, "Gender (M/F): ").await;
        send(&mut stream, "M").await;
        read_until(&mut stream, &mut t, "race").await;
        send(&mut stream, "1").await;
        read_until(&mut stream, &mut t, "class").await;
        send(&mut stream, "1").await;
        read_until(&mut stream, &mut t, "Lawful").await;
        send(&mut stream, "No").await;
        read_until(&mut stream, &mut t, "[HP=").await;
        send(&mut stream, "n").await;
        read_until(&mut stream, &mut t, "Town Square").await;
        send(&mut stream, "list").await;
        read_until(&mut stream, &mut t, "quarterstaff                  4").await;
        assert!(
            !t.contains("quarterstaff                  5"),
            "shelf came back full after restart: {t:?}"
        );
    }

    let _ = std::fs::remove_file(&path);
}
