use super::*;
use crate::emulator::Emulator;
use crate::snapshot::{SnapshotCell, TerminalSize};

/// OSC 133 prompt start.
const A: &str = "\x1b]133;A\x07";
/// OSC 133 prompt end.
const B: &str = "\x1b]133;B\x07";
/// OSC 133 output start.
const C: &str = "\x1b]133;C\x07";

/// A small grid, which keeps the row assertions readable.
fn emulator() -> Emulator {
    Emulator::new(TerminalSize::new(20, 6), 100, Palette::default())
}

/// Everything about a session two byte streams are compared by: what each
/// finished block came to, and the state the open one is in.
#[derive(Debug, PartialEq, Eq)]
struct Session {
    blocks: Vec<(Option<String>, Option<i32>, String)>,
    live: BlockState,
}

impl Session {
    fn of(emulator: &Emulator) -> Self {
        Self {
            blocks: emulator
                .blocks()
                .iter()
                .map(|block| (block.command.clone(), block.exit, block.rows.to_text()))
                .collect(),
            live: emulator.live_block().state,
        }
    }
}

#[test]
fn test_a_clean_cycle_makes_one_block_with_its_command_and_status() {
    let mut emulator = emulator();

    emulator.advance(format!("{A}$ ").as_bytes());
    assert_eq!(BlockState::AtPrompt, emulator.live_block().state);
    assert!(emulator.blocks().is_empty(), "a prompt is not a block yet");

    emulator.advance(format!("{B}echo hi\r\n").as_bytes());
    assert_eq!(BlockState::AtPrompt, emulator.live_block().state);

    emulator.advance(C.as_bytes());
    assert_eq!(BlockState::Executing, emulator.live_block().state);
    assert_eq!(
        Some("echo hi".to_owned()),
        emulator.live_block().command,
        "the command line is read off the screen between B and C"
    );

    emulator.advance(b"hi\r\n");
    emulator.advance(b"\x1b]133;D;0\x07");

    let [block] = emulator.blocks() else {
        panic!("one finished block, got {}", emulator.blocks().len());
    };
    assert_eq!(BlockState::Done, block.state);
    assert_eq!(Some("echo hi".to_owned()), block.command);
    assert_eq!(Some(0), block.exit);
    assert_eq!(Some(true), block.succeeded());
    assert_eq!("$ echo hi\nhi", block.rows.to_text());
    assert!(block.duration().is_some());

    // Nothing is running and nothing is open past the harvest.
    let live = emulator.live_block();
    assert_eq!(BlockState::Done, live.state);
    assert!(live.bottom_row < live.top_row, "{live:?} is not empty");
}

#[test]
fn test_a_completion_carries_its_status_or_says_it_has_none() {
    for (mark, expected) in [
        ("\x1b]133;D;0\x07", Some(0)),
        ("\x1b]133;D;1\x07", Some(1)),
        ("\x1b]133;D;130\x07", Some(130)),
        // A bare `D` is what a shell sends for an empty or cancelled line.
        ("\x1b]133;D\x07", None),
    ] {
        let mut emulator = emulator();
        emulator.advance(format!("{A}$ {B}x\r\n{C}out\r\n{mark}").as_bytes());

        let [block] = emulator.blocks() else {
            panic!("{mark:?} did not finish a block");
        };
        assert_eq!(expected, block.exit, "{mark:?}");
        assert_eq!(expected.map(|status| status == 0), block.succeeded());
    }
}

#[test]
fn test_a_redrawn_prompt_does_not_open_a_second_block() {
    // Powerlevel10k, starship and zsh-vi-mode redraw the prompt several times
    // per keystroke, marks and all. Obeying every `A` would open an empty
    // block per character typed.
    let mut emulator = emulator();
    emulator.advance(format!("{A}$ \r{A}$ ").as_bytes());

    assert_eq!(
        Some(IgnoreReason::PromptRedraw),
        emulator.last_ignored_mark()
    );
    assert!(emulator.blocks().is_empty());

    emulator.advance(format!("{B}ls\r\n{C}a  b\r\n\x1b]133;D;0\x07").as_bytes());
    let [block] = emulator.blocks() else {
        panic!("the redraw split the block in two");
    };
    assert_eq!(Some("ls".to_owned()), block.command);
    assert_eq!("$ ls\na  b", block.rows.to_text());
}

#[test]
fn test_a_right_or_continuation_prompt_is_not_a_boundary() {
    // `k=r` shares a row with the primary prompt and `k=c` appears in the
    // middle of a command still being typed. Neither is the top of a block.
    for kind in ["k=r", "k=c", "k=s"] {
        let mut emulator = emulator();
        emulator.advance(format!("{A}$ {B}").as_bytes());
        emulator.advance(format!("\x1b]133;A;{kind}\x07").as_bytes());

        assert_eq!(
            Some(IgnoreReason::SecondaryPrompt),
            emulator.last_ignored_mark(),
            "{kind}"
        );
        assert!(emulator.blocks().is_empty(), "{kind} opened a block");
    }
}

#[test]
fn test_output_that_starts_without_a_prompt_end_still_runs() {
    // A shell that hooks preexec and precmd but never got its marks into the
    // prompt string. There is no command text to be had, and inventing one off
    // the screen would put the prompt in it.
    let mut emulator = emulator();
    emulator.advance(format!("{A}$ ls\r\n{C}a  b\r\n\x1b]133;D;0\x07").as_bytes());

    let [block] = emulator.blocks() else {
        panic!("a C with no B lost the block");
    };
    assert_eq!(None, block.command);
    assert_eq!(Some(0), block.exit);
    assert_eq!("$ ls\na  b", block.rows.to_text());
}

#[test]
fn test_a_command_that_finishes_without_starting_closes_the_block() {
    // Pressing Enter on an empty line: the shell reports completion with no
    // execution in between. The block is real — the prompt was drawn — and it
    // simply never ran anything.
    let mut emulator = emulator();
    emulator.advance(format!("{A}$ {B}\r\n\x1b]133;D\x07").as_bytes());

    let [block] = emulator.blocks() else {
        panic!("a D with no C lost the block");
    };
    assert_eq!(None, block.command);
    assert_eq!(None, block.exit);
    assert_eq!(None, block.started_at);
    assert_eq!("$", block.rows.to_text());
}

#[test]
fn test_a_prompt_arriving_with_no_completion_closes_the_open_block() {
    // Ctrl-C, Ctrl-D, a shell that crashed mid-command: the next prompt is the
    // only evidence the last one ended. Reconciling on it is what makes those
    // survivable, and the status is honestly unknown.
    let mut emulator = emulator();
    emulator.advance(format!("{A}$ {B}sleep 9\r\n{C}^C\r\n").as_bytes());
    emulator.advance(format!("{A}$ ").as_bytes());

    let [block] = emulator.blocks() else {
        panic!("the new prompt did not close the running block");
    };
    assert_eq!(Some("sleep 9".to_owned()), block.command);
    assert_eq!(None, block.exit, "no status was reported, so none is shown");
    assert_eq!("$ sleep 9\n^C", block.rows.to_text());
    assert_eq!(BlockState::AtPrompt, emulator.live_block().state);
}

#[test]
fn test_a_mark_split_across_two_writes_lands_where_a_whole_one_does() {
    // A pty read boundary can fall anywhere, including inside an escape
    // sequence, and it does so at exactly the moments that are hardest to
    // reproduce. So this is every offset, not one.
    let stream = format!("{A}$ {B}echo hi\r\n{C}hi\r\n\x1b]133;D;0\x07{A}$ ");

    let mut whole = emulator();
    whole.advance(stream.as_bytes());
    let expected = Session::of(&whole);
    assert_eq!(1, expected.blocks.len(), "the whole stream makes one block");

    for split in 0..=stream.len() {
        if !stream.is_char_boundary(split) {
            continue;
        }
        let mut emulator = emulator();
        emulator.advance(&stream.as_bytes()[..split]);
        emulator.advance(&stream.as_bytes()[split..]);
        assert_eq!(expected, Session::of(&emulator), "split at byte {split}");
    }
}

#[test]
fn test_marks_are_ignored_while_the_alternate_screen_is_active() {
    // A full-screen program owns the whole grid, and the marks arriving inside
    // one belong to whatever it is displaying — a log file, a diff, a paged
    // man page — not to the shell that launched it.
    let mut emulator = emulator();
    emulator.advance(format!("{A}$ {B}vim\r\n{C}").as_bytes());
    let before = emulator.live_block().id;

    emulator.advance(b"\x1b[?1049h");
    emulator.advance(format!("{A}$ {B}nested\r\n{C}x\r\n\x1b]133;D;1\x07").as_bytes());

    assert_eq!(Some(IgnoreReason::AltScreen), emulator.last_ignored_mark());
    assert!(
        emulator.blocks().is_empty(),
        "a TUI shredded the block list"
    );
    assert_eq!(before, emulator.live_block().id);
    assert_eq!(BlockState::Executing, emulator.live_block().state);

    // The whole alternate screen reads as the live block while it is up.
    let live = emulator.live_block();
    assert_eq!((0, 5), (live.top_row, live.bottom_row));

    // Leaving it, the shell's own completion closes the block that launched it.
    emulator.advance(b"\x1b[?1049l");
    emulator.advance(b"\x1b]133;D;0\x07");
    let [block] = emulator.blocks() else {
        panic!("leaving the alternate screen lost the block");
    };
    assert_eq!(Some("vim".to_owned()), block.command);
    assert_eq!(Some(0), block.exit);
}

#[test]
fn test_a_hostile_stream_of_marks_cannot_grow_the_list_without_bound() {
    // `cat` a file that contains these bytes and blocks appear: the standard
    // has no way to tell a shell's marks from a file's. What must not happen
    // is a `Vec` growing per line of it.
    let mut emulator = emulator();
    let mut forged = String::new();
    for line in 0..MAX_BLOCKS + 1_000 {
        forged.push_str(&format!("{A}${B}x{C}{line}\r\n\x1b]133;D;0\x07"));
    }
    emulator.advance(forged.as_bytes());

    assert!(
        emulator.blocks().len() <= MAX_BLOCKS,
        "{} blocks is past the cap",
        emulator.blocks().len()
    );
    assert!(emulator.blocks_evicted() > 0, "nothing was evicted");

    // Every surviving block is still whole, and the ids kept going up rather
    // than being reused.
    let mut scratch = Vec::new();
    let mut previous = None;
    for block in emulator.blocks() {
        assert!(previous < Some(block.id));
        previous = Some(block.id);
        for row in 0..block.rows.rows() {
            block.rows.materialise(row, &mut scratch);
            assert_eq!(block.rows.columns(), scratch.len());
        }
    }
    // And the evicted ones are gone rather than dangling.
    assert!(emulator.block(BlockId(1)).is_none());
    assert!(emulator.block(emulator.blocks()[0].id).is_some());
}

#[test]
fn test_a_harvested_block_renders_identically_to_the_grid_it_came_from() {
    let mut emulator = emulator();
    emulator.advance(format!("{A}$ {B}ls\r\n{C}").as_bytes());
    emulator.advance("\x1b[31mred\x1b[0m \x1b[1;4mbold\x1b[0m \u{6f22}e\u{301}\r\n".as_bytes());

    // What the renderer would have drawn from the emulator, one frame before
    // the block was taken out of it.
    let before = emulator.snapshot();
    let drawn: Vec<Vec<SnapshotCell>> = (0..2).map(|row| before.row(row).to_vec()).collect();
    let combining: Vec<Vec<char>> = (0..2)
        .map(|row| {
            (0..before.columns)
                .flat_map(|c| before.zerowidth(row, c).to_vec())
                .collect()
        })
        .collect();

    emulator.advance(b"\x1b]133;D;0\x07");
    let [block] = emulator.blocks() else {
        panic!("the block did not finish");
    };
    assert_eq!(2, block.rows.rows());

    let mut scratch = Vec::new();
    for (row, expected) in drawn.iter().enumerate() {
        let marks = block.rows.materialise(row, &mut scratch);
        assert_eq!(expected, &scratch, "row {row} came back different");
        let harvested: Vec<char> = marks
            .iter()
            .flat_map(|mark| mark.characters.to_vec())
            .collect();
        assert_eq!(combining[row], harvested, "row {row} lost its accents");
    }
}

#[test]
fn test_a_session_with_no_marks_at_all_keeps_one_open_block() {
    // An un-integrated shell, an ssh session, a container: nothing reports a
    // boundary, so there is exactly one block and it holds everything. A
    // half-populated list would be worse than none.
    let mut emulator = emulator();
    emulator.advance(b"$ ls\r\na  b\r\n$ ");

    assert!(emulator.blocks().is_empty());
    let live = emulator.live_block();
    assert_eq!(BlockState::Unknown, live.state);
    assert_eq!(None, live.command);
    assert_eq!(
        0, live.top_row,
        "the block starts at the first line there is"
    );
    assert_eq!(2, live.bottom_row);

    // And it keeps holding everything as the output scrolls into history.
    emulator.advance(b"\r\n".repeat(20).as_slice());
    let live = emulator.live_block();
    assert_eq!(BlockState::Unknown, live.state);
    assert_eq!(
        -(emulator.history_len() as i32),
        live.top_row,
        "the open block let go of the top of its own output"
    );
}

#[test]
fn test_a_submitted_command_line_is_the_blocks_command() {
    // The boundary that needs no cooperation from the shell, and the only one
    // that has the command text exactly rather than as it was echoed.
    let mut emulator = emulator();
    emulator.advance(format!("{A}$ {B}").as_bytes());

    emulator.command_submitted("echo  hi");
    assert_eq!(BlockState::Submitted, emulator.live_block().state);
    assert!(emulator.live_block().started_at.is_some());

    emulator.advance(format!("echo  hi\r\n{C}hi\r\n\x1b]133;D;0\x07").as_bytes());
    let [block] = emulator.blocks() else {
        panic!("the submitted command made no block");
    };
    assert_eq!(
        Some("echo  hi".to_owned()),
        block.command,
        "the echoed text overwrote what the application knows it wrote"
    );
}

#[test]
fn test_a_second_submission_while_one_is_running_is_ignored() {
    let mut emulator = emulator();
    emulator.advance(format!("{A}$ {B}").as_bytes());
    emulator.command_submitted("sleep 9");
    emulator.advance(format!("sleep 9\r\n{C}").as_bytes());

    emulator.command_submitted("typed while it ran");
    assert_eq!(
        Some(IgnoreReason::SubmitDuringCommand),
        emulator.last_ignored_mark()
    );
    assert_eq!(
        Some("sleep 9".to_owned()),
        emulator.live_block().command,
        "typeahead replaced the running command"
    );
}

#[test]
fn test_the_shell_exiting_terminates_the_session_for_good() {
    let mut emulator = emulator();
    emulator.advance(format!("{A}$ {B}exit\r\n{C}").as_bytes());
    emulator.child_exited(crate::ChildExit::from_code(0));

    let [block] = emulator.blocks() else {
        panic!("the block open at exit was lost");
    };
    assert_eq!(BlockState::Terminated, block.state);
    assert_eq!(Some("exit".to_owned()), block.command);
    assert_eq!(BlockState::Terminated, emulator.live_block().state);

    // Absorbing: anything still printing on the pty is a child that outlived
    // the shell, and no mark of its reopens the session.
    for mark in [A, B, C, "\x1b]133;D;0\x07"] {
        emulator.advance(mark.as_bytes());
        assert_eq!(
            Some(IgnoreReason::Terminated),
            emulator.last_ignored_mark(),
            "{mark:?}"
        );
    }
    assert_eq!(1, emulator.blocks().len());
    assert_eq!(BlockState::Terminated, emulator.live_block().state);
}

#[test]
fn test_the_live_block_names_the_rows_it_owns() {
    let mut emulator = emulator();
    emulator.advance(format!("{A}$ {B}ls\r\n{C}").as_bytes());
    assert_eq!((0, 0), row_span(&emulator), "the prompt row is the block");

    emulator.advance(b"a\r\nb\r\n");
    assert_eq!((0, 2), row_span(&emulator));

    emulator.advance(b"\x1b]133;D;0\x07");
    emulator.advance(format!("{A}$ ").as_bytes());
    assert_eq!(
        (3, 3),
        row_span(&emulator),
        "the next block starts under the one that was harvested"
    );
}

/// The viewport rows the open block covers.
fn row_span(emulator: &Emulator) -> (i32, i32) {
    let live = emulator.live_block();
    (live.top_row, live.bottom_row)
}

#[test]
fn test_a_column_change_finds_the_open_block_again() {
    // A resize reflows the scrollback, so every line number stored against the
    // grid moves. What the open block starts at is then read off the reflowed
    // screen: the rows above it were erased when the block before it was
    // harvested, so the first line with anything on it is its own first row.
    let mut emulator = emulator();
    emulator.advance(format!("{A}$ {B}x\r\n{C}one\r\ntwo\r\n").as_bytes());
    assert_eq!(0, emulator.live_block().top_row);

    emulator.resize(TerminalSize::new(40, 6));
    assert_eq!(0, emulator.live_block().top_row);
    assert_eq!(BlockState::Executing, emulator.live_block().state);
}

#[test]
fn test_a_column_change_does_not_hand_a_harvested_block_to_the_open_one() {
    // The rows of a finished command are still on the screen when the next one
    // opens, and a reflow used to give the open block the whole grid. The list
    // then painted a screenful of already-stored output a second time, under a
    // divider, as if the shell had printed it again.
    let mut emulator = emulator();
    emulator.advance(format!("{A}$ {B}x\r\n{C}one\r\ntwo\r\n\x1b]133;D;0\x07").as_bytes());
    emulator.advance(format!("{A}$ {B}").as_bytes());
    let [block] = emulator.blocks() else {
        panic!("the first command made no block");
    };
    assert_eq!("$ x\none\ntwo", block.rows.to_text());

    emulator.resize(TerminalSize::new(40, 6));
    assert_eq!(
        "$",
        live_text(&mut emulator),
        "the open block swallowed the block above it"
    );
}

/// What a list would paint as the open block: the snapshot rows its anchor
/// names, as text.
fn live_text(emulator: &mut Emulator) -> String {
    let live = emulator.live_block();
    let snapshot = emulator.snapshot();
    let last = live.bottom_row.min(snapshot.rows as i32 - 1);
    (live.top_row.max(0)..=last)
        .map(|row| {
            let cells = snapshot.row(row as usize);
            let end = cells
                .iter()
                .rposition(|cell| cell.c != ' ')
                .map_or(0, |at| at + 1);
            cells[..end].iter().map(|cell| cell.c).collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn test_a_shell_that_never_reports_a_mark_never_looks_like_it_is_running() {
    // The composer is taken away while a command runs, and a shell with no
    // integration — `ssh` to a host that has none, a container, the opt-out —
    // reports neither the start nor the end of one. If a line handed to such a
    // shell counted as running, the first Enter of the session would take the
    // field away for good and there would be no way to type a second command.
    let mut emulator = emulator();
    emulator.advance(b"$ ");
    emulator.command_submitted("ls");
    emulator.advance(b"ls\r\na  b\r\n$ ");

    let live = emulator.live_block();
    assert_eq!(BlockState::Submitted, live.state);
    assert!(
        !live.state.is_running(),
        "a line nothing has answered took the pane"
    );
    assert!(emulator.blocks().is_empty(), "nothing reported a boundary");
}

#[test]
fn test_a_transient_prompt_redrawn_after_enter_does_not_split_the_block() {
    // powerlevel10k's `transient_prompt` and starship's `Enable-Transience`
    // re-render PS1 when the line is accepted, and the A and B marks live
    // inside PS1 — so both arrive again after the submit and before
    // `preexec`'s C. Obeying them files an empty, statusless block above every
    // single command.
    let mut emulator = emulator();
    emulator.advance(format!("{A}$ {B}").as_bytes());
    emulator.command_submitted("echo hi");
    emulator.advance(format!("\r\x1b[K{A}> {B}echo hi\r\n{C}hi\r\n\x1b]133;D;0\x07").as_bytes());

    let blocks = emulator.blocks();
    assert_eq!(1, blocks.len(), "the transient prompt split the block");
    assert_eq!(Some("echo hi".to_owned()), blocks[0].command);
    assert_eq!(Some(0), blocks[0].exit);
    assert_eq!("> echo hi\nhi", blocks[0].rows.to_text());
}

#[test]
fn test_rows_below_the_cursor_when_a_command_ends_belong_to_it() {
    // A progress display that redraws its own last lines leaves the cursor
    // above them: `\e[2A` and two writes, then the completion. A block that
    // stopped at the cursor would drop the rows under it out of the harvest
    // and then anchor the next block on top of them, where nobody paints them
    // and the next prompt overwrites them.
    let mut emulator = emulator();
    emulator.advance(format!("{A}$ {B}build\r\n{C}one\r\ntwo\r\nthree\r\n").as_bytes());
    emulator.advance(b"\x1b[3Aone   done\r\n");
    emulator.advance(b"\x1b]133;D;0\x07");

    let [block] = emulator.blocks() else {
        panic!("the command made no block");
    };
    assert_eq!(
        "$ build\none   done\ntwo\nthree",
        block.rows.to_text(),
        "the rows below the cursor were left behind"
    );
    // And the next block opens where the shell will actually print — the row
    // the cursor was left on, not the row after the harvest — so its prompt
    // belongs to somebody.
    emulator.advance(format!("{A}$ ").as_bytes());
    assert_eq!((2, 2), row_span(&emulator));
    assert_eq!("$", live_text(&mut emulator));
}

#[test]
fn test_a_terminal_that_has_seen_nothing_has_the_default_block_open() {
    // What a renderer gets before a single byte arrives, and what
    // `LiveBlock::default()` has to keep meaning.
    assert_eq!(LiveBlock::default(), emulator().live_block());
}

#[test]
fn test_a_stale_prompt_end_does_not_become_the_command() {
    // A `B` the shell printed and then scrolled a long way from — or one a
    // file printed. Reading the screen between it and the output start would
    // put a screenful of text in the command header.
    let mut emulator = emulator();
    emulator.advance(format!("{A}$ {B}").as_bytes());
    emulator.advance("noise\r\n".repeat(40).as_bytes());
    emulator.advance(format!("{C}out\r\n\x1b]133;D;0\x07").as_bytes());

    let [block] = emulator.blocks() else {
        panic!("the block was lost");
    };
    assert_eq!(None, block.command);
    assert_eq!(Some(0), block.exit);
}

#[test]
fn test_a_shell_killed_under_a_full_screen_program_still_closes_its_block() {
    // The one signal the alternate screen does not suspend. A session that has
    // ended has to close its block wherever it ended, and what it takes with it
    // is the command — not a picture of the program's last frame, which never
    // scrolled and is nobody's scrollback.
    let mut emulator = emulator();
    emulator.advance(format!("{A}$ {B}vim\r\n{C}").as_bytes());
    emulator.advance(b"\x1b[?1049h");
    emulator.advance(b"~\r\n~\r\n-- INSERT --");

    emulator.child_exited(crate::ChildExit::from_code(0));

    let [block] = emulator.blocks() else {
        panic!("the block was lost with the shell");
    };
    assert_eq!(BlockState::Terminated, block.state);
    assert_eq!(Some("vim".to_owned()), block.command);
    assert!(block.rows.is_empty());
    assert_eq!(BlockState::Terminated, emulator.live_block().state);
}

#[test]
fn test_the_prompt_end_is_the_cell_after_the_prompt_s_last_one() {
    // What a composer drawn on the prompt's own row needs: the cell the shell
    // would echo the first character of a command line into, which is one past
    // the last cell the prompt painted.
    let mut emulator = emulator();
    emulator.advance(format!("{A}$ {B}").as_bytes());

    let live = emulator.live_block();
    let prompt = live
        .prompt_end
        .expect("the shell said where its prompt ended");
    assert_eq!(
        prompt,
        PromptEnd { row: 0, column: 2 },
        "`$ ` is two cells, so the third is where typing goes"
    );
    assert_eq!(
        prompt.row, live.bottom_row,
        "the prompt is the block's last row"
    );

    // And it is exactly where the shell does put it: the echo of a command
    // lands in that cell.
    emulator.advance(b"echo hi");
    let snapshot = emulator.snapshot();
    assert_eq!(
        snapshot.cell(0, prompt.column).map(|cell| cell.c),
        Some('e'),
        "the shell echoed somewhere else: {:?}",
        snapshot.text()
    );
}

#[test]
fn test_a_prompt_end_moves_with_the_row_it_was_printed_on() {
    // The anchor's whole job. A prompt three rows above the bottom of a
    // twenty-column, six-row grid is two rows above it after two lines scroll
    // past, and a cell that did not move with them would be a caret drawn on
    // somebody else's output.
    let mut emulator = emulator();
    emulator.advance(format!("{A}$ {B}").as_bytes());
    let before = emulator.live_block().prompt_end.expect("a prompt ended");

    emulator.advance(b"\r\nnoise\r\nnoise");
    let after = emulator
        .live_block()
        .prompt_end
        .expect("the mark did not survive");
    assert_eq!(after.column, before.column);
    assert_eq!(
        after.row, before.row,
        "nothing has scrolled off a six-row grid yet"
    );

    emulator.advance("\r\nnoise".repeat(10).as_bytes());
    let scrolled = emulator
        .live_block()
        .prompt_end
        .expect("the mark did not survive");
    assert!(
        scrolled.row < 0,
        "the prompt has gone off the top and should say so, not clamp to row {}",
        scrolled.row
    );
}

#[test]
fn test_the_three_cases_with_no_prompt_end_and_no_fourth() {
    // Each of them is a defined answer rather than a gap: a renderer asking
    // "where does the line being typed start" gets `None` and puts it on a row
    // of its own.

    // One: a session whose shell reports nothing at all.
    let mut bare = emulator();
    bare.advance(b"$ ");
    assert_eq!(None, bare.live_block().prompt_end, "no marks, no cell");

    // Two: a prompt that has started and not finished.
    let mut drawing = emulator();
    drawing.advance(format!("{A}$ ").as_bytes());
    assert_eq!(
        None,
        drawing.live_block().prompt_end,
        "`A` alone does not say where the prompt ends"
    );

    // Three: the alternate screen, where marks are not believed at all and the
    // cell a prompt underneath ended on has been painted over.
    let mut full_screen = emulator();
    full_screen.advance(format!("{A}$ {B}vim\r\n{C}").as_bytes());
    full_screen.advance(b"\x1b[?1049h~\r\n~");
    assert_eq!(
        None,
        full_screen.live_block().prompt_end,
        "a full-screen program owns the grid"
    );
    full_screen.advance(b"\x1b[?1049l");
    assert!(
        full_screen.live_block().prompt_end.is_some(),
        "and the mark is back the moment the program gives the screen up"
    );
}

#[test]
fn test_a_prompt_that_fills_its_row_ends_on_the_row_below() {
    // Alacritty holds the cursor on the last column with a wrap pending rather
    // than storing one past the end of the row, so an anchor on the cursor
    // would name the cell the prompt's own last character is in — and a caret
    // drawn there would sit on top of it.
    let mut emulator = emulator();
    emulator.advance(format!("{A}{}{B}", "-".repeat(20)).as_bytes());

    let live = emulator.live_block();
    assert_eq!(
        Some(PromptEnd { row: 1, column: 0 }),
        live.prompt_end,
        "twenty columns are full, so the next character goes below them"
    );
}

#[test]
fn test_the_prompt_end_is_part_of_what_a_repaint_is_worth() {
    // It is drawn from, so a frame that would show it somewhere else has to be
    // a new revision. It rides on `live_block`, which `same_content` already
    // compares, and this is the test that says so.
    let mut emulator = emulator();
    emulator.advance(format!("{A}$ ").as_bytes());
    let before = emulator.snapshot();

    emulator.advance(B.as_bytes());
    let after = emulator.snapshot();
    assert_ne!(
        before.revision, after.revision,
        "the mark printed nothing, and the caret still moved"
    );
    assert!(!after.same_content(&before));
}

/// The rows of a finished block from where its output starts, as text.
///
/// What "Copy output" takes, spelled out here rather than reached for through
/// the application: the point of these tests is the boundary, and a helper
/// that computed it a second way would agree with itself rather than with the
/// tracker.
fn output_text(block: &Block) -> Option<String> {
    let from = block.output_from?;
    let mut text = String::new();
    for row in from..block.rows.rows() {
        if row > from {
            text.push('\n');
        }
        text.push_str(block.rows.text(row));
    }
    Some(text)
}

#[test]
fn test_a_block_knows_which_of_its_rows_are_the_command_s_own_output() {
    let mut emulator = emulator();
    emulator.advance(format!("{A}$ {B}echo hi\r\n{C}hi\r\n\x1b]133;D;0\x07").as_bytes());

    let [block] = emulator.blocks() else {
        panic!("one finished block, got {}", emulator.blocks().len());
    };
    assert_eq!("$ echo hi\nhi", block.rows.to_text());
    assert_eq!(
        Some(1),
        block.output_from,
        "the prompt and the echoed line are row zero; `C` landed on row one"
    );
    assert_eq!(Some("hi".to_owned()), output_text(block));
}

#[test]
fn test_a_prompt_of_two_rows_keeps_both_of_them_out_of_the_output() {
    // Every prompt framework anybody uses draws two lines, and "the output is
    // everything after the first row" is wrong for all of them. The boundary
    // comes from the mark, so the number of rows above it is whatever the
    // shell drew.
    let mut emulator = emulator();
    emulator.advance(format!("{A}~/work\r\n$ {B}ls\r\n{C}a\r\nb\r\n\x1b]133;D;0\x07").as_bytes());

    let [block] = emulator.blocks() else {
        panic!("one finished block, got {}", emulator.blocks().len());
    };
    assert_eq!("~/work\n$ ls\na\nb", block.rows.to_text());
    assert_eq!(Some(2), block.output_from);
    assert_eq!(Some("a\nb".to_owned()), output_text(block));
}

#[test]
fn test_a_command_that_printed_nothing_has_an_empty_output_rather_than_a_row() {
    let mut emulator = emulator();
    emulator.advance(format!("{A}$ {B}true\r\n{C}\x1b]133;D;0\x07").as_bytes());

    let [block] = emulator.blocks() else {
        panic!("one finished block, got {}", emulator.blocks().len());
    };
    assert_eq!("$ true", block.rows.to_text());
    assert_eq!(
        Some(1),
        block.output_from,
        "one past the last row it has, which is an empty range rather than a \
         row of somebody else's text"
    );
    assert_eq!(Some(String::new()), output_text(block));
}

#[test]
fn test_output_that_scrolled_the_screen_keeps_its_boundary() {
    // The anchor moves with the history exactly as the block's own top does,
    // and the two are subtracted from each other — so a command that printed
    // more than the screen holds still says its output starts on row one.
    let mut emulator = emulator();
    emulator.advance(format!("{A}$ {B}seq\r\n{C}").as_bytes());
    for line in 0..12 {
        emulator.advance(format!("{line}\r\n").as_bytes());
    }
    emulator.advance(b"\x1b]133;D;0\x07");

    let [block] = emulator.blocks() else {
        panic!("one finished block, got {}", emulator.blocks().len());
    };
    assert_eq!(Some(1), block.output_from);
    assert_eq!(
        Some("0\n1\n2\n3\n4\n5\n6\n7\n8\n9\n10\n11".to_owned()),
        output_text(block),
        "everything the command printed, and not the line it was typed on"
    );
}

#[test]
fn test_a_block_that_never_ran_says_nothing_about_where_its_output_starts() {
    // Three ways to have no `C`, and none of them is given a guess: a line
    // submitted into a shell that never answered, a session with no marks at
    // all, and a command whose block was reflowed by a resize while it ran.
    let mut submitted = emulator();
    submitted.advance(format!("{A}$ {B}").as_bytes());
    submitted.command_submitted("ssh far-away");
    // A completion is the only signal that closes a block a submit opened —
    // a prompt arriving there is the redraw a transient prompt makes.
    submitted.advance(b"\x1b]133;D;0\x07");
    assert_eq!(
        vec![None],
        submitted
            .blocks()
            .iter()
            .map(|block| block.output_from)
            .collect::<Vec<_>>(),
        "nothing came back, so nothing said where the output would start"
    );

    let mut silent = emulator();
    silent.advance(b"$ echo hi\r\nhi\r\n");
    silent.advance(format!("{A}$ ").as_bytes());
    assert_eq!(
        vec![None],
        silent
            .blocks()
            .iter()
            .map(|block| block.output_from)
            .collect::<Vec<_>>(),
    );

    let mut resized = emulator();
    resized.advance(format!("{A}$ {B}ls\r\n{C}a\r\n").as_bytes());
    resized.resize(TerminalSize::new(30, 6));
    resized.advance(b"\x1b]133;D;0\x07");
    assert_eq!(
        vec![None],
        resized
            .blocks()
            .iter()
            .map(|block| block.output_from)
            .collect::<Vec<_>>(),
        "the reflow moved every line the anchor was measured against"
    );
}

#[test]
fn the_first_block_of_a_session_still_knows_where_its_command_ran() {
    // A block learns its directory when it opens, from what the shell last
    // reported — and the first block of a session opens before a byte has
    // arrived. Left at that it would be the one block in the list that cannot
    // say where its command was run, which is exactly the block a person is
    // most likely to still be looking at.
    let mut emulator = emulator();
    emulator.advance(b"\x1b]7;file://host/tmp\x07");
    emulator.advance(format!("{A}$ {B}ls\r\n{C}a\r\n\x1b]133;D;0\x07").as_bytes());

    let [block] = emulator.blocks() else {
        panic!("one finished block, got {}", emulator.blocks().len());
    };
    assert_eq!(
        Some(std::path::Path::new("/tmp")),
        block.working_directory.as_deref(),
        "the directory the command was typed in"
    );
}
