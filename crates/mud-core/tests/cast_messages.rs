//! Cast-message renderer: `castmsgb`'s three audience lines with %s/%d
//! substitution (`re/docs/spellcasting.md` §8.6; templates verified against
//! `re/mmud_wgnt.sqlite` messages 3242 / 2 / 7 / 1).

use mud_core::content::{Message, MessageId};
use mud_core::text::{render_cast_line, CastAudience, CastMsgArgs};

fn message(id: u16, lines: &[&str]) -> Message {
    Message {
        id: MessageId(id),
        lines: lines.iter().map(|l| (*l).to_string()).collect(),
    }
}

/// Message 3242 (magic missile) — note line3's damage placeholder is `%s`.
fn magic_missile_msg() -> Message {
    message(
        3242,
        &[
            "You fire a %s at %s for %d damage!",
            "%s fires a %s at you for %d damage!",
            "%s fires a %s at %s for %s damage!",
        ],
    )
}

/// Message 2 (illuminate) — target-less, consumes an order prefix.
fn illuminate_msg() -> Message {
    message(2, &["You cast %s!", "%s casts %s!", "%s casts %s!"])
}

/// Message 7 (blur) — benign with a target, no damage.
fn blur_msg() -> Message {
    message(
        7,
        &["You cast %s on %s!", "%s casts %s upon you!", "%s casts %s on %s!"],
    )
}

#[test]
fn damage_message_renders_for_all_audiences() {
    let msg = magic_missile_msg();
    let args = CastMsgArgs {
        caster: "Vexil",
        target: Some("nasty filthbug"),
        spell: "magic missile",
        damage: Some(13),
    };
    // VERIFIED (oracle §8.6): the caster line, byte-exact.
    assert_eq!(
        render_cast_line(&msg, CastAudience::Caster, &args).as_deref(),
        Some("You fire a magic missile at nasty filthbug for 13 damage!")
    );
    assert_eq!(
        render_cast_line(&msg, CastAudience::Target, &args).as_deref(),
        Some("Vexil fires a magic missile at you for 13 damage!")
    );
    // line3's %s must accept the damage integer.
    assert_eq!(
        render_cast_line(&msg, CastAudience::Room, &args).as_deref(),
        Some("Vexil fires a magic missile at nasty filthbug for 13 damage!")
    );
}

#[test]
fn targetless_message_consumes_order_prefix() {
    let msg = illuminate_msg();
    let args = CastMsgArgs {
        caster: "Vexil",
        target: None,
        spell: "illuminate",
        damage: None,
    };
    // VERIFIED (oracle §8.6): "You cast illuminate!"
    assert_eq!(
        render_cast_line(&msg, CastAudience::Caster, &args).as_deref(),
        Some("You cast illuminate!")
    );
    assert_eq!(
        render_cast_line(&msg, CastAudience::Target, &args).as_deref(),
        Some("Vexil casts illuminate!")
    );
    assert_eq!(
        render_cast_line(&msg, CastAudience::Room, &args).as_deref(),
        Some("Vexil casts illuminate!")
    );
}

#[test]
fn benign_targeted_message_renders_for_all_audiences() {
    let msg = blur_msg();
    // Self-target: the oracle cast was `c blur` on Vexil himself.
    let args = CastMsgArgs {
        caster: "Vexil",
        target: Some("Vexil"),
        spell: "blur",
        damage: None,
    };
    // VERIFIED (oracle §8.6): "You cast blur on Vexil!"
    assert_eq!(
        render_cast_line(&msg, CastAudience::Caster, &args).as_deref(),
        Some("You cast blur on Vexil!")
    );
    assert_eq!(
        render_cast_line(&msg, CastAudience::Target, &args).as_deref(),
        Some("Vexil casts blur upon you!")
    );
    assert_eq!(
        render_cast_line(&msg, CastAudience::Room, &args).as_deref(),
        Some("Vexil casts blur on Vexil!")
    );
}

#[test]
fn empty_message_renders_nothing() {
    // Message 1 is the empty message; the loader trims trailing empty
    // lines, so it arrives with no lines at all.
    let msg = message(1, &[]);
    let args = CastMsgArgs {
        caster: "Vexil",
        target: None,
        spell: "illuminate",
        damage: None,
    };
    for audience in [CastAudience::Caster, CastAudience::Target, CastAudience::Room] {
        assert_eq!(render_cast_line(&msg, audience, &args), None);
    }
}

#[test]
fn empty_middle_line_renders_nothing_for_that_audience() {
    // Only trailing empties are trimmed at load; an interior empty line
    // must render nothing without disturbing its neighbours.
    let msg = message(900, &["You cast %s!", "", "%s casts %s!"]);
    let args = CastMsgArgs {
        caster: "Vexil",
        target: None,
        spell: "illuminate",
        damage: None,
    };
    assert!(render_cast_line(&msg, CastAudience::Caster, &args).is_some());
    assert_eq!(render_cast_line(&msg, CastAudience::Target, &args), None);
    assert!(render_cast_line(&msg, CastAudience::Room, &args).is_some());
}

#[test]
fn literal_percent_passes_through() {
    // Defensive: `%` not followed by `s`/`d` is not a placeholder.
    let msg = message(901, &["You are 100% sure %s hits 50%!", "", ""]);
    let args = CastMsgArgs {
        caster: "Vexil",
        target: None,
        spell: "magic missile",
        damage: None,
    };
    assert_eq!(
        render_cast_line(&msg, CastAudience::Caster, &args).as_deref(),
        Some("You are 100% sure magic missile hits 50%!")
    );
}

#[test]
fn exhausted_args_leave_placeholder_untouched() {
    // Defensive: a template hungrier than the argument order passes the
    // surplus placeholder through instead of panicking.
    let msg = message(902, &["You cast %s at %s for %d!", "", ""]);
    let args = CastMsgArgs {
        caster: "Vexil",
        target: None,
        spell: "blur",
        damage: None,
    };
    assert_eq!(
        render_cast_line(&msg, CastAudience::Caster, &args).as_deref(),
        Some("You cast blur at %s for %d!")
    );
}
