//! The telnet edge: async I/O per connection, one synchronous game-core
//! thread. Connections do login/account work against the state database,
//! then bridge lines to the core over channels. No game logic lives here.

use std::io;
use std::net::SocketAddr;
use std::sync::mpsc as std_mpsc;
use std::sync::{Arc, Mutex};

use mud_core::content::Content;
use mud_core::game::{AccountProfile, Core, CoreConfig, Event, Gender, Player, SessionId};
use tokio::io::{AsyncReadExt, AsyncWriteExt, BufWriter};
use tokio::net::tcp::{OwnedReadHalf, OwnedWriteHalf};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{mpsc, oneshot};

use crate::state_db::{CreateAccountError, StateDb};

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
        let listener = TcpListener::bind(addr).await?;
        let local = listener.local_addr()?;
        let state = Arc::new(Mutex::new(state));
        let (core_tx, core_rx) = std_mpsc::channel::<CoreMsg>();

        {
            let state = Arc::clone(&state);
            std::thread::Builder::new()
                .name("game-core".into())
                .spawn(move || core_thread(content, config, state, core_rx))
                .expect("spawn core thread");
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

/// The single-threaded game loop: applies messages, routes events.
fn core_thread(
    content: Content,
    config: CoreConfig,
    state: Arc<Mutex<StateDb>>,
    rx: std_mpsc::Receiver<CoreMsg>,
) {
    let mut core = Core::new(content, config);
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
}

impl TelnetReader {
    fn new(socket: OwnedReadHalf) -> Self {
        TelnetReader {
            socket,
            buf: Vec::new(),
            line: Vec::new(),
        }
    }

    /// The next input line, or `None` on EOF/error.
    async fn read_line(&mut self) -> Option<String> {
        const IAC: u8 = 255;
        const SB: u8 = 250;
        const SE: u8 = 240;
        loop {
            // Consume buffered bytes first.
            while !self.buf.is_empty() {
                let b = self.buf.remove(0);
                match b {
                    IAC => {
                        if self.buf.first() == Some(&IAC) {
                            self.buf.remove(0);
                            self.line.push(IAC);
                        } else if self.buf.first() == Some(&SB) {
                            // Subnegotiation: drop through IAC SE.
                            while self.buf.len() >= 2 {
                                if self.buf[0] == IAC && self.buf[1] == SE {
                                    self.buf.drain(..2);
                                    break;
                                }
                                self.buf.remove(0);
                            }
                        } else {
                            // Three-byte command (WILL/WONT/DO/DONT/...).
                            let drop = self.buf.len().min(2);
                            self.buf.drain(..drop);
                        }
                    }
                    b'\r' => {
                        // CR LF or CR NUL both end a line.
                        if matches!(self.buf.first(), Some(&b'\n') | Some(&0)) {
                            self.buf.remove(0);
                        }
                        let line = String::from_utf8_lossy(&self.line).into_owned();
                        self.line.clear();
                        return Some(line);
                    }
                    b'\n' => {
                        let line = String::from_utf8_lossy(&self.line).into_owned();
                        self.line.clear();
                        return Some(line);
                    }
                    _ => self.line.push(b),
                }
            }
            let mut chunk = [0u8; 1024];
            match self.socket.read(&mut chunk).await {
                Ok(0) | Err(_) => return None,
                Ok(n) => self.buf.extend_from_slice(&chunk[..n]),
            }
        }
    }
}

async fn write_text(writer: &mut BufWriter<OwnedWriteHalf>, text: &str) -> io::Result<()> {
    writer
        .write_all(text.replace('\n', "\r\n").as_bytes())
        .await?;
    writer.flush().await
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
            line = reader.read_line() => match line {
                Some(line) => {
                    if core_tx.send(CoreMsg::Input { session, line }).is_err() {
                        break;
                    }
                }
                None => {
                    let _ = core_tx.send(CoreMsg::Detach { session });
                    break;
                }
            },
            out = out_rx.recv() => match out {
                Some(OutMsg::Text(text)) => write_text(&mut writer, &text).await?,
                Some(OutMsg::Close) | None => break,
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
            write_text(writer, "Password: ").await?;
            let Some(password) = reader.read_line().await else {
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
        write_text(writer, "Password: ").await?;
        let Some(password) = reader.read_line().await else {
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
