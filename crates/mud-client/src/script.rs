//! Lua script host: oracle scenarios and custom bot logic run as Lua
//! (5.4, vendored) on a dedicated thread, driving the shared [`Session`]
//! through a blocking `mud` API table.
//!
//! API: `mud.send(line)`, `mud.expect(needle[, secs])` (default 15s,
//! errors on timeout), `mud.login()` (-> "ingame" | "create"),
//! `mud.mark()`, `mud.since(m)`, `mud.save_section(name, text)`,
//! `mud.hp()`, `mud.mana()`, `mud.room_name()`, `mud.sleep(secs)`,
//! `mud.log(msg)`.

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use mlua::Lua;

use crate::dialect::{self, LoginOutcome};
use crate::session::Session;

/// What a completed script leaves behind: named cleaned-text sections,
/// compatible with the Python oracle `*_sections.json` files.
#[derive(Debug, Default)]
pub struct ScriptOutcome {
    pub sections: BTreeMap<String, String>,
}

#[derive(Debug)]
pub struct ScriptError(pub String);

impl std::fmt::Display for ScriptError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::error::Error for ScriptError {}

/// Run a Lua script file against a session. The Lua interpreter runs on
/// its own thread; the calling task awaits completion.
pub async fn run_script(path: &Path, session: Arc<Session>) -> Result<ScriptOutcome, ScriptError> {
    let path = path.to_path_buf();
    let rt = tokio::runtime::Handle::current();
    match tokio::task::spawn_blocking(move || run_blocking(&path, session, rt)).await {
        Ok(result) => result,
        Err(join) => Err(ScriptError(format!("script thread panicked: {join}"))),
    }
}

fn run_blocking(
    path: &Path,
    session: Arc<Session>,
    rt: tokio::runtime::Handle,
) -> Result<ScriptOutcome, ScriptError> {
    let source = std::fs::read_to_string(path)
        .map_err(|e| ScriptError(format!("read {}: {e}", path.display())))?;
    let sections: Arc<Mutex<BTreeMap<String, String>>> = Arc::default();
    let lua = Lua::new();
    build_mud_api(&lua, &session, &rt, &sections)
        .map_err(|e| ScriptError(format!("building mud API: {e}")))?;
    lua.load(&source)
        .set_name(path.display().to_string())
        .exec()
        .map_err(|e| ScriptError(e.to_string()))?;
    let sections = std::mem::take(&mut *sections.lock().expect("sections lock"));
    Ok(ScriptOutcome { sections })
}

fn build_mud_api(
    lua: &Lua,
    session: &Arc<Session>,
    rt: &tokio::runtime::Handle,
    sections: &Arc<Mutex<BTreeMap<String, String>>>,
) -> mlua::Result<()> {
    let mud = lua.create_table()?;

    {
        let s = Arc::clone(session);
        mud.set(
            "send",
            lua.create_function(move |_, line: String| {
                s.send(&line);
                Ok(())
            })?,
        )?;
    }
    {
        let s = Arc::clone(session);
        let rt = rt.clone();
        mud.set(
            "expect",
            lua.create_function(move |_, (needle, secs): (String, Option<f64>)| {
                let timeout = Duration::from_secs_f64(secs.unwrap_or(15.0));
                rt.block_on(s.expect(&needle, timeout))
                    .map_err(mlua::Error::external)
            })?,
        )?;
    }
    {
        let s = Arc::clone(session);
        let rt = rt.clone();
        mud.set(
            "login",
            lua.create_function(move |_, ()| {
                let outcome = rt
                    .block_on(dialect::login(&s, &s.profile()))
                    .map_err(mlua::Error::external)?;
                Ok(match outcome {
                    LoginOutcome::InGame => "ingame",
                    LoginOutcome::CharacterCreation => "create",
                })
            })?,
        )?;
    }
    {
        let s = Arc::clone(session);
        mud.set(
            "mark",
            lua.create_function(move |_, ()| Ok(s.mark() as i64))?,
        )?;
    }
    {
        let s = Arc::clone(session);
        mud.set(
            "since",
            lua.create_function(move |_, m: i64| Ok(s.since(m.max(0) as usize)))?,
        )?;
    }
    {
        let sections = Arc::clone(sections);
        mud.set(
            "save_section",
            lua.create_function(move |_, (name, text): (String, String)| {
                sections.lock().expect("sections lock").insert(name, text);
                Ok(())
            })?,
        )?;
    }
    {
        let s = Arc::clone(session);
        mud.set(
            "hp",
            lua.create_function(move |_, ()| Ok(s.state().borrow().hp))?,
        )?;
    }
    {
        let s = Arc::clone(session);
        mud.set(
            "mana",
            lua.create_function(move |_, ()| Ok(s.state().borrow().mana))?,
        )?;
    }
    {
        let s = Arc::clone(session);
        mud.set(
            "room_name",
            lua.create_function(move |_, ()| {
                Ok(s.state().borrow().room.as_ref().map(|r| r.name.clone()))
            })?,
        )?;
    }
    mud.set(
        "sleep",
        lua.create_function(|_, secs: f64| {
            std::thread::sleep(Duration::from_secs_f64(secs.max(0.0)));
            Ok(())
        })?,
    )?;
    mud.set(
        "log",
        lua.create_function(|_, msg: String| {
            eprintln!("[script] {msg}");
            Ok(())
        })?,
    )?;

    lua.globals().set("mud", mud)
}

