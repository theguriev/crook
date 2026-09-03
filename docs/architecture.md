# Crook architecture

Crook is a terminal whose unit of work is an agent. It borrows its architecture, almost
wholesale, from [Warp](https://www.warp.dev): an Entity/Handle application core, views that
rebuild an immutable element tree on every invalidation, Flutter-style constraint layout, and
a `Scene` display list that a GPU renderer consumes. That is a well-tested shape for a
GPU-rendered desktop app in Rust, and Warp has 1.1M lines of evidence that it scales.

The parts Crook does *not* borrow matter as much as the parts it does. Warp's architecture
comes with a build system that carries two complete UI backends, an Objective-C compiler
invocation, a Metal shader compiler, `bindgen`, and 2,245 lines of packaging scripts. Almost
none of that is intrinsic to the architecture; it is the accumulated cost of specific product
decisions Crook has not made. This document explains the inheritance and, section by section,
each deliberate divergence.

It assumes a competent Rust engineer who has never read a line of Warp.

---

## 1. The Entity/Handle core

### The problem it solves

A UI is a graph. A tab strip holds tabs; a tab holds an agent session; a header holds a usage
chip that must repaint when a background poller returns. Expressed directly in Rust — parent
and child holding references to each other — this is either impossible or it is
`Rc<RefCell<T>>` on every node, which gives you reference cycles, runtime borrow panics from
callbacks that reenter, and a lifetime on every function that touches two nodes at once.

The Entity/Handle core sidesteps the whole problem by never letting anything hold a reference
to anything else.

### The shape

There is one owner: `App`, which is an `Rc<RefCell<AppContext>>`. `AppContext` holds every
*entity* in the process — models and views alike — in flat maps keyed by `EntityId`. Views
additionally belong to a `Window`, which owns a view registry, a root view, and a focused view.

Nothing else owns an entity. What other code holds is a **handle**:

```rust
pub struct ModelHandle<T> { id: EntityId, /* + a ref-count token */ }
pub struct ViewHandle<T>  { window_id: WindowId, id: EntityId, /* + token */ }
```

A handle is a typed name, not a pointer. It is cheap to clone, it can be stored in any struct
without a lifetime, and it can be sent through a closure into a background task. It does not
borrow anything, so holding a `ViewHandle<TabStrip>` inside `Header` creates no aliasing
question at all.

To actually *touch* the entity you ask a **context** for temporary access:

```rust
let title = tab_strip.read(ctx).active_title();

tab_strip.update(ctx, |strip, ctx| {
    strip.close(tab_id);
    ctx.notify();
});
```

`update` is a checkout, not a borrow: the entity is moved out of `AppContext`'s storage, the
closure gets `&mut T` plus a context that can still reach the rest of the app, and the entity
is put back when the closure returns. That is why mutation is expressed as a closure rather
than as a returned `&mut` — during the closure, the app is fully usable *except* for the one
entity you are already holding, and the type system makes it impossible to hold two.

Contexts come in three sizes, and they are the same object at different levels of
specificity:

| Context | Is | Extra powers |
| --- | --- | --- |
| `AppContext` | the whole world | read anything, spawn tasks |
| `ModelContext<'a, T>` | `&mut AppContext` + your `EntityId` | `notify`, `emit`, `subscribe`, `observe` |
| `ViewContext<'a, T>` | the above + a `WindowId` | `add_view`, `focus`, dispatch actions |

The narrower two `Deref` to `AppContext`, so a view method that only needs to read a model
takes `&AppContext` and every caller already has one.

### Ref-counting and drops

Ref-counts live *outside* entity storage, in a `RefCounts { entity_counts, dropped }`
structure that handles mutate on clone and drop. When the last handle to an entity dies, the
entity is **not** removed immediately — its id is pushed onto a dropped-set, and it is removed
later, when the effect queue is flushed.

That split is not an optimization; it is the thing that makes callbacks safe. Dropping the
last `ModelHandle<UsageModel>` from inside an event callback must make every surviving
`WeakModelHandle::upgrade()` return `None` *right then*, even though the entity is still
physically in the map (you may be standing on its stack frame). Consulting the dropped-set
rather than the map gives you exactly that.

### Effects

`notify`, `emit`, and `focus` do not call observers synchronously. They append to an effect
queue, which is drained after the current update completes. So an observer never runs while
the entity it observes is checked out, and a chain of notifications is a loop rather than
recursion. Reentrancy is a scheduling question, not a borrow-checker question.

Models and views share **one** `EntityId` namespace, deliberately: subscriptions and
observations are keyed by `EntityId` regardless of what kind of entity is on either end, so a
view can observe a model, a model can observe a model, and none of the plumbing needs to know
which.

### Actions

Typed actions are the whole keyboard/menu dispatch system, and they cost almost nothing:

```rust
pub trait Action: Any + Debug + Send + Sync {}
impl<T: Any + Debug + Send + Sync> Action for T {}
```

A blanket impl over every suitable type, plus an `ActionType(TypeId)` to key handler maps.
Dispatch walks the view-ancestor chain from the focused view outward, offering each ancestor
the action until one handles it — the responder chain, in about forty lines. `CloseTab` is a
struct; the tab strip registers a handler for it; the key binding and the close button emit
the same value.

### What Crook keeps and what it trims

Crook ports this near-verbatim, because it is small (roughly 800–900 lines for the whole core)
and because every subsequent decision leans on it. Two things are trimmed:

- **Autotracking** — Warp's `Tracked<T>` wrapper, which turns a field read during render into
  a dependency edge and a field write into an implicit `notify()`. It is elegant and it is a
  v2 concern. Explicit `ctx.notify()` is fine at Crook's size and is easier to reason about
  while the render loop is still being learned.
- **Renderer-agnosticism in the view registry.** Warp's `StoredView` is an enum with `Gui` and
  `Tui` arms sharing one registry, one ref-count and one focus path. Crook's has one arm. The
  seam is preserved (see §8) but the second arm is not written until there is a second
  front-end.

---

## 2. The render loop

### Immutable render

```rust
fn render(&self, ctx: &AppContext) -> Box<dyn Element>;
```

Note `&self`. A view cannot mutate itself while rendering; rendering is a pure projection of
state into a fresh element tree. When something invalidates the view, that tree is thrown away
and built again from scratch.

There is no diffing, no reconciliation, and no keys. This surprises people who arrive from
React, so it is worth being explicit about why it is affordable: there *is* no previous tree
to compare against, because the tree was never retained. Invalidation is per-view — an entity
calls `notify()`, that view's id lands in the window's invalidation set, and the presenter
re-renders that view's subtree only. A tab title change rebuilds one tab, not the window. The
expensive part of a VDOM is the diff, and skipping the retention skips the diff.

The consequence you must internalize: **element trees are frames, not state.** Anything that
has to survive a rebuild lives in the view. Hover and click state is the standard case —
`MouseStateHandle` is an `Arc<Mutex<MouseState>>` owned by the view and handed *into* the
element that reads it, so the element is still disposable while the state is not.

### Constraints down, sizes up

Layout is Flutter's, and it is a single pass:

```rust
fn layout(&mut self, constraint: SizeConstraint, ctx: &mut LayoutContext, app: &AppContext) -> Vec2;
fn paint(&mut self, origin: Vec2, ctx: &mut PaintContext, app: &AppContext);
```

A parent hands each child a `SizeConstraint { min, max }`. The child returns a size that
satisfies it and caches that size in itself. Then the parent, knowing all the sizes, decides
where each child goes and calls `paint` with an **absolute** window-space origin. Children
never know their own position until paint; parents never know a child's size until layout
returns. Both directions are one-way, so layout is O(n) with no fixpoint iteration.

Flex works exactly as Flutter's does — one pass sizing the inflexible children against an
unbounded main axis, then dividing the remainder by flex ratio — and it uses Flutter's escape
hatch for the information that has to travel *sideways*:

```rust
fn parent_data(&self) -> Option<&dyn Any> { None }
```

`Expanded` returns a `FlexParentData { flex, fit }`; `Flex` downcasts it. This is why
`Expanded` only works as a *direct* child of a `Flex` — a `Container` in between swallows the
parent data. That is the single most common layout bug in this style of code.

### Scene

`paint` does not draw. It appends records to a `Scene`:

```rust
struct Scene { layers: Vec<Layer> }
struct Layer { clip_bounds: Option<Rect>, rects: Vec<Rect>, glyphs: Vec<Glyph> }
```

The `Scene` is the entire GPU-facing interface. The renderer sees nothing else — no elements,
no views, no entities. It walks the layers in order and, per layer, sets a scissor rect from
`clip_bounds` and issues one instanced draw for the rects and one for the glyphs.

Two properties follow, and both are load-bearing:

- **Within a layer, rects paint before glyphs, always.** Text is always above boxes in the
  same layer. Interleaving them means starting a new layer. Accept this; it is what keeps the
  renderer to two draw calls per layer.
- **The Scene is trivially testable.** `build_scene` needs no GPU. Element and layout tests
  construct a presenter, invalidate, build a scene against a fixed size, and assert on layer
  and primitive counts. The entire UI below the renderer is unit-testable headlessly, which is
  the practical reason the `crookui_core` / `crookui` split exists at all.

The frame, end to end: an entity notifies → the window's invalidation set gains a view id →
the presenter re-renders those views into fresh element trees → layout → paint into a `Scene`
→ the renderer uploads one instance buffer and issues per-layer draws.

---

## 3. One UI backend

**This is the decision that shapes the whole repository.**

Warp maintains two complete UI backends. On macOS it has a hand-written AppKit windowing layer
and a hand-written Metal renderer, with the shaders in `.metal` and the platform glue in
Objective-C. Everywhere else it uses `winit` and `wgpu` with WGSL shaders. The two are
reconciled by `cfg_aliases!` in `crates/warpui/build.rs`, which names the policy rather than
the platform:

```rust
cfg_aliases! {
    macos: { target_os = "macos" },
    winit: { not(macos) },
    wgpu:  { any(winit, feature = "experimental-wgpu-renderer") },
}
```

Crook uses **winit + wgpu on all three platforms**, including macOS. `wgpu` has a mature Metal
backend; Warp itself carries a feature flag to run the wgpu renderer on macOS, so the path is
not speculative.

### What that deletes

Precisely, and this list is the justification:

- **No Objective-C.** No `cc::Build` over ten `.m` files, no `cargo:rustc-link-lib=framework=`
  directives, no `clang_rt.osx` link workaround.
- **No `bindgen`.** Warp runs it over a Metal shader-types header. `bindgen` sits in
  `[build-dependencies]` unconditionally there, so Linux and Windows compile it for nothing —
  and, worse, any `bindgen` in the tree makes `libclang` a prerequisite on *all three*
  platforms. That is why Warp's Windows bootstrap installs LLVM and hunts for `libclang.dll`.
  Crook installs neither.
- **No `xcrun metal` / `xcrun metallib`.** Shader compilation does not shell out to a
  toolchain that has to be present, matched to an SDK, and told `-mmacosx-version-min`
  explicitly or it emits AIR that older drivers reject at pipeline-state creation.
- **No `xcodebuild -downloadComponent MetalToolchain`.** On Xcode 16+ the Metal compiler is a
  multi-gigabyte download that is *not* present after a fresh Xcode install. Crook's macOS
  prerequisite is Xcode Command Line Tools, full stop.
- **No `MACOSX_DEPLOYMENT_TARGET` plumbing.** Warp's build scripts `expect()` it, so it is set
  in `.cargo/config.toml` and re-exported by the bundle script, with a comment in each telling
  you to keep them in sync.
- **No `build.rs` anywhere in the workspace.** Not one, in any crate.

That last point is worth stating as an invariant rather than a fact, because it is the one
most likely to erode. **Crook's workspace has zero build scripts, and adding one should be
treated as an architectural change.** Build scripts are where cross-platform projects go to
die: they cannot see `CARGO_TARGET_DIR` (so they reconstruct target paths by hand and break
under `--target`), `env!("PROFILE")` collapses every custom profile to `"debug"` or
`"release"`, and any `cargo:rustc-cfg` they emit without a matching `cargo::rustc-check-cfg`
produces `unexpected_cfgs` warnings on modern rustc. Warp hits all three and works around all
three. Not having a build script is strictly cheaper than working around them.

The practical payoff: a fresh checkout builds with rustup and a linker. Nothing downloads,
nothing is generated, and CI needs no per-OS toolchain provisioning beyond the packages in the
README.

### What that costs

Honestly: the macOS build is less native than Warp's. There is no AppKit menu bar, no Dock
tile plugin, no `NSVisualEffectView` vibrancy behind the header, and the window chrome is
whatever `winit` gives you. Text rasterization is Crook's own rather than CoreText's, so glyph
rendering will not be pixel-identical to other macOS apps. For a v1 whose entire surface is a
tab strip and a chip, none of that is worth a second backend — but if Crook ever wants a real
macOS menu bar, that is the moment to revisit this, and the `platform::current` facade
(below) is where the second arm would go.

### Where the `cfg`s live

Even with one backend, platform differences exist: the window-control inset is 64px on the
left on macOS and 136px on the right on Windows; the Claude Code credential lookup consults
the Keychain on macOS and a plain file elsewhere. Warp's discipline is to put every one of
these behind a single facade:

```rust
pub mod current {
    cfg_if::cfg_if! {
        if #[cfg(target_os = "macos")]        { pub use super::mac::*; }
        else if #[cfg(target_os = "windows")] { pub use super::windows::*; }
        else                                  { pub use super::linux::*; }
    }
}
```

Call sites write `platform::current::Foo` and contain no `cfg` of their own. Crook keeps this
even where `current` has a single arm, because it is twelve lines and it is where every future
`#[cfg]` goes instead of into view code.

One portability rule is enforced by lint rather than by review. `.clippy.toml` bans
`std::process::Command`, because on Windows it flashes a console window unless the spawner
sets `CREATE_NO_WINDOW` — invisible on macOS and Linux CI, and a shipping-blocker for a
terminal, which spawns processes constantly. Everything routes through a wrapper that sets the
flag. The lint is in place before the first spawn exists, which is the only time it is cheap.

---

## 4. Text

Text is where a terminal earns or loses its reputation, and it is the subsystem where Crook
most deliberately does *not* take the shortcut.

### The stack

`cosmic-text` for shaping and font enumeration, `swash` for rasterization, behind **Crook's
own glyph atlas and WGSL glyph pipeline**. Concretely: `cosmic_text::FontSystem::new()`
discovers system fonts (pure Rust — it scans `/System/Library/Fonts` and friends on macOS,
parses fontconfig's *config files* and scans the directories they name on Linux, and reads
`%WINDIR%\Fonts` on Windows), `SwashCache` rasterizes a glyph to a mask or a color bitmap,
Crook packs that into a shelf-allocated atlas texture, and the WGSL pipeline draws one
instanced quad per glyph.

This one constructor call replaces all three of Warp's platform font backends — CoreText,
fontconfig via a forked `font-kit`, and DirectWrite via a forked `dwrote` — which are about
2,500 lines between them. It also deletes the `libfontconfig` dependency entirely: Warp links
it (via `dlopen`, and then has to `patchelf --add-needed` it back in for packaging, because
`dlopen`'d libraries are invisible to packagers). Crook links nothing.

### Why not glyphon

[`glyphon`](https://crates.io/crates/glyphon) is the obvious suggestion. It is
cosmic-text + swash + an `etagere` atlas + a wgpu text renderer, packaged. It would replace
roughly 600 lines of Crook's plan with about forty lines of setup, and it is a one-hour
integration. The recon explicitly recommends it for the GPU/window subsystem.

Crook does not use it, for one structural reason.

glyphon's unit is a retained `glyphon::Buffer` per text area, handed to `prepare()` each frame
as a `TextArea { buffer, left, top, bounds, default_color }`. Note `default_color`: color is a
property of the *area*, with per-run overrides inside the buffer. Crook's `Scene`, following
Warp's, stores a flat `Vec<Glyph>` of individually positioned, **individually colored** glyph
records.

For a tab title, either model is fine. For a terminal grid, they are not remotely equivalent.
A full-screen grid is 10,000+ glyphs a frame, each with its own foreground color, changing
every frame. Warp's model does a hash lookup per glyph and one instanced draw call. Expressing
the same thing in glyphon means one `Buffer` per color run per line, re-shaped whenever a cell
changes — and switching later would mean rewriting the renderer *and* every call site that
produces text.

That grid now exists, and it takes exactly the path this section was written to keep open.
`app/src/workspace/terminal_element.rs` does not shape at all: it asks `glyph_for_char` per
cell — memoized by `(face, character)` — multiplies the column index by a fixed advance, and
pushes one individually coloured glyph record into the `Scene`. A 200x50 screen of dense text
is 10,000 glyph records, 94 distinct atlas entries and **one** instanced draw call, and a
screen that is merely idle is two rectangles and no glyphs at all. Expressing per-cell colour
through a `TextArea`'s `default_color` would have meant one re-shaped buffer per colour run
per line.

Three smaller costs, for completeness: glyphon pins a `wgpu` major version and historically
lags by one or two, so taking it constrains `wgpu` and transitively `winit`; glyphon does
straight alpha blending, where Warp's glyph shader applies a brightness-scaled contrast curve
that is the difference between text that looks right on a dark background and text that looks
anemic; and glyphon clips with a hard rectangle, where Warp fades overflowing text with a
per-instance vertex attribute.

**The trade, settled.** For a project that genuinely stopped at tabs and a chip, glyphon was
the better choice, and this section said so: "if that grid never gets built, this section will
read as over-engineering, and that reading will be correct." The grid got built, and the
monospace fast path it needed is the one thing glyphon's model cannot express. The 600 lines
were the easiest 600 in the text plan and they are the reason the next 400 were possible.

One thing the original plan did get wrong, and it is worth recording: it assumed a grid "is
Latin by construction", so the single-glyph measurement path shipped with no font fallback.
It is not Latin. A spinner is braille, a prompt is powerline, `ls` prints whatever the
filenames are, and a build script prints emoji — and every one of those was a silently blank
column. `CosmicGlyphs::fallback_glyph` now asks cosmic-text's own fallback about one
character at a time and gets back the face that covers it, which is the same answer shaping
gives a tab title.

### One trap worth knowing now

`cosmic-text`'s `ShapeLine::new` panics on multi-paragraph text. A tab title derived from
agent output containing a newline will hit it. Warp sanitizes at the layout boundary by
joining paragraphs with a zero-width space (`\u{200B}`), which keeps byte offsets in the style
runs valid. Do the same, in one place, rather than at every call site.

---

## 5. The channel-binary layout

`app/` is a **library** named `crook`. Every shipped executable is a twenty-line shim:

```toml
[package]
name = "crook"
default-run = "crook-dev"
autobins = false        # or Cargo auto-discovers src/bin/* and the [[bin]] table is a lie

[lib]
name = "crook"
path = "src/lib.rs"

[[bin]]
name = "crook-dev"
path = "src/bin/dev.rs"

[[bin]]
name = "crook"
path = "src/bin/stable.rs"
```

Each `src/bin/*.rs` constructs a `ChannelState` — app id, log filename, feature-flag overlay —
installs it globally, and calls `crook::run()`. Nothing else. All the code, and all the tests,
are in the library.

### Why from commit one

Because retrofitting it is genuinely painful and adopting it early is free.

Once a `main.rs` exists, every module in it is a *binary* module. Moving to a library later
means every path changes, every `use crate::` in every file changes, and every integration
test that wanted to call into the app has to be rewritten (a binary crate cannot be depended
on). Warp's `[[bin]]` table carries a comment explaining exactly this.

What it buys, immediately:

- **Per-channel identity without a runtime flag.** A dev build and a stable build have
  different app ids, different log filenames, and different feature defaults, so they install
  side by side and cannot fight over each other's state. No `--channel` argument to forget.
- **A place for the Windows subsystem attribute.**
  `#![cfg_attr(feature = "release_bundle", windows_subsystem = "windows")]` must be a
  crate-level inner attribute in the *binary*; it cannot live in the library. Development
  builds keep a console for `stdout`; only bundled builds detach. Gating it on the feature
  rather than on `debug_assertions` means the release-flags CI job actually exercises it.
- **Testability.** `cargo test --workspace` covers everything, because everything is in a lib.

Warp has six channels (stable, preview, dev, local, oss, integration) for reasons involving a
private configuration repository, staged rollout, and telemetry projects. Crook has two, and
adding a third is adding a twenty-line file.

---

## 6. Packaging

`script/bundle` dispatches on `uname -s`, supports `--check-only`, and produces a plain
release binary per platform. That is all it does, and it is on purpose.

Warp's `script/{macos,linux,windows}/bundle*` is 2,245 lines: `.app` bundling with a
conditional `install_name_tool -add_rpath`, universal binaries via `lipo` of two full builds,
codesigning and notarization, AppImage via `linuxdeploy` with `APPIMAGE_EXTRACT_AND_RUN=1`
because CI runners lack FUSE, `.deb`, `.rpm`, an Arch `PKGBUILD` built in a container, and Inno
Setup driven by `ISCC` with an app-mutex string that must byte-match what the Rust
single-instance code creates. Reimplementing any of that is a mistake.

The plan is [`cargo-dist`](https://opensource.axo.dev/cargo-dist/), which generates the release
matrix and produces `.tar.gz` / `.zip` / `.msi` / `.dmg` / `.deb` / `.rpm` from a manifest. If
a real macOS `.app` becomes necessary — it will, for the Dock icon and URL schemes —
`cargo-bundle` handles that one artifact while `cargo-dist` keeps the rest. There is a `TODO`
in `script/bundle` at the exact spot.

What `--check-only` does today is the part that earns its keep immediately: it type-checks the
workspace with the release profile and the release feature set. CI runs it on all three
platforms. Without it, "the shipped feature combination does not compile" is discovered at
release time.

---

## 7. The terminal

A pane runs a shell. `crates/crook_terminal/` is the whole of it, and it is deliberately the
one crate in the workspace that owns no thread, no executor and no timer.

### Three pieces and one contract

`Pty` opens a pseudo-terminal and starts a program on it, through `portable-pty` — which
already knows `openpty` and a controlling tty on Unix and ConPTY on Windows, so there is not
one `#[cfg]` in this crate that spawns a process. **Warp vendors a ConPTY; Crook accepts the
system one**, which is the same trade §3 makes everywhere else.

`Emulator` wraps `alacritty_terminal`'s `Term`, which owns the grid, the scrollback, the
alternate screen and the mode flags, and its `vte` parser. That crate is Apache-2.0 and is
credited as such in the README. Writing a VT parser is not the interesting part of a terminal
and getting one subtly wrong is a decade of bug reports.

`Snapshot` is the contract with the renderer, and it is the piece worth arguing about. It is
a flat, owned, row-major `Vec<SnapshotCell>` — a character and two **already-resolved** RGB
colours per cell, twelve bytes — plus a cursor, the palette defaults, the scrollback offsets
and the title. Two properties carry the design:

- **Nothing left to look up.** Indexed colours, named colours, OSC 4/10/11/12 overrides, dim,
  inverse and hidden are all applied while the snapshot is built. The renderer reads
  `cell.foreground` and draws it. It never needs a theme and it cannot disagree with the
  emulator about what a colour means.
- **A revision that only moves on a real change.** `snapshot()` builds a candidate and
  compares it with the one it has; if they would be drawn identically the same `Arc` comes
  back. `Arc::ptr_eq` is therefore a sound "skip this frame" test, and it is one of the three
  filters that keep an idle pane free.

Alacritty's own damage tracking is not used for that, because `Term::damage()`
unconditionally reports the cursor's cell as damaged and so can never say "nothing changed".

Zero-width characters — a combining accent, a virama, a variation selector, a ZWJ — do not
fit in a twelve-byte `Copy` cell, and dropping them turns `José` into `Jose` on a filesystem
that hands out decomposed names. They ride beside the grid in a small side table that is
empty for essentially every screen.

### Who drives it

`app/src/terminal_model.rs`, in the shape `usage_model` and `git_model` established: work off
the UI thread, delivered on it, `ctx.notify` only when something a viewer could see actually
changed. Each terminal gets an OS thread of its own rather than a background-pool worker,
because a pty read blocks for as long as the shell is quiet and the pool is sized for exactly
the two poll chains that park on timers.

**Reading and drawing are throttled separately, and conflating them costs three orders of
magnitude.** A pty master hands out about a kilobyte per `read` however large a buffer it is
given; a reader that paused for a frame after each one would move 64 KB a second, and the
*program* on the far end would run at that speed too, because it blocks on its own writes
once the kernel's buffer fills. So the reader never pauses — it reads and parses as fast as
the child can produce — and only publishing is rate-limited to one frame per pane per 16 ms.
A batch parsed inside that window is left parsed and undrawn, and one shared thread comes
back for it when the window is up. That thread is not a nicety: the batch that misses the
window is always the *last* one of a burst, and without it the final screenful of a `cat`
would sit invisible until the shell next said something.

The emulator's mutex is never held across a frame. The reader takes it to parse, and again to
build a snapshot, and publishes the `Arc` into a slot of its own; painting clones that `Arc`
and walks owned data. Layout — which asks "did the grid move?" on every single frame —
answers from an atomic before it ever asks for the lock.

### The command line is an input field

A pane's next command is composed in a bordered box under its grid — an ordinary GUI text
input, with a caret you can click, selection by drag, word and line movement, undo, the
system clipboard, and a history on the arrows — and only reaches the pty when Enter sends it,
as `line + "\n"`. The shell then echoes and runs it, so the grid above shows prompt, command
and output exactly as it did when every keystroke went straight through.

**Why not leave the shell to do the line editing?** Because it cannot do it as a GUI. The
line lives in the child's own reader, so there is nothing on this side to hit-test, select,
or paste into; a click can only be turned into an approximation of arrow keys, and a
selection into nothing at all. Warp made the same call, and it is the one place where a
terminal has to stop being a glass teletype.

The model is in `app/src/editor/`, and nothing in it draws, touches a clipboard or knows what
a keystroke is: positions are byte offsets on grapheme boundaries, movements arrive as an
already-resolved `Motion`, and clipboard text goes in and out as a `String`. That is what
makes the whole behaviour of the input — every movement from every position, undo grouping,
history, grapheme-aware deletion — testable in microseconds with no window, no GPU and no
fonts. `app/src/pane_input.rs` is the per-pane state the element tree is rebuilt around;
`app/src/workspace/input_element.rs` draws it on the grid's own cell metrics.

### Selecting the output

The grid above the field is selectable with the pointer, and it is the emulator's own model
that makes it so. `alacritty_terminal` already has a `Selection` — four kinds (a plain drag, a
block, a word, a line), a `side` so a selection ends *between* two characters, and
`Selection::rotate`, which `Term` calls on every path that scrolls the grid. That last one is
the whole reason the selection is stored in `Term::selection` rather than beside it: a
selection made around a word stays around that word while a build prints a hundred more lines
underneath it, and a copy kept anywhere else would be wrong the first time a `\n` arrived.
`crates/crook_terminal/src/selection.rs` is a vocabulary over it — `SelectionKind`,
`CellSide`, and two point types — and nothing above the crate ever names an alacritty type.

**The two point types are not fussiness.** A `ViewportPoint` is a cell of what is *on screen*
and a `GridPoint` is a cell of the *text*, with negative lines for scrollback; the display
offset is the distance between them and it moves whenever the shell prints while somebody is
scrolled back. A mouse produces the first, a selection is stored in the second, and confusing
them is exactly the bug where a selection dragged out four screens into the history highlights
the live output instead. They are separate types so the conversion — which happens inside the
emulator, under its own lock, at the moment the press lands — cannot be skipped.

**On the snapshot it is a span, not a flag on every cell.** A selection changes on pointer
moves that change nothing the shell printed. A per-cell `selected` bool lives inside
`Snapshot::cells`, and the only way to change something in `cells` is to *build* `cells` — a
walk of the grid with a palette resolution per cell, ten thousand of them, for every pixel the
pointer travels while a button is held. A span sits beside the cells instead, so the emulator
hands back the very cells it already built with two new points next to them. It is compared in
`same_content` like everything else, because a highlight *is* drawn content: a renderer that
skipped a frame on an unchanged revision would leave the old highlight under a pointer that
had moved on. So the revision stays honest and costs one comparison of two points.

The gesture — "is the button that went down on this grid still down?" — is the one piece the
emulator cannot hold, because it never hears about a button, and the element cannot hold it
either, because the tree is rebuilt between the press and the drag. It lives in
`app/src/pane_selection.rs`, per pane, beside the input field's state and for the same reason.

**And the gesture, once open, is not hit-tested.** A press is: a menu open over a pane must not
be clicked through. The drag and the release that follow are not, because they are the *same*
gesture, and where the pointer has got to since is not a new one. Dragging out of the pane is
how a selection is taken to the end of a line and past the end of the screen, and the button
can perfectly well come up over the tabs panel, which paints in a later layer — an occlusion
test on the release would drop it and leave the pane with a press it still thinks is down, so
that every unrelated drag afterwards, from any other press or from none, rewrote this pane's
selection. What keeps a neighbour's drag out is `PaneSelection`, not the layer: only the pane
the press landed on has a gesture to continue.

**Two elements route each keystroke, so nothing may change the answer between them.** A pane's
grid and the field under it are siblings in a `Flex`, which hands the same `KeyDown` to both —
the grid first. Both ask `TerminalHandle::has_selection` to route it, and both act on a
different half of the answer. So the grid does not release what it copies where it copies it:
it dispatches `WorkspaceAction::ReleaseSelection`, and actions are applied once the whole tree
has seen the event. A grid that released it in place would have the field read the same
`ctrl-c` as the interrupt it is with *nothing* selected, and throw away the half-written
command line; on macOS the field's own `cmd-c` would then overwrite the clipboard the grid had
just written. The invariant is worth stating plainly: **nothing may change what is selected in
the output while a keystroke is being dispatched.**

**Two places where the range the emulator hands back has to be checked.** `SelectionRange::new`
asserts `start <= end` and everything downstream relies on it, but `Selection::range_block`
builds one without going through the constructor: it moves the start a column right when the
drag began on the right of a cell and the end a column left when it ended on the left of one,
and never checks the two did not cross. An alt-drag whose ends share a column comes back
inverted — covering no cell, so nothing is highlighted, while `has_selection` still says there
is something to copy and off macOS spends the interrupt on it — and on the *last* column the
start lands on `columns`, one past the row, which `Term::line_to_string` then indexes the row
with and panics. `snapshot::selection_of` drops such a range, and `Emulator::selection_text`
asks it rather than the terminal, so the text and the highlight come from one decision.

**A double-width character is highlighted across both of its columns.** Its glyph is drawn
once, from the first, across the width of two, and copying already treats the pair as one
character. So `Snapshot::is_selected` lights both in either direction — reaching the trailing
half takes the character, and reaching the character takes the column its right half is drawn
in — or a drag that stopped in the middle of a CJK glyph would cut it down the middle while
`cmd-c` took the whole of it.

### Where the keyboard line is drawn

**One function: `input_keys::route`.** Five rules, in order.

0. **Crook's own chords never arrive.** The window delegate consumes what `input_keys::binding`
   names before any element sees the event, which is what makes `cmd-t` open a tab everywhere
   rather than typing a `t`.
1. **A selection in the output owns the copy chord while it exists.** There are two selections
   on a pane and one `cmd-c`; the one somebody just dragged across the output wins over the
   invisible one in a field they were not looking at. Off macOS that chord is also `ctrl-c`,
   and **the collision is settled by the selection rather than by the key**: with nothing
   selected `ctrl-c` interrupts exactly as it always has, and with something selected it
   copies *and lets go*, so the very next press interrupts. That release is the whole safety
   of the rule, and it is also the only feedback a copy has — including on the copy that could
   not be made, because a selection that survived a clipboard failure would claim the next
   press too, and the one after it. A modal menu suspends the claim entirely, because a
   running command has to stay interruptible. The other four ways a selection is released —
   typing, clicking into the field, clicking elsewhere in the output, the pane closing — are
   the element's; the shell printing is deliberately not one of them. The half-written command
   line in the field is *not* one of them either: a copy is not an interrupt, and the field
   only hears about it because it routes the same keystroke a moment later.
2. **The alt screen belongs to the program.** vim, `top` and `less` drive every cell and read
   every key themselves, so on the alt screen every key goes raw to the pty — and the field is
   not drawn at all. `Snapshot::alt_screen` is the whole test. This is the honest line between
   "a shell reading a line" and "a program driving the screen": it needs no shell integration,
   no prompt marks and no heuristics, and it is a fact the emulator already knows.
   Rule 1 sits above this one: what vim has drawn is still text somebody dragged a pointer
   across, and every other key on that screen is still the program's.
3. **The signal keys reach the shell.** `ctrl-c` interrupts — and throws the half-written line
   away with it, because that is what the gesture means — `ctrl-z` suspends, and `ctrl-d` ends
   the input, but only when the field is empty. The field now holds the line the shell's own
   reader used to hold, so an unconditional `ctrl-d` would be an end of file every time; over
   a written line it is the delete-forward it is in every line editor. A modal menu over the
   window takes the typing away and leaves these three, because a running command has to stay
   interruptible.
4. **Everything else on the normal screen is the field's**, and a key the keymap has no
   meaning for does nothing rather than leaking into the shell.

**Which chords are Crook's** is the other half of the same file, and it lives there rather
than in the workspace for a reason the field made unavoidable: a binding consumed in the
delegate never reaches `route`, so two tables in two modules can silently take the same key
away from each other. Side by side, a test asserts that no chord is in both.

The two platforms differ, and not by taste:

- **macOS** puts Crook's chords on Command, where a Mac application's chords live and where
  nothing the field wants can be. Tab selection is `cmd-alt-left/right` rather than
  `cmd-shift-left/right`, because the latter is how every macOS text field selects to the end
  of a line.
- **Linux and Windows** put them on Control-**Shift**. A bare `ctrl-letter` belongs to the
  tty: `ctrl-c` interrupts, `ctrl-d` ends input, `ctrl-w` erases a word. Tab selection is
  `ctrl-pageup/pagedown`, which leaves `ctrl-shift-left/right` to the field, where it selects
  by word. Copy and undo are `ctrl-shift-c` and `ctrl-shift-z` for the same reason every
  terminal emulator on Linux arrived at.

The platform is a parameter of the keymap rather than a `cfg!` inside it, so both halves are
tested on either machine.

**Three dependencies came with the field**, and each is a pure-Rust crate with no build
script, which is the standing constraint of §3:

- `unicode-segmentation` — grapheme clusters and word boundaries. A caret that lands inside
  `é` written as `e` plus a combining acute is a panic on the next slice, and "one character"
  in a filename is not one `char`.
- `unicode-width` — the same East Asian width table the emulator lays its grid out with, so
  the field gives `日` the two columns the shell will echo it back in. Without it a CJK
  filename is drawn on top of itself and a click lands two columns out.
- `arboard` — the system clipboard, text only (its default features pull in `image`, which
  nothing here needs). Opened once, lazily, per window; a machine with no clipboard records
  the failure and never asks again.

One layout primitive changed for the field, in `crookui_core`: `Flex::with_no_overflow`. A
flex measures a child that is not flexible with an *unbounded* main axis — that is how a child
says how much it wants, and how `Empty` says "as much as there is" — and a child that asks for
more than the flex has is given it and painted past the end. A list with a clip below it wants
exactly that; a pane's column does not, because the field grows downwards *into* the grid and
would otherwise be drawn over the panel's border and off the bottom of the window. So the pane
asks for the other behaviour by name, and an overflowing flex measures those children again
against what is left. The field's own ceiling — at most eight rows, and at most half of a
short pane — is the other half of the same decision.

### What the emulator does not do

Mouse reporting, IME composition, the kitty keyboard protocol, the numeric keypad, and OSC 52
clipboard writes — the *field* reaches the system clipboard, the grid does not, and an OSC 52
is logged rather than silently dropped. Ctrl+Enter and Ctrl+Tab are indistinguishable from the
unmodified key in every legacy encoding, which is the stated reason the kitty protocol exists.

One residue is worth writing down rather than discovering. End-of-file on the pty master is
the only signal this design has that a session is over, and something other than the child
can hold that open — a `sleep 60 &`, a server that outlived the shell that started it. Such a
pane stays open after its shell exits, and closing it by hand leaves its reader thread parked
on a descriptor that nothing can close from outside. What it does *not* leak is the session:
the reader holds it weakly, so the emulator, its ten thousand lines of scrollback and the pty
itself go the moment the pane does.

---

## 8. What is deliberately absent

Everything below is a real feature of Warp, and every one of them is out of Crook v1. The
point of listing them is the second half of each entry: roughly what adding it would touch.
The terminal used to head this list; §7 is what it became, and the parts of *it* that are
still absent are named at the end of that section.

**A TUI front-end.** Warp runs the same views in a terminal by making the core
renderer-agnostic: `StoredView` is an enum whose `Gui` and `Tui` arms share one registry, one
ref-count and one focus path. Crook keeps that seam (the core knows about `Element` and
`Scene`, never about `wgpu` or `winit`) but ships one arm. Adding the second means a parallel
cell-grid element trait, a measure/arrange/paint presenter over a character buffer, a
`crossterm` terminal guard and input thread, and a frame renderer with wide-grapheme
continuation handling — call it 2,000 lines. Crucially it means **no change to the core**,
which is the entire reason for the `crookui_core` / `crookui` split.

**Tab groups, pinning, tear-off.** Each of these converts index arithmetic into
range arithmetic. Groups add a "cannot cross the group boundary" branch to every move and a
"prune the empty group" branch to every close. Pinning splits the tab vector into two implicit
regions that every insertion has to clamp against. Cross-window drag — ghost slots, detached placeholders, collapsed source slots, a drag-preview
window — is the single largest source of complexity in Warp's tab code. Crook v1 has a `Vec`
of tabs, an active index, and an MRU list; the close and hop index fixups are ported verbatim
because that is where tab bugs actually live, and they are unit-tested with no window.

Vertical tabs turned out to be the cheapest of the four and shipped as the default: they are a
second renderer over the same `TabStrip::rows`, not a second model, so `app/src/workspace/`
holds `tab_bar` and `tabs_panel` as mutually exclusive halves and `row_content` holds the one
copy of Warp's which-fact-goes-on-which-line table that both read. What it cost that the
estimate above did not name was the window-control reservation — with a panel down the left
edge the top-left corner belongs to the panel rather than to the header, so
`platform_insets::TabsPlacement` divides one answer between two elements — and, for as long as
`crookui_core` had no scrollable element, a ceiling: the list was `Clipped`, roughly nine tabs
fitted a 640px window, and the rest were drawn, clipped away and unclickable. The settings
page needed a `Scrollable` anyway, so the panel got one too and the ceiling is gone. What is
still missing there is auto-scroll: selecting a tab with the keyboard does not bring its row
into view, because that needs a scrollable that can be told to make a particular child
visible.

**Shell integration, and the two things it would fix.** Crook never tells the shell anything
about itself, and the shell never marks its prompts (OSC 133, or Warp's own bootstrap). Two
consequences are worth naming rather than discovering:

- **No completion.** Tab does nothing in the field, because the shell has never seen the
  partial line and has nothing to complete. There is no way to fake it: completion is the
  shell's, and reaching it means either sending the line for the shell to edit — which is the
  design the field replaced — or asking the shell over an integration channel.
- **A password prompt is composed in the clear.** `sudo`, `ssh` and `read -s` turn echo off
  and read a line on the *normal* screen, so the field takes the keys, shows the secret as
  ordinary text and records it in that pane's history for the life of the pane. The obvious
  signal does not work: `zsh`'s line editor and `bash`'s readline both keep `ECHO` off at
  their own prompt, so "the tty is not echoing" is true nearly all the time and cannot tell a
  password prompt from a shell waiting for a command. Telling them apart needs to know where
  the prompt is, which is what shell integration is for. Until then the field's history is at
  least in memory only, per pane, and dies with the pane.

**A settings *stack*.** The page exists; the machinery under it does not, and that is the
split worth keeping. Warp's stack — a `define_settings_group!` macro, a settings-value crate,
`schemars` schemas, TOML path routing, cloud sync, per-platform gating, a file watcher and a
generated JSON Schema — exists to serve roughly 800 settings. Crook has nine, in two `serde`
structs in one JSON file, read once at startup and written back whole. That is correct at nine
and the migration to something larger is a day; doing it in the other order is a month.

What the page took from Warp is the *presentation* and the *shape*, not the plumbing. The
presentation: a rail of pages, category headings with a rule between them,
label-left/control-right rows with a description line, apply-on-click with no Save button, a
reset button that doubles as the modified indicator, and inert rows drawn greyed rather than
dropped. The shape is the more interesting half — settings are a **pane**, the same thing a
shell lives in, so they open in a tab of their own, sit in the strip beside the work they
configure, split next to a running shell, and close with the same ×, the same middle click and
the same close chord — `cmd-w`, `ctrl-shift-w` off macOS — as everything else. Warp's
`settings_pane.rs` plus its one-per-window pane manager; `TabAction::OpenSettings` is both
halves of that manager, navigating to the existing
pane or opening a tab for it.

That shape has a price and it is worth naming, because the first draft of this page was a
modal card specifically to avoid it. `PaneContent` is now an enum, `Pane::session` and
`Pane::status` return `Option`s, and each of the two row renderers carries one branch for a
row that stands for something other than an agent — a gear where the status dot goes, and one
line where the fact table would have resolved three. `RowFacts::settings` is where that line
is decided, once, for both layouts: without it the "Pane title as: Branch" arm falls back to
the command and the compact subtitle *is* the command, so the row would read "Settings" over
"Settings". `Workspace::open_panes` is the other half of the bill: it is what the terminal
model syncs against, and it filters the settings pane out, because a shell opened for a pane
that draws no grid is a process nobody can see.

The omission is search: Warp filters the rail and the content together from one field, per
widget, with match counts. That needs a text input, and what Crook has is half of one. The
model is there and is general — `app/src/editor` draws nothing, touches no clipboard and
knows no keystroke — but the only element that draws it is `CommandInput`, which measures in
terminal cells against a `CellFont` and reads a pane's `PaneInput`. `crookui_core` still has
no text field of its own, so the gap here is an element, not a model.

**A theme *system* the size of Warp's.** Themes themselves are in — see below — but Warp's
appearance layer is a great deal more than a palette: gradient fills for background, accent and
cursor; background images with an opacity ramp; a `details` block of ten opacity knobs; a theme
creator that k-means five colours out of a photograph; importers for Alacritty and iTerm
configs; and a filesystem watcher that hot-reloads the themes directory. Crook reads the same
file format and ignores every one of those fields rather than refusing a file that carries
them, which is the property that matters: a theme written for Warp loads here.

The one omission worth naming is **OS sync**. Warp resolves the active theme as a pure function
of (a `use_system_theme` flag, an explicit `{light, dark}` pair of theme names, the OS mode) —
a design worth copying exactly when it arrives, because it needs no per-theme pairing metadata.
What it needs first is the OS mode, and that means plumbing `winit`'s system-theme query and
its `ThemeChanged` event through `crookui`, which is a change to the windowing layer rather
than to the theme one.

**Themes, and what a theme is allowed to be.** `app/src/theme.rs` used to be one `const`
struct of twenty colours with a comment saying this type is the shape themes would load into
when they arrived. They arrived, and the type was the right shape: what changed is that
`THEME.surface` became `theme().surface`, a process-wide value behind a lock rather than a
constant. That global is the one architectural compromise in the feature, and it is
deliberate — Warp reads its appearance layer as a singleton entity because every one of its
render functions holds an `AppContext`, and half of Crook's are free functions over borrowed
data that hold nothing. Threading a `&Theme` through sixty signatures buys nothing a lock read
does not.

The file format is Warp's, key for key, because there are hundreds of these files already
written and a format differing by a key name would waste all of them. It is read by a
hundred-line parser for the subset a theme file actually is — a flat map, one nested block,
hex strings — rather than by a YAML crate, which is the same trade as everywhere else in this
repository: `serde_yaml` is unmaintained, and anchors, flow mappings and multi-document files
have never appeared in a theme.

Two ideas were taken wholesale from how Warp derives a palette, and both pay for themselves.
**A theme stores four colours and derives the rest** — surfaces, borders, overlays and muted
text are the background composited with the foreground at fixed percentages, so a palette
nobody anticipated still has surfaces that read and nobody can write an internally
inconsistent theme. And **light or dark is inferred, never declared**: a theme whose text is
dark is a theme for a light background, which is one function over luminance and cannot be got
wrong by a file that never says it.

Crook's own addition is an invariant the derivation makes unrepresentable: there is one
background, and both the chrome and the terminal grid are painted in it. Two backgrounds that
nearly matched is precisely the bug that took two commits to remove from the pane renderer.

**Keymaps.** Warp has editable bindings, fixed bindings, context predicates, and a
user-remappable keymap. Crook reads input directly. The half worth keeping is already kept:
keyboard and mouse produce the *same* action values, so a keymap layer can be inserted later
without touching a single handler.

**Persistence.** No session or window restore. When it arrives, the shape to copy is Warp's:
snapshot types entirely separate from live types, containing only serializable fields and none
of the mouse, drag or handle state — a `Vec<TabSnapshot>` plus an active index, `serde_json`
to a file next to the config.

**Telemetry, crash reporting, autoupdate.** All absent. Worth noting that adding Sentry on
macOS is not a `Cargo.toml` line: Warp's build script downloads an `xcframework` and its
bundler wires an rpath for it, which is precisely the class of thing §3 was written to avoid.
If crash reporting becomes necessary, prefer something that is a pure-Rust crate on all three
platforms, and treat a build script as the cost it is.

---

## 9. Summary of the divergences

| | Warp | Crook |
| --- | --- | --- |
| UI backends | AppKit+Metal on macOS, winit+wgpu elsewhere | winit + wgpu everywhere |
| Build scripts | `app/build.rs`, `crates/warpui/build.rs`, others | none, anywhere |
| Native prerequisites | full Xcode + Metal Toolchain; LLVM, CMake, protoc on Windows; ~15 `-dev` packages on Linux | CLT; VS Build Tools; `build-essential` + `pkg-config` |
| Font stack | CoreText + forked font-kit/fontconfig + forked dwrote | `cosmic-text` + `swash`, one path |
| Glyph atlas and shader | hand-written | hand-written — the one place Crook does *not* take the shortcut (§4) |
| Terminal emulator | its own, in the AGPL half of the repository | `alacritty_terminal` (Apache-2.0) behind a resolved-colour `Snapshot` (§7) |
| ConPTY on Windows | vendored | the system one, through `portable-pty` |
| Geometry | `pathfinder_*` with a patched `pathfinder_simd` | plain vectors and rects |
| Core front-ends | `StoredView::{Gui, Tui}` share one registry | one arm; the seam is kept, the arm is not written |
| Channels | six | two |
| Packaging | 2,245 lines of shell and PowerShell | a release binary today, `cargo-dist` next |
| The command line | an input field, with shell integration behind it | an input field, with the alt screen as the whole test (§7) |
| Autotracking | `Tracked<T>` dependency capture | explicit `ctx.notify()` |
| Settings | ~800, with a macro DSL and cloud sync | 10, two `serde` structs and a name in one JSON file |
| Themes | 21 built in, gradients, images, a creator, OS sync, hot reload | 14 built in, the same file format, a creator without the image, no OS sync |
| Theme chooser | a 240px docked panel with search and virtualisation | a 248px docked panel, no search, every row built |
| Settings UI | a pane, 16 pages, search over ~800 widgets | a pane, 4 pages, no search |

The through-line: Crook keeps every *architectural* idea from Warp and rejects almost every
*build-system* one. The architecture is what makes a GPU terminal tractable in Rust. The build
system is what a decade of platform-specific product requirements does to a repository, and
Crook has not earned any of those requirements yet.
