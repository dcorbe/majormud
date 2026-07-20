//! Crime / fame / legal levels (`crime.md`).
//!
//! Fame is the signed word at `player+0x542`: positive = evil, negative =
//! good. All gameplay fame gains funnel through `add_evil_points`
//! (0x4e49c); this module holds the pure pieces — the tier function
//! (`get_legal_level` 0x44e390), the DLL's tier name/color tables
//! (0x4881a4 / 0x488184), and the NPC-victim charge path (§2.2: no pair
//! timers, no multipliers). The player-victim path (retaliation timers,
//! victim multipliers, forgiveness) lands with rob in the theft slice.

/// The eight legal tiers, numbered as the DLL numbers them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum LegalLevel {
    Neutral = 0,
    Seedy = 1,
    Outlaw = 2,
    Criminal = 3,
    Villain = 4,
    Fiend = 5,
    Good = 6,
    Saint = 7,
}

impl LegalLevel {
    /// The DLL name table at 0x4881a4, indexed by level number.
    pub fn name(self) -> &'static str {
        match self {
            LegalLevel::Neutral => "Neutral",
            LegalLevel::Seedy => "Seedy",
            LegalLevel::Outlaw => "Outlaw",
            LegalLevel::Criminal => "Criminal",
            LegalLevel::Villain => "Villain",
            LegalLevel::Fiend => "FIEND",
            LegalLevel::Good => "Good",
            LegalLevel::Saint => "Saint",
        }
    }

    /// The scan color table at 0x488184 (ANSI parameter bytes). The WHO
    /// list prints no word at all for Neutral (level 0).
    pub fn color(self) -> &'static str {
        match self {
            LegalLevel::Neutral => "\x1b[36m",
            LegalLevel::Seedy => "\x1b[37m",
            LegalLevel::Outlaw => "\x1b[31m",
            LegalLevel::Criminal => "\x1b[33m",
            LegalLevel::Villain => "\x1b[1;33m",
            LegalLevel::Fiend => "\x1b[1;31m",
            LegalLevel::Good => "\x1b[1;37m",
            LegalLevel::Saint => "\x1b[1;37m",
        }
    }
}

/// `get_legal_level` (0x44e390, crime.md §1) — the exact threshold chain.
pub fn legal_level(fame: i16) -> LegalLevel {
    let fame = i32::from(fame);
    if fame < -200 {
        LegalLevel::Saint
    } else if fame < -0x32 {
        LegalLevel::Good
    } else if fame < 0x1e {
        LegalLevel::Neutral
    } else if fame < 0x28 {
        LegalLevel::Seedy
    } else if fame < 0x50 {
        LegalLevel::Outlaw
    } else if fame < 0x78 {
        LegalLevel::Criminal
    } else if fame < 0xd2 {
        LegalLevel::Villain
    } else {
        LegalLevel::Fiend
    }
}

/// Refusal / confirmation strings (DLL addresses in crime.md §3).
pub const WARN_ON_EVIL_REFUSAL: &str =
    "To do this action, you must turn off your evil warnings.";
pub const TOO_EVIL_REFUSAL: &str =
    "You have progressed too far to the evil side to do this action.";
pub const LAWFUL_REFUSAL: &str =
    "You have chosen a way of life which does not allow this action.";
pub const DARK_CLOUD: &str = "A dark cloud passes over you";

/// `add_evil_points` with victim = -1 — the NPC path (crime.md §2.1/§2.2/
/// §2.4). Gate order: Warn on Evil, the 300 action ceiling, committed
/// Lawful. On success the dark-cloud line returns and fame gains
/// `points`, with the good-side minimum-10 bump and the 30000 cap.
/// `Err` = the action is REFUSED (callers abort the attack/cast).
pub fn charge_npc_evil(
    fame: &mut i16,
    warn_on_evil: bool,
    lawful: bool,
    points: i16,
) -> Result<&'static str, &'static str> {
    if warn_on_evil {
        return Err(WARN_ON_EVIL_REFUSAL);
    }
    if *fame > 300 {
        return Err(TOO_EVIL_REFUSAL);
    }
    if lawful {
        return Err(LAWFUL_REFUSAL);
    }
    let mut points = i32::from(points);
    let fame_now = i32::from(*fame);
    // §2.4 minimum-10 bump: one evil act erases good standing.
    if fame_now < 0 && fame_now + points < 10 {
        points = 10 - fame_now;
    }
    // Add-guard (§2.4): only when it increases fame and fame < 30000 —
    // no clamp on the sum itself (the DLL adds raw; shipped points are
    // tiny, and the 300 action ceiling bounds this path anyway).
    if points > 0 && fame_now < 30000 {
        *fame = (fame_now + points).clamp(i32::from(i16::MIN), i32::from(i16::MAX)) as i16;
    }
    Ok(DARK_CLOUD)
}

/// The alignment-ability lattice (crime.md §6.1 — user_can_use 17515-17539
/// and user_can_use_spell 17811-17846 apply the identical table before any
/// class/race checks). `has(id)` reports whether the object carries the
/// alignment ability: Good 97, Evil 98, NotGood 110, NotEvil 111, Neutral
/// 112. NotNeutral (113) is never enforced in the DLL. Returns true when
/// the object is REFUSED at this legal level.
pub fn alignment_refuses(level: LegalLevel, has: impl Fn(u16) -> bool) -> bool {
    match level {
        LegalLevel::Neutral | LegalLevel::Seedy => has(97) || has(98),
        LegalLevel::Outlaw | LegalLevel::Criminal | LegalLevel::Villain | LegalLevel::Fiend => {
            has(97) || has(111) || has(112)
        }
        LegalLevel::Good | LegalLevel::Saint => has(98) || has(110) || has(112),
    }
}
