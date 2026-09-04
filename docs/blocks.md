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
`TextInput`, which outlives every element — but it is not drawn either, and the keys that
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

## 6. Selecting across the list

`app/src/selection.rs` is the model, `app/src/pane_selection.rs` holds it per pane, and the
two elements that draw output supply the arithmetic between a pixel and a cell. There is one
implementation, and that is the point: two of them is what made a finished command
unselectable in the first place.

**The address space.** An anchor is `(block id, row of that block, column, side of the cell)`
— not a cell of the grid. A block id is handed out once and never reused, and a block's rows
never move within it, so a selection holds still through everything that used to move it: a
command running below adds an *item* rather than shifting rows, the emulator scrolling changes
which viewport row the open block's row seven is drawn on but not that it is row seven, and a
command finishing copies its rows into the store in the order they were already in. Ids
increase with position in the list, so ordering two anchors is one tuple comparison and the
open block — always the newest — always sorts last.

**The open block is not a special case.** It is the last item of the same list with its rows
read out of the `Snapshot` instead of out of a store, and `crook_terminal::Rows` is the one
type that knows the difference: `count`, `columns`, `cell`, `line_length`, `wraps`, `write`,
answered twice each and nowhere else.

**The grid is a list of one block.** A pane showing the alternate screen or an overflowing
block has no finished blocks; it addresses as a single item whose rows are numbered from the
oldest line the scrollback still holds, so the wheel moves the viewport without renaming
anything. Copying reads those rows back out of the emulator with `Terminal::harvest_rows`,
into the very store a finished block uses — which is how a drag through the scrollback copies
text the snapshot never held. A screenful of slack is harvested at each end for what a double
or triple click grows onto; a folded line longer than a whole screen is clipped there, and
only there. A row of it that the wheel has taken off the screen has nothing to grow a word or
a line against while it is out of sight, and is grown again when it comes back — the copy,
which does not read the screen, has it whole either way.

**The two numberings do not mix.** A block counts its rows from its own first and a grid counts
from the oldest line of the history, so the same numbers name different characters on the two
surfaces: which space an anchor was made in is part of it (`selection::Cells`), and nothing
resolves it against the other. A pane crosses between the two on its own — a command printing
past the top of the viewport, a full-screen program starting — and the selection is let go of
when it does, exactly as a resize lets go of it. An invisible highlight that still owned the
copy chord would be an interrupt spent on nothing.

**What is knowingly not stable:** a grid selection held while the scrollback is *full*. Rows are
numbered from the oldest line the history holds, and a full history drops that line for every
new one, so the selection slides one row up the text per line printed. Counting the dropped
lines is not something the emulator underneath can answer. It takes a command printing past a
whole scrollback — ten thousand lines by default — while a selection is being held on the grid.

**A word** is one UAX #29 word-bound segment of the folded line under the pointer, from
`editor::text::word_range_at` — the same function the composer uses, so double-clicking a path
in the output and in the line being typed select the same thing. It crosses a fold, because a
fold is where the terminal put a long line; it never crosses a block, because that is a
different command.

**Painting** is one rectangle per row, under the glyphs, so selected text keeps its own
colour. Between two selected blocks the highlight bridges the gap: a selection running out of
one block and into the next is one run of text with a line break in it, the same thing a
selection across two paragraphs is, and a highlight with a hole at every boundary would read
as several selections that happen to touch. The band takes the columns of the row it continues
so it lines up with the rows either side of it, which also makes an alt-drag bridge as the
column it is rather than as a bar across the padding. The highlight is cut down to the columns
the pane is drawing, so a block harvested at a wider width does not paint over the gutter; only
the picture is short, and the copy still takes the whole row.

**Copying** walks the blocks between the two ends: the ends partially, the ones between them
whole, joined with one newline — so the padding, the dividers and the copy controls contribute
nothing and you get the text you saw without the gutter. Inside a block, a row the terminal
folded runs on into the next without a break, trailing blanks stay behind, and a double-width
character copies once. A selection ends at cells, so there is no trailing newline. The block's
own copy control takes the same region — the whole block, with the blank rows at its end
trimmed off — rather than walking the store its own way, which is how the two came to disagree
about where a folded line ends.

A resize that changes the column count re-wraps the rows under a selection, so the selection is
let go of rather than re-anchored onto text nobody selected.

## 7. Chrome

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
* **The controls** — two 26 px squares at the top-right of the *hovered* block, the outer one
  12 px in from the pane's right edge so it clears the thumb. They follow the block into view
  rather than sitting on its top edge, because a block taller than the pane is the one most
  worth acting on and its top edge is usually above the window.

  The first is **copy**. Clicking it puts exactly that block's command and output on the
  clipboard: no neighbour's text, no trailing blank rows, no space injected after a wide
  character and no combining accent dropped. That is the thing scrollback cannot do.

  The second is **three dots**, and it opens the block's menu — `app/src/workspace/block_menu.rs`,
  Warp's shape and eight of Warp's entries. The split between the two controls is what a menu is
  for: copying is the thing done over and over and keeps a click of its own, and everything else
  is a list that gets read. The menu is not the pane's context menu — a secondary click in the
  output belongs to the shell, and taking a button away from every full-screen program would be
  a worse trade than a control that is visible only where it applies.

  What is in it: **Copy**, **Copy command** and **Copy output**; **Copy working directory** and
  **Copy git branch**; **Run again**; **Scroll to top of block** and **Scroll to bottom of
  block**. Every entry is something the block already knows, and the two that are text go
  through the same region a drag over the block makes (`block_list::block_text`), so the menu,
  the square beside it and a selection cannot disagree about where a folded line ends. "Copy
  output" is the one that needed the emulator to grow a field: `Block::output_from` is where the
  `C` mark left the cursor, measured from the block's own first row, which is the only thing
  that knows how many rows a prompt drew. A block whose shell never sent `C` — an `ssh`, a
  container, a resize mid-command, a shell with no integration — has no answer, and that row is
  drawn disabled rather than dropped, because a menu that changes length between blocks is a
  menu whose rows move under the pointer.

  **Run again does not run anything.** It puts the command back in the composer, unsent, at the
  caret. A menu that ran a command would be a menu that runs the `rm` somebody opened it to
  read, and one that replaced the field would throw away a half-typed line.

  The menu hangs from the bottom-right corner of the dots, which is a corner only the *paint*
  of the last frame knows — the controls are drawn into the scene rather than built as elements
  — so the list records it and the popup is anchored against it. The block whose menu is up
  keeps its controls painted for the same reason: a modal underlay takes the pointer off the
  list, and a menu hanging off a button that has just disappeared is a menu attached to nothing.

A hover is re-derived from the pointer on every frame, not only when the pointer moves: a
wheel and a command finishing both put a different block under a still pointer, and a control
drawn on the block that *was* there is an affordance pointing at output a click would not
copy.
* **Thumb** — 4 px wide, 2 px inset, 24 px minimum, in the geometry `Scrollable` paints its
  own with, so the two scrollbars in the application match.

## 8. The composer

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

## 9. Resizing

`body::PaneSizer` is the only thing that resizes a pty, and it measures the **pane's**
rectangle, not the output's. The output's box changes whenever the composer appears, hides or
grows a line, and a `SIGWINCH` on each of those frames is a storm at programs that handle them
badly. Reporting the pane also means a full-screen program that fills the pane fits it.

The consequence is that the surface elements are laid out *after* the resize, so a snapshot
measured for the grid the last frame had is refreshed rather than painted into the new box.

---

## What this does not do

Stage 1 of the port. These are absent on purpose, not overlooked:

* **Block selection.** No click-to-select, no shift-click, no accent wash, no per-block border.
* **Keyboard block navigation.** No Cmd-Up / Cmd-Down. The two scrolls are in the block's menu
  and are reachable with the pointer only: there is no block *selection*, so there is no block
  a chord could be about.
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
* Share, bookmarks, block filters, find-within-block, and everything else that needs a block to
  be addressable rather than merely visible. Running a command a second time is in the menu —
  as text put back in the composer, which needs nothing of the sort.
