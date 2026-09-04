use super::*;

/// The modes a program that asks for clicks and the SGR encoding is in, which
/// is what everything written in the last twenty years asks for.
fn sgr_clicks() -> MouseModes {
    MouseModes {
        click: true,
        sgr: true,
        ..MouseModes::NONE
    }
}

/// The same program, but one that never asked for `?1006`.
fn x10_clicks() -> MouseModes {
    MouseModes {
        click: true,
        ..MouseModes::NONE
    }
}

fn encoded(
    kind: MouseEventKind,
    button: Option<MouseButton>,
    row: usize,
    column: usize,
    modifiers: Modifiers,
    modes: MouseModes,
) -> Option<String> {
    encode(kind, button, row, column, modifiers, modes).map(|bytes| {
        // Every encoding this module produces is ASCII, so a lossy conversion
        // cannot lose anything and makes a failure readable.
        String::from_utf8_lossy(&bytes).into_owned()
    })
}

#[test]
fn test_nothing_is_reported_until_a_program_asks() {
    // The state a shell sits in. A press here belongs to the person selecting
    // text, and a terminal that sent it anyway would put escape sequences into
    // whatever they were typing.
    for kind in [
        MouseEventKind::Press,
        MouseEventKind::Release,
        MouseEventKind::Motion,
    ] {
        assert_eq!(
            encode(
                kind,
                Some(MouseButton::Left),
                0,
                0,
                Modifiers::NONE,
                MouseModes::NONE
            ),
            None,
            "{kind:?} was reported to a program that never asked for the mouse"
        );
    }
}

#[test]
fn test_the_sgr_form_counts_from_one_and_names_the_button_on_release() {
    let press = encoded(
        MouseEventKind::Press,
        Some(MouseButton::Left),
        0,
        0,
        Modifiers::NONE,
        sgr_clicks(),
    );
    assert_eq!(press.as_deref(), Some("\x1b[<0;1;1M"));

    // The top-left cell of the viewport is column 1, row 1 to the program.
    let elsewhere = encoded(
        MouseEventKind::Press,
        Some(MouseButton::Right),
        9,
        41,
        Modifiers::NONE,
        sgr_clicks(),
    );
    assert_eq!(elsewhere.as_deref(), Some("\x1b[<2;42;10M"));

    // A release keeps its button's number and says "up" with the final byte,
    // which is the whole reason a program asks for this encoding.
    let release = encoded(
        MouseEventKind::Release,
        Some(MouseButton::Right),
        9,
        41,
        Modifiers::NONE,
        sgr_clicks(),
    );
    assert_eq!(release.as_deref(), Some("\x1b[<2;42;10m"));
}

#[test]
fn test_the_x10_form_cannot_say_which_button_came_up() {
    let press = encode(
        MouseEventKind::Press,
        Some(MouseButton::Middle),
        0,
        0,
        Modifiers::NONE,
        x10_clicks(),
    );
    assert_eq!(press, Some(vec![0x1b, b'[', b'M', 33, 33, 33]));

    // 3 is "something came up", whatever it was.
    let release = encode(
        MouseEventKind::Release,
        Some(MouseButton::Middle),
        0,
        0,
        Modifiers::NONE,
        x10_clicks(),
    );
    assert_eq!(release, Some(vec![0x1b, b'[', b'M', 32 + 3, 33, 33]));
}

#[test]
fn test_a_coordinate_past_the_x10_limit_clamps_rather_than_wrapping() {
    // A byte biased by 32 runs out at 223. Wrapping would report a click at
    // column 1 for one at column 300, which is worse than reporting the edge.
    let bytes = encode(
        MouseEventKind::Press,
        Some(MouseButton::Left),
        0,
        299,
        Modifiers::NONE,
        x10_clicks(),
    )
    .expect("a press is reported");

    assert_eq!(bytes[4], 32 + X10_MAX as u8);

    // The SGR form has no such limit, which is the other reason to ask for it.
    let sgr = encoded(
        MouseEventKind::Press,
        Some(MouseButton::Left),
        0,
        299,
        Modifiers::NONE,
        sgr_clicks(),
    );
    assert_eq!(sgr.as_deref(), Some("\x1b[<0;300;1M"));
}

#[test]
fn test_the_modifiers_are_the_bits_xterm_gave_them() {
    let held = Modifiers {
        shift: true,
        alt: true,
        control: true,
        logo: false,
    };
    let bytes = encoded(
        MouseEventKind::Press,
        Some(MouseButton::Left),
        0,
        0,
        held,
        sgr_clicks(),
    );

    // 0 for the left button, plus 4 + 8 + 16.
    assert_eq!(bytes.as_deref(), Some("\x1b[<28;1;1M"));
}

#[test]
fn test_motion_is_reported_only_to_a_program_that_asked_for_it() {
    let dragging = MouseModes {
        drag: true,
        sgr: true,
        ..MouseModes::NONE
    };

    // `?1002` wants motion while a button is down and nothing else.
    assert_eq!(
        encoded(
            MouseEventKind::Motion,
            Some(MouseButton::Left),
            2,
            4,
            Modifiers::NONE,
            dragging,
        )
        .as_deref(),
        // The motion bit is 32, so the left button dragging is 32.
        Some("\x1b[<32;5;3M"),
    );
    assert_eq!(
        encode(
            MouseEventKind::Motion,
            None,
            2,
            4,
            Modifiers::NONE,
            dragging
        ),
        None,
        "a bare pointer move was reported to a program that only asked for drags"
    );

    // `?1003` wants every move. With nothing held the button field is the
    // release code with the motion bit set: 3 + 32.
    let all_motion = MouseModes {
        motion: true,
        sgr: true,
        ..MouseModes::NONE
    };
    assert_eq!(
        encoded(
            MouseEventKind::Motion,
            None,
            2,
            4,
            Modifiers::NONE,
            all_motion
        )
        .as_deref(),
        Some("\x1b[<35;5;3M"),
    );
}

#[test]
fn test_the_wheel_is_a_press_and_never_a_release() {
    // A wheel notch has no release. Sending one would make every scroll two
    // events to a program expecting one, and a pager would jump two lines.
    assert_eq!(
        encoded(
            MouseEventKind::Press,
            Some(MouseButton::WheelUp),
            0,
            0,
            Modifiers::NONE,
            sgr_clicks(),
        )
        .as_deref(),
        Some("\x1b[<64;1;1M"),
    );
    assert_eq!(
        encoded(
            MouseEventKind::Press,
            Some(MouseButton::WheelDown),
            0,
            0,
            Modifiers::NONE,
            sgr_clicks(),
        )
        .as_deref(),
        Some("\x1b[<65;1;1M"),
    );
    assert_eq!(
        encode(
            MouseEventKind::Release,
            Some(MouseButton::WheelUp),
            0,
            0,
            Modifiers::NONE,
            sgr_clicks(),
        ),
        None,
    );
}

#[test]
fn test_alternate_scroll_is_not_the_mouse() {
    let pager = MouseModes {
        alternate_scroll: true,
        ..MouseModes::NONE
    };

    assert!(
        !pager.is_reporting(),
        "a pager with alternate scroll set has not asked for the mouse, so a \
         drag across its text must still select"
    );
    assert!(pager.wants_alternate_scroll(true));
    assert!(
        !pager.wants_alternate_scroll(false),
        "on the primary screen the wheel belongs to the scrollback"
    );

    // A program that asked for the mouse gets the mouse; the arrow-key
    // substitute would arrive alongside the report and scroll it twice.
    let both = MouseModes {
        alternate_scroll: true,
        click: true,
        ..MouseModes::NONE
    };
    assert!(!both.wants_alternate_scroll(true));
}
