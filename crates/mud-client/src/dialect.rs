//! Login dialects for the two supported targets.
//!
//! MBBSEmu (original WCCMMUD behind the BBS): `Username:` -> `Password:`
//! -> `Make your selection` -> `A` -> `[MAJORMUD]:` — the flow
//! `tools/oracle/mudlib.py::login` automates.
//!
//! Rust server (`crates/mud-server`): `Account: ` -> existing-account
//! `Password: `, or the create flow (`Create new account? (y/n)` ->
//! `Password:` -> `Gender (M/F):`) which lands in character creation.

use mud_core::text;
use serde::{Deserialize, Serialize};

use crate::profile::Profile;
use crate::session::{ExpectError, Session};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Target {
    #[serde(rename = "mbbs")]
    MbbsEmu,
    #[serde(rename = "rust")]
    RustServer,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoginOutcome {
    /// Logged in; the game prompt (or MajorMUD entry) was reached.
    InGame,
    /// A fresh account was created; the server is asking for race
    /// selection (Rust server only).
    CharacterCreation,
}

/// Drive the race/class/alignment dialogue that [`login`] stops in front
/// of when it returns [`LoginOutcome::CharacterCreation`], leaving the
/// character standing at the game prompt.
///
/// Takes the first race and the first class, and declines Lawful — the
/// choices are irrelevant to every caller so far, and a caller that cares
/// should drive the dialogue itself rather than grow options here.
pub async fn finish_creation(session: &Session) -> Result<(), ExpectError> {
    use std::time::Duration;
    let t = Duration::from_secs(30);
    session.send("1");
    session
        .expect("Please choose a class from the following list:", t)
        .await?;
    session.send("1");
    session.expect("Do you want to be Lawful?", t).await?;
    session.send("No");
    session.expect("[HP=", t).await?;
    apply_evil_preference(session, session.profile()).await?;
    Ok(())
}

/// Put the character's Warn-on-Evil flag into the state the profile asks
/// for. Opt-in only: an unset profile sends nothing at all, because this
/// mutates persistent state on the board.
async fn apply_evil_preference(session: &Session, profile: &Profile) -> Result<(), ExpectError> {
    if profile.disable_evil_warnings {
        ensure_evil_warnings_off(session).await?;
    }
    Ok(())
}

/// Leave the character with Warn on Evil OFF, so the board stops refusing
/// attacks on unprovoked (behaviour 0/4) monsters.
///
/// `set evil` is a TOGGLE that reports the state it landed in, not a
/// setter (`mud_core::game`'s `cmd_set`). So this reads the confirmation
/// instead of assuming: if the first toggle turned the warning ON, the
/// character already had it off and a second toggle puts it back.
/// Converges in at most two commands either way.
///
/// This is a real change to the character: with warnings off, evil acts
/// go through and accrue fame, which moves the legal level toward
/// Criminal. That is why nothing calls this unless the profile says so.
pub async fn ensure_evil_warnings_off(session: &Session) -> Result<(), ExpectError> {
    use std::time::Duration;
    let t = Duration::from_secs(30);
    session.send("set evil");
    let landed_on = session
        .expect_any(&[text::SET_EVIL_WARN_OFF, text::SET_EVIL_WARN_ON], t)
        .await?
        == 1;
    if landed_on {
        session.send("set evil");
        session.expect(text::SET_EVIL_WARN_OFF, t).await?;
    }
    Ok(())
}

pub async fn login(session: &Session, profile: &Profile) -> Result<LoginOutcome, ExpectError> {
    use std::time::Duration;
    let t = Duration::from_secs(30);
    match profile.target {
        Target::MbbsEmu => {
            session.expect("Username:", t).await?;
            session.send(&profile.username);
            session.expect("Password:", t).await?;
            session.send(&profile.password);
            session.expect("Make your selection", t).await?;
            session.send("A");
            session.expect("[MAJORMUD]:", t).await?;
            // `[MAJORMUD]:` is the module's MENU, not a game prompt. The
            // realm is behind "[E] . Enter the Realm", and a caller left
            // at the menu would have every game command it sent parsed
            // as a menu key instead.
            session.send("E");
            session.expect("[HP=", t).await?;
            apply_evil_preference(session, profile).await?;
            Ok(LoginOutcome::InGame)
        }
        Target::RustServer => {
            session.expect("Account: ", t).await?;
            session.send(&profile.username);
            let branch = session
                .expect_any(&["Create new account? (y/n)", "Password: "], t)
                .await?;
            if branch == 0 {
                session.send("y");
                session.expect("Password: ", t).await?;
                session.send(&profile.password);
                session.expect("Gender (M/F):", t).await?;
                session.send("M");
                session.expect("Please choose a race", t).await?;
                Ok(LoginOutcome::CharacterCreation)
            } else {
                session.send(&profile.password);
                session.expect("[HP=", t).await?;
                apply_evil_preference(session, profile).await?;
                Ok(LoginOutcome::InGame)
            }
        }
    }
}
