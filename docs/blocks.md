# Blocks: a pane's output as a list of commands

A Crook pane is not a terminal grid with decorations drawn over it. It is a vertical list
whose items are commands: each one holds its prompt, the line that was run, and everything
that line printed, and it owns those rows rather than borrowing them from the emulator. The
line being composed sits under the list as a sibling, on the same ground, in the same font,
at the same column zero.

This document is the map of that surface: what draws it, where each number comes from, and —
just as important — what it deliberately does not do yet.

---

## 1. Where a block comes from

Two things mark a boundary, and only one of them needs the shell's help.

**The submit.** `Terminal::submit` is what the composer calls on Enter. It records the exact
command text and the exact moment *before* a byte leaves for the pty. No escape sequence is
involved and no shell has to cooperate, which is why a block always knows what was run even
when it never learns how it ended.

**OSC 133.** `crook_terminal::marks` reads the four FinalTerm marks — `A` prompt start, `B`
prompt end, `C` output start, `D;<exit>` command finished — out of the same second `vte` pass
that already watched for OSC 7. `app/src/shell_integration` installs the snippet that emits
them into zsh, bash and fish without touching anybody's dotfiles: it writes a scratch
`ZDOTDIR` / `--rcfile` / `vendor_conf.d` stub, and removes it when the pane closes.

**There is nothing to install and nothing to configure.** Opening a pane on zsh, bash or fish
is all it takes; the snippets chain onto whatever hooks they find rather than replacing them,
and `~/.zshrc` and friends are never written to. Setting `CROOK_NO_SHELL_INTEGRATION` to
anything but `0` or the empty string turns injection off for every pane.

**What it cannot reach** is any shell Crook did not start: the far side of an `ssh`, a
container, a `docker exec`, or a shell Crook has no snippet for (`pwsh`, `nu`, `ksh`, `tcsh`).
For the first three, the same text — `app/src/shell_integration/crook.{zsh,bash,fish}`,
returned by `shell_integration::snippet` — can be pasted at the end of the configuration file
on *that* machine; the snippets are written to be sourced that way, guarded against being
sourced twice and interactive-only.

A shell with no integration emits nothing. Then nothing in the block table ever fires, the
session keeps exactly one open block in `BlockState::Unknown`, and that block holds
everything from the first byte to the last. That is the honest rendering — one continuous
stream, no chrome claiming boundaries nobody reported — and it is also what Crook drew before
blocks existed. From its first screenful onwards such a pane is drawn as a plain grid with no
composer under it, and every key goes to the shell: see §3.

**A submitted line is not a running command.** `BlockState::Submitted` says a line was handed
to the shell and nothing has come back; only `C` says it started. The difference matters
because the chrome that means "running" — the accent stripe, and taking the composer's space —
must never latch on a shell that will never answer. `ssh` to a host with no integration, a
container, the opt-out variable: all of them leave the block `Submitted` for the rest of the
session, and a rule that read that as "running" would take the field away at the first Enter
and never give it back.

**A doubtful mark is ignored rather than obeyed**, because merging two blocks is recoverable
and splitting one is not. The case worth naming: every prompt framework with a *transient*
prompt — powerlevel10k's `transient_prompt`, starship's `Enable-Transience` — re-renders `PS1`
when a line is accepted, and the `A` and `B` marks live inside `PS1`, so both arrive again
between the submit and `preexec`'s `C`. Obeying them would file an empty, statusless block
above every single command, so a prompt start arriving while a line is in flight is treated as
the redraw it is. Nothing is lost: a prompt that really is the next one is followed by a `C`,
and that closes the block.

## 2. Harvesting, and why the grid's history is dropped

When a block closes, its rows are **copied out of the emulator** into a compact per-block
store (`crook_terminal::harvest`) and the emulator's scrollback is cleared. The rows are kept
as text plus run-length style runs, not as `SnapshotCell`s: a cell is twelve bytes, and ten
thousand rows of two hundred columns would be 24 MB per pane, for ever, with nothing evicting
it.

Dropping the history at every harvest is what keeps a block's anchor exact. Alacritty's
history is capped, evicts from the top without counting, and reflows on resize, so an
absolute line number is stable across neither. After a harvest the history can only hold lines
the *open* block pushed off the screen — so it can only saturate when the open block is longer
than the whole scrollback, at which point "this block starts above the oldest line we have" is
not an approximation, it is the truth.

The **screen** above the new block is erased at the same moment, and for the same reason: what
is left in the grid is then exactly the open block and nothing else. Two answers depend on
that invariant.

* **A column change.** A reflow moves every line number stored against the grid, so the open
  block's first row is re-found afterwards by looking for the first line with anything on it.
  That is only the right answer because everything above the block has been blanked; without
  the erase the block claims the last screenful of its predecessor, and the list paints a
  screenful of already-stored output a second time, under a divider, as if the shell had
  printed it twice.
* **Rows below the cursor.** A progress display that redraws its own last lines with `\e[3A`
  leaves the cursor above them, and a harvest that stopped at the cursor would drop those rows
  and then re-parent them into the next block. Since nothing above the open block survives,
  anything below the cursor was printed by the block that is open now, and the harvest takes
  it. The next block still opens at the *cursor*, because that is where the shell will print.

## 3. The surfaces

`app/src/pane_surface.rs` is one function that answers both "what draws the output?" and "is
there a composer?", and every element that needs either asks it rather than testing a flag of
its own.

| condition | output |
|---|---|
| the alternate screen is up | the grid |
| the open block starts above the viewport | the grid |
| otherwise | the block list |

| condition | composer |
|---|---|
| the output is drawn as the grid | no |
| the shell reported a command running ≥ 50 ms ago | no |
| otherwise | yes |

The **overflow rule** is what an un-integrated shell gets from its first screenful onwards.
Once the open block's first row is above the emulator's viewport, a `Snapshot` no longer holds
all of it, and drawing it as a block would silently show only the screenful that is left. The
grid, scrolled by the wheel through the emulator's own scrollback, shows all of it. It is also
where a program that addresses the whole primary screen ends up, and that is the right place
for it: a `less` redrawing row four means row four of the *screen*, not of a block with
paddings above it.

**The grid never has a composer under it**, and that is not a preference. `PaneSizer` tells
the pty how many rows the *pane* holds; a grid laid out above a composer has fewer rows than
that to draw them in, so the newest ones — the prompt, the line being echoed — end up under
the field and are never painted at all. It is also the honest arrangement: this surface is the
one for "there is no block structure here", so the input for it is the pty itself, which is
where every key goes once there is no field. A shell with no integration is therefore a plain
terminal, and everything a plain terminal does still works.

A line half-composed when a pane changes surfaces is not lost — the text belongs to
`PaneInput`, which outlives every element — but it is not drawn either, and the keys that
follow it go to the shell. In practice an un-integrated pane has crossed to the grid before
anybody has typed into it, and an integrated one only crosses while a command is running,
which is the case the composer is taken away for anyway.

The **long-running rule** is the one that earns its keep. Without it `less`, `top`, `git log`
and any pager that does not take the alternate screen become an infinitely growing block with
a text field underneath that the program cannot see and the person cannot use. Fifty
milliseconds is Warp's number: fast enough that no command anybody waits for is missed, slow
enough that `ls` never flickers the composer away and back. `TerminalModel` arms a one-shot
repaint for the moment the threshold passes, because a `sleep 5` submitted into a quiet shell
produces no further output and nothing else would ever draw that frame.

Note what it does *not* do: it takes the composer's space, not the block list. A `cargo build`
printing for a minute is still one command among the ones before it, and throwing a session's
blocks away for the duration would be a worse answer than the text field the rule exists to
remove. A program that genuinely owns the screen reaches the grid by outgrowing the viewport.

## 4. The list element

`app/src/workspace/block_list.rs`. One element that owns its scroll offset and paints only
what is in view — **not** a `Scrollable`, which lays its child out at infinite height and
paints all of it.

Layout keeps a prefix sum of item heights (`pane_blocks::Heights`) in the view between frames,
binary-searches it for the first visible item, and walks forward until it passes the bottom of
the box, recording a small vector of `(index, top, height)`. Paint replays that vector. The
same arithmetic runs a second time *inside* an item, so a fifty-thousand-row block costs a
screenful. The finished part of the sum is rebuilt only when a command ends; the open block is
the last item, so following a printing command is one addition.

A finished block's rows are materialised one at a time into a scratch `Vec<SnapshotCell>`
reused down the whole list and handed to the same three passes the grid uses — merged
background runs, underline and strikeout rules, then a glyph id and a fixed advance per cell.
There is one cell painter, not two, and a command that has ended is not drawn differently from
one still printing: the store keeps every cell's background and every rule, which is what a
`git diff` hunk, a `grep --color` match, `ls`'s directory colours and a powerlevel10k prompt
are made of.

The open block is painted straight from the `Snapshot`, from the rows its anchor names and no
others: the rows above those are stale copies of blocks already harvested, and drawing them
would show the same output twice.

## 5. Scrolling

The position is a mode, not a number:

```
FollowBottom          // re-resolved to the current maximum every frame
Fixed(lines_from_top) // measured from the top, so a block growing below does not move you
```

Everything that moves it goes through one function with a named cause (`Wheel`, `Resize`,
`Submit`, `KeyToPty`), so "does typing snap me back to the bottom?" has one readable answer:

* a wheel that lands at the maximum re-enters `FollowBottom` rather than pinning the number
  that was the maximum a moment ago;
* a submit and any key that reaches the pty return to `FollowBottom`;
* a resize keeps the mode, and only an offset that no longer has content under it gives way.

Heights are fractional lines summed through a prefix array, so every comparison — including
"is there anything to scroll at all" — goes through one tolerance of 0.01 lines.

A list shorter than its box sits on the **bottom** of it. The composer is pinned under the
output, and the whole reason it reads as the next line of the terminal is that the open
block's last row is immediately above it.

## 6. Chrome

There is no box. A block has no border, no corner radius and no fill in its resting state.

* **Divider** — 1 px, full pane width, `overlay_2` (the foreground at 10 %, translucent so it
  composites over whatever background the shell has made), drawn along the top edge of every
  block that has one before it. Never along the top of the pane, which separates the pane from
  nothing.
* **Padding** — 1.1 lines above a block's first row, 1.0 below its last: 2.1 lines between two
  commands, which is Warp's normal spacing. A block with no command collapses to its rows. The
  open block has no bottom padding, because what follows it is not another command but the
  line being typed at its prompt.
* **Failed** — the theme's ANSI red at 10 % over the block's whole height, plus a 5 px stripe
  down its left edge at full strength. Neither Ctrl-C's 130 nor SIGPIPE's 141 counts as
  failure, and a command that reported no status has no verdict.
* **Running** — the same 5 px stripe in the accent, on the open block from the moment the
  shell reports `C` until it reports the end. A line the shell has not answered draws nothing:
  see §1.
* **Copy** — a 26 px square control at the top-right of the *hovered* block, 12 px in from the
  pane's right edge so it clears the thumb. It follows the block into view rather than sitting
  on its top edge, because a block taller than the pane is the one most worth copying and its
  top edge is usually above the window. Clicking it puts exactly that block's command and
  output on the clipboard: no neighbour's text, no trailing blank rows, no space injected
  after a wide character and no combining accent dropped. That is the thing scrollback cannot
  do.

A hover is re-derived from the pointer on every frame, not only when the pointer moves: a
wheel and a command finishing both put a different block under a still pointer, and a control
drawn on the block that *was* there is an affordance pointing at output a click would not
copy.
* **Thumb** — 4 px wide, 2 px inset, 24 px minimum, in the geometry `Scrollable` paints its
  own with, so the two scrollbars in the application match.

## 7. The composer

`app/src/workspace/input_element.rs`, assembled in `body.rs`. **It is not a box.** No
background, no corner radius, no side or bottom border, no margin, and no focus ring: the
caret existing is the entire focus affordance. It fills nothing of its own, so a shell that
changes its background with OSC 11 carries the composer with it exactly as it carries the
pane.

The only thing that ever separates it from the output is a 1 px rule in the same role, at the
same width, running the same full-bleed extent as the divider the list draws between two
blocks — and it is drawn **only when there is output cut off underneath it**. While the list
is following its own end there is nothing below the fold, and there is no seam at all.

There is **no prompt glyph**. The shell's own prompt is on the row above, in the open block,
and a second invented one under it is two prompts on screen, which is what makes a composer
read as a widget. Column zero in the composer is column zero in the output: one `GUTTER`
constant is the list's left inset, the composer's left padding, and the width `PaneSizer`
takes off the pane before working out how many columns to tell the pty about.

The text and the caret are painted in the colours the *shell* resolved — `Snapshot::foreground`
and the cursor colour the grid paints the shell's own cursor in — rather than the theme's. A
shell that changes them at runtime (OSC 10 and OSC 12, which is what every light-or-dark theme
script sends) takes the pane's ground and every block on it with it, and a field left behind in
the theme's grey would be the one thing on the pane that did not follow.

The caret is a 3 px pill-rounded bar. While the composer is up the output draws **no cursor at
all**: two cursors in one pane say nothing about which of them is listening. Once the composer
has gone, the shell's own cursor is the only one there is, and it is filled where the pane has
the keyboard and hollow where it does not.

## 8. Resizing

`body::PaneSizer` is the only thing that resizes a pty, and it measures the **pane's**
rectangle, not the output's. The output's box changes whenever the composer appears, hides or
grows a line, and a `SIGWINCH` on each of those frames is a storm at programs that handle them
badly. Reporting the pane also means a full-screen program that fills the pane fits it.

The consequence is that the surface elements are laid out *after* the resize, so a snapshot
measured for the grid the last frame had is refreshed rather than painted into the new box.

---

## What this does not do

Stage 1 of the port. These are absent on purpose, not overlooked:

* **Cross-block selection.** Selection is still the emulator's, which means it works inside
  the **open block and nowhere else**: a finished block's cells have been harvested out of the
  emulator, so a drag cannot reach across two of them. Copying a whole finished block needs no
  selection and is exact — that is what the hover control is for. The list-level selection that
  would fix this is Stage 2, and it must be *one* implementation rather than two that have to
  agree.
* **Block selection.** No click-to-select, no shift-click, no accent wash, no per-block border.
* **Keyboard block navigation.** No Cmd-Up / Cmd-Down, no scroll-to-top-of-block.
* **The sticky header.** A block taller than the window scrolls like any other content; there
  is nothing pinned to say which command you are inside.
* **Jump-to-bottom.** No button when a block continues below the fold.
* **`clear` as a gap.** Ctrl-L does what the emulator does with it.
* **Lazy reflow on a column change.** A harvested block keeps the width it was harvested at.
* **The prompt hoisted into the composer.** Warp lifts the shell's own prompt out of the grid
  and re-draws it as a one-line lead-in inside the field, so that exactly one prompt is on
  screen and the line being typed is attached to it. Crook leaves the prompt where the shell
  drew it — the last row of the open block — and puts the field on the row under it. The
  consequence is visible and worth stating: pressing Enter moves the text up one row and right
  by the width of the prompt, to where the shell echoes it. The marks that would fix this now
  exist (`B` is exactly where the echoed command starts), so this is the next thing to build
  rather than a limitation of the design.
* Share, re-run, bookmarks, block filters, and everything else that needs a block to be
  addressable rather than merely visible.
