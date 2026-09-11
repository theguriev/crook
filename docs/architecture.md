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

A UI is a graph. A tab strip holds tabs; a tab holds an agent session; a header holds a chip a
plugin drew, which must repaint when that plugin's background poll returns. Expressed directly
in Rust — parent and child holding references to each other — this is either impossible or it is
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
last `ModelHandle<GitModel>` from inside an event callback must make every surviving
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

### The window's frame is Crook's

Crook opens a client-decorated window. `WINDOW_CHROME` in `app/src/lib.rs` is `Client`, the
header *is* the window's title bar, and the window's own controls sit on Crook's surface rather
than in a strip of system chrome above it. It is the one decision in the application that means
something genuinely different on each of the three platforms, which is why it is written down
here rather than left in the code.

**macOS keeps its frame.** Turning decorations off there takes the traffic lights away with
them, and no Mac application draws its own. The arrangement AppKit actually offers is a
transparent title bar over a full-size content view — `with_titlebar_transparent`,
`with_title_hidden` and `with_fullsize_content_view`, with `with_decorations(true)` still in
place. The window keeps its buttons, its shadow, its resize edges and its own drag behaviour;
what goes is the *bar*, so Crook's header paints to the top of the window and the lights are
painted over it. Moving, resizing and closing the window are therefore not Crook's problem
there. The room is: `platform_insets` reserves the 70 logical pixels the three buttons occupy —
measured off a screenshot of the running window, where they span x = 9.0 to 68.5 — at whichever
element owns the window's top-left corner, and gives it back in fullscreen, where macOS moves
them into the menu-bar overlay.

**Windows and Linux have no frame at all.** `with_decorations(false)` is the whole of it, and
everything the frame was doing becomes the application's:

- **The controls: there are none.** Crook drew minimise, maximise-or-restore and close for a
  while — 45×30 squares on Windows, 30px circles on GNOME and KDE — and they are gone. A
  frameless window with three buttons in its corner is a second title bar inside the one the
  desktop already has a shortcut, a gesture and a window menu for, and on a tiling compositor
  it is three controls that do nothing anyone wants. What is left is the window plugin's
  `minimise`, `maximise` and `close-window` commands, bindable like any other, and the
  desktop's own. `platform_insets` therefore reserves nothing at either end on these two
  platforms, and the header runs to both corners of the window.
- **Moving it.** `Window::drag_window`, from a press on the header's *empty space* — which is
  not a list of rectangles but whatever the row's own children did not claim, so adding a
  control to the header stops it being draggable there on the same frame.
- **Resizing it.** An invisible five-pixel border, eight zones with the corners winning over the
  edges, `Window::drag_resize_window` on a press and the matching cursor on a move. It runs
  *before* the element tree, because a press five pixels into a frameless window belongs to the
  window manager however interesting the element under it is — with one exception, below.
- **The shadow**, on Windows only: DWM hides it for an undecorated window, so the attributes ask
  for it back with `with_undecorated_shadow(true)`. Without it there is no edge of any kind
  between Crook and a dark window behind it.

**One seam exists only because of this**, and there used to be two. The one that is gone was the
resize border against the caption buttons: the border consumes a press before any element sees
it, and the close button sat in the corner a person throws the pointer at without aiming, so
`edge_at` was told a box to answer "no edge" inside. With no buttons to protect, the corner is
the `NorthEast` zone again and `edge_at` is eight zones and nothing else. The seam that remains
is a release that never arrives: a move or a resize runs inside the window manager's own loop,
which swallows the button-up that ends it, so the windowing layer forgets every held button when
a gesture starts — *after* dispatching to the application, because a window move is started by
the header, inside that dispatch.

**Linux has now been run, and Windows has not.** The borderless window was opened on Wayland
under Hyprland 0.56: it comes up with no frame, the header runs to both corners, a real shell
draws in it, and the compositor reports the size and position the application asked for. One
thing was wrong and is fixed — the window had no `app_id`, so no window rule could match it, no
dock could group it and no `.desktop` file could be tied to it. winit only sets that when asked
and nothing asked; `crookui::windowing::chrome` asks now, and Hyprland reports `class: crook`.

What is still unrun on Linux is what needs a pointer rather than a window: the resize edges, the
drag that moves the window and the double click that maximises it. On **Windows**, none of it
has opened a window at all.

What was done instead is worth stating precisely, so it is not mistaken for more. `--controls macos|windows|linux`
lays the header out for another platform's controls on this one — the real element tree, real
hit-testing, the real action path — so every platform's reservation, and the corner that is now
title bar on all three, was laid out and pressed here; `cargo clippy --target
x86_64-pc-windows-msvc` and `--target x86_64-unknown-linux-gnu` check that both compile and warn
nowhere; and the geometry that needs no window — the eight resize zones, the per-platform
reservation — is unit-tested.

**Two macOS behaviours worth knowing**, both found rather than written. AppKit keeps a drag band
roughly 28 points tall at the top of a window with a transparent title bar: a *drag* there moves
the window whatever Crook draws underneath, though clicks pass through normally, and a double
click there is macOS's own zoom rather than Crook's. It also keeps the outermost 8–10 points of
each corner for resizing, so the literal corner pixel is never the application's on macOS.
Neither can be turned off through winit, and neither applies where Crook draws the controls
itself.

### Where the `cfg`s live

Even with one backend, platform differences exist: the window-control inset is 70px on the
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

### Icons, in the same atlas

The chrome's marks are [Lucide](https://lucide.dev) — the set `lucide-react` publishes — and
they go through the machinery above rather than beside it. An icon is a mask, a mask is what
a glyph already is, so an icon is one more entry in the glyph atlas, one more instance in the
glyph pipeline, and one more branch of nothing at all in the shader.

Three decisions make that work.

**The geometry is vendored, not parsed.** `script/icons` pins a version of `lucide-static`,
resolves every arc, shorthand, `<circle>`, `<rect>` and `<polyline>` into absolute moves,
lines and cubics, and writes one Rust file. Nothing parses SVG at runtime, because the icons
cannot change at runtime — an SVG parser in the binary would be work done on every launch to
produce a constant. It also means the runtime has no arc code, no shorthand state and no XML.

**The rasterizer is a distance field, not a path filler.** Every Lucide icon is a *stroke*
with round caps and round joins and no fill. The usual way to draw one is to build the
outline of the stroke — a quad per segment, an arc per join, a cap at each end — and fill
that under a winding rule; all of that machinery exists to answer "is this pixel within half
a stroke of the path", which for round joins and round caps *is* the distance to the path. So
`crookui_core::icons::raster` flattens the cubics and asks each pixel how far it is from the
nearest segment. Round joins and caps come out exact rather than approximated, there is no
winding rule, and the whole thing is 200 lines. Coverage from that distance is the overlap of
two bands — half a pixel either side of the centre, half a stroke either side of the path —
which is exact for a straight run at any width, including the two-thirds-of-a-pixel stroke a
16px icon has.

**A mask per size, cached forever.** The key is the mark, the size in whole device pixels and
the stroke width; there is no subpixel bucket, because an icon is snapped to the pixel grid on
both axes where a glyph is snapped only vertically. Nothing is rasterized twice, and the whole
set at the three sizes the chrome uses is a few dozen kilobytes of atlas.

The one place the stroke rule does not reach is `crookui_core::icons::art`, whose marks are
*pictures*: a Pac-Man pirate in an eyepatch, three frames of him, drawn in the same 24-unit
grid as the stroked set. Nothing in the binary draws him for itself — the feature that used to
is a plugin now, outside the binary, and it asks for a frame by name (`pirate`, `pirate-open`,
`pirate-wide`) through the second tier's `Node::Icon`. The artwork stays here because the
*host* is what paints it: a sandboxed plugin ships no pictures, and a picture it could ship
would be the wrong weight beside everything else on the row. Three things follow from a
picture, and each is smaller than it sounds.
A picture needs a **fill**, which is `raster::fill` — signed area accumulated per edge and run
along each row, no sorted crossing list and no winding rule to configure, in about forty lines
beside the distance field rather than in place of it. A picture has **more than one colour**,
and a mask has none, so each frame is two layers — the yellow head and the black on it —
stacked as two `Icon` elements, which is also why they are cached and tinted like everything
else. And a picture is **clipped by its own artwork**: the strap runs off both sides of the
box and the grin is an arc of a circle that is mostly outside it, so every layer is multiplied
by the coverage of the layer beneath — the ink by its frame's face, a bitten face by the whole
head — rather than by a clip rectangle nobody could see in the geometry. `icons::Mark` is the
pair of the two kinds, and it is what the element, the scene and the atlas all hold: only the
rasterizer ever asks which one it has.

What this replaces is worth naming, because it is the argument for having done it at all.
Before this, a gear was `⚙` and a close button `×` — codepoints, drawn out of whatever font
the machine happened to have, which is a flat gear on one machine and a colour emoji on the
next. Everything a font would not draw was built out of `Container`s: the two density marks
in the gear menu were seven rectangles, and the git branch beside a tab title was three.

### The second text field

For most of Crook's life there was exactly one place to type: the composer
under a pane. That element draws in the terminal's cell grid so it lines up
with the output above it, draws its first row on the shell's own prompt row,
takes its colours from the pty's palette, sends its line to a shell, and routes
its keys through a policy whose first three rules are about a selection, an
alternate screen and a signal. Two files said, in as many words, that a second
field was a refactor of the input layer rather than a feature: the Themes panel
has no search box, and the settings page had none either.

The settings page's rail now has one, and the refactor turned out to be three
seams rather than a rewrite.

- **The state was never a pane's.** `app/src/text_input.rs` — called
  `pane_input.rs` while a pane was the only thing that could hold one — is an
  editor, a caret blink, a drag, and one `has_keys` flag, behind an `Rc` so it
  survives the element tree that draws it. Two of its lines assume a shell:
  the one that hands back a line to send, and the one that throws a line away.
  A field with nowhere to send a line simply never asks for those.
- **The keymap was never a pane's either.** `input_keys::route` is the keyboard
  policy of a *pane*; `input_keys::intent` under it is the platform's text
  editing — word movement, the line ends, the clipboard chords, undo, and the
  emacs bindings macOS puts in every text field — with no pane in it. Making it
  `pub` is the whole of what the search box needed to get all of that from the
  same table the composer gets it from, which is what stops the two drifting.
- **The painting is not shared, and should not be.** The composer is drawn in
  cells because it has to line up with a grid. A search box in a sidebar is
  drawn through the shaper, like every label beside it, and gets its caret
  position from the shaped line rather than from a column times a cell width.
  Those are two different elements, and each is short.

What is still missing is a *focus* concept: which field has the keyboard is one
boolean per field, written by `Workspace::sync_input_keys` whenever focus could
have moved. For a field a sidebar section brought with it the rule is Warp's —
the box has the keyboard whenever its section is showing — and it holds because
a section replaces the panes rather than sitting beside them, so there is
nothing else on that screen that takes a key.

The tabs panel's own search box is where that rule ran out, exactly as this
paragraph said a second field on the same surface would. It is drawn *beside* a
running shell: the list it filters and the pane the keyboard would otherwise
belong to are on screen together, so "my surface is showing" cannot decide who
gets a keystroke. The answer is one function — `Workspace::search_takes_keys` —
and it is deliberately the only place the two are settled against each other:
the box has the keyboard when somebody put it there (a press, or `cmd-k` /
`ctrl-shift-k`) *and* it is on screen *and* nothing modal is over it, and
`sync_input_keys` reads that one answer to decide both what the box gets and
what the pane loses. The two ways back out — Escape, and Enter on a match — are
claimed by `Workspace::action_for` rather than by the field, because a field
cannot hand the keyboard to something that is not a field. A focus *system*
would be this rule generalised; one function is what two fields cost.

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
  rather than on `debug_assertions` means the release-flags job actually exercises it whenever
  the workflow is asked for.
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
workspace with the release profile and the release feature set. It is one of the five the
`Crook CI` workflow runs on all three platforms when somebody dispatches it — see
CONTRIBUTING.md, and note that nothing dispatches it for you. Without it, "the shipped feature
combination does not compile" is discovered at release time.

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

### Which shell, and how it is started

Two decisions live in `crates/crook_terminal/src/pty.rs`, and both of them are the difference
between "my terminal" and "a terminal".

**Which shell.** `$SHELL` when it names something this user may execute, otherwise the shell
in the password database — `pw_shell`, what `chsh` writes and what `login(1)` reads — and
`/bin/sh` only as the last resort. Both fallbacks earn their place. `$SHELL` is *absent* from
the environment of anything a desktop launches rather than a shell: an application bundle
opened from the Dock, a `.desktop` entry, a container. And `$SHELL` *outlives* the shell it
names whenever a Homebrew or Nix package is removed or a `chsh` target moves — the first case
would hand the user `/bin/sh` with none of their configuration on the launch path most people
use, and the second a pane that cannot open at all. The answer comes from `portable-pty`'s own
resolution on purpose: this same name is what `shell_integration::Shell::of` reads to decide
which stubs to write, and a Crook that wrapped zsh's stubs around a `/bin/sh` would be worse
than one that wrote none.

**How.** As a *login* shell, which is what `login(1)`, Terminal.app, iTerm2 and WezTerm all
start, because that is the only kind that reads the files a person's `PATH` is assembled in —
and on macOS the only kind that runs `path_helper` at all. There are two conventions for
asking, they are different mechanisms, and Crook uses both:

- **`-l`** for zsh, bash and fish, whose switch is known. The shell is named explicitly, which
  matters because `app/src/shell_integration` has by then written startup stubs shaped for
  *that* shell.
- **`argv[0]` with a leading hyphen** — `-tcsh`, `-ksh`, `-dash` — for everything else. It is
  what `login(1)` itself does, it needs no option parsing, and so it works on a shell nobody
  anticipated. Guessing `-l` there does not: tcsh answers ``Unknown option: `-l'`` and the
  pane opens with nothing in it. `portable-pty` offers this only through `new_default_prog`, a
  builder that takes no arguments and resolves the shell itself out of the `SHELL` it will
  hand the child — which is why that variable is set to the shell being asked for.

Windows gets neither, and should. PowerShell and cmd have no login mode: `PATH` is in the
registry and every process already has all of it, and `$PROFILE` is read by every interactive
PowerShell. Inventing an equivalent would be Crook making up a startup convention the platform
does not have.

The *default* — as opposed to the mechanism — is per platform, in
`shell_integration::login_by_default`, and it is set by what the desktop's own terminal does:
on for macOS, where every terminal starts a login shell and `path_helper` is only reachable
that way, off for Linux, where GNOME Terminal and Konsole start non-login shells and
configurations are written to match. `GeneralOptions::login_shell` is the switch either way,
and the README's "Which files your shell reads" is the user-facing half of this.

**Your files must not be able to tell.** `app/src/shell_integration/launch.rs` reaches zsh by
pointing `ZDOTDIR` at a scratch directory of stubs that source the real files, and that
borrowed variable is a trap in two directions. A `.zshenv` containing
`ZDOTDIR=${ZDOTDIR:-$HOME/.config/zsh}` — a common spelling — takes the wrong branch against a
`ZDOTDIR` somebody else set, and the person then loses `.zprofile`, `.zshrc` and `.zlogin`
entirely; so each stub restores `ZDOTDIR` to the state it was in *before* Crook — unset, for
almost everybody — before sourcing, and the `.zshrc` stub's last act restores it for good, so
that a nested `zsh` or an `exec zsh` in the pane starts from the state a first zsh does.
The other direction is macOS's `/etc/zshrc`, which runs *between* the stubs and does
`HISTFILE=${ZDOTDIR:-$HOME}/.zsh_history` — pointing the history file inside a directory the
pane deletes when it closes. Testing only for an empty `HISTFILE`, which is what every
framework's own guard does, never fires there; the stub also tests for one that resolves
inside Crook's own directory, which is the one answer that cannot have come from the user.

bash is the shell that will not take the arrangement at all: `bash --rcfile <file> -l`
silently never opens the file, and `bash -l --rcfile <file>` does not start. So its rc file
plays the login shell itself — `/etc/profile`, then the first of `~/.bash_profile`,
`~/.bash_login`, `~/.profile`, then `~/.bashrc` only as the fallback for a home with no
profile in it — and plays the logout shell too, chaining an `EXIT` trap onto whatever it finds
so `~/.bash_logout` still runs and `logout` still closes the pane.

**What the child is told beside that.** `TERM=xterm-256color` and `COLORTERM=truecolor`,
because `alacritty_terminal` implements those sequences and the entry is in every terminfo
database old enough to matter. `LINES` and `COLUMNS` are *removed* rather than set: whatever
started Crook may have had them, they described its window, and the kernel's `winsize` — which
the pty is opened at before the child exists — is the truth. `TERM_PROGRAM=Crook` and
`TERM_PROGRAM_VERSION` come from `shell_integration` and reach every shell, marks or no marks.

One gap is worth naming because it is visible now that profiles run. A pane's pty is opened at
80×24 and only the *first layout* resizes it, so a shell that prints its whole startup —
a `~/.zprofile` banner, a greeting sized with `tput cols` — can do so before that. What it
printed is copied out of the grid into a block the moment the first prompt mark arrives, so a
later resize cannot reflow it. Closing that means measuring a pane that has no terminal in it
yet, which is a change to how panes are laid out rather than to how shells are started;
`INITIAL_GRID` in `app/src/terminal_model.rs` carries the note.

### Who drives it

`app/src/terminal_model.rs`, in the shape `git_model` established and `usage_model` shared
before it left for a plugin: work off the UI thread, delivered on it, `ctx.notify` only when
something a viewer could see actually changed. Each terminal gets an OS thread of its own
rather than a background-pool worker, because a pty read blocks for as long as the shell is
quiet and the pool is sized for exactly the chains that park on timers — five of them, counted
one by one in `PARKED_WORKERS`, and a pane is not one.

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

### Blocks: the output is a list of commands

A pane draws its output as a list whose items are commands — prompt, command line, output —
and the line being composed sits under that list as a sibling on the same ground. Three pieces
in `crook_terminal` and three in `app/` make it, and the interesting decisions are all in the
seam between them.

**Where a boundary comes from.** Two things, and only one needs the shell. `Terminal::submit`
is what the composer calls on Enter: it records the exact command text and the exact moment
before a byte leaves for the pty, so a block knows what was run even when it never learns how
it ended. The other is OSC 133 — `A` prompt start, `B` prompt end, `C` output start,
`D;<exit>` finished — read out of the *same second `vte` pass* the emulator already ran for
OSC 7, so the mark is applied while the cursor still stands where the shell left it. That is
also why `Emulator::advance` feeds the stream in pieces cut at the marks: feeding a whole
chunk and looking for marks afterwards would place every boundary wherever the pty read
happened to end.

`blocks::TABLE` is the whole state machine — one row per state the open block can be in, one
column per thing it can be told, every cell either a transition or a *named reason* for doing
nothing. There is no if-chain anywhere else deciding what a mark means, and the rule that
fills the doubtful cells is **merging two blocks is recoverable; splitting one is not**. A
block that swallowed its neighbour still shows every byte in order; a block cut in half has
lost the connection between a command and its output and no later mark can put it back. The
case that proves it: every prompt framework with a transient prompt re-emits `A` and `B` when
a line is accepted, between the submit and `preexec`'s `C`, and obeying that would file an
empty statusless block above every single command.

**Harvesting, not slicing.** When a block closes, its rows are *copied out of the emulator*
into an owned per-block store and the emulator moves on. The obvious alternative — remember
that block seven is grid lines 412 to 480 and slice the scrollback when drawing — cannot be
made to work over `alacritty_terminal`, for three independent reasons: its history is capped
and evicts from the top *without counting*, so a stored line number silently drifts; it
reflows on a column change, so every stored number moves at once; and `clear` throws the lines
away entirely while the block that printed them is still on screen. Owning the rows makes a
block survive all three. The other alternative — one grid with decorations drawn over it — is
worse in a way that is not obvious until it is built: every row of a grid is in *one*
coordinate space, so per-block padding, per-block height, a per-block hover target and a
per-block copy all have to fight arithmetic that says row 41 is 41 cells below row 0.

What that costs is the reason `harvest::BlockRows` is not a `Vec<SnapshotCell>`. A cell is
twelve bytes; ten thousand rows of two hundred columns is 24 MB per pane, for ever, with
nothing evicting it. So a block is stored the way its text actually is — one `String` for
every row end to end with trailing blanks trimmed, one run-length `Vec<StyleRun>` for how it
is painted, `u32` prefix offsets into both — and ordinary output is one run a row. A screenful
measures under an eighth of the cell array it came from, and a thousand-row block is five
allocations rather than three thousand. Exactly one method gets rows back out:
`materialise(row, &mut cells)` fills a caller-supplied scratch `Vec<SnapshotCell>` reused down
the whole list, so a harvested row goes through the *same* three painting passes a live one
does — merged background runs, underline and strikeout rules, then one glyph id and one fixed
advance per cell. A second cell painter would be a second set of bugs.

**The grid is emptied at every harvest, and that is load-bearing.** Closing a block drops the
emulator's history *and* blanks the screen above the block that opens next, so the grid holds
the open block and nothing else. Three answers depend on that one invariant:

- the open block's anchor stays exact, because the history can now only fill up when the open
  block is longer than the whole scrollback — at which point "this block starts above the
  oldest line we have" is not an approximation but the truth;
- a column change, which reflows every stored line number into meaninglessness, is answered by
  looking for the first line in the grid with anything on it;
- rows *below* the cursor when a command ends — what a progress display that redraws with
  `\e[3A` leaves behind — can be harvested with the block that printed them, because nothing
  else can have put anything there.

**Two surfaces, one function.** `pane_surface::of` answers both "what draws the output?" and
"is there a composer?" from one `Snapshot`, and every element that needs either asks it rather
than testing a flag of its own — three copies of that rule is how a pane ends up with two
cursors. The block list is the ordinary answer; the *grid* is the answer on the alternate
screen and when the open block has grown past the top of the viewport, which is also what a
shell with no integration looks like from its first screenful onwards. The grid never has a
composer under it: the pty is sized from the pane, so a grid laid out above a field has fewer
rows to draw than the child was told it had, and the newest ones would be painted under the
field.

The composer also goes when the shell reports a command running for longer than 50 ms — Warp's
number, fast enough that nothing anybody waits for is missed and slow enough that `ls` never
flickers the field away and back. It takes the composer's space and *not* the block list: a
`cargo build` printing for a minute is still one command among the ones before it. And
"running" means the shell said `C`, never merely that a line was sent: a shell that reports no
marks never answers, and a rule that read a sent line as a running command would take the
field away at the first Enter of such a session and never give it back.

**The list is a virtualiser, and `Scrollable` is not.** That element lays its child out at
infinite height and paints all of it — a clipper. So `block_list` owns its own offset, keeps a
prefix sum of item heights in the per-pane view between frames, binary-searches it for the
first visible item and walks forward until it passes the bottom of the box; the same
arithmetic runs again *inside* an item, so a fifty-thousand-row block costs a screenful. The
finished part of the sum is rebuilt only when a command ends. Scroll position is a *mode*
(`FollowBottom`, or `Fixed` lines from the top) rather than a number, so following the end
cannot die silently, and every mutation names its cause — a wheel, a resize, a submit, a key
that reached the pty — in one function.

**What it does not do yet** is in `docs/blocks.md`. Selecting text across it is below.

### One agent, one worktree

The unit of work is an agent, a tab is that agent's workspace, and a git worktree is the same
statement made on the filesystem: one branch, one checkout, one place to work that nothing
else is standing in. Two agents editing one checkout overwrite each other; two agents in two
worktrees of one repository do not. That is the whole argument for the feature, and it is why
it is a *menu on a tab* rather than a panel of its own.

Right-clicking a tab's row opens it, in the strip and in the panel alike — the secondary
button, because this is a context menu and that is the button a context menu opens with
everywhere else. It opens only where the focused pane is inside a repository, which is the
same promise the branch chip already makes. The left button was tried first, on the row you
were already in, where the click was otherwise free; it cost nothing to dispatch and it cost
a person a menu they did not ask for every time they reached for the tab they were in.

The model is [herdr](https://herdr.dev)'s, which is the tool this borrows from rather than
Warp: there a worktree is not a thing you administer but a workspace with a git checkout
behind it, and creating one *opens* it. So the menu lists the repository's checkouts and
opens a **tab** in whichever one is chosen — or brings forward the pane already there, because
two agents in one worktree is the thing the feature exists to prevent. The tab opens in the
*group* the tab the menu was opened on belongs to, making the group out of the two of them if
there is not one yet, which is what keeps one repository's branches together. Its `--base` is
deliberately not taken: a worktree made from anything other than the head you are looking at
is a question a menu cannot ask well.

It opened a *pane* first, splitting the tab, and that was the wrong claim made in the right
place. Belonging together and being on screen together are two different statements: a split
puts two agents in one rectangle, half a window each, which is what a person asks for when
they want to watch two things at once — not what "give this branch a checkout of its own"
means. A group says the first without saying the second, so that is what a worktree opens
into now, and splitting a tab went back to being a thing a person asks for on purpose.

Three things about it are load-bearing.

**Nothing git-shaped is on the render path.** `git worktree list` is a subprocess, so it runs
on the background pool when the menu opens and lands through `ctx.spawn` — which is why the
menu has a state for "reading" at all. The 15-second poll that feeds the branch chip is not
involved: this is read once, on a gesture, and thrown away when the menu closes.

**Crook records where a shell is; it never drives it.** A new pane's directory is written onto
its session *before* the shells are synced, because that is the moment a pty's cwd is decided
and the only moment it can be. Nothing ever issues a `cd` into a running pty, and the session
directory stops being authoritative the instant the shell reports a different one over OSC 7.

**Removal asks, and asks about the right thing.** `git worktree remove` refuses over modified
and untracked files — and, measured rather than assumed, *not* over ignored ones, which it
deletes without a word. So the ignored count is the one most worth showing before the button,
and it is the one git will never raise on its own. A checkout that is locked, that is the main
worktree, or that a tab is open in is not offered for removal at all: Crook's own agent
worktrees are locked by the session holding them, and that lock is what stops one agent tidying
away another's work.

**A sweep asks once and never forces.** The same offer made about the list — remove every
checkout nothing is working in — is deliberately the weaker one. A confirmation about a single
checkout can say "there is work in there" and offer to delete it anyway, because a person is
looking at the one thing they asked about; a confirmation about six cannot, so anything git
would refuse is left standing and named as left standing, and the × on its row is still there
for the one somebody means. It is also why the question opens *before* it can be answered:
which checkouts git will actually let go is a `git status` apiece, so the face says it is
looking, then names the branches a press would really take. The row that opens it is absent
where there is nothing free, which is the same promise the menu itself makes by not opening
outside a repository.

**A wait is drawn as the pirate eating it.** Every face of the menu that is waiting on git —
the list being read, a checkout being looked in, six of them being looked in, six of them being
deleted — draws the pirate chewing, and where the wait is a list he stands in a row of pellets,
one per checkout, having eaten the ones dealt with. A sentence that does not move for as long
as `git worktree remove` takes to delete a `target/` is what a hang looks like, and it was the
whole of what the sweep used to show; the pellets say *which* checkout is taking its time. The
sweep runs one background task per checkout rather than one for the lot, so that each answer
lands on its own — which is what lets the pirate move, and what lets Stop mean something: the
checkout under the knife finishes going (a kill halfway through a delete leaves one neither
there nor gone) and the ones behind it are spared. The chain that moves his mouth parks a pool
worker for a frame at a time and only while something is running; there is no timer under a
list nobody is waiting on. `app/src/pirate.rs` is the artwork both he and the usage chip's
plugin draw from.

`app/src/git/worktree.rs` is the whole of the git side — list, add, remove, and a count of what
is loose in a checkout — with a timeout on every call (a removal's is the long one, because it
deletes whatever was built in the checkout and a kill halfway through leaves a worktree the
sweep will never touch again), two reader threads per call so a repository with a fat
`target/` cannot deadlock a pipe, and an error type whose variants are
the things a UI can offer to do about them.

### The agent says what it is doing

The dot on a tab's row has four colours and, until this section, one real source: a bell in a
pane nobody was looking at. `AgentStatus::Running`, `NeedsInput` and `Failed` were written by
the snapshot fixture and by nothing else, which made the sidebar a list of shells with a
promise painted on it. The promise is now kept by the program the promise is about, and it is
kept over the one channel that program already has.

**The report is an escape sequence.** `OSC 6340 ; <status> [; <title>] BEL`, read by the same
watcher that reads OSC 7 and the completion channel's 6339, in `crates/crook_terminal/src/agent.rs`.
Four words — `idle`, `running`, `needs-input`, `failed` — and an optional name for the work,
which lands in `derived_title` because that is what `derived_title` has always been: what the
agent calls what it is doing. The alternative was a socket, and a socket is the wrong shape
three times over. It needs an address, which the agent would have to be told; it stops at this
machine, where the terminal crosses `ssh` and `docker exec` without noticing; and it needs to
say *which pane*, where the terminal a program writes to *is* the pane. Every other terminal
drops an OSC it does not know, so a program that reports this way costs nothing in one of
them.

**The CLI writes it.** `crook --agent running --title "port the tab bar"` opens `/dev/tty` —
`CONOUT$` on Windows — and writes the sequence there, not to standard output. The caller is a
hook, and a hook's standard output belongs to the program that ran it: Claude Code reads what
its hooks print. `crook --agent-hooks claude` prints the fragment of Claude Code's settings
that makes it say all of this by itself — running on a prompt and around every tool, needing
input on every notification, idle on stop — naming the binary by its full path, since a hook
runs in whatever `PATH` Claude Code was started with. It is printed rather than installed:
Crook writes no file it does not own, and it has never opened that one.

**The shell takes the status back.** An agent that was interrupted never says it stopped, so
the emulator listens to the shell's marks beside the report: `D` ends the command a running or
waiting agent was, and takes the claim with it. A failure is the one status worth seeing after
the fact, so it outlives its `D` and goes with the next command's `C` — new work being the
thing that answers it. All of that is in `Emulator::settle_agent`, and none of it in the
workspace, which only ever hears a status change.

**Attention is a separate fact.** The bell used to write `NeedsInput` and looking used to
clear it, and that was right for a bell and wrong for an agent: an agent waiting for an
approval is still waiting after somebody glanced at its row. So `AgentSession` carries
`status`, which is what the agent said, and `attention`, which is whether something happened
while nobody was looking — the bell, or a status changing to anything but running in a pane
without the keyboard. Looking clears attention and only attention; running clears it too,
since the stop it announced is over. The dot shows the status, with one exception kept from
before: an idle pane that asked for attention is drawn as needing input, because a bell in a
pane nobody is looking at is a program saying exactly that. `is_waiting` is the two combined,
and it is what the count in the header and the "next waiting" chord read.

**The strip answers.** A waiting row is washed in the amber its dot shows, faintly, because a
dot is nine pixels and a person scanning a long list wants the row to say it. The header's
left end — a second single-item slot, `header.left`, beside the one the usage chip takes — is
a chip that counts the waiting panes and goes to the next one when pressed, and is nothing at
all at zero, since a count of zero is not information. The same move is
`crook/tabs/next-waiting`, suggested on `cmd-j` (`ctrl-shift-j` off macOS): the next waiting
pane after the active tab in the panel's order and round the end of it, so the chord pressed
three times visits three tabs rather than the same two in turn. Focusing it is what answers
the request for a look; an agent's own question stays asked until the agent says otherwise,
which is why the pane you just left can be waiting again the moment you leave it.

What is deliberately not here is a plugin. `docs/plugins.md` planned this seam as an `Agent`
service a plugin provides, and that is still the right shape for anything that *drives* an
agent — spending its budget, reading its transcript. Saying what it is doing needed none of
that: a word on a wire, written by the agent itself, which is why it reached the sidebar in a
day and why a plugin that wants to do more starts from a status that is already true.

### The command line is an input field

A pane's next command is composed under its output — an ordinary GUI text input, with a caret
you can click, selection by drag, word and line movement, undo, the system clipboard, and a
history on the arrows — and only reaches the pty when Enter sends it, through
`Terminal::submit`, which records the boundary before the bytes leave. The shell then echoes
and runs it, so the output above shows prompt, command and result exactly as it did when every
keystroke went straight through.

**It is not a box.** No background, no corner radius, no side or bottom border, no margin and
no focus ring: the caret existing is the entire focus affordance. It fills nothing of its own,
so the pane's ground is its ground; it is drawn in the terminal's own font and in the colours
the *shell* resolved, so a script that changes them with OSC 10 or 12 carries the field with
the output rather than leaving it behind in the theme's grey; and one `GUTTER` constant is the
list's left inset, the field's left padding and the width the pty is measured short by, which
is what puts a typed line and the shell's echo of it in the same column. The only thing that
ever separates it from the output is a one-pixel rule in the same role and the same full-bleed
extent as the divider between two blocks — and that rule is drawn only when there is output
cut off underneath, so the seam is invisible until it means something. What is left of the
"field" is behaviour, which is the point: a composer that reads as a widget bolted under a
terminal is one people stop believing is part of it.

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
fonts. `app/src/text_input.rs` is the per-pane state the element tree is rebuilt around;
`app/src/workspace/input_element.rs` draws it on the grid's own cell metrics.

### Selecting the output

A selection spans every block in a pane, and there is exactly **one implementation** of it.
There were briefly two — the emulator's, which is where selection started, and the list, which
is where the blocks moved to — and the result was the bug that made this section worth
rewriting: the moment a command finished, its rows were harvested out of the grid and its
output stopped being selectable. Everything a person had already run, which is the whole
reason to look at a terminal, could not be dragged across. Two implementations is where the
disagreements live; there is one, in `app/src/selection.rs`.

**An anchor is a place in the list, not a cell of the grid**: a block id, a row of that block,
a column, and which side of the cell the pointer is in. That is what makes it hold still. A
block id is handed out once and never reused; a block's rows never move within it. A command
running below adds an item rather than shifting rows, the emulator scrolling changes which
viewport row the open block's row seven is drawn on but not that it is row seven, and closing
a block copies its rows into the store in the order they were already in — so the anchors that
were made before any of that name the same characters after all of it. Ids increase with
position in the list, so ordering two anchors is a tuple comparison, "is this row inside the
selection" is two of them, and the open block — always the newest — always sorts last.

**The open block is not a second code path.** It is the last item of the same list with its
rows read out of the `Snapshot` rather than out of a store, and `crook_terminal::Rows` is the
one type that knows the difference: how many rows, how wide, one cell, how long the line
actually is, whether the terminal folded it, and write these columns out as text. Six
questions, answered twice each and nowhere else. Everything above it — the region, the
highlight, the copy — is written once.

**A grid is a list of one block.** The alternate screen and an overflowing block have no
finished blocks to address, so they address as a single item whose rows are numbered from the
oldest line the scrollback still holds; the wheel then moves the viewport without renaming
anything, which is the same property block ids give the list. Copying reads those rows back
out of the emulator through `Terminal::harvest_rows` — into the very store a finished block's
rows live in — so a drag through the scrollback copies text the snapshot never held, through
the same walk.

**The two numberings do not mix, and the surface can change under a selection.** A block counts
its rows from its own first; a grid counts from the oldest line of the history. The same pair
of numbers therefore names different characters on the two surfaces, so which space an anchor
was minted in is part of it — `selection::Cells` — and nothing resolves a selection against the
other one. A pane crosses between them without being asked to: a command that prints past the
top of the viewport falls back to the grid mid-drag, and a full-screen program does it with no
output at all. The selection is let go of when that happens, for the reason a resize lets go of
it: the picture it was drawn on is gone. Keeping it would be worse than losing it — the
highlight is not on screen, and an invisible selection that still owns the copy chord is an
interrupt spent on nothing.

**One thing here is knowingly not stable.** Grid rows are numbered from the oldest line the
history holds, and that line only stays the same line while the scrollback is still filling.
Once it is full — ten thousand lines by default — every new line drops the oldest, and a
selection held on the grid slides one row up the text per line printed. The count of dropped
lines is not something alacritty keeps and cannot be recovered from what it exposes; the same
limit is why a block's own anchor clamps at the oldest line rather than tracking further back.
`selection::grid_first_row` says so, and a test pins the behaviour so that it changes on
purpose.

**What a word is, now that the emulator is not answering.** One UAX #29 word-bound segment of
the folded line under the pointer, from `editor::text::word_range_at` — the same function the
composer's own double click uses. One rule in one place, so double-clicking a path in the
output and double-clicking it in the line being typed take the same thing. It crosses a fold,
because a fold is where the *terminal* put a long line; it never crosses a block, because that
is a different command.

**Copying** walks the blocks between the two ends, taking the ends partially and the ones
between them whole, and joins the pieces with a single newline — so the padding, the dividers
and the copy controls contribute nothing and you get the text you saw without the gutter.
Inside a block, a row the terminal folded runs on into the next with no break in the text,
trailing blanks stay behind because they are a grid having to hold something rather than
anything anybody typed, and a double-width character copies as the one character it is. A
selection ends at cells, so nothing appends a trailing newline. The block's own copy control
is the same walk over the same region — the whole block, with the blank rows at its end
trimmed off — rather than a second way through the store. Two walks is how the control came to
break a wrapped path into three lines while a drag over the same block did not.

**The highlight is one rectangle per row, drawn under the glyphs**, so selected text keeps its
own colour — selecting changes a cell's ground, never its ink. Between two selected blocks it
bridges the gap: a selection running out of one block into the next is one continuous run of
text with a line break in it, which is what a selection across two paragraphs is, and a
highlight with a hole in it at every boundary would read as several selections that happen to
touch. The band takes the columns of the row it continues rather than the pane's width, so it
lines up with the rows either side of it — and an alt-drag, which takes the same few columns
out of every row it crosses, bridges as the column it is rather than as a bar across the
padding. Nothing in the gap is copied. The highlight is also cut down to the columns the pane
is actually drawing: a block keeps the width it was harvested at, so a pane narrowed since
holds rows it cannot draw, and only the picture is short — the copy still takes the whole row,
because those cells are the block's text.

**A resize lets go of it.** Changing the column count re-wraps every row the open block holds,
so the cells a selection named hold other text afterwards. There is no honest way to
re-anchor that, and a highlight left over re-wrapped rows is a copy of something nobody
selected.

**The press is not the selection.** A click that is never dragged anywhere selects nothing at
all, which is what makes a plain click on the output *clear* the last selection rather than
leave a one-cell highlight where it landed. So `PaneSelection` keeps two things: the press
that is open — one anchor and a kind, from the button going down until it comes up — and the
selection, which is only ever stored once it covers cells. Whoever moves it has the blocks to
resolve it against and passes the answer in, so "is anything selected?", which is the question
`ctrl-c` is settled by, is a `bool` read rather than a walk of the blocks. It is asked of a
surface — a selection the pane is not drawing answers no — because the whole point of the
question is whether there is a highlight the person can see. The press itself cannot live in
the element, because the tree is rebuilt between the press and the drag; both live in
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
output and the field under it are siblings in a `Flex`, which hands the same `KeyDown` to both
— the output first. Both ask the same `PaneSelection` whether anything is selected, and both
act on a different half of the answer. So the output does not release what it copies where it
copies it: it dispatches `WorkspaceAction::ReleaseSelection`, and actions are applied once the
whole tree has seen the event. An output that released it in place would have the field read
the same `ctrl-c` as the interrupt it is with *nothing* selected, and throw away the
half-written command line; on macOS the field's own `cmd-c` would then overwrite the clipboard
the output had just written. The invariant is worth stating plainly: **nothing may change what
is selected in the output while a keystroke is being dispatched.**

**A double-width character is one character in two columns.** Its glyph is drawn once, from
the first, across the width of two. So the region grows to whole characters before either the
highlight or the copy is taken from it — reaching the trailing half takes the character, and
reaching the character takes the column its right half is drawn in — and the two cannot
disagree, because they are the same range.

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
2. **A pane with no composer belongs to the program.** vim, `top` and `less` drive every cell
   and read every key themselves — and so does an agent or a REPL that never leaves the
   primary screen and is read by the very same keys. What those panes have in common is not a
   screen buffer, it is that there is no field on them: `pane_surface::of` takes the composer
   away on the alt screen, on an overflowing block, and once a command has been running longer
   than `LONG_RUNNING`, and every one of those is "the program is reading the keyboard now".
   So the test is whether a composer was attached, which is the same question asked once and
   the reason `Down` reaches a menu inside a long-running program instead of walking the
   field's history. Rule 1 sits above this one: what vim has drawn is still text somebody
   dragged a pointer across, and every other key on that screen is still the program's.
3. **The signal keys reach the shell.** `ctrl-c` interrupts — and throws the half-written line
   away with it, because that is what the gesture means — `ctrl-z` suspends, and `ctrl-d` ends
   the input, but only when the field is empty. The field now holds the line the shell's own
   reader used to hold, so an unconditional `ctrl-d` would be an end of file every time; over
   a written line it is the delete-forward it is in every line editor. A modal menu over the
   window takes the typing away and leaves these three, because a running command has to stay
   interruptible.
4. **Everything else on the normal screen is the field's**, and a key the keymap has no
   meaning for does nothing rather than leaking into the shell.

**Which chords are Crook's** is `app/src/keybindings.rs`, and it is VSCode's model whole: a
binding is `{ key, command, when }`, the shipped table and a person's `keybindings.json` are
the same kind of thing in the same list, the last rule that matches wins, a `-` in front of a
command takes it off a chord, and a key may be a sequence like `ctrl+k ctrl+s`. A command is
an action name — the window's own are `crook/window/*`, registered by a plugin like
anything else — so nothing enumerates the bindable set and the settings page lists commands it
has never heard of.

**A chord is recorded rather than typed into a file.** The Keyboard Shortcuts page is VSCode's
editor: clicking a row's chord starts a recording, and `Workspace::action_for` hands every
keystroke to it before anything else in the window can want it — which is what makes a chord a
pane, a panel or another binding would otherwise take recordable at all. Enter keeps it,
Escape leaves the binding alone, and keeping it writes VSCode's two lines into
`keybindings.json`: the command taken off every chord it had, then the chord that was pressed.
The write is a *text* edit — `keybindings::document` finds the span of each entry and inserts
or cuts one — so a comment somebody wrote survives a button being clicked in a settings page.
The recorder ends with the page: leaving the settings section or changing page cancels it, and
`--record <command>` opens the page with a row already recording, for a picture of it.

The danger of keeping that table away from `route` is real: a binding consumed in the delegate
never reaches `route`, so two tables in two modules can silently take the same key away from
each other. What answers it is a test rather than proximity —
`no_shipped_binding_takes_a_chord_the_input_field_needs` routes every shipped binding through
`input_keys` and insists the field has no use for it.

The two platforms differ, and not by taste:

- **macOS** puts Crook's chords on Command, where a Mac application's chords live and where
  nothing the field wants can be. Tab selection is `cmd-alt-left/right` rather than
  `cmd-shift-left/right`, because the latter is how every macOS text field selects to the end
  of a line.
- **Linux and Windows** put them on Control-**Shift**. A bare `ctrl-letter` belongs to the
  tty: `ctrl-c` interrupts, `ctrl-d` ends input, `ctrl-w` erases a word. Tab selection is
  `ctrl+pageup/pagedown`, which leaves `ctrl-shift-left/right` to the field, where it selects
  by word. Copy and undo are `ctrl-shift-c` and `ctrl-shift-z` for the same reason every
  terminal emulator on Linux arrived at.
- **`ctrl-tab` and `ctrl-shift-tab` step through the tabs on both**, and are the one chord the
  two tables spell identically. They can be: Tab is already a control code, so `ctrl-Tab` has
  never had a spelling a terminal could send, and the only program that can hear it is one
  that turned the kitty keyboard protocol on — which is a `-crook/window/next-tab` line away
  from having it back.

- **The two families that are not on either.** Tab-by-position is `cmd-1`…`cmd-9` on macOS
  and `alt-1`…`alt-9` off it — not `ctrl-shift-<digit>`, because `control_code` folds
  `ctrl-shift-3`, `+4` and `+8` to ESC, FS and DEL, so that family would take Escape away from
  vim. Pane focus is `ctrl-shift-<arrow>` on macOS and `alt-<arrow>` off it: each platform
  takes the arrow modifier the other spends on text, since `word_chord` is Alt on macOS and
  Ctrl everywhere else.

The platform is a parameter of the keymap rather than a `cfg!` inside it, so both halves are
tested on either machine.

**Most commands ship with no chord, and that is the arrangement.** A shipped chord is a key
taken away from the shell in every pane forever, so the table spends one only on what somebody
arrives expecting — which is the list `MUST_HAVE_A_CHORD` names in `keybindings_tests.rs`, and
what a test holds the shipped table to. Everything else is reached by *name*: `register_command`
puts it in the palette and on the Keyboard Shortcuts page, where a person binds it to whatever
they like. Two tests hold the other end of that promise — every command in the window's table
resolves to a binding, and no two of them share a name — because a command that is neither
bound nor registered is one no keyboard can reach at all.

**A chord that cannot act declines.** `Workspace::command` returns `None` — for a direction the
split has no pane in, a tab index past the end of the strip, a block command at a prompt with
nothing selected — and `action_for` then returns `None`, so the keystroke goes on to the
element under it and from there to the shell. That is what lets Crook bind `ctrl-shift-up`
without `ctrl-shift-up` ceasing to mean anything in vim.

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

Key *releases*, which is the one kitty flag that is carried and not acted on, because a
release never reaches `crook_terminal`.

**IME composition is in**, and it lives in the composer rather than in the emulator, which is
where it belongs: the field is what a line is typed into. `winit` is told
`set_ime_allowed(true)` — without which the platform never starts a composition and the keys
that would have begun one arrive as themselves — and its four `Ime` events become one
`Event::Ime`. The preedit is kept *beside* the editor, in `PaneInput`, never in it: a preedit
is not text, it is replaced wholesale by the next one, and putting it in the editor would put
it in the undo history, in a copy and in a submitted line. Only the drawing composes the two,
and `CommandInput` underlines the result so a half-converted word does not read as a committed
one. A click cannot move the caret while a composition is open, because the offsets a pointer
resolves against are offsets into a line the editor has never seen. The caret's painted
rectangle travels back out to the window through `Proxy::set_ime_area`, so the candidate list
stands beside the text being composed; it has to come from the paint path, because where the
caret is is the result of wrapping the line at the width the field was given.

**The kitty keyboard protocol is in**, in `crook_terminal::input`. Alacritty's `Term` already
maintained the mode stack behind a config flag that was off; turning it on and reading the
five `TermMode` bits is the whole of the plumbing. What the encoder does with them is narrow
on purpose: arrows, function keys and the `CSI n ~` family already carry a modifier parameter,
so the protocol leaves them exactly as they are, and what it replaces is the handful of keys
whose legacy bytes genuinely collide — Escape, Enter, Tab, Backspace, and every Ctrl
combination that folds to a C0 code. That collision is the entire reason the protocol exists:
Ctrl+Enter, Ctrl+Tab and Ctrl+I used to be indistinguishable from Enter, Tab and Tab.

**The numeric keypad is in** too, and it is the one place a *physical* key matters. The
keypad's `5` reports the same logical key as the `5` above the letters, and in application
keypad mode — `DECPAM`, which every full-screen editor sets — they send different bytes. So
`crookui`'s event translation names the keypad apart, `numpad5`, and only when the logical key
agrees a digit was typed: with NumLock off that key *is* End, and naming it otherwise would
send a digit where every terminal sends a cursor movement.

**Mouse reporting is in**, in `crook_terminal::mouse`. A program asks for it with `?1000`,
`?1002` or `?1003` and gets presses, drags or every move; `?1006` switches the encoding to the
SGR form, which is the one with no 223-column limit and the only one that can say which button
came up. `?1007` is separate and on by default, as it is in xterm: it is what turns the wheel
into arrow keys for a pager that never asked for the mouse, which is why `less` and `man`
scroll out of the box. **Shift suspends all of it** — that is the only way to select text out
of a program that has taken the pointer, and copying what `htop` is showing is a thing people
do constantly.

The gesture is classified once, when the button goes down, and remembered in `PaneSelection`
as `Selecting` or `Reporting`. Asking the terminal again on each move would be asking a
question whose answer can change mid-drag, and a program that turned reporting off while a
button was held would leave the release unreported and half a selection dragged out of a
screen nobody selected in.

What it reaches is **the grid, not the block list**. That is where a program which reads the
mouse actually draws: every one of them takes the alternate screen, which is the first rule in
`pane_surface::of`. A program that reads the mouse and stays on the primary screen — `fzf`
with a fixed height is the only common one — gets no reports, and the block list goes on
selecting under it.

**URLs are clickable**, on both surfaces. `crook_terminal::url` finds the link under one cell
of one row, on demand: walking the whole grid every frame to build a table nobody reads would
be work proportional to the screen for an answer about a single cell. Holding the platform's
own chord key — Command on macOS, Control elsewhere — underlines it and makes a click open it;
without the key the pointer goes on selecting, because a terminal where clicking a URL opened a
browser is a terminal you cannot copy a URL out of. `app::browser` hands it to `open`,
`xdg-open` or `cmd /c start`, and checks the scheme against a list first: a program's output is
not trustworthy, and nothing printed into a pane should be able to ask the platform to open a
scheme some application has registered a handler for.

**OSC 8 is not read**, and that is a decision rather than an omission. Carrying a per-cell
hyperlink to the renderer means either putting it on `SnapshotCell` — twelve bytes and `Copy`
precisely so a full screen is one flat allocation — or threading a side table through the
harvest path too, so a link keeps working after its command ends. Doing it on the live grid
alone would be worse than not doing it: a link that dies when the command finishes is a link
nobody can trust. Most OSC 8 links a terminal sees have the URL as their own text, and those
work through the scan above.

OSC 52 clipboard writes are in too: the write reaches the window's one clipboard through
`TerminalUpdate::ClipboardStore`, an empty payload is dropped rather than destroying what
somebody had copied, and the *read* direction stays refused in the emulator, where answering
it would hand any program that can print to a pty the contents of the clipboard.

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

**Pinning and tear-off.** Both convert index arithmetic into range arithmetic. Pinning splits
the tab vector into two implicit regions that every insertion has to clamp against.
Cross-window drag — ghost slots, detached placeholders, collapsed source slots, a drag-preview
window — is the single largest source of complexity in Warp's tab code. Crook has a `Vec` of
tabs, an active id, and an MRU list; the close and hop index fixups are ported verbatim
because that is where tab bugs actually live, and they are unit-tested with no window.

**Tab groups shipped**, and they were the cheapest of the three because the estimate above
named the right two branches and there turned out to be only one more. Membership is one
`Option<GroupId>` on the tab — Warp's shape — and the group holds a name and a fold and no
membership at all, so there is no second ordering to disagree with the vector's. Everything
that could break a group's contiguity is one function, `slot_for`: a target names a group and
a neighbour, and the two are clamped against each other rather than trusted, which is what
lets a drop be computed from a pointer position by a pure function that is allowed to be
approximately right. The drag itself needed no new element in `crookui_core` — a press is
noted, moves past a threshold become a gesture, and the row that was picked up is the one that
reads the boxes every row wrote down during paint. It is Warp's gesture and not a line drawn
in a gap: the row is painted on an overlay layer at the pointer, its slot stays open behind
it, and the strip reorders itself one step per event while the hand is still moving, so
letting go resolves nothing because everything has already happened. Which group a row is in
is answered by the group's own box rather than by the row under the pointer, which is the only
way the gap under a group's last member can mean "out of this group" — the question a list
whose groups are decided row by row cannot answer at all.

Groups exist because worktrees needed them: a checkout opened from a tab has to land somewhere
that says it belongs with that tab, and the answer that was there before was a split.

Vertical tabs turned out to be the cheapest of the four and shipped as the default: they are a
second renderer over the same `TabStrip::rows`, not a second model, so `app/src/workspace/`
holds `tab_bar` and `tabs_panel` as mutually exclusive halves and `row_content` holds the one
copy of Warp's which-fact-goes-on-which-line table that both read. What it cost that the
estimate above did not name was the window-control reservation — with a panel down the left
edge the top-left corner belongs to the panel rather than to the header, so
`platform_insets::TabsPlacement` divides one answer between two elements — and, for as long as
`crookui_core` had no scrollable element, a ceiling: the list was `Clipped`, roughly nine tabs
fitted a 640px window, and the rest were drawn, clipped away and unclickable. The settings
page needed a `Scrollable` anyway, so the panel got one too and the ceiling is gone. Auto-scroll
came after it: every row records where it was drawn, and selecting a tab by any means scrolls
the panel until that row is in view — except under a hand that is carrying one, where the
row's slot is wherever the last step of the drag put it.

**The rest of shell integration.** Crook now installs the four OSC 133 marks into the zsh,
bash and fish it starts (`app/src/shell_integration`, and "Blocks" in §7), which is the half
that says where a command starts and ends. Warp's channel does more than that, and the two
things still missing are worth naming rather than discovering:

- **Completion is in**, and it took the second channel this paragraph used to ask for. OSC 133
  is an announcement — the shell says where a prompt began and how a command ended — and
  completion is a *question*, so there is a second protocol beside it, in `app/src/completion.rs`
  and the three snippets.

  The question is a **file**: Crook writes the line up to the caret into the session's own
  scratch and sends `ESC [ 6339 ~`, a key the snippet has bound. The answer is a file too, and
  the `ESC ] 6339 ; n BEL` that says it is ready carries nothing but the request's number. A
  command line can hold a semicolon, a newline and bytes that are not UTF-8, and escaping every
  one of them past a shell *and* past an OSC parser — twice, on the way back — is a protocol
  nobody should have to debug, in a shell that may have no `base64` to do it with. The number is
  what makes a stale answer discardable: pressing Tab twice quickly leaves two outstanding, and
  only the second is about the line on screen.

  What each shell can answer differs, and the difference is the shells'. **fish** answers with
  `complete -C`, which is the real question and every `complete` definition it has. **bash**
  answers with `compgen`: its own command, file and variable completion, but *not* the `_git`
  and `_docker` functions `bash-completion` installs — driving one means setting `COMP_WORDS`,
  `COMP_CWORD`, `COMP_LINE` and `COMP_POINT` by hand and calling a function whose name has to be
  dug out of `complete -p`, and getting any of it wrong runs somebody's completion script
  against a line it was never given. **zsh** is the weakest: its completion system runs inside a
  ZLE widget and reports through `compstate` rather than returning anything, so there is nothing
  to ask from outside one, and what the snippet offers is commands, files and variables out of
  zsh's own hashes and globs.

  What Crook does with an answer is what every shell's Tab does: one candidate is inserted
  whole, several insert as much as they agree on, and an answer that adds nothing is listed
  under the field instead. A list rather than a menu — a menu with a selection in it would want
  the arrow keys, which the field spends on its history.
- **A password prompt is composed in the clear — in one remaining case.** `sudo`, `ssh` and
  `read -s` turn echo off and read a line on the *normal* screen. With marks this is now
  handled by the rule that hides the composer: the prompt happens while a command is running,
  so the field is gone and the keys go to the pty with echo off, where they belong. The hole
  left is a shell that reports no marks at all *and* has not yet filled a screen — the only
  state where a field is on screen and nothing knows a command is running. The obvious signal
  does not close it: `zsh`'s line editor and `bash`'s readline both keep `ECHO` off at their
  own prompt, so "the tty is not echoing" is true nearly all the time and cannot tell a
  password prompt from a shell waiting for a command. The field's history is at least in
  memory only, per pane, and dies with the pane.

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
terminal cells against a `CellFont` and reads a pane's `TextInput`. `crookui_core` still has
no text field of its own, so the gap here is an element, not a model.

**A theme *system* the size of Warp's.** Themes themselves are in — see below — but Warp's
appearance layer is a great deal more than a palette: gradient fills for background, accent and
cursor; background images with an opacity ramp; a `details` block of ten opacity knobs; a theme
creator that k-means five colours out of a photograph; importers for Alacritty and iTerm
configs; and a filesystem watcher that hot-reloads the themes directory. Crook reads the same
file format and ignores every one of those fields rather than refusing a file that carries
them, which is the property that matters: a theme written for Warp loads here.

**OS sync is in**, and it is Warp's design copied exactly: the active theme is a pure function
of a `use_system_theme` flag, an explicit `{light, dark}` pair of theme names, and the OS mode.
That is `Settings::theme_for`, which needs no per-theme pairing metadata — nothing has to
declare itself light or dark, and either theme can be either half of the pair — and which is
therefore testable with no window and no desktop. The OS mode reaches it as an ordinary
`Event::SystemTheme`: `winit`'s `ThemeChanged`, plus one query when the window opens, because
winit only ever reports a *change* and an application that waited for one would open in the
wrong half. A desktop that will not answer is taken as dark, which is what a terminal has
always been. Choosing a theme while following writes only the half in force, so the other half
stays whatever somebody chose for it.

**Hot reload is in, as a poll rather than a watcher, and only while the Themes panel is open.**
A filesystem watcher is a dependency, a thread and a per-platform API for a folder that changes
when a person is editing a theme — which is exactly when that panel is open. Closed, it costs
nothing at all: the chain ends at the first tick that finds the panel gone. The re-read happens
on the background pool, and the theme in force is looked up again *by name*, because the
palette in force is the old one and a lookup by palette would find the row it used to be and
conclude nothing had happened.

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
user-remappable keymap. Crook has that layer now, and it is VSCode's rather than Warp's —
`app/src/keybindings.rs`, `keybindings.json`, `when` clauses over a small set of context keys,
chord sequences, removal by name — and VSCode's editor with it: the Keyboard Shortcuts page
records a chord from the keyboard and writes the file. It cost nothing at the handlers because the half worth
keeping was already kept: keyboard and mouse produce the *same* action values, so the layer
sits above every handler and touches none of them.

**Persistence is in**, in `app/src/session.rs`, and it is exactly the shape this paragraph used
to prescribe: snapshot types entirely separate from the live ones, holding a title, a directory
and a share of a split and nothing else — a `Vec<TabSnapshot>` plus an active index and a
window size, `serde_json` to a file beside the settings.

Three decisions in it are worth naming. It is written **on every change rather than on the way
out**, because there is no reliable way out: a window closed by the window manager, a process
killed, a machine that lost power — none of them runs a shutdown path, and a file written only
at exit is missing exactly when somebody wanted it. Every id in a restored strip is **minted
fresh**, so a restored window is indistinguishable from one somebody opened by hand and nothing
keyed by pane id can collide with a previous process's. And a file this build did not write is
**bounded rather than validated**: sixty-four tabs and sixteen panes, because there is no
correct number and a truncated or hand-edited file must not be able to open ten thousand ptys
before the first frame.

What is *not* remembered is the point: no scrollback, no output, no process. A window that
redrew yesterday's output over a shell that had never run any of it would be lying about the
state of the machine. The settings pane is left out too — it is something somebody opened to
change a setting, not work in progress.

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
| The command line | an input field, with shell integration behind it | an input field, with OSC 133 behind it (§7) |
| Blocks | a block per command, with selection, navigation and re-run | a block per command: harvested rows, a virtualising list, hover-to-copy (§7, `docs/blocks.md`) |
| Shell integration | its own bootstrap, a request/response channel | the four OSC 133 marks, injected into zsh, bash and fish |
| Autotracking | `Tracked<T>` dependency capture | explicit `ctx.notify()` |
| Settings | ~800, with a macro DSL and cloud sync | 10, two `serde` structs and a name in one JSON file |
| Themes | 21 built in, gradients, images, a creator, OS sync, hot reload | 13 built in, the same file format, a creator without the image, no OS sync |
| Theme chooser | a 240px docked panel with search and virtualisation | a 248px docked panel, no search, every row built |
| Icons | its own `WarpIcon` and `UiIcon` sets, rendered from SVG | Lucide, vendored as path commands, one distance-field rasterizer — and one drawn mark, filled (§4) |
| Settings UI | a pane, 16 pages, search over ~800 widgets | a pane, 4 pages, search over 30 (§4) |
| Git worktrees | none | a right-click menu on a tab, after herdr's model (§7) |

The through-line: Crook keeps every *architectural* idea from Warp and rejects almost every
*build-system* one. The architecture is what makes a GPU terminal tractable in Rust. The build
system is what a decade of platform-specific product requirements does to a repository, and
Crook has not earned any of those requirements yet.
