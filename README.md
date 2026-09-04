# Crook

A terminal whose unit of work is an agent, not a tab.

Every terminal ever written treats a shell session as the thing you open, arrange and close.
Crook treats an *agent* as that thing. A tab is one agent's workspace: its transcript, its
working directory, its state, its budget. The tab strip is therefore a list of what is
currently being worked on, and the header tells you what that work is costing.

Crook copies the architecture of [Warp](https://www.warp.dev) — an Entity/Handle application
core, immutable `View::render`, constraint-based layout, a `Scene` display list handed to a
GPU renderer — while deliberately taking one backend instead of two, and no build scripts at
all. See [`docs/architecture.md`](docs/architecture.md) for what was inherited, what was
dropped, and why.

## v1 scope

Seven features, and the page that configures them:

- **Tabs.** Open, close, switch, reorder. One agent session per tab, with a derived title.
  They live in a panel down the left edge or in a strip across the header, and the gear menu
  says what a row of them shows.
- **A Claude Code usage chip** in the header, showing how much of the current session's token
  budget is spent and when it resets. It reads the session Claude Code already stores locally
  (`~/.claude/.credentials.json`, plus the macOS Keychain) and polls the usage endpoint.
- **A shell in every pane.** A real pseudo-terminal and a real xterm-compatible emulator:
  colour, bold and italic faces, underline and strikeout, the alternate screen, ten thousand
  lines of scrollback, `SIGWINCH` on resize, and titles and working directories the shell
  reports with OSC 0, 2 and 7 — which is what makes a tab rename itself and its git chips
  follow a `cd`. A tab splits into panes with `cmd-d` and `cmd-shift-d` (`ctrl-shift-d` and
  `ctrl-shift-e` off macOS), and a shell that exits closes its pane, its tab, and with the
  last tab the window. With shell integration the scrollback moves into the blocks: each
  finished command owns its rows, so ten thousand *commands* survive a `clear`, a resize, and
  the emulator's own history evicting anything.
- **Output as a list of commands.** A pane is not a grid with decorations drawn over it: each
  command is a block holding its prompt, the line that was run and everything it printed, with
  a hairline between one and the next, a red wash on one that failed and an accent stripe on
  one that is still running. Hovering a block reveals a control that copies exactly that
  command and its output — no neighbour's text, no trailing blank rows, no over-selection —
  which is the thing scrollback cannot do. The list virtualises: a frame costs a screenful,
  not a session. **This needs shell integration**, which Crook installs into zsh, bash and
  fish by itself; see below for what a shell without it looks like.
  [`docs/blocks.md`](docs/blocks.md) is the map of the whole surface.
- **Output you can select and copy, across every command in it.** Drag across a pane's output
  to select it, double click for a word, triple click for a line, alt-drag for a column; the
  drag keeps going when the pointer leaves the pane, a drag past the top or bottom edge scrolls
  the list under the pointer, and the selection stays on its own text while the shell prints
  more underneath and while the list scrolls over it. **A selection is a block, a row of that
  block and a column** rather than a cell of the grid, so it spans the commands that have
  finished as well as the one still running: a drag from one block into the next copies the
  ends partially and the blocks between them whole, joined with one newline and none of the
  padding between them. A line the terminal folded comes back as the one line it is, trailing
  blanks stay behind, and a double-width character copies as one character.
  `cmd-c` — `ctrl-c` or `ctrl-shift-c` off macOS — copies it and lets it go, so the next
  `ctrl-c` interrupts the shell the way it always has. The half-written command line in the
  field below is left exactly where it was: a copy is not an interrupt. Copying a *whole*
  finished block still needs no selection at all: that is what its hover control is for, and it
  takes exactly what dragging across that whole block takes.
  A pane that is drawing one grid rather than a list — the alternate screen, or a command that
  has printed past the top of the viewport — is selectable in the same way, over its scrollback
  as well as its screen. What no selection survives is the picture under it changing: resizing
  the pane re-wraps the rows, and crossing between the list and the grid renumbers them, so the
  selection is let go of rather than re-read against text nobody selected.
- **A command line that behaves like a text field.** Under each pane's output is the line
  being composed — not a box and not a raw terminal line: no border, no fill, no focus ring,
  on the pane's own ground, in the terminal's own font and colours, at the same column zero as
  the output above it. It is an editor: a caret you can click, selection by drag, double and
  triple click, word and line movement, undo, the system clipboard, a per-pane history on the
  up and down arrows, and multi-line commands with shift-enter. Enter sends the line to the
  shell, which echoes and runs it exactly as before. A full-screen program — vim, `top`,
  `less` — takes the whole keyboard back, and so does any command that has been running for
  longer than a blink: the field goes away and its space goes to the block. `ctrl-c`, `ctrl-z`
  and an end-of-input `ctrl-d` always reach the shell. That line is drawn in one function,
  `app/src/input_keys.rs`, and the architecture doc's §7 says why it is drawn there.
- **Shell integration, installed by itself.** A pane running zsh, bash or fish emits the four
  OSC 133 marks that say where a prompt starts, where a command starts and how it ended. There
  is nothing to install and nothing to configure: Crook writes a scratch `ZDOTDIR`, `--rcfile`
  or `vendor_conf.d` stub, chains onto whatever hooks are already there, never touches
  `~/.zshrc`, and removes the stub when the pane closes. Set `CROOK_NO_SHELL_INTEGRATION` to
  anything but `0` to turn it off. It reaches only shells Crook itself starts — not the far
  side of an `ssh`, not a container, and not a shell it has no snippet for (`pwsh`, `nu`,
  `ksh`, `tcsh`) — and on those machines the same text can be pasted at the end of the rc file
  by hand: it is `app/src/shell_integration/crook.zsh` and its two siblings.

  It also **answers**, which is what makes Tab work. Command marks are an announcement and
  completion is a question, so there is a second channel beside them: Crook writes the line
  into the pane's own scratch, sends a key the snippet bound, and the snippet writes the
  answer back and says so with an escape sequence carrying only the request's number. fish
  answers with `complete -C`, which is its real completion; bash with `compgen`; zsh with its
  own hashes and globs. One candidate is inserted whole, several insert as much as they agree
  on, and an ambiguous answer is listed under the line — which is what every shell does.
  **Without it Crook is a plain terminal**: one continuous stream of output drawn as a grid,
  scrolled through the emulator's own scrollback, with every key going straight to the shell.
  No blocks, no per-command copy, and no composer — everything else, including selection and
  copying, works exactly as it does with it.
- **Themes**, in a panel of their own. Thirteen built in — Crook Dark, Crook Light, Midnight,
  and ten of the palettes Omarchy dresses a desktop in (Catppuccin, Everforest, Gruvbox,
  Kanagawa, Nord, Rosé Pine, Tokyo Night and three more, each its own project's, read from
  Omarchy's files) — plus any number of your own from `<config>/crook/themes/*.yaml`, in
  **Warp's own theme file format**, so a theme written for Warp works here unchanged.

  A theme names a background, a foreground, an accent and the sixteen ANSI colours; everything
  else — surfaces, borders, overlays, muted text — is derived from those, which is why a theme
  file is twenty lines rather than sixty. Choosing one applies it to the chrome, to the grid
  and to every shell already running.

  The **Themes panel** is a second side panel, opened from the settings page's current-theme
  row: preview cards, arrow keys that browse by applying, and a `+` that **makes a theme** out
  of the one you are looking at — five candidate colours clustered out of its palette, one
  click to choose the background, and everything else decided so the result is legible. What
  it writes is a file in your themes folder, in the same format as any other.
- **Git worktrees, one click away.** Click the tab you are already in and, if it is inside a
  repository, its menu lists that repository's checkouts: the one this tab is in, the ones
  other tabs are in, and the rest. Choosing one opens a tab there — or brings forward the tab
  already in it, because two agents editing one checkout is exactly what a worktree exists to
  prevent. `New worktree…` asks for a branch name, fills one in that nothing is using, shows
  where the checkout will go, and opens a tab in it. Removal is offered only for a checkout
  that is not locked, not the main one, and not one a tab is working in; it says what it will
  delete first, and it never deletes the branch. Checkouts go in a store of Crook's own —
  neither inside the repository, where git will happily let you put one and every build and
  every search then trips over it, nor beside it in a directory somebody else laid out.

- **A settings page**, which opens the way a shell does: `cmd/ctrl-,` — or the gear menu's
  last entry — puts it in a **tab of its own**, listed in the strip beside the work it
  configures, splittable next to that work, and closed by the same × and the same close chord
  (`cmd-w`, `ctrl-shift-w` off macOS) as any other pane. Four pages: Appearance, Usage, Keys
  and About. Every option on it is one the application actually reads; there is nothing there
  that does not do something. Changes apply on the click and are written to
  `<config>/crook/settings.json`, which is the same eight keys the gear menu writes plus the
  theme, the light and dark pair it follows the desktop between, the terminal's type size,
  whether the tabs come back, and — set in the file rather than on the page — its font family.
  The type size is also on `cmd/ctrl-plus`, `-minus` and `-0`, and every pane resizes with it:
  a pane's columns and rows are its box divided by a cell, so the ptys follow. Bindings a
  person writes down live beside it, in `keymap.json`.
  It is the one pane with no shell under it and no field: every control on it is a click.

  At the top of its rail is a **search box**, and it narrows both halves of the page at once:
  the rail keeps only the pages that hold an answer and says how many each of them holds, and
  the page keeps only the rows that are one. A row is found by its own name, by the line under
  it, by the value on its right — so `cmd-w` finds "Close the focused pane" and a path finds
  the settings file — by the page and category it is in, and by a hand-written list of the
  words somebody would actually type: nothing on the "Tab placement" row says *sidebar*.

Everything else is out of scope on purpose. There is no telemetry, and OSC 8 hyperlinks are
not read — though a URL a program *printed* is clickable, because the scan that finds one
works the same on a finished block as on the live grid, which an OSC 8 carried on the grid
alone would not. Blocks are stage one — no
block-level selection, no keyboard navigation between blocks, no sticky header,
no jump-to-bottom, and the shell's prompt stays on its own row rather than being lifted into
the composer; [`docs/blocks.md`](docs/blocks.md) lists those and says what each would touch.
The terminal grid still reaches no clipboard of its own: the input field copies and pastes, an
OSC 52 from the shell does not. The list of what is absent — and what adding each item would
touch — is the last section of the architecture doc.

## Prerequisites

Crook has **zero build scripts and links no system library**. Every dependency is a crates.io
crate that compiles with nothing but a Rust toolchain and a linker. The per-platform
prerequisites are therefore small and, on two of the three platforms, already present.

Everywhere: a Rust toolchain via [rustup](https://rustup.rs). The exact channel is pinned in
`rust-toolchain.toml`; the rustup proxies download it on first use, so you do not select it
yourself.

### macOS

**Xcode Command Line Tools.** That is the whole list.

```sh
xcode-select --install
```

Crook does not need full Xcode, does not need `xcodebuild -runFirstLaunch`, and does not need
the separately-downloaded Metal Toolchain. It reaches Metal through `wgpu`, which ships
precompiled shader translation, so nothing in the build ever invokes `xcrun metal`.

### Linux

Build time: a C toolchain (rustc shells out to `cc` to link) and `pkg-config`.

```sh
sudo apt-get install -y build-essential pkg-config
```

Run time: the X11, Wayland, EGL and mesa shared objects that `winit` and `wgpu` `dlopen` when
the window is created. These are *not* build dependencies — the workspace compiles without
them and then fails to open a window — which is why they are listed separately and why none
of them is a `-dev` package.

```sh
sudo apt-get install -y \
  libx11-6 libxcb1 libxi6 libxcursor1 libxkbcommon-x11-0 \
  libwayland-client0 libwayland-egl1 \
  libegl1 libgl1 libgl1-mesa-dri mesa-vulkan-drivers \
  fontconfig
```

`fontconfig` is here for its *configuration files*, not its library: Crook's font stack parses
them in pure Rust to learn which directories hold fonts. Nothing links `libfontconfig`. You do
need at least one installed font family — on a minimal container image, add `fonts-dejavu-core`.

On distributions that are not Debian-derived, install the equivalent packages;
`./script/bootstrap` prints the list.

### Windows

**Visual Studio Build Tools** with the MSVC toolchain and the Windows SDK. The `-msvc` target
cannot link without them.

```powershell
winget install --id Microsoft.VisualStudio.2022.BuildTools --override `
  "--quiet --wait --add Microsoft.VisualStudio.Workload.VCTools --add Microsoft.VisualStudio.Component.VC.Tools.x86.x64 --add Microsoft.VisualStudio.Component.Windows11SDK.22621"
```

No CMake, no protoc, no LLVM/`libclang`, no Inno Setup. Crook takes no dependency that runs
`bindgen`, so nothing hunts for `libclang.dll`.

## Build and run

```sh
./script/bootstrap    # install or explain this platform's prerequisites
./script/run          # cargo run -p crook --bin crook-dev
```

Both scripts are POSIX `sh` and work identically on macOS, Linux, and Windows under Git Bash.

If you would rather drive Cargo directly — note that rustup was installed here with
`--no-modify-path`, so the scripts export it and a bare shell may not have it:

```sh
export PATH="$HOME/.cargo/bin:$PATH"
cargo run -p crook --bin crook-dev
```

To produce a release binary:

```sh
./script/bundle                # dist/<platform>/crook  (crook.exe on Windows)
./script/bundle --check-only   # type-check with release flags, build nothing
```

`--check-only` is what CI runs on all three platforms. It compiles the workspace with the
exact profile and feature set a shipped build uses, which catches the class of bug where a
release-only feature combination does not compile — cheaply, and without producing artifacts.

## Repository layout

```
app/                     the `crook` library, plus two ~20-line channel binaries
crates/crookui_core/     entities, handles, contexts, elements, layout, Scene   (MIT)
crates/crookui/          winit windowing, wgpu renderer, cosmic-text font stack (MIT)
crates/crook_usage/      Claude Code credentials and usage polling              (MIT)
crates/crook_terminal/   pty, emulator, and the snapshot the renderer draws     (MIT)
docs/architecture.md     the design, and the reasoning behind each divergence
docs/blocks.md           the block surface: what draws it, and what it does not do yet
script/                  bootstrap, run, bundle
```

## Checks

Whatever you touch, these four commands are what CI runs, in this order, on all three
platforms:

```sh
cargo fmt --all --check
cargo clippy --locked --workspace --all-targets -- -D warnings
cargo check --locked --workspace --all-targets
cargo test --locked --workspace
```

See [`CONTRIBUTING.md`](CONTRIBUTING.md) for the two style rules that are not enforced by a
linter.

## Licensing

Crook is MIT throughout. The full text is in [`LICENSE-MIT`](LICENSE-MIT).

Warp is dual-licensed: its `warpui` and `warpui_core` crates are MIT, and everything else in
that repository is AGPL-3.0. Crook stays clear of the AGPL half:

- `crookui` and `crookui_core` port real code and shaders from Warp's two MIT crates. MIT
  permits that and asks one thing in return — that the copyright notice travel with the code.
  `LICENSE-MIT` therefore carries Denver Technologies' notice alongside this project's.
- `crook_usage` descends from a Claude Code usage indicator written for a personal fork of
  Warp and never contributed upstream. It is its author's own work, licensed here by that
  author, and it borrows nothing from Warp beyond the shape of the surrounding app.
- `app/` was written against a description of how Warp's tab strip and header behave, not by
  copying either. Where its comments mention Warp they are recording a divergence — an
  index-versus-identity bug not inherited, a public field not repeated.
- `crook_terminal` is Crook's own code over two crates.io dependencies: `portable-pty`
  (MIT) for the process side and **`alacritty_terminal` (Apache-2.0)** for the grid, the
  scrollback and the escape-sequence parser. Nothing in it comes from Warp, whose terminal
  lives in the AGPL half of that repository.

- The icons are **[Lucide](https://lucide.dev)**, whose licence is ISC — permissive, and
  compatible with MIT redistribution. Their geometry is vendored into
  `crates/crookui_core/src/icons/data.rs` by `script/icons`, which pins a version of the
  `lucide-static` package and rewrites its SVGs as path commands; the licence that came with
  it is [`LICENSE-ISC-lucide`](LICENSE-ISC-lucide), and it names Feather (MIT) for the icons
  Lucide inherited from it. Nothing else about the icons is anyone else's: the rasterizer
  that turns those commands into pixels is Crook's own.

[Alacritty](https://github.com/alacritty/alacritty) is a fast, cross-platform terminal
emulator by Joe Wilm and the Alacritty contributors, released under the Apache License 2.0.
Crook uses its `alacritty_terminal` crate — the emulator without the window — and would be a
great deal poorer without it. Apache-2.0 is permissive and compatible with MIT
redistribution; it asks that the licence and any `NOTICE` travel with the code, which
`Cargo.lock` and the dependency's own vendored licence do, and it grants a patent licence
MIT does not. A binary built from this repository therefore contains Apache-2.0 code, and
that fact belongs in whatever notice a distribution ships.

None of this is legal advice; it is a record of where each file came from, so that someone
who needs to answer the question properly has the facts to work from.
