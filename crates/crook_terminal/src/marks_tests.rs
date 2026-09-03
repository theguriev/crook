use super::*;

/// Splits an OSC payload the way `vte` does, so the tests name the sequence
/// rather than the parameter vector it becomes.
fn parse(payload: &str) -> Option<ShellMark> {
    let parameters: Vec<&[u8]> = payload.split(';').map(str::as_bytes).collect();
    ShellMark::parse(&parameters)
}

#[test]
fn test_the_four_marks_of_the_standard() {
    assert_eq!(
        Some(ShellMark::PromptStart(PromptKind::Initial)),
        parse("133;A")
    );
    assert_eq!(Some(ShellMark::PromptEnd), parse("133;B"));
    assert_eq!(Some(ShellMark::OutputStart), parse("133;C"));
    assert_eq!(Some(ShellMark::CommandFinished(Some(0))), parse("133;D;0"));
}

#[test]
fn test_a_bare_d_means_finished_with_no_status() {
    assert_eq!(Some(ShellMark::CommandFinished(None)), parse("133;D"));
    assert_eq!(Some(ShellMark::CommandFinished(None)), parse("133;D;"));
    // fish sends this on ctrl-c at an unexecuted prompt, and a shell that
    // reports something unparseable means the same thing: it ended, somehow.
    assert_eq!(Some(ShellMark::CommandFinished(None)), parse("133;D;oops"));
}

#[test]
fn test_an_exit_status_is_decimal_and_may_be_a_signal() {
    assert_eq!(Some(ShellMark::CommandFinished(Some(1))), parse("133;D;1"));
    assert_eq!(
        Some(ShellMark::CommandFinished(Some(130))),
        parse("133;D;130")
    );
}

#[test]
fn test_the_prompt_kind_is_read_and_unknown_parameters_are_not() {
    assert_eq!(
        Some(ShellMark::PromptStart(PromptKind::Initial)),
        parse("133;A;k=i")
    );
    assert_eq!(
        Some(ShellMark::PromptStart(PromptKind::Right)),
        parse("133;A;k=r")
    );
    assert_eq!(
        Some(ShellMark::PromptStart(PromptKind::Continuation)),
        parse("133;A;k=c")
    );
    assert_eq!(
        Some(ShellMark::PromptStart(PromptKind::Secondary)),
        parse("133;A;k=s")
    );

    // A conforming consumer ignores parameters it does not know rather than
    // rejecting the mark for carrying them.
    assert_eq!(
        Some(ShellMark::PromptStart(PromptKind::Right)),
        parse("133;A;aid=7;cl=line;k=r")
    );
    assert_eq!(
        Some(ShellMark::PromptStart(PromptKind::Initial)),
        parse("133;A;aid=7")
    );
    assert_eq!(Some(ShellMark::PromptEnd), parse("133;B;aid=7"));
}

#[test]
fn test_p_is_the_other_spelling_of_a_prompt_start() {
    // iTerm2's and Warp's parameterised prompt start. Same `k=`, same meaning.
    assert_eq!(
        Some(ShellMark::PromptStart(PromptKind::Initial)),
        parse("133;P;k=i")
    );
    assert_eq!(
        Some(ShellMark::PromptStart(PromptKind::Right)),
        parse("133;P;k=r")
    );
}

#[test]
fn test_everything_else_is_not_a_mark() {
    assert_eq!(None, parse("133"));
    assert_eq!(None, parse("133;"));
    // kitty's "newline unless already at column 0", which this does not act on.
    assert_eq!(None, parse("133;L"));
    assert_eq!(None, parse("133;Z"));
    // A letter that only looks like one of the four.
    assert_eq!(None, parse("133;AA"));
    assert_eq!(None, parse("133;Done"));
    // Another OSC entirely.
    assert_eq!(None, parse("7;file:///tmp"));
    assert_eq!(None, parse("1337;A"));
}
