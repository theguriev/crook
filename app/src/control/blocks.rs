//! `pane.blocks`: a pane's finished commands, and what they printed.
//!
//! What a lead reads after a worker's command has finished: `crook pane
//! blocks 7 --last 1` is the command line, its exit status, where it ran, how
//! long it took and its output — the structure a block terminal keeps and a
//! scrollback does not. The output is the text a copy of the block's output
//! gives, through the same region a drag over the block makes — see
//! `block_text` in the workspace — so that what a script reads and what a
//! person copies cannot disagree about where a folded line ends.
//!
//! # What is not a block
//!
//! A full-screen program, and an agent's TUI, are drawn as one live grid — see
//! [`pane_surface`] — and nothing in a grid has finished: an interactive
//! Claude Code in a pane is one block that is still open, and its answer is on
//! a screen, not in a block. A pane like that with no finished command is
//! refused as `no-blocks` with a sentence that says so, rather than answered
//! with an empty list that reads as "nothing ran"; one with finished commands
//! from before is answered with those and `grid` set. So is a shell that
//! reports no command marks, whose output is one block that never closes. A
//! worker whose answer is meant to be read runs headless — `claude -p` — and
//! its answer is then its block's output.
//!
//! # Caps
//!
//! An answer is built between two frames and written down a socket, so every
//! part of it is bounded: [`MAX_BLOCKS`] blocks,
//! the last [`MAX_ROWS_READ`] rows of each read, the last
//! [`MAX_OUTPUT_PER_BLOCK`] bytes of those kept, and [`MAX_OUTPUT`] bytes in
//! all, spent newest first. Cut from the front, since the end of what a
//! command printed is where its errors and its summary are, and marked
//! `truncated`.

use std::time::Instant;

use crookui_core::prelude::*;
use serde_json::Value;

use super::protocol::{BlockEntry, BlocksRead, MAX_BLOCKS, ReadBlocks, Refusal, code};
use super::watch;
use crate::pane_surface::{self, Surface};
use crate::workspace::Workspace;

/// The most bytes of one block's output an answer carries: its end.
///
/// Sixty-four kilobytes, a thousand lines or so: past a test run's summary and
/// its failures, and short of a build log nobody reads whole.
pub const MAX_OUTPUT_PER_BLOCK: usize = 64 * 1024;

/// The most bytes of output one answer carries, over all its blocks.
///
/// Half a megabyte, spent on the newest block first, so that asking for a
/// hundred blocks costs what asking for eight does and an older block past the
/// total is answered with its facts and an empty, `truncated` output.
pub const MAX_OUTPUT: usize = 512 * 1024;

/// The most rows of one block read to find its end.
///
/// What bounds the window's work for a block that printed a hundred thousand
/// lines: only its last rows are turned into text, and the bytes kept are cut
/// from those.
pub const MAX_ROWS_READ: usize = 4096;

/// Answers one `pane.blocks`.
pub fn read(
    workspace: &Workspace,
    asked: &ReadBlocks,
    token: Option<&str>,
    app: &AppContext,
) -> Result<Value, Refusal> {
    let (_, pane) = watch::observed(workspace, token, asked.pane, app)?;
    let number = asked.pane;
    let no_blocks = |why: &str| Refusal::new(code::NO_BLOCKS, format!("pane {number} {why}"));

    let (Some(history), Some((_, snapshot))) = (
        workspace.terminal_blocks(pane, app),
        workspace.terminal(pane, app),
    ) else {
        return Err(no_blocks(
            "has no shell running, and so no finished command",
        ));
    };
    let grid = pane_surface::of(&snapshot, Instant::now()).surface == Surface::Grid;
    if history.is_empty() {
        if grid {
            return Err(no_blocks(
                "is drawn as one live grid — a full-screen program or an agent's TUI is \
                 running there — and no command has finished in it: what it shows is a screen, \
                 not a finished block; run a worker whose answer you want to read headless, \
                 like `claude -p`, and read its block when it has finished",
            ));
        }
        if workspace.shell_marks(pane, app) == Some(false) {
            return Err(no_blocks(
                "runs a shell that reports no command marks, so its output is one block that \
                 never finishes",
            ));
        }
    }

    let last = asked.last.unwrap_or(MAX_BLOCKS).min(MAX_BLOCKS);
    let mut left = MAX_OUTPUT;
    // Newest first, which is the order the output budget is spent in.
    let mut blocks: Vec<BlockEntry> = (history.len().saturating_sub(last)..history.len())
        .rev()
        .filter_map(|index| history.get(index))
        .map(|block| {
            let from = block.output_from.unwrap_or(0);
            let rows = block.rows.rows();
            let cap = MAX_OUTPUT_PER_BLOCK.min(left);
            let (output, truncated) = if cap == 0 {
                (String::new(), rows > from)
            } else {
                let start = from.max(rows.saturating_sub(MAX_ROWS_READ));
                let text = workspace
                    .block_rows_text(pane, block.id, start, app)
                    .unwrap_or_default();
                let (kept, cut) = tail(text.trim_end(), cap);
                (kept.to_owned(), cut || start > from)
            };
            left = left.saturating_sub(output.len());
            BlockEntry {
                command: block.command.clone(),
                exit: block.exit,
                cwd: block
                    .working_directory
                    .as_deref()
                    .map(|directory| directory.to_string_lossy().into_owned()),
                duration_ms: block
                    .duration()
                    .map(|took| u64::try_from(took.as_millis()).unwrap_or(u64::MAX)),
                output,
                truncated,
            }
        })
        .collect();
    blocks.reverse();

    Ok(serde_json::to_value(BlocksRead {
        pane_id: number,
        blocks,
        grid,
    })
    .expect("a block is numbers and strings, which encode"))
}

/// The end of `text`, at most `cap` bytes of it, and whether anything was
/// cut.
///
/// When something is, what is kept starts at a line — the first whole one in
/// the last `cap` bytes — so the answer does not open on the second half of a
/// line; a last line longer than `cap` on its own is kept from a character
/// boundary instead, since a line cut is better than no output.
pub fn tail(text: &str, cap: usize) -> (&str, bool) {
    if text.len() <= cap {
        return (text, false);
    }
    let mut start = text.len() - cap;
    while !text.is_char_boundary(start) {
        start += 1;
    }
    let kept = &text[start..];
    let kept = match kept.find('\n') {
        Some(newline) if newline + 1 < kept.len() => &kept[newline + 1..],
        _ => kept,
    };
    (kept, true)
}
