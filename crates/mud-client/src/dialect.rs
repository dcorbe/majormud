//! Login dialects for the two supported targets.
//!
//! MBBSEmu (original WCCMMUD behind the BBS): `Username:` -> `Password:`
//! -> `Make your selection` -> `A` -> `[MAJORMUD]:` — the flow
//! `tools/oracle/mudlib.py::login` automates.
//!
//! Rust server (`crates/mud-server`): `Account: ` -> existing-account
//! `Password: `, or the create flow (`Create new account? (y/n)` ->
//! `Password:` -> `Gender (M/F):`) which lands in character creation.

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
                Ok(LoginOutcome::InGame)
            }
        }
    }
}
