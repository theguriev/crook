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

Four features, and the page that configures them:

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
  last tab the window.
- **Output you can select and copy.** Drag across a pane's output to select it, double click
  for a word, triple click for a line, alt-drag for a column; the drag keeps going when the
  pointer leaves the pane, a drag past the top or bottom edge scrolls the screen under the
  pointer, and the selection stays on its own text while the shell prints more underneath.
  `cmd-c` — `ctrl-c` or `ctrl-shift-c` off macOS — copies it and lets it go, so the next
  `ctrl-c` interrupts the shell the way it always has. The half-written command line in the
  field below is left exactly where it was: a copy is not an interrupt.
- **A command line that behaves like a text field.** Under each pane's output is an input
  box, not a raw terminal line: a caret you can click, selection by drag, double and triple
  click, word and line movement, undo, the system clipboard, a per-pane history on the up and
  down arrows, and multi-line commands with shift-enter. Enter sends the line to the shell,
  which echoes and runs it exactly as before. A full-screen program — vim, `top`, `less` —
  takes the whole keyboard back and the field goes away while it runs; `ctrl-c`, `ctrl-z` and
  an end-of-input `ctrl-d` always reach the shell. That line is drawn in one function,
  `app/src/input_keys.rs`, and the architecture doc's §7 says why it is drawn there.
- **Themes.** Three built in — Crook Dark, Crook Light and Midnight — and any number of your
  own, read from `<config>/crook/themes/*.yaml` in **Warp's own theme file format**, so a
  theme written for Warp works here unchanged. A theme names a background, a foreground, an
  accent and the sixteen ANSI colours; everything else — surfaces, borders, overlays, muted
  text — is derived from those, which is why a theme file is twenty lines rather than sixty.
  Choosing one applies it to the chrome, to the grid and to every shell already running.
- **A settings page**, which opens the way a shell does: `cmd/ctrl-,` — or the gear menu's
  last entry — puts it in a **tab of its own**, listed in the strip beside the work it
  configures, splittable next to that work, and closed by the same × and the same close chord
  (`cmd-w`, `ctrl-shift-w` off macOS) as any other pane. Four pages: Appearance, Usage, Keys
  and About. Every option on it is one the application actually reads; there is nothing there
  that does not do something. Changes apply on the click and are written to
  `<config>/crook/settings.json`, which is the same eight keys the gear menu writes plus one.
  It is the one pane with no shell under it and no field: every control on it is a click.

Everything else is out of scope on purpose. There is no keymap system, no persistence, no
telemetry, no shell integration — the field composes a line without knowing where the shell's
prompt is, so there is no completion and no command blocks — and no mouse reporting or IME
composition. The terminal grid still reaches no clipboard of its own: the input field copies
and pastes, an OSC 52 from the shell does not. The list of what is absent — and what adding
each item would touch — is the last section of the architecture doc.

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
