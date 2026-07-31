//! The telnet edge: async I/O per connection, one synchronous game-core
//! thread. Connections do login/account work against the state database,
//! then bridge lines to the core over channels. No game logic lives here.

use std::io;
use std::net::SocketAddr;
use std::time::{SystemTime, UNIX_EPOCH};
use std::sync::mpsc as std_mpsc;
use std::sync::{Arc, Mutex};

use mud_core::content::Content;
use mud_core::cp437;
use mud_core::game::{AccountProfile, Core, CoreConfig, Event, Gender, Player, SessionId};
use tokio::io::{AsyncReadExt, AsyncWriteExt, BufWriter};
use tokio::net::tcp::{OwnedReadHalf, OwnedWriteHalf};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{mpsc, oneshot};

use crate::state_db::{CreateAccountError, StateDb};

/// Telnet protocol bytes. We speak only enough of it to do what the
/// board does: announce SGA once, and take echo away around password
/// entry. There is no option state machine — see `read_password`.
const IAC: u8 = 255;
const SE: u8 = 240;
const SB: u8 = 250;
const WILL: u8 = 251;
const WONT: u8 = 252;
const OPT_ECHO: u8 = 1;
const OPT_SGA: u8 = 3;

/// Line editing bytes.
const BS: u8 = 0x08;
const DEL: u8 = 0x7f;

/// Messages into the game-core thread.
enum CoreMsg {
    AttachSaved {
        player: Box<Player>,
        out: mpsc::UnboundedSender<OutMsg>,
        reply: oneshot::Sender<SessionId>,
    },
    AttachNew {
        profile: AccountProfile,
        out: mpsc::UnboundedSender<OutMsg>,
        reply: oneshot::Sender<SessionId>,
    },
    Input {
        session: SessionId,
        line: String,
    },
    Detach {
        session: SessionId,
    },
    /// One second of game time (the background_fast metronome).
    Tick,
}

/// Messages from the core to a connection task.
enum OutMsg {
    Text(String),
    Close,
}

pub struct Server {
    addr: SocketAddr,
}

impl Server {
    /// Binds `addr`, spawns the core thread and the accept loop, and returns.
    pub async fn start(
        content: Content,
        config: CoreConfig,
        state: StateDb,
        addr: &str,
    ) -> io::Result<Server> {
        Self::start_with_spawns(content, config, state, addr, Vec::new()).await
    }

    /// `start` plus dev fixture monster spawns (kept alongside the M6
    /// spawner as the test/staging placement path).
    pub async fn start_with_spawns(
        content: Content,
        config: CoreConfig,
        state: StateDb,
        addr: &str,
        spawns: Vec<(u16, u16, u16)>,
    ) -> io::Result<Server> {
        let listener = TcpListener::bind(addr).await?;
        let local = listener.local_addr()?;
        let state = Arc::new(Mutex::new(state));
        let (core_tx, core_rx) = std_mpsc::channel::<CoreMsg>();

        {
            let state = Arc::clone(&state);
            std::thread::Builder::new()
                .name("game-core".into())
                .spawn(move || core_thread(content, config, state, core_rx, spawns))
                .expect("spawn core thread");
        }

        {
            let core_tx = core_tx.clone();
            tokio::spawn(async move {
                let mut interval = tokio::time::interval(std::time::Duration::from_secs(1));
                interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
                loop {
                    interval.tick().await;
                    if core_tx.send(CoreMsg::Tick).is_err() {
                        break;
                    }
                }
            });
        }

        tokio::spawn(async move {
            while let Ok((socket, _)) = listener.accept().await {
                let core_tx = core_tx.clone();
                let state = Arc::clone(&state);
                tokio::spawn(async move {
                    // Connection errors just end that connection.
                    let _ = handle_connection(socket, core_tx, state).await;
                });
            }
        });

        Ok(Server { addr: local })
    }

    pub fn local_addr(&self) -> SocketAddr {
        self.addr
    }
}

/// Wall-clock seconds since the Unix epoch, for the spawn persistence
/// stamps (the core deals only in elapsed seconds).
fn wall_now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// The single-threaded game loop: applies messages, routes events.
fn core_thread(
    content: Content,
    config: CoreConfig,
    state: Arc<Mutex<StateDb>>,
    rx: std_mpsc::Receiver<CoreMsg>,
    spawns: Vec<(u16, u16, u16)>,
) {
    let mut config = config;
    {
        // Restored spawn persistence must land in the config BEFORE
        // Core::new — the boot population walk consults it.
        let db = state.lock().expect("state db lock");
        let now = wall_now();
        match db.load_monster_kills() {
            Ok(rows) => {
                config.restored_population = rows
                    .into_iter()
                    .map(|(t, at)| (mud_core::content::MonsterId(t), (now - at).max(0)))
                    .collect();
            }
            Err(e) => eprintln!("failed to load monster kills: {e}"),
        }
        match db.load_room_stamps() {
            Ok(rows) => {
                config.restored_room_stamps = rows
                    .into_iter()
                    .map(|(map, room, at)| {
                        (mud_core::content::RoomId { map, room }, (now - at).max(0))
                    })
                    .collect();
            }
            Err(e) => eprintln!("failed to load room stamps: {e}"),
        }
        match db.load_gangs() {
            Ok(gangs) => config.restored_gangs = gangs,
            Err(e) => eprintln!("failed to load gangs: {e}"),
        }
        match db.load_gang_members() {
            Ok(members) => config.restored_gang_members = members,
            Err(e) => eprintln!("failed to load gang members: {e}"),
        }
        config.wall_base = now;
    }
    let mut core = Core::new(content, config);
    {
        let db = state.lock().expect("state db lock");
        match db.load_shop_stock() {
            Ok(rows) => {
                let rows: Vec<_> = rows
                    .into_iter()
                    .map(|(shop, slot, now)| (mud_core::content::ShopId(shop), slot, now))
                    .collect();
                core.restore_shop_stock(&rows);
            }
            Err(e) => eprintln!("failed to load shop stock: {e}"),
        }
        match db.load_gang_shops() {
            Ok(rows) => {
                let rows: Vec<_> = rows
                    .into_iter()
                    .map(|(shop, state)| (mud_core::content::ShopId(shop), state))
                    .collect();
                core.restore_gang_shops(&rows);
            }
            Err(e) => eprintln!("failed to load gang shops: {e}"),
        }
    }
    for (id, map, room) in spawns {
        use mud_core::content::{MonsterId, RoomId};
        if core
            .spawn_monster(MonsterId(id), RoomId { map, room })
            .is_none()
        {
            eprintln!("--spawn: unknown monster {id} or room {map},{room}");
        }
    }
    let mut outputs: std::collections::HashMap<SessionId, mpsc::UnboundedSender<OutMsg>> =
        std::collections::HashMap::new();

    while let Ok(msg) = rx.recv() {
        match msg {
            CoreMsg::AttachSaved { player, out, reply } => {
                let id = core.attach_player(*player);
                outputs.insert(id, out);
                let _ = reply.send(id);
            }
            CoreMsg::AttachNew { profile, out, reply } => {
                let id = core.attach_account(profile);
                outputs.insert(id, out);
                let _ = reply.send(id);
            }
            CoreMsg::Input { session, line } => core.input(session, &line),
            CoreMsg::Detach { session } => core.detach(session),
            CoreMsg::Tick => core.tick(),
        }
        for event in core.drain_events() {
            match event {
                Event::Output { session, text } => {
                    if let Some(out) = outputs.get(&session) {
                        let _ = out.send(OutMsg::Text(text));
                    }
                }
                Event::Persist(player) => {
                    let db = state.lock().expect("state db lock");
                    if let Err(e) = db.save_player(&player) {
                        eprintln!("failed to persist {}: {e}", player.name);
                    }
                }
                Event::PersistShopStock { shop, counts } => {
                    let db = state.lock().expect("state db lock");
                    if let Err(e) = db.save_shop_stock(shop.0, &counts) {
                        eprintln!("failed to persist shop {} stock: {e}", shop.0);
                    }
                }
                Event::PersistMonsterKill { template } => {
                    let db = state.lock().expect("state db lock");
                    if let Err(e) = db.save_monster_kill(template.0, wall_now()) {
                        eprintln!("failed to persist kill of {}: {e}", template.0);
                    }
                }
                Event::PersistRoomStamp { room } => {
                    let db = state.lock().expect("state db lock");
                    if let Err(e) = db.save_room_stamp(room.map, room.room, wall_now()) {
                        eprintln!("failed to persist stamp {}/{}: {e}", room.map, room.room);
                    }
                }
                Event::PersistGang(gang) => {
                    let db = state.lock().expect("state db lock");
                    if let Err(e) = db.save_gang(&gang) {
                        eprintln!("failed to persist gang {}: {e}", gang.display);
                    }
                }
                Event::PersistOfflineGangMember { name, or_mask, and_mask, clear_gang } => {
                    let db = state.lock().expect("state db lock");
                    let result = if clear_gang {
                        db.clear_player_gang(&name)
                    } else {
                        db.set_player_gang_flags(&name, or_mask, and_mask)
                    };
                    if let Err(e) = result {
                        eprintln!("failed offline gang write for {name}: {e}");
                    }
                }
                Event::PersistGangShop { shop, state: shelves } => {
                    let db = state.lock().expect("state db lock");
                    if let Err(e) = db.save_gang_shop(shop.0, &shelves) {
                        eprintln!("failed to persist gang shop {}: {e}", shop.0);
                    }
                }
                Event::DepositGangGold { stocker, copper } => {
                    let db = state.lock().expect("state db lock");
                    if let Err(e) = db.deposit_gang_gold(&stocker, copper) {
                        eprintln!("failed gang deposit for {stocker}: {e}");
                    }
                }
                Event::DeleteCharacter { name, fame } => {
                    let db = state.lock().expect("state db lock");
                    if let Err(e) = db.bank_evil(&name, fame, wall_now()) {
                        eprintln!("failed to bank evil for {name}: {e}");
                    }
                    if let Err(e) = db.delete_player(&name) {
                        eprintln!("failed to delete {name}: {e}");
                    }
                }
                Event::Disconnect(session) => {
                    if let Some(out) = outputs.remove(&session) {
                        let _ = out.send(OutMsg::Close);
                    }
                }
            }
        }
    }
}

/// Reads telnet input: strips IAC command sequences, yields complete lines.
struct TelnetReader {
    socket: OwnedReadHalf,
    buf: Vec<u8>,
    line: Vec<u8>,
    /// The previous line ended on CR; swallow one following LF or NUL.
    after_cr: bool,
}

impl TelnetReader {
    fn new(socket: OwnedReadHalf) -> Self {
        TelnetReader {
            socket,
            buf: Vec::new(),
            line: Vec::new(),
            after_cr: false,
        }
    }

    /// The next input line, or `None` on EOF/error.
    ///
    /// An IAC sequence that has only partly arrived is left in the buffer
    /// until the rest of it turns up. Consuming it early would push the
    /// stragglers into the command text — a real failure now that we
    /// negotiate and clients answer back mid-line.
    async fn read_line(&mut self) -> Option<String> {
        loop {
            // Consume buffered bytes first.
            while !self.buf.is_empty() {
                // A CR ended the previous line; an LF or NUL arriving now
                // completes that terminator and is not a line of its own.
                if std::mem::take(&mut self.after_cr) && matches!(self.buf[0], b'\n' | 0) {
                    self.buf.remove(0);
                    continue;
                }
                match self.buf[0] {
                    IAC => {
                        // The verb byte decides the length; without it we
                        // cannot tell the shapes apart yet.
                        let Some(&verb) = self.buf.get(1) else { break };
                        match verb {
                            // Escaped 0xFF: one literal byte of data.
                            IAC => {
                                self.buf.drain(..2);
                                self.line.push(IAC);
                            }
                            // Subnegotiation runs to IAC SE.
                            SB => match self.subnegotiation_end() {
                                Some(end) => {
                                    self.buf.drain(..end);
                                }
                                None => break,
                            },
                            // Three-byte command (WILL/WONT/DO/DONT/...).
                            // We never answer: see `read_password`.
                            _ => {
                                if self.buf.len() < 3 {
                                    break;
                                }
                                self.buf.drain(..3);
                            }
                        }
                    }
                    b'\r' => {
                        // CR ends the line on its own — the board's
                        // full-screen entry sends a bare one. Any LF or
                        // NUL completing a CRLF/CRNUL pair is swallowed
                        // by the `after_cr` check above, whenever it
                        // arrives, so a split pair is still one line.
                        self.buf.remove(0);
                        self.after_cr = true;
                        return Some(self.take_line());
                    }
                    b'\n' => {
                        self.buf.remove(0);
                        return Some(self.take_line());
                    }
                    // A caller correcting a typo. One CP437 byte is one
                    // character, so erasing a byte erases a character.
                    // The board edits per keystroke because it echoes per
                    // keystroke; we edit the line before reading it,
                    // which is the same simplification our line-level
                    // receipt echo already makes.
                    BS | DEL => {
                        self.buf.remove(0);
                        self.line.pop();
                    }
                    b => {
                        self.buf.remove(0);
                        self.line.push(b);
                    }
                }
            }
            let mut chunk = [0u8; 1024];
            match self.socket.read(&mut chunk).await {
                Ok(0) | Err(_) => return None,
                Ok(n) => self.buf.extend_from_slice(&chunk[..n]),
            }
        }
    }

    /// The offset just past the `IAC SE` closing a subnegotiation at the
    /// head of the buffer, or `None` while it is still arriving.
    fn subnegotiation_end(&self) -> Option<usize> {
        let mut i = 2;
        while i + 1 < self.buf.len() {
            if self.buf[i] == IAC {
                // Escaped 0xFF inside the payload is data, not the end.
                if self.buf[i + 1] == SE {
                    return Some(i + 2);
                }
                i += 2;
            } else {
                i += 1;
            }
        }
        None
    }

    fn take_line(&mut self) -> String {
        let line = cp437::decode(&self.line);
        self.line.clear();
        line
    }
}

/// The one place game text becomes wire bytes: bare LF becomes CRLF (the
/// core writes `\n` throughout) and the result is encoded as CP437, which
/// is what a period client reads no matter what we meant to send.
async fn write_text(writer: &mut BufWriter<OwnedWriteHalf>, text: &str) -> io::Result<()> {
    let bytes = cp437::encode(&text.replace('\n', "\r\n"));
    writer.write_all(&bytes).await?;
    writer.flush().await
}

/// Protocol bytes, which must not go through `write_text`'s newline
/// rewriting.
async fn write_raw(writer: &mut BufWriter<OwnedWriteHalf>, bytes: &[u8]) -> io::Result<()> {
    writer.write_all(bytes).await?;
    writer.flush().await
}

/// Asks for a password with the echo suppressed, the way the board does:
/// `IAC WILL ECHO` claims responsibility for echoing, and since we echo
/// nothing, the typed characters never appear. `IAC WONT ECHO` hands the
/// job back, and a fresh line stands in for the Enter we swallowed.
///
/// Fire-and-forget — we never read the answer. Clients that refuse (our
/// own `mud-client` refuses every option it is offered) still get
/// suppression: what the real board does against a refusing client is
/// unverified, and suppressing regardless is the safe reading of "the
/// board takes echo for passwords".
async fn read_password(
    reader: &mut TelnetReader,
    writer: &mut BufWriter<OwnedWriteHalf>,
) -> io::Result<Option<String>> {
    write_raw(writer, &[IAC, WILL, OPT_ECHO]).await?;
    write_text(writer, "Password: ").await?;
    let password = reader.read_line().await;
    write_raw(writer, &[IAC, WONT, OPT_ECHO]).await?;
    write_text(writer, "\n").await?;
    Ok(password)
}

/// Login dialogue -> attach to core -> bridge lines/output until close.
async fn handle_connection(
    socket: TcpStream,
    core_tx: std_mpsc::Sender<CoreMsg>,
    state: Arc<Mutex<StateDb>>,
) -> io::Result<()> {
    let (read_half, write_half) = socket.into_split();
    let mut reader = TelnetReader::new(read_half);
    let mut writer = BufWriter::new(write_half);

    // Suppress-Go-Ahead: this is a full-duplex stream, not half-duplex
    // line-at-a-time. Announced once and never revisited.
    write_raw(&mut writer, &[IAC, WILL, OPT_SGA]).await?;

    let Some(profile) = login(&mut reader, &mut writer, &state).await? else {
        return Ok(()); // connection dropped during login
    };

    let saved = state
        .lock()
        .expect("state db lock")
        .load_player(&profile.name)
        .ok()
        .flatten();

    let (out_tx, mut out_rx) = mpsc::unbounded_channel::<OutMsg>();
    let (reply_tx, reply_rx) = oneshot::channel();
    let msg = match saved {
        Some(player) => CoreMsg::AttachSaved {
            player: Box::new(player),
            out: out_tx,
            reply: reply_tx,
        },
        None => CoreMsg::AttachNew {
            profile,
            out: out_tx,
            reply: reply_tx,
        },
    };
    if core_tx.send(msg).is_err() {
        return Ok(());
    }
    let session = reply_rx.await.map_err(|_| io::ErrorKind::BrokenPipe)?;

    loop {
        tokio::select! {
            // Biased, output first: a queued reply must reach the wire
            // before the NEXT command's echo, or the fixture would emit
            // "echo A, echo B, reply A" — a schedule the real board
            // never produces.
            biased;
            out = out_rx.recv() => match out {
                Some(OutMsg::Text(text)) => write_text(&mut writer, &text).await?,
                Some(OutMsg::Close) | None => break,
            },
            line = reader.read_line() => match line {
                Some(line) => {
                    // Echo the accepted line back before dispatch, like
                    // the real board: WCCMMUD echoes every line it
                    // accepts, at menus and in the realm alike, and the
                    // reply follows the echo (docs/board-correlation.md,
                    // 94.1% of corpus room blocks). The client's
                    // request/response correlation stands on this, so a
                    // fixture that stayed silent would exercise only the
                    // deadline fallbacks.
                    //
                    // This is the RECEIPT echo. A command that queues
                    // behind the round timer is echoed a SECOND time
                    // when it actually runs; that one comes from the
                    // core, next to the reply it belongs to.
                    //
                    // Recorded simplification: we always emit the
                    // receipt echo, while the board sometimes withholds
                    // it — a command arriving mid-round can sit unread
                    // until the round boundary and then be echoed just
                    // once (oracle_blur_duration_timing.log: `w` then
                    // `n`, and `n` is not heard from for 1.15s). Echoing
                    // is the safe direction: an echo means acceptance
                    // either way, and a duplicate re-confirms rather
                    // than advancing the client's correlation.
                    write_text(&mut writer, &format!("{line}\n")).await?;
                    if core_tx.send(CoreMsg::Input { session, line }).is_err() {
                        break;
                    }
                }
                None => {
                    let _ = core_tx.send(CoreMsg::Detach { session });
                    break;
                }
            },
        }
    }
    writer.shutdown().await.ok();
    Ok(())
}

/// The account dialogue. Returns `None` if the connection drops mid-login.
async fn login(
    reader: &mut TelnetReader,
    writer: &mut BufWriter<OwnedWriteHalf>,
    state: &Arc<Mutex<StateDb>>,
) -> io::Result<Option<AccountProfile>> {
    loop {
        write_text(writer, "Account: ").await?;
        let Some(name) = reader.read_line().await else {
            return Ok(None);
        };
        let name = name.trim().to_string();
        if name.is_empty() {
            continue;
        }

        let exists = state
            .lock()
            .expect("state db lock")
            .account_exists(&name)
            .unwrap_or(false);

        if exists {
            let Some(password) = read_password(reader, writer).await? else {
                return Ok(None);
            };
            let verified = state
                .lock()
                .expect("state db lock")
                .verify_login(&name, &password)
                .unwrap_or(None);
            match verified {
                Some(profile) => return Ok(Some(profile)),
                None => {
                    write_text(writer, "Invalid credentials.\n").await?;
                    continue;
                }
            }
        }

        write_text(writer, "Create new account? (y/n) ").await?;
        let Some(answer) = reader.read_line().await else {
            return Ok(None);
        };
        if !answer.trim().eq_ignore_ascii_case("y") {
            continue;
        }
        let Some(password) = read_password(reader, writer).await? else {
            return Ok(None);
        };
        let gender = loop {
            write_text(writer, "Gender (M/F): ").await?;
            let Some(g) = reader.read_line().await else {
                return Ok(None);
            };
            match g.trim().to_ascii_uppercase().as_str() {
                "M" => break Gender::Male,
                "F" => break Gender::Female,
                _ => {}
            }
        };
        let created = state
            .lock()
            .expect("state db lock")
            .create_account(&name, &password, gender);
        match created {
            Ok(()) => {
                return Ok(Some(AccountProfile {
                    name,
                    gender,
                    saved_evil: 0,
                }));
            }
            Err(CreateAccountError::NameTaken) => {
                write_text(writer, "That name was just taken.\n").await?;
            }
            Err(CreateAccountError::Other(e)) => {
                eprintln!("account creation failed: {e}");
                return Ok(None);
            }
        }
    }
}
