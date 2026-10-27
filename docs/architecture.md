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
re-renders that view's subtree only. The expensive part of a VDOM is the diff, and skipping the
retention skips the diff.

Per-view is the mechanism, and Crook barely uses it yet: the workspace is the one view the
window has, so a notify anywhere in it rebuilds the whole window, and what is allowed to notify
is the budget that matters. So the presenter counts. `FrameCounts` says how many frames it has
built and how many times it has rendered each view — counts rather than times, so that a test
can hold the window to a budget and get the same number on every machine and all three
platforms. The first budget is the terminal's, in §7: output streaming into a pane nobody can
see renders the window no times at all.

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

The pointer's shape comes out of the same walk. An element that wants the I-beam, a hand or
a resize arrow over part of itself asks the scene for it as it paints
(`Scene::request_pointer`), and the windowing layer asks `Scene::pointer_at` where the pointer
is on every move and after every new frame, keeping the shape a drag started with until the
button comes up. The answer follows the rule a press does — the topmost layer with anything at
the point decides, and a hit rect painted after a request in its own layer covers it — so the
pointer says what a press there would do: a menu over the output takes the I-beam away for
the same reason it takes the click. Only a frameless window's resize border overrides it,
because the border takes the press before the element tree sees it.

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
The id is `WindowOptions::app_id` — `crook` for every channel, or what `--app-id` named, which is
how a launcher tells one Crook window apart from the rest — and `packaging/linux/crook.desktop`'s
`StartupWMClass` is the same word, which a test holds it to.

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

And not everything on a grid is text, even when it is a character. A font's `│` and `█` are
drawn at the font's own height, and a row is 1.2 times the font size, so in most families a
column of them had a gap at every row — every box an agent draws its interface in was a
dashed outline. The lines and blocks of U+2500 to U+259F, and braille, are therefore not
glyphs at all: `app/src/workspace/box_drawing.rs` draws them as rectangles from the cell's
own edges, snapped to device pixels so one row's line ends exactly where the next one's
begins, which is what every terminal that draws them well does.

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

Each `src/bin/*.rs` names its channel — `Channel::Dev` or `Channel::Stable` — and calls
`crook::run(channel)`. Nothing else. All the code, and all the tests,
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
  rather than on `debug_assertions` means the release-flags job actually exercises it on every
  pull request to `main`.
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

The one thing it adds to the binary is a Linux desktop's: the Linux archive carries `share/`
beside `crook` — `applications/crook.desktop`, `icons/hicolor/<size>/apps/crook.png` out of
`assets/Crook.iconset`, and `metainfo/id.crook.Crook.metainfo.xml` — laid out the way a package
installs them, from `packaging/linux/`, which is also where the AUR package takes its entry. The
entry is a terminal's: it advertises `-e`, `--working-directory`, `--title` and `--app-id` as
`X-TerminalArg*` keys, which is how `xdg-terminal-exec` learns to start one. `script/install
--desktop` copies that tree under `~/.local/share`; the updater replaces `<stem>/crook` and reads
nothing else out of the archive.

The Mac's counterpart is in `Crook.app`'s `Info.plist`, which `script/macos-app` writes and
`--info-plist` prints alone, on any machine, for a test to read: `CFBundleDocumentTypes` for
`public.unix-executable` and Terminal's own `com.apple.terminal.shell-script`, with the role
`Shell` and the rank `Alternate`. Launch Services hands a file only to an application that
declares its type, so the settings' Default terminal button — which makes Crook the shell
handler of `public.unix-executable` — needs them: before them, pressing it left a script
double-clicked in Finder with nowhere to go. AppKit then tells the application's delegate
`application:openURLs:`, which winit's delegate does not have; `crookui` adds it at run time, as
it adds `applicationShouldTerminate:` (`windowing/open_documents.rs`), and posts the paths on
the event loop's proxy. It also registers `NSTreatUnknownArgumentsAsOpen` as `NO`: AppKit would
otherwise send the same request for every loose word of Crook's own command line, which in
`crook -e vim deploy.sh` is `deploy.sh`, run in a tab of its own. The window opens a tab for each
(`Workspace::open_files`), started in the file's folder, with the file typed at the shell's first
prompt — not `exec`'d, as `-e`'s program is, so what a script prints stays on screen when it ends.
A request that launches Crook comes before the loop's first turn and waits in the proxy's queue,
and its tab waits for its shell with the rest.

What `--check-only` does today is the part that earns its keep immediately: it type-checks the
workspace with the release profile and the release feature set. It is one of the five the
`Crook CI` workflow runs on all three platforms, on every pull request to `main` and every push
to it — see CONTRIBUTING.md. Without it, "the shipped feature combination does not compile" is
discovered at release time.

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

**The pty is behind a link.** `Terminal` holds a `Box<dyn PtyLink>` rather than a `Pty`: a
reader, a writer, a resize, `try_wait`, `wait`, `kill` and a process id, which is everything
`Terminal` and `terminal_model` ask of a child and nothing more, and `Pty` is the
implementation whose child runs on a pty this process opened. What the seam buys today is a
session with no process in it — `terminal_tests.rs` drives a prompt, a submit, output, a
query's answer, a resize and an exit through an in-memory link — and what it leaves room for
is a far end that is not this process's own, without deciding there will be one. Two pieces
go with it that the app does not use yet. `Terminal::feed_mirrored` draws a stream without
answering the queries in it, for a stream some other emulator is already answering, where a
second answer would reach the child as typed input; a terminal fed that way once never
answers again. And the stream it takes is made of `Fed` items, so that the one boundary
`Terminal::submit` hands the emulator out of band can travel with the output as an item of
its own. It is not spliced into the child's bytes, because a submit falls between two reads
and a read can end inside an escape sequence: bytes spliced in there would end the sequence.
The items are not all a replay has to keep: it makes the blocks the stream's own emulator made
only when each read is fed as the item it was, and at the pace the reads came. The pace,
because a synchronized update the child never ended is let go by the clock, at the first feed
or paint 150 ms after it opened, so a replay that runs ahead of that places the marks after it
against a screen that has not drawn the update. The reads, because each one's arrival is a
moment that clock is looked at, and the end of one is where the first row of a block is looked
for again after `clear`. Those are the conditions known to matter, not a list proven complete.
The link is a trait object rather than a type parameter because no keystroke passes through it:
a write goes through the writer the link handed out once and output through the reader it
handed out once, both of them boxed trait objects before the link existed, so the box costs one
indirect call per resize and per child poll.

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

`crook --shell <path>` replaces that answer for every pane of one run, and it is the only
thing that does: there is no setting, because the resolution above is the right one and a
setting would be a second copy of `$SHELL` to fall out of date. The path goes through the same
`Shell::of`, so stubs are written for what will actually run; `Settings → Shell` reads the
terminal model's answer rather than the environment's, so the page cannot say "zsh, marks
installed" over panes running `/bin/sh` with none; and a path that does not exist is the one
way to a pane whose shell could not be started, which is the notice that carries a reason and
had never been on screen on purpose before the flag existed.

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

**Where the scratch lives, and who can read it.** Every pane's directory — the stubs, and the
`complete.in` that holds the command line as far as the caret — sits in one root that is this
user's alone: `$XDG_RUNTIME_DIR/crook` when the runtime directory is set, absolute, and a real
directory the user owns; otherwise `crook-<uid>` in the temporary directory; on Windows,
`crook-shell-integration` in a temporary directory that is per user already. It used to be one
`crook-shell-integration` in `/tmp` for everybody, made `0755` by whoever came first, so any
user on the machine could read what another had half typed, or make the directory before them
and own it. Now the root is made `0700` in one step (`DirBuilder` with its mode, never made and
then narrowed), and one that is already there is checked with `lstat` — a directory, not a link,
owned by this uid, no group or other bits — and refused otherwise. A refused root is what an
unwritable temporary directory always was: that pane runs without marks, and one line in the
log says why. Inside it, directories are `0700` and every file Crook writes is `0600`, written under
another name and renamed into place, because the completion request is rewritten on every Tab
while the shell may still be reading the last one. `shell_integration::scratch::Root` is the
checked root, and it is the only thing the sweep of dead sessions' directories accepts: the
sweep deletes, and a root it had not checked could be a link to anywhere.

**What the child is told beside that.** `TERM=xterm-256color` and `COLORTERM=truecolor`,
because `alacritty_terminal` implements those sequences and the entry is in every terminfo
database old enough to matter. `LINES` and `COLUMNS` are *removed* rather than set: whatever
started Crook may have had them, they described its window, and the kernel's `winsize` — which
the pty is opened at before the child exists — is the truth. `TERM_PROGRAM=Crook` and
`TERM_PROGRAM_VERSION` come from `shell_integration` and reach every shell, marks or no marks,
and so does `CROOK_PANE_ID`, the pane's bare number — the same one a plugin is handed — set by
`shell_integration::Session` because the session is the one thing that is per pane where the
launch planners are per shell. It is what `WEZTERM_PANE` and `KITTY_WINDOW_ID` are: a fact a
script or an agent gates on or names a log after, not an address anything routes by — the agent
status channel writes to its own tty and needs no pane id. The one address a pane is given is
`CROOK_SOCKET`, where its window answers questions — see "The window answers" below — and it is
set empty when the window has no socket, so a Crook started inside another Crook's pane does not
hand its shells the outer one's. Beside it is the one secret, `CROOK_TOKEN`, which is how a
request says which pane it comes from, set empty on the same terms. `CROOK_BIN`, the absolute
path of the running binary, is set in the same place, read once at the first pane: a hook runs
in whatever `PATH` its agent was started with, where a Crook.app nobody linked is not, and on
Linux a binary an upgrade has replaced reads its own path back as `… (deleted)`. The same binary
is what a person types `crook` for, so when no `crook` answers on a pane's `PATH`, its directory
is appended there — appended, so it stands in front of nothing, and only then, since behind a
`crook` of the person's own it would change nothing. Every shell is handed the appended `PATH` in
its environment, the one thing a startup file that execs another shell or tmux passes on, and
macOS's `path_helper` — it rebuilds `PATH` from `/etc/paths` and keeps what it was handed after
that — keeps it too; but not when the directory is a `bin` inside the home, which is the person's
own files' to place: one that adds a directory only when it is missing (uv's `~/.local/bin/env`,
rustup's `~/.cargo/env`) would find it there, at the end, and leave it behind `/usr/bin`. A shell
with the integration gets it again from its snippet, after the person's startup files have run —
fish's at the first prompt, since `vendor_conf.d` is read before `config.fish` — so a file that
assigns `PATH` outright cannot drop it. In bash and zsh that line sits above the snippet's guard,
which a copy pasted into the person's own rc file before the line existed sets first. Not on
Windows: `portable-pty` builds a Windows child's `PATH` out of the registry rather than this
process's, and a `PATH` written from the process's own would replace that.
`CLAUDE_CODE_FORCE_STRIKETHROUGH=1` is set beside it: Claude Code strikes `~~deleted~~`
text through only in terminals it names, and prints the tildes in the rest. A program that
asks the terminal rather than its environment, with XTVERSION (`CSI > q`), is told the same
name and version, as `Crook <version>`, which reaches it through ssh and sudo where the
variables do not. `vte` has no hook for that question, so the emulator's OSC watcher stops on
it and answers it between the answers `Term` gives to the queries either side. Claude Code asks
whether synchronized output is supported only once it has been answered, and until then draws
frames a pane can paint half done when a read ends in the middle of one.

**The pty opens at the pane's size.** A shell prints its whole startup — a `~/.zprofile`
banner, a greeting sized with `tput cols` — before any resize can reach it, and what it
printed is copied out of the grid into a block the moment the first prompt mark arrives, so a
later resize cannot reflow it. So the window's shells open *after* its first frame rather than
before: the frame lays every pane out, a pane with no terminal yet records the grid it holds
(`PaneMeasurer` in `app/src/workspace/body.rs`, into `terminal_model::Measured`), and
the pty starts at that, read by `TerminalModel::launch`. `Workspace::expect_terminals` is said before the
frame so that the pane draws its ground rather than a notice for that one frame, and
`start_terminals` runs from the window's `frame_drawn`. A pane no layout will see before its
shell opens — one in a tab behind the active one, or one a split just made, whose layout comes
a frame after the split — is given the grid the layout *would* give it, worked out from the box
the active tab's panes share (`BodyMeasurer`) and the tab's own weights (`body::estimated_grid`,
the same arithmetic as the layout's). `INITIAL_GRID` — 80×24 — is left for a shell started
before any frame at all.

### Who drives it

`app/src/terminal_model.rs`, in the shape `git_model` established and `usage_model` shared
before it left for a plugin: work off the UI thread, delivered on it, and a word to the window
only when something it draws could have changed. A changed grid is not a `notify` but a
`TerminalUpdate::Repainted`, which the workspace weighs (below); the model's own `notify` is
kept for the one-offs, a palette change and a shell that opened in place or failed to open — one
opened behind the active tab arrives as a `Repainted` too. Each
terminal gets an OS thread of its own rather than a background-pool worker, because a pty read
blocks for as long as the shell is quiet and the pool is sized for exactly the chains that park
on timers — five of them, counted one by one in `PARKED_WORKERS`, and a pane is not one.

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

**And a frame is spent only on a pane somebody can see.** The workspace answers a
`TerminalUpdate::Repainted` with a frame only when the pane is on screen — in the active tab,
not hidden by a zoom, with no section of the sidebar over the body — in a window in sight
(`Workspace::pane_can_be_seen`). It asks at the moment the repaint arrives, from its own
state, rather than keeping a set of visible panes pushed into the model: what decides it
changes in a tab switch, a split, a zoom, a close, a restored session
and a plugin switched off under the section it was showing, and a push missed in any one of
them would freeze a pane somebody is looking at. A pane nobody can see goes on being read,
parsed and published, and everything its *row* shows — a title, a directory, a bell, an agent's
status, a command finishing — still arrives as it happens, because the row is on screen. The
frame that brings the pane back reads a current snapshot, not the last one drawn: the one
published last, or one built then when no publication built it (below). Ten thousand lines
printed in a background tab used to rebuild the window six to eight times on an idle laptop;
`the_frame_budget` in `app/src/workspace/tests.rs` holds it to none. It holds the
row to its changes as well: a report that leaves the session as it was — Claude Code's glyph
turning in front of the same name about once a second, its hooks saying running again around
every tool — asks for no frame of its own, because `Workspace::update_session` returns before
its notify when `AgentSession::says_the_same_as` says nothing changed. Its own frame is all that
goes. Both reports come only while an agent runs, and while one runs its mark breathes:
`Workspace::keep_breathing` redraws the window every `BREATH_FRAME`, 80 ms, for as long as a
running mark is inside the tab list's viewport. With the list up — the default — and the
agent's row in view, an agent at work in a tab nobody looks at still draws the window about
twelve times a second; it goes quiet with the list hidden, a settings page over it, the row
scrolled out of view, motion kept still, or the window out of sight (see "Out of sight is
another bit"). Most of the budget's tests run in a harness that never lets the marks breathe,
and the one that does hides the list. The gate sees the window as well, as far as the desktop
says: a window that has said it is minimised or covered (`Event::WindowOccluded`) draws no
pane's output at all, and asks for the frame that draws every pane current when it says it is
back; `ten_thousand_lines_in_a_window_out_of_sight_draw_nothing` holds it to no frame. That
bit is the desktop's word rather than the workspace's own state, and costs something when the
desktop is wrong: a window said to be out of sight while somebody can see it draws a pane's
output with the next frame anything else asks for — a key, a click, the focus moving, a row
changing — rather than as it comes. The one way it is known to be wrong, an X11 answer that
lags a restore, crookui puts right by itself, at once for a window given the keyboard and
within a second for one that was not (see "Out of sight is another bit").

**Nor does a pane nobody can see build snapshots.** A snapshot is every cell of the viewport
resolved through the palette and compared with the last — about 160 µs for a 200 by 50 grid, at
up to the repaint ceiling — and a window of agents can have dozens of panes printing behind the
one in front. So the workspace's render, which is where the answer is drawn, also hands it to
the model (`TerminalModel::draw_only`), and a publication for a pane no frame draws hands over
its block list, its open block and its events with no snapshot built. The open block is all the
model reads of a publication — whether a command is running, whether the shell has reached its
prompt — save the grid's text for a shell with no marks until its first prompt, which it asks
for. The snapshot is built when something asks for it — the frame that brings the pane back, a
find, a copy, the control socket — under the emulator's mutex, from what the emulator holds by
then, with the block list synced beside it. That is what makes this push safe where the one
above would not be: it decides where a snapshot is built and never which one is drawn, and a
pane wrongly said to be off screen is still reported repainted, and drawn current by the frame
that asks for its snapshot. In a window out of sight no frame draws any pane, and that is the
one change no frame need follow, so `Workspace::set_window_occluded` says it to the model as it
hears it, and says the panes on screen again when the window is back, before the frame that
draws them. A shell that opened since the last frame — a restore's arrival off the pool, a tab
opened over the control socket — is drawn until told otherwise, so a repaint reported for a
pane nobody can see says it of that pane as well (`TerminalModel::not_drawn`). Nor does a
repaint ask an open find bar's questions of a pane nobody can see: they are asked of the
snapshot, which the UI thread would otherwise build for every publication. A question asked a
moment before the pane went is still answered off the UI thread, after the read's pause, from
the grid as printed since; that answer is dropped when it comes back (`PaneFind::drop_grid`),
and the pane coming back on screen asks again.

**Nothing writes to the pty on the UI thread or under the emulator's mutex.** A write waits
for the child to read, a child that is printing reads nothing until its output has been read,
and the reader needs the mutex to parse that output — so a paste that held the mutex while it
wrote into a `cat` that had stopped reading waited on the reader, which waited on the paste,
and the window froze for good at about eighteen kilobytes; the reader answering a program that
asked questions faster than it read the answers wedged itself the same way. Each session
therefore has a writer thread as well, which owns the pty's writer
(`crook_terminal::PtyWriter`, handed out by `Terminal::take_writer`) and drains one queue:
keys, pastes, submitted lines and the emulator's answers are copied onto it under the mutex,
in the order the mutex was taken, which is the order they used to be written in. A keystroke
costs a copy and a wake. A person's input is kept whole, behind a 64 MiB backlog past which
more is refused; answers are dropped oldest first past 64 KiB, since a program that is not
reading can use none of them. On Unix the master is non-blocking and both halves wait in
`poll`, the writer on a pipe the `Pty` closes as well, because a blocking write on Linux can
stay asleep after its child has been killed and reaped — and the writer thread must end when
its pane does. It does, save for a write that began to wait in a process short of descriptors:
out of them, so the pipe could not be made, or on macOS past what `select` can watch. That
wait is on the master alone, and lasts as long as an orphan such as a `sleep 60 &` holds the
pty open.

**Nothing ends a child on the UI thread either.** `portable-pty`'s kill hangs the child up and
looks at once, before any child can have gone, then every 50 ms until it sends `SIGKILL` at
200: on the thread that draws, a close cost 50 ms, or 200 for a shell that ignores the hang-up,
one pane after another — "Close other tabs" on fifteen froze the window for most of a second.
A closed pane leaves the model at once, and its shell is killed and reaped on a thread of its
own (`Endings` in `terminal_model.rs`), under the emulator's mutex as before, which is what
keeps a kill from signalling a process id the system has handed out again. Those threads die
with the process, so the window's delegate ends every shell as the event loop stops and waits
for all of them at once, a second at most: macOS's Quit ends the process there, without
dropping the model whose `Drop` would do the same.

**Nor does the UI thread start a shell nobody is about to see.** `portable-pty` sets a child up
in a `pre_exec`, so std forks and the parent waits until the child has exec'd: a millisecond on
an idle machine, and up to a quarter of a second each while a restore's earlier shells are
starting beside it. A restore of sixty tabs started every one in place and held the window for
most of a second after its first frame. `TerminalModel::start` starts only the active tab's
shells in place, for the frame after it, and queues the rest for the background pool — one at
a time in the strip's order, each arrival sending the next, so a restore holds one worker
however many tabs it brings back. The integration's files, the spawn and the reader and writer
threads are made on the worker, and the session comes home through `ctx.spawn` as a
`Repainted`, a frame only if its pane is on screen. Until then the pane's shell is on its way
(`is_opening`), drawn as its ground; a pane that closes meanwhile has its shell ended by
whichever side holds it (`Opening`), and a window that goes while one is being spawned waits
for it with the rest. A new tab or a split still starts in place: one shell, with a frame
waiting to draw it. So does a tab switched to before its turn came (`open_shown`, from the
workspace's `sync_terminals`): a pane with no shell draws no composer and no grid, and what was
typed into it while it waited would reach nothing. Only the one shell already on the pool is
left to arrive.

**On macOS the writer never lets unread input wrap the kernel's ring.** XNU keeps a child's
unread input in a ring of 1024 bytes, and when the child turns canonical mode on, the kernel
feeds what is unread through the line discipline again by reading the ring into itself:
whatever had wrapped around its end is overwritten before it is read. A shell running a pasted
block switches to canonical mode between every two lines, and a writer that kept the ring full
had it wrapped nearly always — a 730-line block submitted to the system `bash` ran with lines
mangled and missing every time, and in a third of the runs the rest of the block was thrown
away past the kernel's line limit. A ring starts again at its first byte once it empties, so
the writer counts what it has written since `FIONREAD` on the master last said the child had
read everything. A write that keeps the count within 1022 bytes goes at once — keys typed ahead
of a busy program, a short paste — and one that does not waits, in a `kqueue`, for the child to
read it all, and then goes at most 1022 bytes at a time, ending at a line end when it can,
since the count leaves out a line a program in canonical mode has not finished. The rest of a
line longer than that waits for the program to read it, a second at most: a shell back in its
line editor reads it at once, and a program that stays in canonical mode never can, so that
line is given up on until the next line end. The keys the line discipline acts on and never
queues — `Ctrl-C`, `Ctrl-\`, `Ctrl-Z`, `Ctrl-S` and `Ctrl-Q`, and `Ctrl-T` in canonical mode,
or whatever the terminal's modes make them — never wait on the count, since they are how a
person stops a program that has stopped reading. One pressed behind input the writer is
already holding still waits for that input, as it waited behind the kernel's full queue
before. Neither they nor the other keys the line discipline takes in place — `Ctrl-V` and
`Ctrl-O`, and in canonical mode `Backspace`, `Ctrl-U`, `Ctrl-W` and `Ctrl-R` — add to the
count, so a line typed ahead of a busy program and corrected as it goes does not fill it. See
`crates/crook_terminal/src/typeahead.rs`.

The emulator's mutex is never held across a frame, or across a write. The reader takes it to
parse, and again to build a snapshot, and publishes the `Arc` into a slot of its own before it
lets go; painting clones that `Arc` and walks owned data. The UI thread publishes too, after a
keystroke or a resize, and so does whoever asks for the snapshot of a pane that was published
off screen without one. Every writer keeps the mutex until its snapshot is in the slot: one
that installed it after letting go could be overtaken in the gap and put an older snapshot back
beside a newer block list, which painted a finished command twice. Where the slot and the
mutex are held together the mutex is taken first, and a reader that finds the snapshot missing
lets go of the slot before it takes the mutex to build one, so nothing waits on them the other
way round. What the child asked for in those bytes — a title, a bell, a command's end — is set
aside as it is parsed and handed over by the same publication, once the snapshot and the block
list are in place: handed over at once, a command's end answered `crook pane wait --until
finished` up to a frame before `crook pane blocks` could read the block it ended. Layout —
which asks "did the grid move?" on every single frame — answers from an atomic before it ever
asks for the lock.

### Blocks: the output is a list of commands

A pane draws its output as a list whose items are commands — prompt, command line, output —
and the line being composed sits under that list as a sibling on the same ground. Three pieces
in `crook_terminal` and three in `app/` make it, and the interesting decisions are all in the
seam between them.

**Where a boundary comes from.** Two things, and only one needs the shell. `Terminal::submit`
is what the composer calls on Enter: it records the exact command text and the exact moment
once the line is on its way, before anything the shell prints in answer is parsed, so a block
knows what was run even when it never learns how it ended — and a line refused behind the
backlog opens no block for a command that never ran. The other is OSC 133 — `A` prompt start,
`B` prompt end, `C` output start, `D;<exit>` finished — read out of the *same second `vte`
pass* the emulator already ran for OSC 7, so the mark is applied while the cursor still stands
where the shell left it. That is also why `Emulator::advance` feeds the stream in pieces cut at
the marks: feeding a whole chunk and looking for marks afterwards would place every boundary
wherever the pty read happened to end.

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

- the open block's anchor names a line the grid still has, because the history can now only
  fill up when the open block is longer than the grid's share of the scrollback. Past that the
  emulator cuts the history back itself and counts what it cut, so the anchors of the pictures
  in it still follow their lines (`BlockTracker::trim_history`), and what it cuts is the open
  block's own oldest rows, which it keeps — see the next paragraph. Past the whole scrollback,
  "this block starts above the oldest line we have" is not an approximation but the truth;
- a column change, which reflows every stored line number into meaninglessness, is answered by
  looking for the first line in the grid with anything on it;
- rows *below* the cursor when a command ends — what a progress display that redraws with
  `\e[3A` leaves behind — can be harvested with the block that printed them, because nothing
  else can have put anything there.

**The open block keeps its scrollback the way a finished one does.** The grid itself holds five
hundred lines of history (`GRID_LINES`), or twice the screen's height when that is more, so
that a taller screen finds the rows it pulls back out of the history there; what the cut takes off its old end is spilled, into a
`Spill` beside the open block (`BlockTracker::spilled`): `BlockRows` in pieces of up to 512
rows, so that the oldest go a piece at a time once the scrollback is full, and nothing kept
moves. An agent such as Claude Code is one command for its whole session, and a shell with no
integration one block for its whole life, so without this their scrollback sat in alacritty's
cells — 24 bytes for every column of every row, printed into or not. Measured with ten 200x50
panes each printing ten thousand rows of an agent's session: 46.9 MiB a pane resident with the
grid keeping all of it, 6.3 MiB spilling, the spilled rows at about 110 bytes each (text, a few
runs of colour, three offsets). Alacritty grows its storage a thousand rows at a time, so the
grid's share below that saves nothing more.

Everything that reads the scrollback reads both halves as one: the emulator's view goes on up
past the grid's oldest line into the spilled rows, a snapshot of it materialises them through
the same `materialise` the list paints from, `Terminal::harvest_rows` — what a copy and the
find bar read — numbers them first, and the block takes them when it finishes. An anchor
follows its line over both, since the spilled rows are exactly the lines the cut counted. A
resize keeps a view where it was: a taller screen takes it down by the rows it added, as
alacritty does its own, and a column change keeps the line at its top on top — a row of the
spilled rows tracked through their reflow, or a line of the grid's history found again by
counting lines from its oldest. Under a full-screen program the shell's view stays where it
stood in the spilled rows, across any number of reads and a column change. A column change
lays the spilled rows out again as alacritty reflows its own
(`BlockRows::rewrapped_tracking`), a run at a time for a line with no double-width or zero-width
character, after the grid's oldest rows that go on a line whose head was spilled are moved up
to it — all of them, as far as the history goes; a line of 51 KB at 200 columns costs a column
change 1.03 ms — so that a folded line is not cut at the seam. An erase of the scrollback or a
reset ends the watcher's piece, so the spill is dropped before the lines after it in the same
read are fed. Inside a synchronized update the spill is dropped at the erase while `Term` is
still holding the update, and the two agree again when it is let go of.

A slice of output is cut for a line a byte, and a few escapes — `CSI n S`, an erase of a
screen with something on it, lines deleted at the top of a scroll region — scroll a screenful
in a handful of bytes, so a slice of them can take the history past alacritty's room, and the
lines alacritty evicts then were never spilled. `trim_history` sees that the history is full,
and drops the spill, which no longer ends where the history begins, and takes the open block's
first row to the oldest line, since an anchor read across the uncounted lines names one too
low: what is left has no hole in it, and is the whole scrollback long. A taller screen under a
full-screen program can hide that the history was full, after an update that switched
screens. So the rows taller screens pull out of the shell's history under the program, less
the rows shorter ones push back, are counted, and the history found on the way back is taken
to have been full only when it is short of full by no more than that: then what is left is
short of the scrollback by those rows. A `:!cmd` in an editor that draws in synchronized
updates, and a pane made taller under it, scroll the history nowhere near full and keep all
of it — 400 random sessions of numbered lines, bursts, programs, switches inside updates,
erases and resizes, compared against an emulator keeping everything in its grid, found no
history shorter and no line missing. A screen that grows by more than the grid's share in one resize — from 50 rows to
600 at once — still finds blank rows at its foot where the spilled lines would have been.

Cutting is the cost. A row is harvested where it is cut, straight into the last piece: the
cells after the last one printed into are counted rather than read, and a run of cells
painted alike is resolved once. Output is fed in slices that may take the grid 256 lines past
its share before the cut. Feeding 200,000 log lines in 4 KiB reads (release build): 208 ms
with the grid keeping all of it, 248 ms spilling at 80 columns and 293 ms at 200, where the
blank ends of wide rows are most of what is read. A column change at 12,000 rows of 200
columns: 0.26 ms keeping all, 0.94 ms spilling.

Two things differ from rows in the grid: a spilled row keeps the colours it was cut with when
the theme changes, as a finished block's do, and the blank rows a block's screen opened on
above its first row are not spilled, so they no longer show above it when the view is scrolled
to the top. Alacritty is still allowed the whole scrollback and its headroom, so a synchronized
update let go of at once is spilled rather than evicted uncounted.

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
there is not one yet, which is what keeps one repository's branches together.

The creator asks where the new branch starts — herdr's `--base`, asked as a list with a check
on it under the name rather than as a flag. The tab's own `HEAD` comes first and is picked
when the creator opens, so a person who does not look gets a branch from the same commit
as before there was a question. The repository's default branch comes next, one arrow away:
`origin/HEAD`, else `init.defaultBranch`, else `main` or `master` — each only if the
repository really has it, because the configuration is the machine's. Every other local branch
follows, for work that is meant to stack on another. The question was left out at first, on
the grounds that a menu could not ask it well, and leaving it out answered it wrongly: a tab on
a feature branch cut every worktree made from it from that feature, and an agent's work
stacked on another's without anybody having said so. The default branch is read with the rest
of the menu when it opens, on the background pool. The new branch is made with `--no-track`,
so starting it from `origin/main` does not also make `main` its upstream — and so no new
branch gets an upstream at all, including one made from `HEAD` under
`branch.autoSetupMerge=always` or `inherit`, which used to. A base that begins with a dash
is refused before git sees it, since past `--` git still reads a lone `-` as the branch
checked out before this one; so is a branch name that does, since `git worktree add` hands
the name to `git branch` where `-m` is an option and would rename the tab's own branch.

A new checkout is a fresh one, and what git leaves out of it is exactly what an agent's first
run trips on: the `.env`, the local settings file. So a checkout Crook makes is given what the
main checkout's `.worktreeinclude` names, by Claude Code's rules rather than rules of Crook's
own, because the point is one file in a repository serving the worktrees both tools make:
`.gitignore` syntax, and a file is copied only when a pattern names it *and* git ignores it,
to the same place — a tracked file is already there as the branch has it. git does the
matching, twice, since gitignore's grammar cannot say "ignored by one list and named by
another": `status --ignored=matching` for what is ignored, which lists a wholly ignored
`target/` as one line without walking it, then `ls-files --others --ignored --exclude-from`
for what the file names, with the ignored directories no pattern reaches left out of the walk
— Claude Code's rule for those, under which `**/config.json` does not reach into `vendor/`
and `vendor/**/config.json` does. It runs on the worker that ran `git worktree add`, before
the tab opens, because a shell that reads `.env` as it starts reads it once. Crook adds a rule
of its own: a file the *new* checkout does not ignore — its base branch predates the
`.gitignore` line, or the main checkout's edit to it is uncommitted — is left out, because
there it would be an untracked file an agent's `git add -A` commits; so is one inside a
submodule of the new checkout, which would be the submodule's file and a directory
`git submodule update --init` then refuses to clone into. It never follows
a symbolic link, in either checkout, and never overwrites; past a thousand files or 256 MB it
copies nothing — half a `node_modules` a `*` caught is worse than none — and a file over 64
MB is left out alone. None of that can fail the checkout, which exists by then: it is logged,
and what did not arrive is said on the worktree menu reopened on the new tab, since the
creator has closed. A copy that went as asked says nothing, because a menu that came back
after every checkout would take the keyboard from the tab a person had just asked for. A
checkout `crook tab new --worktree` asks for is made by the same function, and says what did
not arrive in its answer instead, since that tab never takes the keyboard.

The same creator starts the agent, which is the other half of "one agent, one worktree", and
it is the creator rather than a dialog or a `Task` object of its own for the reason the session
file keeps its groups on its tabs: a second list of the same tabs is a second answer that can
disagree with the first. Under the places to start from it lists the coding agents it found —
the table `crook --agent-hooks` already knows, each looked for as an executable file on `PATH`
(and, for a window the Dock started with launchd's four directories, in Homebrew's prefix,
`/usr/local/bin` and `~/.local/bin`, then in the directories the agents' own installers use —
`~/.opencode/bin`, `~/.bun/bin`, `~/.volta/bin`, `~/.npm-global/bin`, `~/.cargo/bin`, mise's
shims and every Node nvm has, the newest first — where one found only there is typed by its
path, since the pane's shell may not have that directory either), on the background pool with
the repository's reads, never by running one — with "Shell only" first and picked. Picking an
agent brings up a prompt field, and the branch is named after the prompt as it is typed —
`worktree/fix-the-login-bug`, ASCII words cut to forty characters, the suggestion when there
are none — for as long as the name field still says what the creator put in it. When the
checkout has opened in its tab, the agent's line goes into that tab's composer:
`claude 'fix the login bug'`, `gemini -i '…'`, the prompt put on one line, stripped of leading
dashes the agent would read as its own options, and quoted by the host with the same function
that quotes a plugin's typed argument, so a `$(…)`, a backtick or a quote in a pasted prompt
arrives at the agent as characters. That quoting is
trusted only in the shells it has been proven in — sh, dash, bash, zsh and fish — and nowhere
on Windows yet, where a PowerShell single-quoted string also ends at a curly quote and an
npm-installed agent is a `.cmd` file that cmd.exe reads again; there the line is the agent's
name alone and no prompt is asked for. "New task…" in the palette, `crook/worktrees/new-task`,
is this creator opened with the first agent picked and the keyboard in the prompt; the tab's
own "New worktree…" opens it on "Shell only", which makes exactly what it made before and types
nothing.

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
A report that names another machine — the URL's host, which the snippets fill in with the
name the system gave when the shell started, and which is the far side of an `ssh` when it is
not this machine's — takes the session's directory away rather than moving it: the path is a
directory over there, and everything that reads a session's directory, the worktree menu
included, reads this machine's disk. The row prints the host and the path instead; see
`app/src/tab/remote.rs`. A block keeps the host it was filed under beside its directory, since
one pane's blocks can come from both ends of an `ssh`, and a block from the far end is read for
no branch. No path in it is a link, and none on the grid or in the open block of a pane over
there, relative or not, since each names a file on that machine; see `app/src/pane_link.rs`.
The agent's line keeps the same rule: it is put in the new tab's *composer*, unsent, where a
person reads a line before pressing Enter on it, and it reaches the shell only because somebody
pressed — Enter there, or the creator's Start, which is a press for exactly that. Enter in the
creator is Create and never Start, since Enter is the key at the end of a sentence and not a
decision to run a line nobody has read; and a Start followed by Cancel while git is still
checking out leaves the line unsent, even when the creator is opened again before git answers
— the press is tied to the question it was made in, and a second creator is a second question.
"New task…" does not open over a menu that is still waiting on git or asking something else,
because the answer on its way would land on a creator that never asked for it.

**Removal asks, and asks about the right thing.** `git worktree remove` refuses over modified
and untracked files — and, measured rather than assumed, *not* over ignored ones, which it
deletes without a word. So the ignored count is the one most worth showing before the button,
and it is the one git will never raise on its own. A checkout that is the main worktree, that a
tab is open in, or that somebody else has locked is not offered for removal at all.

**A checkout Crook makes is locked while the window that made it is working in it.** Right
after `git worktree add`, and before the tab opens, it runs `git worktree lock --reason "crook:
<branch>"`, and the window remembers that it took that lock. It takes it off again the moment no
pane in the window is working in the checkout — the last one there closes, or its shell `cd`s
out — and whatever it still holds when the window closes, since closing a window closes no pane:
those come off as the event loop stops, whatever stopped it — the last tab, the title bar's ×,
the window manager, macOS's Quit — with a few seconds' patience for a git that hangs. A checkout
git is still making as the window closes is locked only once it is made, which is after all of
that, so the creation takes its own lock straight back off. Opening the checkout again later, from
the list or from a restored session, does not lock it again: the lock covers the stretch from
making a checkout to leaving it, which is the agent it was made for. The lock is what tells
everything outside Crook that touches the repository —
`git worktree remove` typed by hand, `git worktree prune`, another agent's tidy-up — that an
agent is in there, and it is what stops one of them taking the checkout from under it. A lock
does not record which Crook took it, which is why a window takes off only the ones it remembers:
a pane of this window passing through a second window's checkout and closing leaves that
window's lock alone. The worktree menu tells Crook's lock from anybody else's by the `crook: `
prefix alone, and it treats the two differently: somebody else's lock (Claude Code's `claude
session …`, a person's own) is never taken off and keeps its checkout out of the × and the
sweep; Crook's own lock on a checkout no pane in the window is working in is taken for one a
crashed or killed Crook left behind, so that checkout is free, and removing it takes the lock off
first. That reading has one blind spot, which the menu had before there was a lock at all: a
window knows only its own panes, so a second Crook window's checkout looks the same as an
abandoned one. A removal is not asked of git for a checkout a pane of *this* window is in,
and the lock is not what stops it: a checkout opened again from the list or a restored
session, or `cd`ed into, has a pane in it and no lock, so git would have nothing to refuse
over. The × and each step of a sweep ask where the panes are at the moment they would start —
the sweep runs on after its popup closes, for minutes when there is a build in each
checkout — and compare with links resolved, on the background pool, because a shell reports
the directory it `cd`ed into spelled through whatever link it took, and git lists the real
path. A checkout a pane has entered since is left standing, and the list says it is in use.
The list itself compares spellings, so it can still offer a checkout a pane is in through a
link; the removal is what leaves it. The lock is the second line: a removal never takes off a
lock the window holds for a pane of its own, which covers a checkout this window has just made
and whose tab has not opened yet, and, while the lock is on, a pane that arrives between that
question and git. All of it — the lock, reading whose a lock is, the
unlock — runs on the background pool like the rest of the menu's git, except the unlocks at
exit, which have no frame left to hold up.

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

**Work that has landed is proved, never assumed.** A removal keeps the branch, and git cannot
see a squash merge, so every finished task used to leave a branch `git branch --no-merged`
lists for ever — this repository had eighteen, and every one had landed. `app/src/git/merged.rs`
proves it from the repository alone, with no network: by ancestry (the tip is on the base), or
by patch (the branch's whole diff since its merge-base has the `patch-id` of one commit that
reached the base after the branch left it, which is what a squash is), found in one
`git log -p | git patch-id --verbatim` pass capped at a thousand commits and ten seconds —
`--verbatim` because plain `--stable` ignores whitespace, and a follow-up commit that only
re-indents a line of Python or YAML is still work the squash does not have; a git older than
2.39, which lacks the flag, proves nothing by patch and only by ancestry. Neither proof counts
for a branch whose own reflog records no commit made on it: a checkout nobody has committed
in — fresh, only brought up to date, or cut from the tip of a branch that was squashed — is
not "merged", whatever `--merged` says. Nor is one that moved off work of its own on the way:
a branch committed on and then reset back onto the base has its tip there and a commit in its
reflog, and that commit is on no branch — the reflog is the last thing that remembers it, and
deleting the branch deletes the reflog. So every commit the reflog records as made on the
branch has to be on a branch, tag or remote still, or its change has to be in what the tip or
the base holds: copied by a rebase, replaced by an amend, or the whole branch as it stood then
found as one commit, which is a squash. That change is looked for by the lines added and
removed alone (`--unified=0`), since a rebase onto a base that edited a line near the branch's,
or one whose conflict was settled by keeping both sides, gives a copy new surroundings and the
same lines. The lines only find it: the same edit made to another, identical line of the same
file has them too and leaves the branch's own line as it was, so a match counts only from a
commit that is a copy git made — the same author at the same second, which a rebase and a
cherry-pick keep — or that has the same lines around them, which is all a squash can show. A
conflict settled by rewriting the branch's own lines leaves the branch unproved until `gc`
expires the entry, and a git older than 2.39 accounts for no change at all, only for an amend.
The whole look, its passes and every read after them, shares one more ten-second deadline. A
reflog is not a complete record, though: a month after an amend or a rebase, `gc` expires the
entry of every commit it replaced and leaves only the branch's creation. So a reflog git has
pruned, told by a newest entry that no longer ends at the branch's tip, is left to the proofs,
as is no reflog at all; a rebase less than a month old of commits more than a month old still
goes unproved until its own entry expires. The base is what `origin/HEAD` names, then
`init.defaultBranch` if that branch exists, then `main`, then `master`.
Anything it cannot prove — a stack squashed in pieces, a branch reworked during review, a pass
cut off by its cap — is simply unproved, because a miss costs a badge and a false proof costs
somebody's branch: a branch is deleted on the strength of one, below. The menu asks after the
list is in — about every local branch, not only the checkouts' — marks a proved row `merged`,
and offers `Remove N merged checkouts…`: the free checkouts narrowed to the proved ones, with
the sweep's own safety — looked in first, never forced, the branches kept. On this repository
it proves 14 of those eighteen in a second or two; the four it misses are stacked or were
reworked, and are the ones it should miss. Every removal, sweep and Finish opens the menu again,
and proving sixty branches again each time came to three seconds and more, so the menu keeps
what it was told (`merged::Proofs`): per repository against the base and its tip, and per branch
against its tip and its reflog's size and time — the reflog because a branch committed on and
reset back to the very commit it was proved at holds work only that reflog remembers. One
`for-each-ref` shows what moved, and only that is proved; a base that moved proves everything
again. An unproved answer is kept only from a proof none of whose reads ran out of time — killed
at its deadline, or not started because the deadline it shared was spent — since such a proof
may have run out of time rather than evidence; `git::run` counts those per thread, and the proof
runs every read on its own. How long the proof took is not the sign: one over a few hundred
branches outlasts any read's deadline without a read running out. What a proof also leans on —
every other ref, which may be what holds a commit a branch moved off — is not watched, so a
badge can go stale until a tip moves; nothing is deleted on it, because the deletion proves
again from nothing. A pass the menu no longer wants, because it opened again or closed, stops
before the next branch's reads, the next pass over history, or the next branch's look for work
it moved off.

**A branch is deleted only on a proof.** `git::worktree` said from the start that a removal
never deletes the branch — "not as an option, not under `force`" — because a branch is the work
and deleting it was "not a thing a terminal gets to decide". That was right for as long as
nothing could tell a finished branch from an unfinished one. A branch whose work is proved to
be on the base is not where the work is any more, only a name git lists as unmerged for ever,
so the rule now reads *never without a proof*, and a removal on its own still never deletes
one. `worktree::delete_branch` takes a `merged::Landed`, which only `merged()` can make, so
nothing can hand it a bare name; and it does not trust the proof for longer than it takes to
use it. It refuses the base and the local branch a remote base shares its name with, proves the
branch again against the same base, refuses a branch any checkout has out, and reads the tip
last and refuses it unless it is the commit that was proved. It deletes with `git branch -D`,
because a squash-merged branch is never "merged" to git and `-d` would refuse every one of
them — the checks are what `-d` stood in for. Three gestures reach it:

- `Delete N merged branches…`, under the worktree list, offers the proved branches no checkout
  has out — mostly the ones earlier removals left behind — and names each with its evidence:
  the commit on the base with its change and that commit's subject, or its own tip on the
  base. Six to a question, because the face does not scroll and a press deletes only what it
  named; the rest are counted and offered again once those have gone. One pass proves them all
  again, and a branch that moved is kept and named.
- **Finish task**, on a tab in a checkout Crook made, is one confirmation that says what will
  happen: the panes working in the checkout close, the checkout is removed unforced, and the
  branch is deleted if it is proved, or kept with `kept feat/x: not on main yet`. The panes, not
  the tab's group: a worktree opens into the group of the tab it came from, so a group is the
  repository, and closing it would end the main checkout and every task beside this one. It is
  refused, in a sentence naming the pane, while an agent there is running or waiting on an
  answer or a command is open in its shell — and, in a sentence naming it, over a checkout with
  a submodule checked out in it, however clean, or still holding the repository of one that was
  deinitialised or `git rm`ed there, which git removes only when forced. The panes
  close first — a shell sitting in a directory keeps Windows from deleting it — and since the
  menu hung off a row that is now gone, it moves to a tab in the main checkout and says what
  happened under the list. A removal git could only half do, over a file it may not delete,
  has still unregistered the checkout before failing — `worktree::remove` tells it from a
  refusal by the directory still standing and the checkouts listed again without it, never by
  git's sentence, which git translates, and answers `Error::PartlyRemoved` — so the branch
  is dealt with as after a whole removal, on the same proof made again, and the list names the
  folder the rest is in, with `Show the folder` to open it. The × says the same, and a sweep
  counts such a checkout as gone, not as one that would not go. For several, it names the
  folder holding them all only when that is in the store or the one they are each directly
  in; one in the store and one in `~/code` share only the home directory, so the log names
  each instead.
- **Discard task** is the one way to delete a branch nobody proved, and it asks twice. The
  second question names the commits only that branch holds — on no other branch, tag or
  remote, since one that is elsewhere is not lost — then those only its history holds, which
  it was moved off and its reflog still remembers, and the modified or untracked files the
  forced removal throws away, six of each by name and the rest counted, over a button that
  says "for good" and is never Enter's. The branch is deleted only if its tip is still the one
  those commits were counted from. It names the commits only a submodule of the checkout holds
  too, checked out or only kept, and counts its stashes: its repository lives in the checkout's
  own git directory, and the forced removal deletes it. A commit counts when nothing says a
  remote has it: no remote-tracking branch reaches it, it is not a commit a shallow clone's
  history stops at, and no tag at or under the commit the superproject points at reaches it. A
  tag anywhere else, on a branch or past that commit, may be one made by hand in the repository
  the removal deletes, over the very work it marks. The commit the superproject points at is
  taken for the remote's when a `--depth 1` fetch brought it; one a deeper or full clone
  fetched by name, on no remote branch, is still counted, since nothing tells it from the
  commit an agent made there and pointed the superproject at.

What a question says is what its press does, however long it was up. It stays up while an agent
finishes — that is what lights its button — and an agent's last act is often a file or a
commit, so the checkout is looked at again three times: when a pane in it stops working, so the
question shows what was left; when a button is pressed, before any pane closes — a checkout
that no longer holds what the question said is asked about again, as it is now, and nothing is
done, which is also how Discard's second question comes to be about the checkout as it is and
not as it was when the first opened; and once the panes have closed, just before git runs,
when anything new the press would take — ignored files included, which either removal deletes
without git saying a word — keeps both the checkout and the branch.

**A row counts the work, not only what is left to commit.** The diff chip used to be `git diff
--shortstat HEAD` alone, the working tree against the last commit — so the moment an agent
committed everything its row went blank, at exactly the moment there was most to look at. The
15-second gather in `app/src/git_model.rs` now also asks, for a pane on a branch that is not
the base, `git rev-list --count <base>..HEAD`, and when that is not zero `git diff --shortstat
--merge-base <base>`: the working tree against where the branch left the base, committed and
uncommitted work as one number. The chip then reads `2 commits, +40 -3`, and the hover card
adds `since main`. The row leaves the base's name out because its metadata line shares about
170 pixels with the branch, and the chip is never cut — the branch gives way to it, as it always
did; the card, when a narrow window narrows it, gives up the base's name before the numbers.
The base is `merged.rs`'s, found once per repository — by the common git directory, so every
worktree of one shares it — and carried from cycle to cycle with the gather's ticket, and none
of it runs while nothing shows the chip. A lookup that found no base is kept only for the
branch that asked, because a repository made a minute ago has no `main` until its first commit,
and one no gather has asked about for a whole cycle is forgotten. The base itself — a local `main` against `origin/main`
as much as against `main` — a detached `HEAD`, an unborn branch and a repository with no base
keep the plain count, and so does the plugin API's `Where`, whose `added` and `removed` are
still what is not committed yet.

**A cycle reads only what can have changed.** The three reads each go through the whole
working tree, and what made that dear was not the counting: a read takes no lock, so it never
refreshes the index, and a checkout `git worktree add` has just made is every file written in
the second its index was — *racily clean*, which git settles by hashing each such file on
every read. On Crook's own checkout of 450 files that was a quarter of a second per read on a
busy machine, against six milliseconds once the index had been refreshed. So the ticket also
carries, per working tree, what the last cycle read and from what — `HEAD`'s commit and the
base's, from one `rev-parse`, and a stamp of the tree (`app/src/git/stamp.rs`): the index's
size and times and every tracked file's, from one `ls-files` and an `lstat` apiece, with the
inode and change time on Unix, never a byte of any file. The count of commits is read again only
when either commit moved; the diff against `HEAD` when `HEAD` moved or the stamp differs; the
lines since the base when anything did. An edit nobody has `git add`ed moves no commit and leaves
the index alone, which is why the stamp covers every file and not just the index. A stamp taken
while anything it covers is younger than three seconds vouches for nothing — a file system keeps
times to a grain, two seconds on FAT, and a file written twice inside one, the same length both
times, looks unchanged — and neither does one over a submodule, whose dirt does not show on its
directory; both are simply read again. In a sparse checkout the stamp covers only the files
checked out, as the diffs do — the ones left out are passed over, and a sparse index is listed
without being expanded — so a monorepo's millions of paths are not listed and stat'ed every
cycle for a checkout of a few thousand. A working tree is read once a cycle however many panes
sit in it. The wait after a cycle is fifteen seconds or three times what the cycle took,
whichever is longer, so that sixty checkouts cannot keep git busy for most of every minute; it
is four times that while the window is behind another, and coming back to it cuts the wait to
what a focused window would have had. The directories whose rows are on screen — the active
tab's, and the rows inside the tab list's viewport — are gathered and shown before the rest.

**A removal takes its empty directories with it.** `git worktree remove` deletes the checkout
and nothing above it, so the store kept a directory per repository after its last checkout
went. After any removal Crook deletes the directories of its own store the checkout leaves
empty, from the nearest up — only inside the store, never the store itself, and only when
empty, which `remove_dir` checks and does in one step.

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

`app/src/git/worktree.rs` is the git side of the menu — list, add, remove, a count of what is
loose in a checkout, the store's tidying, and deleting a branch on a proof — with an error type
whose variants are the things a UI can offer to do about them, and `app/src/git/merged.rs` is
the proof. `app/src/workspace/finish.rs` is Finish and Discard: the question, the look behind
it, and the order things go in. Both spawn git
through `app/src/git/run.rs`, which puts a timeout on every call (a removal's is the long one,
because it deletes whatever was built in the checkout and a kill halfway through leaves a
worktree the sweep will never touch again), gives every call two reader threads so a repository
with a fat `target/` cannot deadlock a pipe, and joins `log -p` to `patch-id` with a pipe of
their own so a history's patches never pass through Crook. The row's count since the base, in
`app/src/git/diff.rs`, spawns its two through the same runner.

### What the agent changed

A worktree gives an agent somewhere to work; the Changes column is how a person sees what it
did there without leaving for another terminal to type `git log` and `git diff`. The row's
`+214 −37` chip counts only what is not committed yet, so the moment an agent commits, the one
thing Crook said about its work used to go blank — and checking an agent's claim against its
diff is exactly the step a person reviewing ten agents a day skips when it costs a context
switch.

`crook/changes/toggle`, from the palette or a tab's menu, docks a column where the Themes panel
docks — composed once in `Workspace::render`, beside the work rather than over it — about the
focused pane's repository. `app/src/git/changes.rs` reads it: the base (the merge-base of `HEAD`
and `merged::base_of`'s branch, which is what a pull request compares from, so the base moving
on is not shown as the task undoing it), `git log <merge-base>..HEAD`, `git diff --raw
<merge-base>` against the working tree plus `git ls-files --others --exclude-standard`, and one
file's unified diff when somebody opens that file. A read never refreshes the index, so a file
whose bytes are what they were but whose mtime moved — a formatter that changed nothing, `touch`,
a test run writing the same snapshot picture — is one `--raw` calls modified without having read
it; those files, text or binary, go through one `git hash-object --stdin-paths`, and a file that
hashes to the id it had at the base is left out. The list never diffs a file: a diff's cost is
whatever `diff.algorithm` somebody chose makes of the file's lines (twenty seconds for one data
file under `histogram`), and one file it cannot read fails it, where hashing reads each file once
and keeps a file it cannot read. A new link to a directory is the one diff written by hand,
because `diff --no-index` follows the link and fails inside the directory. Every call goes
through `git/run.rs`'s deadline on the background pool, with `--no-ext-diff`, `--no-textconv`
and `core.fsmonitor` off, because each of those is a program a repository's configuration names
and git would run it for a column that only reads; the `a/` and `b/` prefixes and the spelling
of a blank context line are pinned too, so `diff.noprefix` cannot make "Copy diff" hand on a
patch `git apply` refuses. A single file's diff goes through `run_capped`, which stops reading
one byte past half a megabyte and drops the pipe so git ends there, and is cut again at three
thousand lines — both say so on screen, and "Copy diff" is not offered for a diff that is not
whole.

**It reads on beats that already exist.** On opening, when the focused pane moves to another
repository, on its Refresh button, and at the end of every cycle of `GitModel` — which now emits
an event per cycle, changed or not, because what the column shows is nothing the cycle gathers.
No timer of its own; a cycle that arrives while a read is in flight is not a second read, and an
answer is taken only if it is for the question still being asked. Which repository the pane is
in is a walk up for `.git` — no subprocess, but on a network mount whose server has gone a `stat`
that blocks for as long as the mount does — so it is found on the background pool too, once per
directory however often the pane is gone back to while it is stuck, and the column keeps what it
shows through a `cd` until the walk says the checkout is another one. That is the walk for the
focused pane only: Send still walks up from every agent pane of the tab on the thread that
draws, to find the one working in the reviewed repository (see `checkout_of`).

**A frame costs a screenful.** Every row — headings, commits, files, a file's hunk lines — is
one entry of one flat list with a fixed height per kind and a running sum of heights beside it,
rebuilt when what is listed changes. A frame binary-searches the scroll offset for the first row
and builds rows until it passes the bottom of the box, with a spacer standing for the rest: the
arithmetic `block_list.rs` draws a pane's output with. A file's diff is read when the file is
opened and kept while it is folded away, until the next refresh: that reads again the files
that are open and drops the folded ones' diffs, which are read again when they are opened. A
line of a diff is kept to 256 bytes for drawing, because a line is shaped whole on every frame
before it is cut to the column, and one line of a minified bundle is milliseconds of that;
"Copy diff" copies the patch with its lines whole.

**It is read-only, and that is the design rather than the first slice of an editor.** There is
no stage, no revert and no in-place edit, because the tree it shows is one an agent is writing
to: a revert that lands between the agent's read of a file and its write is one the agent
silently undoes, and an edit made under it is an edit it did not read. "Revert this" is a thing
to tell the agent. What the column offers instead is a way out to where the change can be made
safely — "Open" in `$VISUAL` or `$EDITOR` at the first changed line (at the top of the file for
an editor not known to take a line, since one that does not reads `+12` as a file to open),
"Copy path", "Copy diff" — and "Open" only for an editor with a window of its own, since an
editor that draws in the terminal it was started from has none when Crook starts it detached
(Neovim, measured, waits for one for ever). No syntax highlighting: added and removed lines are
the theme's `diff_added` and `diff_removed`.

**What a person finds goes to the agent as words.** A press on a changed line or a hunk header
opens a one-line `TextField` under it, and Enter keeps what was typed as a comment.
`changes_panel/review.rs` holds them: per tab, repository and base, in memory only, because a
review is minutes of reading about a diff that is itself moving. A comment remembers its line by
what the line says, marker included, and by its number — counted from the hunk header, on the
new side for an added line and the old side for a removed one. Every refresh reads again the
diff of each file with a comment on it, folded or not, so nothing is sent about a diff nobody
read again; the comment moves to the line that says the same thing nearest to where it was (a
header by the function name after its numbers, which move), and one whose line is gone is
dropped and named — unless the read was cut short (`MAX_DIFF_LINES`, `MAX_DIFF_BYTES`), which
proves nothing about the lines past the cut: that comment is kept with its last line and number
until a read finds the line or a whole one does not. It is listed where the diff stops, under
the line it was last found on and with its ×, rather than only counted, since not being in the
part that was read is all that is known of it — pushed past the cut, or deleted from above it —
and the person has to be able to see what will be sent and take it out.
`Send N comments to the agent` composes one message — a heading, then
`path:line`, the quoted line and the comment for each, in the order the diff reads, with no
newline at the end — and hands it to `TerminalHandle::paste_bracketed`, the terminal model's
paste behind a check that the program asked for bracketed paste, made under the same lock as the
write. Without the markers `crook_terminal::input::paste` turns every newline into the Enter key,
so a program that has not asked is refused rather than handed a review a line at a time. The
pane is the one in the tab whose agent has reported (`StatusSource::Agent`, or for one with no
hooks its title: `Title`, `TitleAtStart` for what it wears before its first turn: Claude
Code's `✳`, Gemini CLI's `◇`, codex's bare folder, and `TitleStoppedAsking` after a question its
title asked, or `TitleBetweenHooks` for the `✳` an Esc leaves under a hook's running), whose shell is not listening, and which works in the reviewed repository, else the
focused one; a pane whose shell is listening is refused, because a review's quoted lines are
commands there (bash
5.3 turns bracketed paste on at its prompt, so the whole review would sit in its line editor one
Enter from running), and `> +x` is a redirection that writes a file into the tree this column
promises not to touch. Whether it is listening (`a_shell_is_listening`) is the shell's word
first, and not the report's, because a report outlives its agent: the hooks say `idle` when a
session ends, and `Emulator::settle_agent` takes back only a report of running or waiting when
the command ends. So marks saying the shell is at its prompt (`AtPrompt`, `Done`) refuse the
paste whatever the pane last heard from an agent, and a command running (`C`) or a full-screen
program never does. What the marks leave open is a line handed to the shell and not answered
(`Submitted`), and that the report decides: an agent that reported since the line was handed
over is running under it. bash runs no DEBUG trap for a top-level `( … )`, so an agent started
in one never gets a `C`, and a shell without Crook's marks stays `Submitted` from its first
line on. Short of such a report, a shell that reports marks is taken to be still reading the
line — an open quote, a here-document — and one that reports nothing is judged by its
composer. Which of the two a shell is, its open block says: only a mark closes the session's
first, and a reflow, which forgets where the prompt ended, keeps that. A paste that
went clears the comments and focuses the pane the ordinary way, so the person reads it in the
agent's own prompt and presses Enter; any refusal keeps them and says why, and `Copy review` is
the same text on the clipboard. The field takes the keyboard the way the tab search box does, as
a wish granted by `changes_takes_keys` and claimed Escape and Enter in `action_for` — and the
pane beside it is given no keys at all while it has them (`body.rs`), since every element sees
every keystroke and an agent with no composer would otherwise be typed into alongside the
field. A click on a pane, a section shown, another field pressed, the find bar opened, Enter or
Escape takes the keyboard back, and so does the column moving to another repository or tab,
which takes the field with it; a read coming home never does. A read that takes the field's
line away — the line gone, pushed past the cut, or a read that failed — keeps the field, its
words and the keyboard and moves it to the top of the column under the reason (`Adrift`); Enter
there adds nothing. Its file is read again on every refresh, folded or not, and a read that
finds the line puts the field back under it and shows the file, since most of what sets a field
adrift — one timed-out read, a line past the cut — says nothing about the line. Dropped
instead, the field would hand the keyboard back to the focused pane
— usually the agent being commented on — and the rest of the comment, and its Enter, would be
typed into it. Against the agents themselves, a multi-line bracketed paste was checked to land
in the prompt unsent in Claude Code 2.1.280 (as `[Pasted text #1 +3 lines]`) and OpenCode
1.18.33 (as `[Pasted ~4 lines]`); Codex 0.149.1 and Gemini CLI 0.61.0 turn bracketed paste on at
start-up, but stopped at sign-in and a trust question here, so a paste into their prompts is not
verified.

### The agent says what it is doing

The dot on a tab's row has four colours and, until this section, one real source: a bell in a
pane nobody was looking at. `AgentStatus::Running`, `NeedsInput` and `Failed` were written by
the snapshot fixture and by nothing else, which made the sidebar a list of shells with a
promise painted on it. The promise is now kept by the program the promise is about, and it is
kept over the one channel that program already has.

**The report is an escape sequence.**
`OSC 6340 ; <status> [; <title> [;; <message> [;; <session> [;; start]]]] BEL`, read by the same
watcher that reads OSC 7 and the completion channel's 6339, in
`crates/crook_terminal/src/agent.rs`. Four words — `idle`, `running`, `needs-input`, `failed` —
an optional name for the work, which lands in `derived_title` because that is what
`derived_title` has always been: what the agent calls what it is doing — and, after that, what
the agent is waiting for. The title may hold a `;`, and always could — everything after the
status is the title — so the message is not a third field but the tail after the first *empty*
field, `;;`, and the writer squeezes a `;;` out of either text so neither can forge the cut.
A sequence without the tail reads exactly as it did; a Crook older than the field reads the
whole tail as the title, which is wrong on the row and nothing worse, so a new `crook --agent`
is safe to run into an old pane. The alternative was a socket, and a socket is the wrong shape
three times over. It needs an address, which the agent would have to be told; it stops at this
machine, where the terminal crosses `ssh` and `docker exec` without noticing; and it needs to
say *which pane*, where the terminal a program writes to *is* the pane. Every other terminal
drops an OSC it does not know, so a program that reports this way costs nothing in one of
them.

**The CLI writes it.** `crook --agent running --title "port the tab bar"` opens `/dev/tty` —
`CONOUT$` on Windows — and writes the sequence there, not to standard output. The caller is a
hook, and a hook's standard output belongs to the program that ran it: Claude Code reads what
its hooks print. A hook often has no `/dev/tty` either: Claude Code starts every command hook
in a session of its own, with no controlling terminal, on macOS and Linux alike. The pane is
still one or two processes up, as the terminal of the program that ran the hook, so a process
that cannot open its own writes to the controlling terminal of the program that started its
session: the first ancestor outside that session, past the shells inside it — `session` and
`tty_nr` in `/proc/<pid>/stat` on Linux, `getsid` and `e_tdev` from `proc_pidinfo` on macOS,
and the character device under `/dev` with that number (`app/src/agent/ancestors.rs`). The
walk stops at that program whether or not it has a terminal. One with none is an agent that
another agent's Bash tool runs, in a session of its own, or one whose pane has hung up, which
takes the terminal from its whole session; the next terminal up is the outer agent's tab, which
a nested `claude -p` would call idle mid tool call, or the terminal Crook was started from. It
is a fallback and never a second route: with a terminal of its own the process writes there,
and a process whose session was started by one with no terminal — `cron`, a CI runner — fails
the way it always did. The one exception is tmux, which reads everything its panes write and
keeps an OSC it does not know to itself (`app/src/agent/tmux.rs`). With `TMUX` set, the process
asks `tmux` once, under a second, for the pane's terminal and the clients attached to its
session; when that terminal is the process's own — by the same device numbers, so a pane of a
Crook started from inside tmux, which only inherited the variable, is not taken for it — the
report goes to each client's terminal that is not in control mode, side by side and 200 ms for
all of them, and any that is not Crook drops it unread. tmux's DCS passthrough is the way only
where tmux cannot be asked, names no such client, or none took the report in time, whatever
its `allow-passthrough`: tmux throws a passthrough away with the rest of a client's pending
output when that client falls behind (`client_discarded`), and the redraw that follows never
brings it back. A test that hands a hook a stand-in binary, one that never opens a
terminal, passes while every real report fails, so `app/tests/claude_code_hooks.rs` runs each
of the plugin's commands, and each one `--agent-hooks` prints for Claude Code, Codex, Gemini CLI
and Copilot CLI, with the real binary under `sh -c` in a session of its own below a process on a
pty, and reads the report off the pty; and runs each once more below a shell with no terminal,
where nothing may arrive and each still exits 0, and the printed ones again with the binary
gone, where each exits 0 without a word.
`--message "run rm -rf build?"` is what a `needs-input` is waiting for, or what went wrong
beside a `failed`, cut to
one line of 200 characters in the writer so the sequence never carries a novel; `--message -`
reads it from standard input, the `message` of a hook's JSON when the input is one — else the
`last_assistant_message` or the `error` of a StopFailure, else the tool Codex's permission
request names — and the whole input otherwise. `crook --agent-hooks claude` prints the fragment of
Claude Code's settings that makes it say all of this by itself — running on a prompt and around
every tool, after one that failed (PostToolUseFailure) as after one that worked, needing input on every notification with the notification's text piped in as the
message, idle on stop, failed with the error on a StopFailure, which an API error runs in place
of Stop. A StopFailure a subagent's turn ran, whose input carries an `agent_id`, writes nothing:
the agent that started it is handed the error and works on. Nor does a needs-input take the
place of a hook's failed with nothing run between — Claude Code's idle reminder a minute after
the failed turn would otherwise wipe the error off the row — and nor does the same reminder sent
down its notification channel as well, an OSC 9, 777 or 99. PermissionRequest reports no
status: Claude Code also runs it in `claude -p` and for a background agent, where no dialog is
drawn, and its hook is the one that asks the window instead — see "Answering an agent". Esc runs no
hook at all, so there the title's `✳` takes a hook's running back, quietly: Claude Code draws
the same `✳` the moment a permission dialog opens, six seconds before the notification that
asks, so that idle asks for no look, posts no notice and answers no wait
(`StatusSource::TitleBetweenHooks`). It is printed rather than installed:
Crook writes no file it does not own, and it has never opened that one. What it prints first is
the way that needs no merging at all: the repository is a Claude Code plugin marketplace
(`.claude-plugin/marketplace.json`) whose one plugin, `packaging/claude-code`, carries the same
hooks and the skill, and `claude plugin marketplace add theguriev/crook` with
`claude plugin install crook@crook` is Claude Code writing its own settings. The printed hooks
and the plugin's are the same commands. Codex CLI and GitHub Copilot CLI read the same
marketplace and install the same plugin (`codex plugin add crook@crook`,
`copilot plugin install crook@crook`), and `--agent-hooks codex` and `copilot` lead with their
two commands too. Each reads a manifest of its own in the plugin's directory before Claude
Code's — `.codex-plugin/plugin.json`, `.github/plugin/plugin.json` — and it names a hooks file
of its own, `hooks/codex.json` or `hooks/copilot.json`, which is what that agent's
`--agent-hooks` prints. Without one, both run Claude Code's `hooks/hooks.json` under its event
names: Codex has no Notification, so the plugin never said it had stopped for an approval, and
those commands pass no `--session`, which would let Copilot's subagents rename the tab and end
its turn. Claude Code reads `hooks/hooks.json` and nothing else, so neither file gives it
Codex's PermissionRequest, which says needs-input: its own runs the ask. A hook runs in whatever `PATH` its agent was started
with, and a path written into the file goes stale — a moved Crook.app, a deleted build — while
the file stays, so each calls `"$CROOK_BIN"`, the binary its pane names, and `crook` on `PATH`
when a pane names none or the one it names has gone. Each is behind a guard that exits 0 unless
`TERM_PROGRAM` is Crook, `CROOK_PANE_ID` is set or `TMUX` is, since a UserPromptSubmit hook that
fails is a notice on every prompt and one that prints is text the model reads. `TMUX` is there
because a tmux server started anywhere but a Crook pane hands its panes neither of the other
two, whichever terminal its client is attached from, and only tmux can name the terminals its
clients are on. For the guard's reason each command exits 0 after the report too, whatever the
report did: a Crook too old to reach the pane from a hook is a line of stderr in Claude Code's
debug log, not a notice on every prompt and tool call — and GitHub Copilot CLI denies the tool
call a `preToolUse` hook failed over, so a hook that passed a failed report on would refuse
every tool of a `copilot -p` with no terminal.
The guard and the lookup are where the hooks fall short of the sequence: `ssh` and
`docker exec` carry neither variable, nor the `CROOK_BIN` a pane sets, which is a path on the
pane's machine anyway, so an agent on the far side of either reports nothing through them until
`TERM_PROGRAM` is passed on (`SendEnv`/`AcceptEnv`, `docker exec -e TERM_PROGRAM`) and a
`crook` is on that side's `PATH` or `CROOK_BIN` is set there to one
(`docker exec -e CROOK_BIN=/opt/crook/crook`). `crook --agent` run there by hand, which has no
guard, reaches the pane as it always did, and so do the OpenCode plugin and aider's command,
which run the binary by the path they were printed with and have no guard either. Tests in
`agent.rs` hold each of the plugin's hooks files to what `--agent-hooks` prints for its agent,
and each manifest to the file it names, run the commands of `hooks/hooks.json` under `sh`, and
hold its `SKILL.md` byte for byte to `skill.md`. Codex CLI and Gemini
CLI read the same object under their own event names, and GitHub Copilot CLI a versioned
cousin of it with `bash` for the command, so `--agent-hooks codex`, `gemini` and `copilot`
are the same table with a different first column; OpenCode has no command hooks, and
`--agent-hooks opencode` prints the TypeScript plugin its plugin directory loads instead,
which reports through Bun's shell. Codex's permission request names a tool and no text, so
`--message -` says "permission to use" the tool when the JSON has a `tool_name` and no
`message`. An agent with no hooks at all — `aider` — gets a sentence saying so and what to
do instead, on stdout with exit 0, since the sentence is the whole answer.

**The shell takes the status back.** An agent that was interrupted never says it stopped, so
the emulator listens to the shell's marks beside the report: `D` ends the command a running or
waiting agent was, and takes the claim with it. A failure is the one status worth seeing after
the fact, so it outlives its `D` and goes with the next command's `C` — new work being the
thing that answers it. All of that is in `Emulator::settle_agent`, and none of it in the
workspace, which only ever hears a status change. The message lives as long as the wait:
`AgentSession::message` is written with a `NeedsInput` and cleared by whatever status follows
it — running, failed, or the idle the shell's `D` reports — and while it is there the row's
second line is the question rather than the branch or the directory, since "wants to run
`rm -rf build`" is what decides whether a person comes now, and which branch the pane is on is
not. The header's count and the window title are unchanged by it.

**A subagent is not the pane.** An agent can hand part of a turn to a subagent in a session of
its own, and GitHub Copilot CLI runs every hook for that session as for the one the person is
talking to: the subagent's `userPromptSubmitted` carries the prompt the agent wrote for it, and
its `agentStop` comes while the parent is still working — 14 ms before the parent's
`postToolUse` in the capture. Taken as the pane's, that prompt named the tab until the next one,
and that stop woke a lead's `crook pane wait --until idle` partway through the turn. So a report
can name its session, after a second empty field — `running;<title>;;;<session>`,
`needs-input;;<message>;;<session>` — and both texts lose a `;` at either end so that neither
can forge that cut. `crook --agent <status> --session -` reads it out of the hook's input
(`sessionId` for Copilot, `session_id` for the agents that spell it so), and every hook
`--agent-hooks copilot` prints passes it. `agent::Sessions` is the rule, applied in the watcher
as each report is read, so a report the pane does not take never replaces one earlier in the
same read that it does: the first session to say `running` while nobody holds the pane holds it
— a question does not take it, since an agent at rest can still ask for a look and its next turn
may be another session's; from any other, a title is dropped and a stop — `idle` or `failed` —
is dropped whole, while a question and the `running` after one are taken, since they wait on a
person all the same; a report that names no session is taken whoever holds the pane. The holder
lets go when it stops, because one Copilot process can go on in a new session (`/clear`), and on
the shell's `D`, because an agent interrupted mid-turn never says it stopped. And a session that
says it starts — a third empty field and `start` after the session,
`crook --agent <status> --session-start <id>` — takes the pane whoever holds it: Copilot says no
`agentStop` for a turn interrupted with Esc, and `/new` or `/resume` go on in a new session in
the same process with the old one's `sessionEnd` still to come, so without it every turn after
was taken for a subagent's until Copilot exited. Its `sessionStart` hook is the one it runs for
no subagent — once a session, on its first prompt, 13 ms after that prompt's
`userPromptSubmitted` in the 1.0.89 capture, with the prompt as `initialPrompt` — so the printed
hook reports `running --title - --session-start -`, and the title the prompt's own report lost
comes back with it. Once a session in the life of the process is all it says, though: `/resume`
back to a session the process has already run says no second start, and with the turn before
it interrupted in another session, every turn of the one resumed was again taken for a
subagent's. So `Sessions` remembers the sessions that started until the command ends, at most
64 of them, and a remembered session's `running` takes the pane back from whichever holds it.
And it takes every other session, once one has started, for a subagent's even with nobody
holding the pane: Copilot's `task` can run a subagent in the background, the parent's
`agentStop` can come before the subagent's first prompt, and the first to say `running` on an
empty pane was then the subagent, which named the tab and held the pane against the person's
next prompt. In such a pane a subagent names nothing, and with the agent at rest only its question
is taken, and the `running` that answers it, which holds the pane for it until it stops. A pane
where no session has started keeps the first rule. The prompt Copilot hands the parent when a
background subagent finishes, `<system_notification>` and a line saying which, is reported
with no title. The writer counts the session's pieces, its start's and the cut before the
message first, so `vte`'s sixteen never drop them. A Crook older than the field reads the
session into the message, which the row shows only beside `needs-input`. OpenCode's subagents
need none of this on the wire: its plugin keeps the ids `session.created` gives a `parentID` —
the way `opencode run` finds the sessions it waits on — and asks OpenCode's server, through the
`client` a plugin is handed, about a session it did not see made, once each: the `task` tool
goes on in an earlier subagent's session when handed its `task_id`, which makes no new session,
and after OpenCode is started again the plugin has seen none of them made. It sends nothing for
a subagent but its questions and the going on after one, and takes a prompt's title only from
text that is not `synthetic`, since a background task's result comes back to its session as a
prompt OpenCode wrote, `<task id=…>`. Its event handlers are not awaited, and each report is a
process, so the plugin also sends its reports through one promise, in the order it made them,
and leaves a status said again unsaid. Claude Code needs neither: its subagents fire
`SubagentStop`, which no hook reports.

**The pull request is a second sequence.** `OSC 6342 ; pr ; <url> BEL`, read by the same watcher
and written by `crook --agent <status> --pull-request <url>` in the same write as the status.
A sibling number rather than a field on 6340, because 6340's tail is already cut — on `vte`'s
`;` and on the `;;` before a message and before a session — and an older Crook reads that tail
as the title, where a number it does not know is simply a link that is not there; 6341 is the
question's channel, and only an id ever travels on it — see "Answering an agent". An address holds `:` and `/` always and
`;` almost never, so the pieces after the word are joined back as a title's are, and the one
thing that cannot be undone — `vte` keeping sixteen pieces and dropping the rest without a
word — is refused at both ends: the writer will not send an address that fills the list, and
the reader takes a full list as cut. `crook_terminal::agent::pull_request_url` is the whole
rule either end applies: `https://` with a host, at most 512 bytes, no control character and
no white space. The row opens it through `app/src/browser.rs`, whose allow-list checks it a
second time. The wire checks the scheme and nothing else, and any program's output can write
the sequence, so the chip reads `PR #<n>` only for github.com and names the host anywhere else
(`gitlab.com #42`, `github.com.example.net #12`) — the host a browser goes to, read the way the
URL standard reads an https address: after any `user@`, and ended by a `\` as well as a `/`,
so `https://attacker.example\@github.com/o/r/pull/12` is attacker.example's — cut at a fixed
width from the start, and the hover card prints the whole address. `AgentSession::pull_request`
records it against the branch the pane is on at the moment it arrives and the repository that
branch is in — its common git directory, which every worktree of it shares and a submodule
does not — read from `HEAD`, not from the row's git facts, which are up to a poll old and
would record a branch made, pushed and opened inside one poll as the one it started on. It is
dropped when `HEAD` names another branch or the pane leaves the repository, for another one or
for none, which is asked whenever the git model reports a change and whenever the pane moves,
never on a clock. A rebase detaches `HEAD` for as long as it runs, conflicts and all, so
`git::branch_at_work` reads a detached `HEAD` with a `rebase-merge/head-name` or
`rebase-apply/head-name` beside it as the branch being rebased, as `git status` does; any other
detached `HEAD` in the same repository names no other branch and keeps the link, since nothing
would report it again when the branch came back. One in another repository — a submodule's, or
a tag checked out — is that repository's, and a branch there of the same name is not this one.
The hooks carry it with no second
process: the Claude Code and Codex hook after every tool is `running --pull-request -`, which
finds the `https://…/pull/<n>` a `gh pr create` in `tool_input.command` printed into
`tool_response` and finds nothing after any other tool. The chip is on the row's metadata line
in `Expanded`, under the "Show: PR link" toggle, and it is a link: the row's own press stands
aside while it is hovered, as it does for the close button. The menu carries **Open pull
request** for the density that draws no chip and **Check pull request**, which is the one
place Crook asks the network about the work — the person's own `gh pr view <url> --json
state,isDraft,statusCheckRollup`, on the background pool under `git::worktree`'s deadline shape
(`app/src/forge.rs`), once per press and never polled. `gh` holds the token and knows the
host; Crook holds neither. It is looked for on Crook's own `PATH` and then in
`forge::usual_places` — Homebrew's prefixes, MacPorts', `/usr/local/bin`, `~/.local/bin` and
a Nix profile — because a `Crook.app` opened from the Dock is handed launchd's
`/usr/bin:/bin:/usr/sbin:/sbin`, which holds `git` and no `gh`, and Crook runs no login shell
to learn the person's. The answer — open, draft, merged or closed, and how many checks pass, fail
or are pending — is written onto the session and printed on the hover card until the next
press, or the reason there is none: `gh` not found, not signed in, or unable to reach the
forge. The hook reads a PostToolUse input it cannot parse as saying nothing — only text that
is not a JSON object is taken as `gh`'s own output piped in.

**Attention is a separate fact.** The bell used to write `NeedsInput` and looking used to
clear it, and that was right for a bell and wrong for an agent: an agent waiting for an
approval is still waiting after somebody glanced at its row. So `AgentSession` carries
`status`, which is what the agent said, and `attention`, which is whether something happened
while nobody was looking — the bell, or a status changing to anything but running in a pane
without the keyboard. Looking clears attention and only attention; running clears it too,
since the stop it announced is over. The dot shows the status, with one exception kept from
before: an idle pane that asked for attention is drawn as needing input, because a bell in a
pane nobody is looking at is a program saying exactly that. A person can ask for the same
look by hand — "Mark as waiting" on the row's menu, `crook/tabs/mark-waiting` — and that
`marked` flag outlives every glance and every running report, going only when they arrive at
the pane or clear it from the same entry. `is_waiting` is the three combined, and it is what
the count in the header and the "next waiting" chord read. "Why this status" on the same menu,
`crook/tabs/explain-status`, reads all of it out in words — the status and whether the agent
said it or the shell's `D` took it back (`AgentSession::source`, an `Instant` subtracted when
the panel is drawn and never ticked), the title and the question, which of the bell or a
status change set `attention` and whether `marked` is set, and what the shell is running —
on a read-only panel that hangs off the menu's corner where the worktree list does.

**Other terminals' notifications ask for the same look.** OSC 6340 is spoken only by a program
that has been told about Crook, and the ones that have not already say "look here" in three
sequences other terminals show: OSC 9, iTerm2's; OSC 777 `notify`, rxvt's, which Ghostty
reads; and OSC 99, kitty's. Claude Code writes one of them when it stops to ask, in whichever
form its notification channel names, and it does so on a remote box with no Crook binary as
readily as next door. Its channel starts as `auto`, which names a form by `TERM_PROGRAM` and
none for Crook, so `crook --agent-hooks claude` and the skill tell a person without the plugin
to set `preferredNotifChannel` to one of the three.
The same watcher reads them, in `crates/crook_terminal/src/notify.rs`,
into a `Notification` of a title and a body, putting a message `vte` split on `;` back
together and marking one that ran past `vte`'s sixteen pieces with `…`, the way a report cut in
the writer is. ConEmu's commands on OSC 9 — `9;4` progress, which winget among others
writes, and the rest of `9;1` to `9;12` — are not notifications and are dropped; a kitty
notification sent in chunks is put back together by its `i=`, and a payload in base64 is
skipped rather than shown as it arrived. It is not a status, and the status is left as it was:
an agent that says it is running is still running after it notifies. What it sets is
`attention` — `Attention::Notification`, which carries the text, joined `title: body` since
the row has one line for both — in a pane without the keyboard, exactly where a bell would:
the row washes, counts as waiting, and prints the text on its second line in place of the
table's, ahead of a `needs-input` message since it is the newer of the two. Over a `failed` the
agent reported, with nothing since, it sets nothing at all (`Workspace::agent_failed`): Claude
Code sends its idle reminder down its notification channel right after its Notification hook,
and with the plugin installed and the channel left at `ghostty`, `kitty` or `iterm2`, that
reminder would put "Claude Code: Claude is waiting for your input" on the row in place of what
went wrong, and light a failure somebody had already looked at. Whatever clears a
bell's attention clears it — a look, an agent reporting running — and the text goes with it; a
bell after it keeps it, because Claude Code's `iterm2_with_bell` channel sends that pair. A
burst with no report between is handed over as its last one, beside the bell's rule. In one
read it keeps its place among the reports and marks around it, so a status written after it —
a `running`, or the `D` that ends a running agent — takes it away and one written before it
does not, wherever the pty split the bytes. The watcher stops where a report and a
notification meet, as it stops on a mark, so a report before the notification is handed over
ahead of it rather than folded into one written after it: `needs-input`, `9;done`, `running`
is three changes, not a notification and a `running` that changed nothing. Nothing here posts
a desktop notification of its own; a row it turns amber is told to the desktop the way a
bell's is.

**Looking needs the window in front.** "Nobody was looking" meant "the pane without the
keyboard", and that made the one-pane window the one that could never be waiting: its pane
has the keyboard the whole time somebody is in another application, so an agent that stopped
to ask there never reached the count, the title or the chip. crookui forwards winit's
`WindowEvent::Focused` as `Event::WindowFocused`, the workspace keeps the bit, and
`Workspace::looking_at` — the pane with the keyboard while the window has the desktop's
focus, and nothing while it does not — is the one answer `ring`, `agent_reported`, `attend`
and the tabs plugin's count, chord and group roll-up read. A window starts out focused, which
is what every window was before the bit existed, so a desktop that never says changes nothing.
`attend` does nothing while the window is behind another, because a pane the strip moves to
then is a pane nobody saw. Coming back to the window runs it for the pane with the keyboard,
with the pane that had it on leaving as `before`: the same pane is a glance, which answers the
attention asked for while nobody was there and leaves a person's mark, since the strip did
not move; a pane the strip moved to in the meantime — a shell that exited closed the one in
front — is arrived at, and its mark goes as it would have on the move itself. The panel's
rows keep asking only whether a row is the selected one: the selected row is painted as
selected whether or not it is waiting, so the window's focus would change nothing on it.

**Focus reports ride the same bit.** A program that sets `?1004` is sent xterm's `CSI I` and
`CSI O` (`crook_terminal::input::focus`, read against the emulator's mode by
`Terminal::send_focus`), and a pane stands in for xterm's window: the window going behind
another and coming back is out and in for the looked-at pane, and a tab switch or a split's
focus move inside a window in front is out for the pane left and in for the pane reached,
in that order. `Workspace::report_focus` compares the looked-at pane before and after, from
`set_window_focused` and from `settle`, so no gesture has a path of its own. Nothing is sent
until a program asks, which is what keeps `[I` off a shell's command line.

**So does the caret.** A window behind another draws no caret in its composer and outlines a
program's cursor, which is what a pane without the keyboard already drew inside a window in
front and what every other terminal draws in a window without the keyboard — two windows that
both blink look as though both are listening. A `TextField` with the keys — the find bar, a
search box, the palette, a rename — is drawn as one without them there too: no caret and no
accent ring. `Workspace::shows_a_caret` answers no while the window is behind, so the blink
chain repaints nothing there. Only the drawing reads the bit: `Keys` and `has_keys` are decided
exactly as before, so the window that comes back types where it left off.
A window that opens without the keyboard is told so before its first frame: `crookui`'s
`resumed` sends `WindowFocused(false)` when winit's `has_focus` says no, the way it sends the
theme. X11 and macOS send that much themselves as the window appears, but Wayland says nothing
until the keyboard first arrives, so a window a script opened behind another was taken as in
front and asked nobody for a look until it had been visited once. The caret comes with the
keyboard, which a window opened in front is given as it appears.

**Out of sight is another bit, which the breath and the panes read.** crookui folds three ways
of hearing it into one `Event::WindowOccluded`, in `InputState`. winit's
`WindowEvent::Occluded` says a window is covered or minimised on macOS, but on X11 only that it
is covered, and only with no compositor running, which reports every window in view. Windows
sends none, and a window resized to nothing is the minimised one there (`InputState::resized`).
X11 says nothing of a minimised window, which its window manager unmaps, and the server sends
no visibility event for a window that stops being viewable. So crookui asks X11, and only X11:
winit's `is_minimized` reads `_NET_WM_STATE_HIDDEN`, and that is a round trip to the server. It
is asked when the focus moves and before each frame while the window lacks the keyboard or was
minimised when last asked (`InputState::may_be_minimised`), and an `Occluded(false)` overrides
the answer, since a window manager may map the window it restores before it takes the property
off. So does the keyboard (`InputState::occlusion`): X gives it only to a window that is
mapped. A window restored without it, and asked by its first frame too soon, would have nothing
to ask again — a window out of sight asks for no frame of its own, its panes' output included —
so while the answer is all that holds the window out of sight the event loop asks again by
itself, once a second (`MINIMISED_LOOK`, `InputState::held_by_an_answer`), for four seconds
after the focus or a frame last asked and no longer (`MINIMISED_LOOKS_FOR`). A restore comes
with both: X11 says the focus again as it maps a window, and exposes it, which asks for a frame.
So a window left minimised is asked four times more and then waits for an event, since the
window keeps no idle timer, and nothing is asked anywhere but X11. macOS and Windows already
say it, and are not asked (`Window::is_minimised`): AppKit tells winit that a window it
restores is in view while `isMiniaturized` may still say yes. A covered window on Windows or
under an X11 compositor goes unremarked. The breath chain ends while the window cannot be seen
and starts again when it can, and a pane's output asks for no frame and builds no snapshot
meanwhile (see "Nor does a pane nobody can see build snapshots"), because X11 and Windows draw
every frame asked of a hidden window. Wayland holds those frames back by itself. Neither reads
the focus itself, on purpose: a window behind another but beside it is where a working agent is
watched from. Within a window in sight, a breath repaints only while a running mark is inside
the panel's viewport, on a row or on a group heading — the panel's `RowGeometry` keeps where
both were drawn — since a frame is the whole window and its cost grows with every tab.

**And the window can be asked to keep still.** The Appearance page's *Motion* resolves, with
the desktop's own reduce motion switch for *System* (`crate::motion`, asked on the pool when the
window opens and when the settings are shown), to whether anything moves on its own. Kept still,
a running mark is drawn at frame zero of its breath and every caret is solid — the caret's half
is a thread-local beside the clipboard's PRIMARY switch, `text_input::let_carets_blink`, since a
caret is drawn by a dozen fields a plugin may own — and neither chain runs: each ends on its next
tick, and allowing motion again starts them. A caret also rests, lit, `CARET_REST` after its
field was last used, and the blink chain repaints once more as it does, since its last frame may
have been the dark half.

**The strip answers.** A waiting row is washed in the amber its dot shows, faintly, because a
dot is nine pixels and a person scanning a long list wants the row to say it. A group folded
away carries the worst of the rows it hides on its heading — the same wash when any member is
waiting, and the host's own mark before the name, needs-input over failed over running and
nothing for idle — because a wash on a row nobody can see says nothing, and an open group's
heading stays plain since its rows say it themselves. The header's
left end — a second single-item slot, `header.left`, beside the one the usage chip takes — is
a chip that counts the waiting panes and goes to the next one when pressed, and is nothing at
all at zero, since a count of zero is not information. The same move is
`crook/tabs/next-waiting`, suggested on `cmd-j` (`ctrl-shift-j` off macOS): the next waiting
pane after the active tab in the panel's order and round the end of it, so the chord pressed
three times visits three tabs rather than the same two in turn. Focusing it is what answers
the request for a look; an agent's own question stays asked until the agent says otherwise,
which is why the pane you just left can be waiting again the moment you leave it.

**And outside the window.** The window's title carries the same count in front of the active
tab's name, and when the count *rises* while the window is behind something else the shell
asks the desktop to point at it through `Proxy::request_attention`, winit's
`request_user_attention` with the informational kind: one dock bounce on macOS, the urgency
hint on X11, an xdg-activation request on Wayland, a taskbar flash on Windows, all from winit
and no dependency of Crook's own. A rise rather than a count above zero, because the panes
waiting as a person leaves are panes they just saw — the pane they leave with a question on
it joins the count at that moment without being news — so the count on leaving is the mark
the next one has to pass (`Urgency` in `app/src/lib.rs`). crookui asks only while the window
does not have the focus, checked when the request reaches the window rather than trusted to
the platform, and takes the request back itself when the window next gains the focus, which
only X11 needs: Windows stops its flash when the window comes to the front, macOS bounces
once and a Wayland compositor clears its own. It is not a notification: it names only the
window. Both the title and the request follow every change to the window's views — the
invalidation callback that asks for a frame, through `Beacon` — and not the frame, because
the window they are for may get no frames: a Wayland compositor sends no frame callback to a
surface it is not showing and winit holds every redraw back until that callback comes, and
wgpu refuses to present to an occluded window on macOS. That covers the window's opening
too: registering the callback is itself an update, and its flush runs the callback for
everything the window opened with, since no frame has taken any of it yet. On macOS `Beacon`
also puts the title's count on the dock icon as its badge when the count moves
(`Proxy::set_badge`, the dock tile's `badgeLabel` in `crookui::windowing::dock`, none at zero).
Since macOS 12, by the reports of the apps that hit it, the dock draws a bundled app's badge
only once the app has asked Notification Center for leave to badge, and then only as its
Badges switch says, so with the first count above zero `Beacon` asks
(`notify::ask_to_badge`) and a yes sets the badge again (`Proxy::show_badge_again`); whether
the dock draws one for a binary outside Crook.app, which cannot ask, is unverified. `Beacon`
asks only while notifications are wanted (`plugins::notifications::are_wanted`: the plugin
loaded and one of its switches on), because macOS words that prompt as leave to send
notifications, and a person who turned them off would be asked for what they had just refused
— and, answering no, would find them refused when they turn them back on. The badge is set all
the same.

**And in words, on Linux and macOS.** When a pane's row turns to needs-input while the window
is behind another one — its agent asking, its agent saying it is done while nobody is looking,
or a bell in a pane with nothing else to say, which is how Codex asks in a terminal it does not
recognise — `Workspace::tell_the_desktop` posts a desktop
notification: `Crook — <the pane's name>`, the name its row shows (`row_content::row_name`, so
a shell at a prompt is its directory and not `agent 1`), with the agent's question on one line
and cut to `notify::MESSAGE_CHARS` under it. A *turn* of the row's status rather than a
report: the agent saying needs-input again with another word in its question is the same stop,
and the question on screen as a person leaves is one they saw — the rule `Urgency` gives the
request for a look.
An agent that failed and a command that ran `notify::LONG_COMMAND` or longer take the same
path and are off out of the box. The three switches are `GeneralOptions::notify_on_*`, on the
Notifications page `crook/notifications` owns, and the workspace posts only while that plugin
is loaded, so the Plugins page's switch for it is a switch for the feature. `notify::Cooldown`
keeps a pane quiet for `notify::QUIET` after it posts, counted from the post and not from the
refusals, so an agent whose hooks flap posts once: #352's rule for the bell, one level up.
The quiet also ends when somebody looks at the pane (`Workspace::attend` calls
`Cooldown::seen`), so the next question of an agent a person just answered is posted however
soon it comes.
Posting is a `Notifier` the window that runs on a desktop hands in (`Workspace::set_notifier`);
a test and a snapshot keep `Silent`. On Linux that is `notify::linux::NotifySend`,
`notify-send` started on the pool through `crate::process::command` and waited on by a thread
of its own, which is `plugins::wasm::sound`'s arrangement with `pw-play` — no D-Bus crate,
because `zbus` is a whole bus client for one method call and `dbus` binds libdbus. It passes
only the flags every `notify-send` has, the two texts after `--`, and the body with `&`, `<`
and `>` escaped, since servers read a body as markup and a program's words are not Crook's, and
its backslashes doubled, since `notify-send` reads C escapes out of the body. A
machine without `notify-send` is one warning and then nothing is started again. There is no
click: `--action` implies `--wait`, one process held for as long as each notification lives,
and a click heard could switch the pane but not bring the window forward — winit's
`focus_window` does nothing on Wayland, with no way to hand it the activation token a server
sends. On macOS it is `notify::macos::NotificationCenter`, `UNUserNotificationCenter` through
the objc2 bindings, asking leave with each post and, while notifications are on, with the
dock's first badge — macOS prompts the first time and answers from the setting after — and
made only for a process with a bundle identifier (`notify::Service::of`), because asking for
the center without one throws;
a click brings Crook forward and not the pane. A binary outside Crook.app and Windows post nothing; the page
says so and its switches have no handler there. The session bus is a socket on this machine
and Notification Center a daemon on it, so nothing of this reaches a network.

What is deliberately not here is a plugin. `docs/plugins.md` planned this seam as an `Agent`
service a plugin provides, and that is still the right shape for anything that *drives* an
agent — spending its budget, reading its transcript. Saying what it is doing needed none of
that: a word on a wire, written by the agent itself, which is why it reached the sidebar in a
day and why a plugin that wants to do more starts from a status that is already true.

### The window answers

A status travels one way, from the program to the row. The other direction — a script or an
agent asking a running Crook what is open, or asking it to open something — is a question whose
answer has to come back, and it goes over a socket: `crook pane list` asks the window its pane
is in and prints every pane that window has, `crook tab new` asks it to open a tab beside the
pane and run a command there, and `crook pane wait`, `crook pane blocks` and `crook events
--follow` let that pane watch the tabs it opened — wait for one to stop, read what its commands
printed, hear everything that happens to them. They are the first verbs of `docs/plugins.md`'s
Phase 4. The code is `app/src/control/`.

**Why control is a socket when status is an OSC.** The three reasons above are reasons against
a socket *for reporting*. Control turns the third one round. A request written into the pane
would be read off the stream the pane's output arrives on, and anything that can print can put
anything there — `cat` of a file, a `git log`, an LLM streaming its answer, a host at the far
end of `ssh`. A forged status paints a dot; a forged request is escape-sequence injection with
the window's authority behind it, and the moment a verb opens a tab or types into a pane it is
a path to running commands. Completion's OSC is safe only because Crook asks first and the
shell's reply carries nothing but the request's number. So status stays on OSC 6340, unchanged,
and the window answers on a socket, paying for it with the address — `CROOK_SOCKET`, in every
pane's environment beside `TERM_PROGRAM` and `CROOK_PANE_ID` — and with the stop at this
machine, which for control is the point.

**Who can connect: this user, on this machine.** There is no TCP listener and there will not be
one, which is what keeps the README's promise that nothing touches the network until asked. The
socket is `<pid>.sock` in a directory of its own: `$XDG_RUNTIME_DIR/crook-control` when that
variable names an absolute, real directory this user owns, and `crook-control-<uid>` in the
temporary directory otherwise — the per-user `$TMPDIR` on macOS. The directory is made `0700`
and the socket `0600`, and a directory that is already there is refused — no socket, a line in
the log — when it is a link, not a directory, somebody else's, or open to anyone else, because
whoever made it first could otherwise leave a socket in it that answers the CLI with lies. It is
not a corner of the shell integration's scratch, which is swept of entries a week old, and a
window can be open longer than a week. The listening socket and every accepted one are
close-on-exec, so no shell a pane starts inherits them.

**What it exposes.** What the panel shows, per pane: its number and its tab's, its title and
its tab's, its group, whether it is the focused one, the agent's status and message, its
directory and its branch, and `asking: true` while its agent waits on a question it asked
through Crook that the window could answer — left out otherwise, and never what it asks, which
the row and the card already show — and nothing of a pane's scrollback and nothing of its input. All of that
is already on the screen, in the session file, and in the shells' own process entries, every one
of them this user's to read, so a socket only this user can reach that only reads gives nobody
anything new. That is why `pane.list` needs no token: a listing any process of this user can
already put together is not a thing to guard, and a script outside every pane is answered as it
always was.

**Authority by lineage: the token.** Opening a tab is not reading. Anything this user runs can
connect to the socket — a script, a cron job, a tool in another terminal, a helper some other
program left listening — and none of those is a piece of work somebody opened a pane for. So
every pane's shell is handed a secret of its own, `CROOK_TOKEN`, beside `CROOK_SOCKET`: 32 bytes
from the operating system's random source (the `getrandom` the TLS stack already builds with),
minted when the shell opens, known to the window alone, and gone with the shell. A request
carries it, the window finds the open pane that holds it, and that pane is the caller. A request
with no token, an empty one, or one no open pane of this window holds is refused `tab.new` as
`unauthorized`, and may still list. The pane's number is not a credential and never becomes one:
every listing prints it. The token is in the environment of everything the pane runs, so
whatever the agent starts — a tool call, a sub-process, a script it wrote — can open tabs as the
agent. That is the boundary meant: a pane is one piece of work, what runs in it acts for it, and
the budget below counts it all as one. What the token is not is a wall inside this user's
account. A process of this user that sets out to can read another's environment
(`/proc/<pid>/environ`), and could as well write to the pty or run the command itself; the token
keeps a stray client from opening tabs by connecting, and makes every tab that opens one a pane
answers for. It does not make one user's processes strangers to each other, and nothing on one
machine under one user could.

**Opening a tab.** `tab.new` opens one tab in the caller's window — the socket a pane is told
about is its own window's, and no token names a pane of another — beside the caller's tab, in
its group when `in_my_group` says so (a group of the two is made when there is none), and never
selected: the person may be typing, and a tab that took the keyboard would take the rest of
their line with it. With `worktree`, the window makes the checkout the tab menu's creator makes
— the branch name trimmed and required and otherwise git's to judge, the checkout filed under
Crook's store by repository, `git worktree add -b` from the caller's `HEAD`, locked and given
what `.worktreeinclude` names, through the one function the creator makes its checkouts with —
on the pool, and the tab opens in it when git is done. The command arrives as words, and the
window joins them with the quoting the plugin host types an argument with, which is proven to
read back word for word in `sh`, `bash`, `zsh` and `fish` (#342). A new pane that would run any
other shell is refused as `unsupported-shell`, and a word holding a control character as
`bad-request`, since a newline in a command line is a second command; the refusal says to put a
prompt with lines in a file the command names, rather than escape it in a second quoting with
rules of its own in each shell, and its example gives the worker leave to read that file and no
other — `claude --allowedTools "Read(/$PWD/task.md)" -- "do the task in $PWD/task.md"` — since a
`--worktree` tab starts in another directory and Claude Code asks before reading outside its
own. It is not `--add-dir "$PWD"`, which lets the worker write in the lead's checkout too, and
under `acceptEdits` without asking, undoing what `--worktree` is for. A word arrives as itself:
an alias is not expanded, `&&` is an argument, and a pipeline is `sh -c '…'`. The line is typed
into the new pane's field at once, where a person can see it, and sent by the field's own submit
at the shell's first prompt — the first `A` for a shell with marks, since a line submitted into
the block open from before it is one the shell never reports a boundary for, and the first thing
it prints for a shell without. That is an event and not a clock, so a shell whose integration
never marks a prompt keeps the line in its field for a person to send, and a person who edits
the line before its prompt owns it, and it is not sent. The reply is the new pane's number, its
tab's and its directory, and a `note` when the worktree was not given everything its
`.worktreeinclude` names — left out when there is nothing to say, so the reply is the three
fields it always was. The new row's hover card and its "Why this status" say which pane opened
it.

**The budget, and refusals.** Eight tabs at once on behalf of one pane a person opened. The
count is by *root*: a tab opened by a tab that was itself opened counts against the pane the
chain began at, so a worker that opens workers spends its lead's eight rather than eight of its
own — counted per caller, a loop would be eight tabs each opening eight more. Tabs still waiting
on git count, and a tab that closes gives its place back. Refusals are bounded as a plugin's
are: sixteen in a row and the window stops answering that pane's `tab.new` until it closes, as
`too-many-refusals`, with a line in the log at the sixteenth; every refusal before it is logged,
including a worktree git would not make, and only a tab that opens starts the count again —
never once the pane is stopped, since a worktree agreed to before the stop can still open after
it and must not lift it. Requests with no token are logged sixteen in a row and then refused
quietly, since there is no pane to stop answering. What comes after keeps to the rule these
start: a caller may act on its own pane and on the tabs it opened, and reading or typing into a
pane a person opened is a grant that person answers on a card. An agent that reads one pane and
types into another is a confused deputy waiting for a prompt injection, which is why that is
never a default.

**Watching what it opened: authority by lineage.** `pane.wait`, `pane.blocks` and
`events.follow` read what a pane's agent says, what its commands printed and when they ended.
`pane.list` needs no token because everything in it is already on the screen; a block's output
is more than that — it is what an agent reading it would take instructions from — so these are
scoped the way typing will be. The caller, found by its token, may watch its own pane and the
tabs it opened, and the tabs those opened: up the chain of lineage through the panes still open
and the closed tabs the window remembers, or directly when a pane's lineage names the caller as
the root the chain began at, so a lead still watches a worker's worker after the worker in
between has closed. Nothing else: a pane a
person opened, or one another pane's agent opened, is refused as `needs-grant`, with a sentence
saying that watching it needs a grant from the person who opened it, which this version cannot
ask for. The grant card belongs to the verb that types, and until it exists the answer is no.
Refusals come in an order that tells a stranger nothing: no pane's token is `unauthorized`
first, then a number no open pane has is `no-such-pane`, then `needs-grant`. A closed pane is
`no-such-pane` to everyone, except to a `pane.wait` from a pane that may watch it — the one that
opened it, or any pane up that chain (below).

**`pane.wait`** answers when the pane gets to `until`: `idle` — its agent said so; the idle a
pane starts in, before anything has reported, is not it, so a worker whose agent has not
started is not taken for one that has finished, and nor is the `✳` Claude Code's title wears
from the moment it starts (Gemini CLI's `◇`, codex's folder with no spinner before it), which is
a report only once the title has said running (until then it is `StatusSource::TitleAtStart`,
which says an agent is there and nothing more), and nor is the idle a title's question ends in
when what replaces it says nothing outright — codex's folder with its animations off, its spinner
where the pane runs it through `sh -c` or an alias — since the agent works on after an approval
(`StatusSource::TitleStoppedAsking`, until the end of its command makes it a `CommandEnded`,
which counts) — `needs-input`, which a Gemini CLI or codex title says as well, `finished` or
`exited`.
`finished` is a state and not an event: nothing is running, no line is waiting for the shell's
first prompt (a tab `tab.new` just opened has not run its command yet), and a command has been
closed by the shell's own `D`, whose exit status the answer carries — a command's, not an empty
line's, which a shell ends with a `D` too. That is what keeps a
command that ended before the wait was asked from being missed, and a tab that has not started
from being taken for one that has finished; it is refused as `no-blocks` for a shell with no
marks, where nothing ever says a command ended. A pane cleared by hand (Clear blocks) between
the command's end and the wait still answers: with no command left in its history, the newest
one the clear let go of is the one that finished, exit and all. `exited` is the pane closing by
whichever road.
A state already reached answers at once, and a pane that closes first answers at once with
`reached: false`. A pane that closed before the wait was asked answers at once too: the window
remembers who opened the last 256 tabs `tab.new` opened that have closed, whether anybody was
watching or not, and a wait on one the caller may watch answers `closed: true`, `reached` for
`exited` only — so `crook pane wait 7 --until exited && …` does not depend on whether the worker
ended before the lead got round to asking. A wait is at most an hour, which is also the
default; `timeout: 0` answers with where the pane is now. The answer is `{pane_id, until, reached, status, message, exit,
closed}` — and when the time runs out, the connection asks the window once more with no time to
wait, so a script reads the state the pane was left in rather than a bare `timeout`.

**`pane.blocks`** answers the newest `last` finished commands, a hundred at most and all of
them up to that by default: the command (display-only, as `Block.command` warns), exit status,
directory, duration, and output — the text a copy of the block's output gives, through the same
region a drag over it makes. Every part is capped, since it is built between two frames and
written down a socket: the last 4096 rows of a block are read, the last 64 KiB of those kept,
and 512 KiB in all, spent newest first; cut from the front, at a line, and marked `truncated`,
because the end of what a command printed is where its errors and its summary are. Only
commands' blocks count — one the shell ran, sent or started or ended with a status — and not
what a shell printed on its own before its first prompt or between two, nor an empty line sent
from the field, which the block tracker gives no start. A command still running
has not finished, whatever the pane draws it as: a build on the primary screen, a full-screen
program on the alternate one, an agent's interactive TUI. With nothing finished such a pane is
refused as `no-blocks` with a sentence saying which — wait `--until finished` for a command,
and a screen is never a block — rather than answered with an empty list that reads as "nothing
ran"; with finished commands from before, it gets those and `running: true`. A pane at rest whose
commands a clear let go of is refused the same way, with a sentence saying the clear took what
the last one printed and how it exited, since `pane.wait --until finished` has just said it
finished; one running something since says nothing has finished since the clear. A worker whose
answer is meant to be read runs headless.

**`events.follow`** answers `{panes}`, what it follows as of now, then one line per event until
the client hangs up: `status` (only when the status or the message changed, or an idle became
one `pane.wait` takes — an agent reports `running` around every tool it calls — and never an
idle `pane.wait` would not take, so a question a title stopped asking is not told as a turn that
ended until its command ends), `started`, `finished` with the command line, exit and duration,
`opened` for a tab the caller opens later, `closed`, `lagged`, and `asked` and `answered` for
the questions its agent asks through Crook (see "Answering an agent"). `started` is read
off a frame the pane's terminal published, since there is no event for a command running: one
that starts and ends between two frames — `true`, `git status` — is never seen running, and its
`finished` is what names it. With `pane` it follows that one pane,
and the stream ends after the `closed` of the pane it is anchored on — the one named, or the
caller's own. The events come from where the workspace already applies what a pane's shell did
(`apply_terminal_update`), a status from `agent_reported`, which every status is written by —
a hook's report, the title's glyph, and the end of a command taking either back — and from
`settle`, which every road a pane opens or closes by goes through: no poll, no timer, and
nothing done at all while nobody watches.

**`pane.split`** opens a pane rather than a tab: split off the caller's own pane, or off one it
may watch (`beside`), `right` or `down`, by the rule a person's split keys follow, and never
focused — the anchor's group grows a pane beside it and the focus, the zoom and the active tab
stay where they were (`TabStrip::split_beside`, which is not a `TabAction`, since no key, click
or menu opens a pane nobody is moved to). It is `tab.new` in every other way: the token, the
command words quoted for the new shell and typed at its first prompt, a `title`, a `cwd`, the
lineage, the same eight panes on behalf of the pane a person opened, and the same run of
refusals. A split has no `worktree`: it shares its tab with the pane it is split off. Both verbs
take a `cwd`, an absolute path the window takes as given — a look at the disk is not made
between two frames, and a directory that has gone is one a shell falls back from as a restored
pane's does — which the CLI and `crook mcp` check is a directory before asking.

**`pane.close`** closes a pane the caller may watch, less its own, and its tab with it when it
was the tab's last pane: what a lead does with a worker whose answer it has read. Refused as
`own-pane` for the caller's own pane, which ends by its shell exiting; as `not-opened` for a
pane a person or another agent opened; as `working` while anything in the pane is working —
`AgentSession::is_working`, which is the close question's own rule, or a question its agent's
hook is waiting on — and as `focused` for the pane a person is in. A close that would end work
is one Crook asks a person about, and a request is never that person: the verb never puts the
question up, and what it closes is only ever what would close with no question at all.

**Kept connections.** A wait with time to wait, and a stream, keep their connection, and neither
fits a five-second deadline or one of the eight places a question is answered in: a lead
waiting on each of its eight workers would leave its own `pane list` refused as `busy`. Such a
connection moves to a lane of its own, thirty-two at once and the next refused as `busy`. Before
it asks, its thread makes a bounded feed; the window registers the feed beside what was asked
and pushes into it on the main thread — a lock and a notify — and the connection's thread is the
one that blocks. A stream's feed holds 256 events, and a reader that falls further behind loses
the oldest and is sent `{"event":"lagged","dropped":N}` where they were, rather than the window
holding everything for a client that is not reading. A second thread per kept connection blocks
on a read of it, to see the client hang up — a script killed at its own timeout, an agent's tool
call cut short — and ends the wait then rather than an hour later. A client that only closes its
half after the request — `nc -N` — is still reading: on Linux the read's end is followed by a
`poll` that asks for nothing, which only a close of both ways wakes, and the wait is answered;
elsewhere nothing that blocks tells the two apart — macOS's `poll` says `POLLHUP` for both — so
the end of what the client says is taken for its leaving, and a client of a kept verb keeps its
half open until the reply comes. The connection answers
nothing after the verb, and closes when it is over; a window that closes ends every feed, and a
wait still open is refused as `gone`.

**The transport.** One socket per process, because every `crook` launch is its own process with
one window. At startup the name is probed: a socket there that refuses a connection was left by
a Crook that crashed, and is removed and taken; one that answers belongs to somebody alive —
another PID namespace sharing the directory — and the next name, `<pid>-1.sock`, is tried. Other
refusing sockets in the directory are swept once they are a minute old, the minute being what
keeps a sweep off a socket another Crook bound a moment ago and has not started listening on.
The socket is removed when the window closes. One OS thread accepts, because it blocks for as
long as nobody connects — the reason a pty reader is a thread and not a pool worker — and each
connection gets a short-lived thread of its own, eight at once and the ninth refused as `busy`.
A connection has five seconds all told and a line 64 KiB; a longer line is refused as `too-long`
and the connection closed, since nothing says where the next line starts. The one exception is a
tab in a new worktree, which waits on git, its `post-checkout` hook, the lock and the
`.worktreeinclude` copy: that reply is given every deadline the making of the checkout runs
under, an allowance for the copy and a margin, and the connection closes after it. A question that needs
the window is posted to it and answered on the main thread through the `ctx.spawn` a pty's
output comes home by, while the connection's thread waits for the answer until a quarter of a
second before its five are up — the quarter being what it writes `timeout` back in, so a window
that could not answer says so rather than hanging up: the UI thread never blocks on a socket,
and a client that stops reading costs its own thread and nothing more. **Windows has no socket
yet**: nothing listens there, and the CLI says the command is not available on that platform. An
owner-only named pipe with `PIPE_REJECT_REMOTE_CLIENTS` is the route when it comes.

**The protocol.** Newline-delimited JSON, one object a line each way. A request is
`{"v":1,"verb":"pane.list"}`, or
`{"v":1,"verb":"tab.new","token":"…","args":{"command":["claude","…"],"worktree":"fix-x","in_my_group":true,"title":"…"}}`,
or `{"v":1,"verb":"pane.wait","token":"…","args":{"pane":7,"until":"needs-input","timeout":600}}`,
`{"v":1,"verb":"pane.blocks","token":"…","args":{"pane":7,"last":1}}`,
`{"v":1,"verb":"events.follow","token":"…","args":{"pane":7}}`,
`{"v":1,"verb":"pane.split","token":"…","args":{"command":["make"],"direction":"down","beside":7}}`,
`{"v":1,"verb":"pane.close","token":"…","args":{"pane":7}}`,
with an optional `id` of any JSON value the reply echoes and an optional `min_version`. An
argument a verb does not take is refused as `bad-request` rather than ignored, since ignoring a
misspelt `worktree` would open an agent in the caller's own checkout. A reply is
`{"v":1,"ok":true,"result":…}` or
`{"v":1,"ok":false,"error":{"code":…,"message":…}}`. Every reply carries `v`, the version the
window speaks. A request's `v` is the version it was written against: a window refuses, as
`version`, one older than the oldest it still answers — 1 today — and one whose `min_version` is
newer than the window, so a script written against a later verb is told to update Crook rather
than handed a half-understood answer. A verb is a promise that is hard to take back, so within a
version verbs and fields are only ever added, and a field that changes meaning is a new version.
The codes are `bad-request`, `too-long`, `unknown-verb`, `version`, `busy`, `timeout`, `gone`,
`unauthorized`, `budget`, `too-many-refusals`, `unsupported-shell`, `failed`, `no-such-pane`,
`needs-grant`, `no-blocks`, `declined`, `own-pane`, `not-opened`, `working` and `focused`. An `events.follow` stream's lines after the first reply
are events, `{"event":"status","pane_id":7,"status":"needs-input","message":"…"}`, not replies.

**The CLI is the other end.** `crook pane list` prints a table, and `--json` prints the window's
own array with every field in it, so a field a newer window adds reaches a script through an
older CLI. It asks `$CROOK_SOCKET`. An empty one means the pane's Crook could not open its
socket, and a `CROOK_PANE_ID` with no `CROOK_SOCKET` beside it means a Crook from before there
was one; both are refused rather than looked around, since either way the window the pane is in
cannot be asked. Outside every pane it takes the one live socket in this user's directory when
there is exactly one, and refuses when there are several, naming them: each window is its own
process, and choosing the newest would be a guess that hands a script another window's panes.
Whichever way it found the socket, the CLI asks it only once it has been checked, since an
inherited variable proves nothing about what is at the name now: the path absolute, its
directory passing the window's own check, the socket a socket and not a link, this user's and
`0600` or tighter, and — once connected — the process listening this user's, by `SO_PEERCRED`
on Linux and `getpeereid` on macOS, since a name can be swapped between a look and a connect. A
socket that fails is not connected to, and the refusal names the rule it failed.
`crook tab new [--worktree B] [--in-my-group] [--title T] [--json] -- CMD…` prints the new
pane's number, or the window's answer with `--json`; the `--` is required, since a command is
the part of the line most likely to hold a word that starts with a dash. A `note` in the answer
goes to standard error beside the number, so `id=$(crook tab new …)` still reads a number and
whoever ran it still sees what the worktree was not given.
`crook pane wait <ID> --until <STATE> [--timeout <SECS>]` prints where the pane got to and exits
with a failure, having printed where it is, when it did not get there, so `&&` after it reads
the way it looks; `crook pane blocks <ID> [--last <N>]` prints each block as its command line,
its output and a line of how it ended; both take `--json`. `crook events --follow [--pane <ID>]`
prints the events as one JSON object a line, always, since what reads a stream is a program.
Every request carries `$CROOK_TOKEN` when there is one, and the CLI refuses nothing on the
token's account: the window is the one place that knows what a token is worth.
A reader that stops early — `crook pane blocks 7 | head -5` — is the end of the answer, and the
command succeeds: every answer printed without a window goes through one function that takes a
closed pipe as done, where `println!` panicked with exit status 101, since Rust leaves SIGPIPE
ignored and the write fails with EPIPE. `events --follow` writes its stream itself and stops at
a closed pipe too, and the app's crate warns on `println!`, so that a new answer cannot go round
the function.
What it prints carries no control character. A pane's directory arrives percent-decoded from the
OSC 7 its shell printed, so `%1b` in it is an ESC, and a listing that printed what it was given
would replay a pane's escape sequence into the terminal the listing runs in; the table writes
every control character out as its escape, and `--json` escapes the DEL and C1 characters a JSON
encoder leaves as they are. A block's output printed as text keeps its newlines and writes out
every other control character the same way, and an event line is escaped as `--json` is.
`crook pane split [--right|--down] [--beside ID] [--title T] [--cwd DIR] [--json] -- CMD…` prints
the new pane's number as `tab new` does, and `crook pane close <ID> [--json]` prints `closed`.

**`crook mcp`: the same verbs as MCP tools.** A stdio Model Context Protocol server — JSON-RPC
2.0, a message a line, the log on stderr and nothing but JSON-RPC on stdout — that an agent's
MCP client starts as its child, so it runs with the pane's `CROOK_SOCKET` and `CROOK_TOKEN` and
asks as that pane: `crook_tab_new`, `crook_pane_split`, `crook_pane_list`, `crook_pane_wait`,
`crook_pane_output` (`pane.blocks`), `crook_pane_close` and `crook_events_wait`. There is no
second way into the window: each tool is one request through `cli::unix`, checked as the CLI
checks the socket, and answered by the window's own rules, so an agent gains nothing it could
not do with `crook tab new` and `crook pane …`. No tool types into a pane, none answers an
agent's question — the socket has no verb that does — and the descriptions the model reads say
both. It is hand-written on `serde_json` rather than an SDK: the protocol a stdio server needs is
four methods and two notifications, every MCP crate pulls an async runtime this app does not
have, and the dependency policy at the top of the workspace manifest is that a crate earns its
place. Both eras are answered: `initialize` for the handshake revisions up to `2025-11-25`, and
`server/discover` and per-request `_meta` for `2026-07-28`, with `-32022` for a version it does
not speak. A tool call runs on a thread of its own, sixteen at once, so a wait does not hold up
the calls behind it. A wait waits 55 seconds unless asked for longer, at most 600, and sends
`notifications/progress` every ten seconds to a call that carried a `progressToken`, which most
clients take as a reason not to cut it off; `notifications/cancelled` hangs its connection up,
which the window hears as the client leaving, and so does the client closing standard input,
after the calls answered at once have been. `crook_events_wait` keeps one `events.follow`
connection from its first call on, holding up to 256 events between calls, so nothing is lost
while the model is busy elsewhere. Every string in an answer has its escape sequences taken out
and its other control and bidirectional characters written out, and a block's output comes
between two fence lines tagged with eight random bytes, under a line that calls it untrusted
program output. Outside a Crook pane — no `TERM_PROGRAM=Crook`, no `CROOK_PANE_ID` — the server
still starts and lists its tools, and every call says it is not in a pane, so a plugin installed
everywhere costs a session elsewhere nothing; on Windows every call says there is no socket yet.
The Claude Code plugin declares it in `packaging/claude-code/.mcp.json`, started through `sh` as
`$CROOK_BIN mcp` or `crook mcp` from `PATH`, and a missing binary is a server that failed to
start, with a line on its stderr.

### Answering an agent

When Claude Code is about to ask a person whether it may run a command, it runs its
`PermissionRequest` hook first, hands it the tool and the tool's input on stdin, and reads what
the hook prints as the decision — or, when it prints nothing, goes on with the dialog it already
has open. `crook --agent ask claude` is that hook. It hands the window the pane is in the whole
request and waits on its connection for what a person says there. The window holds the question
for as long as the hook waits, and shows it — on the row's second line while the pane needs
input, and in "Why this status", in the terminal's family and wrapped at any character so that no
line it shows is cut, the first twelve lines and 1,024 characters of it with what is left counted
in words — and a person who opens its card can answer it Allow once or Deny, while Claude Code's
own dialog stays up beside it; whichever is answered first wins. Claude Code runs it as its
`PermissionRequest` hook, from the plugin and from `crook --agent-hooks claude`, with a `timeout`
of 330 seconds: Claude Code's timeout is the one that kills the hook, so it comes after the hook's
own five minutes and a little more (`CLAUDE_ASKS` in `app/src/agent.rs`). The code is
`app/src/agent/ask.rs` and `request.rs` on the hook's side, `app/src/control/ask.rs` on the
window's, and `app/src/workspace/answering.rs` for the card.

**Status stays on 6340.** The hook reports no status, and no status hook's command changes. The
row still learns that Claude Code is asking from the notification hook and the title, exactly as
before; the question is a record beside the status, never a rewrite of it.

**The question rides the socket, as a kept verb.** `agent.ask` carries
`{agent, session, cwd, tty, wait, request}` — the request
`{"kind":"command","command":…,"note":…,"timeout_ms":…,"background":…,"outside_sandbox":…}`,
`{"kind":"edit","path":…,"edits":[{"old":…,"new":…,"replace_all":…}],"resolves_outside":…}` or
`{"kind":"write","path":…,"content":…,"resolves_outside":…,"replaces":null|{"lines":…,"bytes":…,"more":…}}` —
with every unknown field refused at every level, so a window never shows less than will happen,
and a window from before a kind refuses it and its hook passes. The
request is what the hook read through an allowlist of Claude Code's keys for the tool: `Bash`'s
(`command`, `description`, `timeout`, `run_in_background`, `dangerouslyDisableSandbox`), `Edit`'s
(`file_path`, `old_string`, `new_string`, `replace_all`), `MultiEdit`'s (`file_path` and `edits` of
the last three) or `Write`'s (`file_path`, `content`), each with its type: a
key Crook has never heard of may change what runs, so it means no question at all, and the
agent's own prompt is the only one. A command is 1 byte to 48 KiB with no NUL, a note at most 1
KiB; a path is absolute, at most 256 characters written out, with no control character and no
part of it starting or ending with a space, and a change holds no NUL and is at most the 3000 lines the Changes column draws a diff to, counted as
the card draws it; and a line over 64 KiB is not asked. The card draws an edit's old text as
removed lines and its new text as added ones, in the Changes column's colours, a tab as a muted
`→` and a space a line ends in as a muted `·`, and a write's content as added lines under
"Creates this file" or "Replaces a file of N lines (M bytes)". The window reads no file: the
hook, a process of its own with Claude Code's 330 seconds, looks for at most a second
(`agent::on_disk`) and sends what it found as the two required fields. `resolves_outside` is
whether the file, followed through every link on the way to it — the deepest part of the path
that exists, resolved whole, at most 64 parts — is not under the working directory, also
resolved, and is true for anything it could not follow; `replaces` is the file a write would
replace, its lines counted over at most 4 MiB, or `null` for none, and a write whose file could
not be looked at in time is not asked. Being required, both are refused by a window from before
them, and its hook passes. They describe the disk when the agent asked, not the request, so the
digest and the done fingerprint leave them out: the hook after the tool reads the same input
with nothing looked at, and has to name the same question. A path not under the agent's
directory by its text — a `..` in it counting as outside — or through a link, as the hook found
it, is warned about above the change. Questions have a lane of their own, sixteen at once and
four in a pane, counted apart from the thirty-two waits and streams. The window answers
`{"asked":<id>}` within a second; later it writes exactly one
`{"answer":{"decision":"allow"|"deny"|"pass","message"?:…,"why"?:…}}`, or a `gone` refusal; the
hook sends back at most one line, `{"delivered":true}`, once it has printed an answer, and the
window reads no more than that one line: anything after it is let go unheard until the hook
leaves. A
question's id is never handed to another, and its digest — SHA-256 over `crook-ask-v1`, the
session, the tool and the request as JSON with its keys sorted by hand — is what an answer will be
checked against, so a press can only ever reach the question it was given about.

**The binding is the terminal a status would be written to.** The hook's controlling terminal,
or, when it has none — Claude Code starts every hook in a session of its own — the terminal of
the first process outside its session, which is the program that ran it: the walk `--agent`
already takes to find the pane (#388), returning the device number it reads rather than opening
anything. The window matches that number against each pane's pty, whose slave's `st_rdev` it read
when the shell started, in the low 32 bits Linux's `tty_nr` and macOS's `e_tdev` carry. So a
question reaches the pane its agent's status reaches, and no other: a `claude -p` that another
agent's Bash tool runs is in a session with no terminal and reaches no pane, as its status does.
Not the token, which every process a pane starts inherits — a nested agent, and a tmux server
started in a pane together with every window of it — and which would put the inner agent's
question on the outer pane; the token is only a hint, and one naming another pane makes the
question read-only. Not files in `CROOK_SCRATCH`, which only a marked shell has, which go stale
inside tmux, which tell the window nothing when the hook is killed, and which a killed hook leaves
behind. Not a nonce printed as an OSC and read back, which costs a round trip through the
emulator and a deadline, and which anything that prints can replay.

**Inside tmux the binding is tmux's clients' terminals.** There the agent's terminal is one of
tmux's panes, which no Crook pane is on, and the server's `CROOK_SOCKET` and `CROOK_TOKEN` are those
of whichever pane started it — another window's, or one since closed. So the hook takes the route a
status takes there (#516): `tmux` asked once, under a second, for the pane's terminal and the
clients attached to its session, the pane's terminal checked to be the agent's own. Each client's
terminal that is a character device this user owns gives its `st_rdev`, low 32 bits — at most
eight — and the question carries `multiplexer: {kind: "tmux", clients: [u32], pane: "%N"}` in place
of `tty`, and no token; the pane id is checked as `%` and digits and only shown. A window from
before the field refuses it, and the hook passes. The window binds it to the first open pane, in
the panel's order, whose pty is one of the clients, and skips the token and lineage checks, which
say nothing here: the tmux server is not below that pane's shell. What it checks instead is the
kernel's account of the asker's ancestors, read on the connection's thread (`agent/holders.rs`,
at most 16 of them and 1024 descriptors each): one of them — never the asker itself — has to hold
the pane's pty slave open and run the same executable, by device and inode, as the leader of the
pane's foreground process group (`tpgid`, from `/proc/<shell>/stat` or `e_tpgid`). tmux's server
is that ancestor for every process in its panes; a process that only names the terminal, one
under a program that opened it, and one under a tmux that is not the pane's foreground or holds
another pane's terminal are each shown read-only, "not from the tmux whose client is in this tab".
Questions from inside tmux are counted apart from the pane's own four, never replace one, and come
after one on the row and the card. A remote pane still makes it read-only. The row, the list and "Why this status" say "Bash inside tmux: …", and the card names
tmux's pane and "inside tmux (also attached in N other tabs)" when other tabs hold a client of the
same session; Go to tab goes to the pane holding the client. No window is told the request until it
has said it holds one of those terminals: the hook first sends `pane.find {ttys: [u32]}` — a verb
with no other field, no token and no kept connection, answering `{found: bool}`, and `false` while
answering is off — to `CROOK_SOCKET`, and when that one is gone or says no, to at most four other
sockets that connect in this user's checked control directory, in name order, all within two
seconds; the question goes down the connection of the first that said yes, and nowhere else. A
hook with no `CROOK_SOCKET`, or whose tmux names no client, asks nothing. screen and zellij cannot
be asked where their clients are, and pass as before. Subagents pass inside tmux as everywhere.

**What the window checks.** The kernel must say the process at the other end is this user's, or
the question is refused as `unauthorized`. On the connection's thread the window reads that
process's ancestors, at most 64, and a question whose asker is not below the pane's shell, or
whose pane is working on another machine, is held and shown read-only. A read-only question is
one the window cannot place, so it is counted apart from the answerable ones in the pane's four,
never replaces an answerable one from the same session, comes after one in "Why this status", and
is never the row's second line. A person who turns
answering from Crook off ("Let agents ask in Crook", under Tabs on Settings' Appearance page,
`answer_agents_from_crook`, on by default) has every question refused as `declined`, at once,
before anything about the panes is said — and every question the window already holds ended as a
pass, `answering-off`, so the card up on one turns to say so and nothing held stays answerable;
the press checks the switch again besides.

**How a question ends.** A reader on the hook's half of the connection sees the hook go — Claude
Code kills it once its dialog is answered No or Esc, or its parent dies — and tells the window
through a notice that wakes it, so the question leaves the row the moment the hook does. A Yes in
Claude Code's own dialog tells the hook nothing, and it would hold on, the window with it, until
the turn ended; what follows a Yes is the tool running and then its PostToolUse, or its
PostToolUseFailure, whose `tool_input` is the request's byte for byte (C11 and C15 in
`docs/answering.md`). So the hook after every tool, the one that already says `running` and looks
for a pull request, also writes `OSC 6341 ; done ; <fingerprint> BEL` after the status, in the
same write — for a `Bash`, `Edit`, `MultiEdit` or `Write` call of the main thread with a session
the wire takes and an input the allowlist reads whole, the fingerprint being the first half of the digest in 32 lowercase hex
digits, computed from that input as the window computed it from the question — and the window
ends the questions held in that pane with that fingerprint as `answered-in-agent`, which its
history says as "Answered in the agent's own dialog". A tool a person interrupts runs PostToolUseFailure
with `is_interrupt`, and ends the turn with no Stop, so that one writes the done alone: a
`running` after the title's `✳` would leave the row running. No hook's command changes for it;
PostToolUseFailure is a new hook with a command of its own, `running --done -`, which reads the
input for the done alone and finds no pull request in it. Not PostToolUse's command, because a
Crook older than the done would read that as PostToolUse and say `running` after the interrupt;
every release since 0.1.13 refuses a flag it does not know before it writes anything, so an older
binary handed `--done` writes nothing, and the hook's `|| exit 0` keeps that quiet. The emulator hands every done in a read over, eight at most and
never as a repeat dropped, ahead of the status from the same read, so a question has gone before
the running that answered it arrives. Only an id ever travels on 6341, never the request, and
anything that is not exactly the three pieces — another word, a capital, a digit short or over, a
fourth piece — is no sequence. Anything in the pane can write one, and that is safe: a done can
only withdraw a question from the window, never answer one, it names only a question in the pane
it was written in, and a withdrawn question leaves the agent's own dialog the place to answer, as
it is for a question the window never held. A "No, and tell Claude what to do differently"
runs no tool and no PostToolUse, so its question still waits for the turn to end or a newer one
to replace it. Its time running
out (its wait and five seconds) is a pass, `expired`. Its pane closing, the agent's command
ending, the agent's own hook saying the turn is over or failed, and a newer question from the same
session in the same pane are each a pass, with the reason as `why`. A window that closes tells
every waiting hook `gone`. A connection that stopped waiting for the window to say it held the
question — a frame stalled past the connection's patience — has its question refused if the window
has yet to hold it, and hidden and then dropped if it has. The title's `✳`, which Claude Code draws as the dialog opens, and a
hook's `running`, which parallel tools and subagents say while the dialog is up, end nothing.
Each ending is one line in the log — `ask 3 claude Bash: turn-ended after 4210 ms (sha256
1f2e3d4c5b6a)` — and an entry in a history of the last 64, in memory only.

**What never carries the command.** `pane.list`, `events.follow`, `pane.wait`, `session.json`,
desktop notifications and the log. The row's second line says it in a line, through the same
writing-out "Why this status" uses: newlines kept, tabs expanded, a carriage return as `␍`, and
every other control, DEL, C1, bidirectional, zero-width, format and default-ignorable character,
and every space but the plain one, as `⟨U+XXXX⟩`, so a
command that hides `rm -rf ~` behind a carriage return or reorders itself with a right-to-left
override shows the trick rather than doing it. A question's `Debug` prints the agent, the kind and
a twelve-digit digest prefix.

**The card.** A question arriving opens nothing and takes no focus. The card opens from a
gesture only — a row menu's "Answer…", offered only on a row whose agent's hook is waiting, and
"Answer waiting agent" (the palette, the header's "M to answer" before "N waiting", and the
suggested chord `cmd-i`, `ctrl-shift-i` off macOS) when the window holds one question; with more
it opens the list in `workspace/answer_list.rs`, the popup's other face. Its rows, oldest first and
appended at the end, open a card and nothing else, so the card's gates start fresh each time;
Escape walks back from the card to the list to nothing, an answer sent brings the list back and
opens no other card, the keys held after a card close are kept from the list too (but for
Escape), and an emptied list closes. Its rows have no handler for a second after it comes back
from a card or any row comes or goes, so the second press of a double click on Deny, or a press
aimed at a row that has just moved, opens nothing. The header chip draws "to answer" first, so a
"N waiting" turning up never takes its place; neither half takes a press for a second after the
chip changes shape (`workspace/header_chip.rs`, one one-shot wait started from `frame_drawn`), and
after "N waiting" focuses a pane the keys are kept from it for that second, as after Go to.
Below the rows it shows the last twenty outcomes, read-only.
The card keeps the close question's rules (`workspace/closing.rs`): Not now is the filled
default at the trailing edge, Escape, a press off it and Enter or Space on Not now close it
answering nothing, and Allow, Deny and Go to tab — Go to too, since it hands the keyboard to a
dialog whose own default is Yes — wait for the keyboard to have been still for a second after the
card came up, its face changed or the window took the keyboard, with every key typed too soon or
not the card's taken and the wait restarted; after Go to, the agent's tab is kept from the keyboard
until it has been still that second too, so a held Enter does not repeat into the dialog. For the
same second they are drawn with no handler,
and one one-shot wait makes them live with mouse state of their own, so a press begun on an inert
one cannot finish on a live one. A command longer than its twelve-line box, which builds only the
lines on screen, keeps Allow inert until its last line has been shown. The focused pane hears no
key while the card is up, ctrl-c included, and the close question beats the card: a close takes it
down, and it does not open over one. Allow and Deny are typed actions only, never commands, names
or answers the host knows, so no binding, palette row, `--action`, plugin, notification or socket
reaches them. Every press carries the question's id and digest and is carried out in one place,
`Workspace::answer_ask`, which checks again that the card is up on that question, the digest is
the registry's, the question is held and answerable, the time gates have passed and an Allow's
command was read to its end, and sends nothing otherwise. An answer sent closes the card, which
never moves on to another question, and leaves the row's status alone: the agent's next report
says what it did — a confined session drops a hook's allow. A deny's message is always "Denied in
Crook by the person at the keyboard."; there is no "always", no change to the request and no
interrupt. A question that ends under the card turns it to say Claude Code is no longer waiting
on it and why, with only Go to tab and Close.

**What was answered, and how.** The window remembers the last 64 questions that ended, in memory
and per window, as metadata and the line the row said them in — when, the agent, the tool, the
first line of the command or the file, and how each ended — never the request, and nothing of it
is written to disk. "Why this status" lists the pane's last five, newest first, and the inbox's
"Earlier" the window's last twenty, in one wording, `How::said` in `control/ask.rs`: "Allow
sent" or "Deny sent", then "and its hook took it" or "but its hook was gone", or what ended one
nobody answered here — "Timed out", "Answered in the agent's own dialog", "The agent's turn
ended". Never that the agent did it. A lead following the pane — the caller's own and the tabs
it opened, by the lineage a status is told by, through the same `Watches` — is told two lines
more: `{"event":"asked","pane_id":7,"agent":"claude","tool":"Bash"}` when a question the window
could answer is held, and `{"event":"answered","pane_id":7,"by":…,"decision"?:…}` when it ends,
with `by` one of `here` (a person pressed Allow or Deny on the card, and `decision` says which),
`in-agent`, `released` (answering from Crook was turned off), `expired`, `withdrawn` (the hook
went, the turn or the command ended, a newer question replaced it) and `closed`, told before the
pane's `closed`. Neither carries the command, the tool's input, the path, the directory, the
digest or the fingerprint, nor anything about a read-only question, which the window cannot
place. A lead sees that a worker waits, and must ask its person: no verb answers, and nothing a
lead sends types into the worker. `crook events --follow` prints each line as the window sent
it, so a client from before them prints them unchanged.

**What the hook prints.** One line Claude Code reads as its decision, or nothing, and it exits 0
on every path, since a hook that fails can fail its agent. It opens and writes no terminal.

| What happens | What the hook does |
|---|---|
| `CROOK_SOCKET` empty or unset | prints nothing, connects to nothing |
| `STY` or `ZELLIJ` set | prints nothing, connects to nothing |
| `TMUX` set, and tmux names no client of the agent's session this user owns | prints nothing, connects to nothing |
| `TMUX` set, and no window among `CROOK_SOCKET`'s and four others says it holds a client within two seconds | prints nothing; those windows were sent only `pane.find` |
| not `PermissionRequest` for `Bash`, `Edit`, `MultiEdit` or `Write`, a subagent's `agent_id`, no session or no absolute `cwd` | prints nothing, connects to nothing |
| `CLAUDE_CODE_ENTRYPOINT` not exactly `cli`, unset included | prints nothing, connects to nothing: under `claude -p` an empty answer is Claude Code's own deny |
| a tool input the allowlist does not read whole, a line over 64 KiB, stdin over 1 MiB | prints nothing, connects to nothing |
| no terminal found | prints nothing, connects to nothing |
| a socket that fails the CLI's checks | prints nothing, connects to nothing |
| an older window (`unknown-verb`), `busy`, `declined`, `no-such-pane`, `unauthorized`, any refusal | prints nothing |
| no `{"asked"}` within a second | prints nothing |
| a pass, `gone`, an answer with a field it does not know, garbage, the connection closing | prints nothing |
| its parent gone | prints nothing, within half a second |
| nothing after 315 s | prints nothing |
| `allow` | `{"hookSpecificOutput":{"hookEventName":"PermissionRequest","decision":{"behavior":"allow"}}}` |
| `deny` | the same with `{"behavior":"deny","message":…}` |
| stdout gone (EPIPE) | nothing more |
| Windows | prints nothing, at once |

Never `updatedInput`, `updatedPermissions`, `interrupt` or anything that would write a rule. An
unknown agent after `ask`, or a word after the agent, is one line on stderr, nothing on stdout,
and exit 0, which is how a hook a newer Crook writes reaches an older binary.

**No verb answers.** An answer will be a person's press on a card in the window, and it goes down
the asking hook's own connection and no other. A verb any process of this user could send would
be a way round the press, so there is none: `agent.answer` is an unknown verb, and a test holds
it to that. A process that makes a question up is answered about the question it made up.

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
lines is not something alacritty keeps. The emulator keeps one for its own anchors — it cuts a
full history back itself, see `BlockTracker::trim_history` — but no snapshot hands it out, so a
grid selection cannot follow it yet.
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

**On Linux the end of a drag is PRIMARY, and a middle press pastes it.** X11 keeps a second
selection beside the clipboard, and every Linux terminal both sets it from a selection and
pastes it on a middle click; `app/src/clipboard.rs` holds the argument. The release that ends a
gesture — `Output::release` for the output, `TextInput::release` for the composer and every
text field — offers what it selected, and nothing else writes it: selecting is not copying, so
the clipboard proper is untouched. A middle press that the program did not take (mouse
reporting off, or Shift held, the rule a drag already follows) dispatches
`WorkspaceAction::PastePrimary`, because the read is a question to whichever program holds
PRIMARY and can wait half a minute for an answer; the workspace asks on the pool, one read at a
time — a press while one is out joins it, unless a selection in Crook has taken PRIMARY since,
when it reads again and the old read's answer is dropped — and pastes when it comes home: into
the composer at its caret if one is drawn by then (added to the line, never over what is
selected in it), into the pty through the bracketed-paste path if not. A test build keeps
PRIMARY in memory, so a test run never overwrites the selection of whoever is running it; the
one test that reaches the desktop's own is ignored unless run by hand.

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
   away on the alt screen, on an overflowing block, once a command has been running longer
   than `LONG_RUNNING`, and while the shell's own line editor has the keyboard (rule 4), and
   every one of those is "the program is reading the keyboard now".
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
   interruptible — except the question a close asks, which takes all three: it has an Escape,
   so nothing is left uninterruptible, and a reflexive `ctrl-c` to get out of it would
   interrupt the very program it is asking whether to end.
4. **Everything else on the normal screen is the field's**, and a key the keymap has no
   meaning for does nothing rather than leaking into the shell — **except over an empty field
   at a prompt the shell has marked**, where it is a key the shell's own line editor answers
   and the field never did: Control with a letter the field does not use (`ctrl-r`'s history
   search, fzf's `ctrl-t`), a bare Escape (vi mode, and every shell's Meta prefix), and off
   macOS Alt with a character (`alt-b`, `alt-f`, `alt-d`, `alt-.`, fzf's `alt-c`). On a Mac
   Option types in the field as it does in every text field there. That key goes to the pty
   and hands the line editor the keyboard: the emulator's `line_editor` flag goes up on the
   snapshot, `pane_surface::of` takes the composer away as it does for a running command, and
   rule 2 sends every key after it to the shell — from the very next key, which both elements
   route against the live snapshot rather than the frame they were built in. The flag comes
   down only once the shell has finished a line, with `D` (it ran, or an empty Enter or
   `ctrl-c` ended it), and drawn the next prompt with nothing on it: the cursor on its `B`
   cell, and that cell blank. Not on a prompt drawn again, because bash redraws its prompt,
   marks and all, with the match a search kept or the path fzf picked on the line after it —
   and a composer back over a line the shell still holds would send the two as one — and
   because a prompt drawn again over an empty line can still be a line editor that is not
   done: vi's normal mode after an Escape, which fish, a zsh theme and bash's
   `show-mode-in-prompt` all redraw the prompt for, or a Meta prefix waiting for its key when
   a resize redraws it. Not on `C`, which an older copy of `crook.bash`, or another
   terminal's integration, reports for a `bind -x` widget like fzf's `ctrl-t` while it puts a
   line on the prompt. And not on a `D` the shell has a line for: what was typed while the
   command ran went to the shell, which draws it on the next prompt only after the prompt
   itself, in a write of its own — so the read the prompt arrives in shows an empty line, and
   the cells cannot say. `Terminal::write` tells the emulator what it sent instead, and while
   the line editor has the keyboard and the shell is away from its prompt the emulator counts
   it: a prompt for each line ended with an Enter, which the shell will run, and one for the
   start of a line after them, which it will keep. Each prompt after a `D` takes one of them
   and keeps the keyboard. `ctrl-c`, `ctrl-\` and `ctrl-z` throw the count away with the
   input the line discipline drops for them, and keys typed to a program that asked for the
   alternate screen, bracketed pastes, the mouse, focus reports or kitty's keyboard — none of
   which a shell leaves on while a command runs — are that program's, and not counted. A
   command that read what was typed without asking for any of those, a `read` or a password
   prompt, leaves the keyboard with the shell a line longer than it needed, which is the
   side to be wrong on. So a search given up over an empty line keeps the keyboard with the
   shell until the next Enter, and in fish, which ends an empty line with no `D`, until the
   next command.
   With a line in the field none of this happens, since the shell's own line is empty and a
   search started from it would end with two lines and one Enter, nor with a completion the
   shell offered standing in it, which is not an empty field to anyone looking; and a shell
   that marks no prompt is never handed the keyboard, because nothing would ever say it had
   given it back.

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
`--record <command>` opens the page with a row already recording, for a picture of it. The
file can be edited by hand too: while the settings section is showing it is re-read on the
themes folder's cadence — the same poll-not-watcher arrangement, for the same reason, and
ending the same way when the section is left — and adopted only when it differs, only while
the section still shows, and only while no save of the page's own is on its way to disk
(`SaveOrder::is_settled`), since a file read between a recorded chord and its write is the
file from before the edit.

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

**Option on a Mac is two keys in one.** To a terminal program Alt is Meta, ESC in front of the
key, and on a Mac Option is also the key a layout types its third level with: `@ [ ] { } | \`
on a German keyboard, `#` on a British one. Linux and Windows give that job to AltGr, which
arrives as no Alt at all, so there Alt is only ever Meta. On a Mac `terminal_keys::key_for`
decides per key, under `option_as_meta` in the settings. By default Option types a character
when it is ASCII and is not what the key types without Option, which is exactly the symbols a
non-US Mac has no other way to type, and it is Meta for everything else, which on a US keyboard
is every key: its Option layer has no ASCII in it. `left`, `right`, `both` and `none` are the
answers kitty, Alacritty and iTerm2 give. The window carries what the decision needs and a
keystroke's name cannot say (`KeyOrigin`): the character under the key with no modifier, since
German Option-8 is *named* `{`, which Option was held, and where the key is. Meta on a Mac is
the layout's own ASCII character for the key, so German Meta-Z is `ESC z`, except on a
number-row key the layout prints no ASCII letter or digit on, which is Meta on the US digit in
its place. French Meta on the `&` and `(` keys is `ESC 1` and `ESC 5`, the digits a French
keyboard types there with Shift, while a one-handed Dvorak's `f` up there stays `ESC f`. No
setting reaches an Option combination the layout makes a dead key (US Option-E, German Option-N
for `~`): the input method takes it to start an accent before winit sends a key press. The
composer never asks. It is a Mac text field, and Option types there as it does in every other
one.

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
`Event::Ime`. The preedit is kept *beside* the editor, in `TextInput`, never in it: a preedit
is not text, it is replaced wholesale by the next one, and putting it in the editor would put
it in the undo history, in a copy and in a submitted line. Only the drawing composes the two,
and `CommandInput` underlines the result so a half-converted word does not read as a committed
one. A click cannot move the caret while a composition is open, because the offsets a pointer
resolves against are offsets into a line the editor has never seen. The caret's painted
rectangle travels back out to the window through `Proxy::set_ime_area`, so the candidate list
stands beside the text being composed; it has to come from the paint path, because where the
caret is is the result of wrapping the line at the width the field was given.

A pane with no composer — vim, `less`, Claude Code — composes into its program instead. The
preedit is kept per pane in `PaneComposition`, while that pane's output has the whole keyboard,
and drawn by `terminal_element::paint_composition` over the cells from the program's cursor,
underlined, on the grid and on the list's open block alike; it is never written to the pty, and
only the commit is. It is drawn where the cursor *is* rather than where it is shown: the
snapshot carries `cursor_cell` beside `cursor`, because a program that draws its own caret hides
the real one and parks it on the field it reads — Claude Code does — and a composition that
waited for a visible cursor would never appear there. Where the input method's caret lands is
written back the way the composer's is, and `Workspace::caret_rect` reads it whenever the
output, not a composer, is listening. On the list the open block is laid out down to the
cursor's row while no composer is drawn under it: a program reading at the start of a line —
`cat`, `read`, a Python `input()` after a print — has its cursor one row past the last row it
printed on, and that row is where its cursor and the composition are drawn. Once taken, the row
is kept while it is still the one under the block's own rows (`PaneBlocks::live_reach` carries
it from one layout to the next): a display that rests there and goes back up with `\e[3A` to
redraw its last lines would otherwise move every line in the pane a row each time. A one-line
field — the tab search box, a find bar, a plugin's field — takes an input method's commit but
draws no composition and reports no caret, so while one has the keys the window leaves the
candidate list where it last put it. A composition belongs to the keyboard: a commit or the
input method giving up ends it, and so does focus moving, a menu, a find bar or a box beside
the pane taking the keys, a section of the sidebar replacing the panes, the composer coming
back, and the window going behind another. Whether the keyboard is a pane's is one predicate,
`Workspace::pane_with_keys`, which the composers and the frame both ask.

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

The pointer says which of the two a press will be: the grid asks for the I-beam while a press
selects and the arrow while a program has the mouse, read from the snapshot's
`mouse` modes so that a frame never takes the lock the reader parses under. That is what
kitty and Alacritty show over `vim` with the mouse on. Alacritty brings the I-beam back while
Shift is held; Crook does not, because nothing is drawn for Shift alone. A link the chord key
has lit, below, gets the hand, on either surface, for as long as its underline is drawn.

A move with no button down asks the same snapshot whether the program wants every move,
`?1003`, and only once the pane has found the pointer over it. Every move reaches every pane on
screen, and asking the terminal instead was its lock and a snapshot rebuilt on the thread that
draws, per pane and per move — behind a reader that might be parsing a flood.

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
scheme some application has registered a handler for. A URL longer than the pane is wide is
still one link: `app::pane_link::find` joins the row under the pointer with the rows the
terminal folded it across — the same `WRAPLINE` flag a copy reads — scans the joined line, and
maps the span back onto every row it crosses, so the underline and the click both cover the
whole URL. A newline the program printed carries no flag, and two URLs on adjacent rows stay two
links.

**So are paths**, which is what an agent or a compiler prints most: `src/main.rs:42:5`,
`Update(app/src/view.rs)`, `src/app.ts(42,5)`, `./README.md`. Where there is no URL under the
cell, `crook_terminal::path` scans the same joined line for a run with a separator in it or an
extension on it, and the location after it, and `pane_link::find` reads it against the
directory its command ran in — the one a finished block was filed under, or the one the pane's
shell last reported for the open block and the grid. Nothing in the scan touches the disk, and
a path is not underlined when it is found: `PaneLink` keeps it waiting, the element dispatches
`WorkspaceAction::LookForPath`, and the workspace `stat`s it on the background pool and
underlines it when there is something there to open. Never on the thread that draws, for the
reason #489 moved the Changes column's walk for `.git` off it: on a network mount whose server
has gone, a `stat` blocks for as long as the mount does. And one look per pane at a time, since
each look into such a mount is a worker of the pool that does not come back. What was found is
kept while the chord key is held and forgotten when it comes up; a look still out is kept, so
a dead mount is looked into once. A chord-click on a path that was found dispatches
`WorkspaceAction::OpenPath`, and the workspace starts `$VISUAL` or `$EDITOR` at the line through
the Changes column's launcher, or — with no editor that opens a window named, or for a folder —
hands `app::browser` a `file://` URL, through the same list of schemes. A program, which the
platform's handler would run rather than show, is opened only in an editor: the exec bit on
unix, Windows' programs on every platform (`pane_link::WINDOWS_PROGRAMS`, since Wine's desktop
entry runs an `.exe` that `xdg-open` is handed), and a per-platform list of extensions (`RUNS`,
and the macOS bundles in `BUNDLES`), checked against both the printed path and what it
canonicalizes to, since the handler follows a link.

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

**Colour-scheme reports are in**, as contour specified them and kitty and Ghostty answer
them. `CSI ? 996 n` asks whether the background is dark or light, and is answered `CSI ? 997 ;
1 n` or `; 2 n`, from the background OSC 11 answers with — the palette's, or the program's own
— split at the middle by the Rec. 709 weights the window's theme reads a colour's brightness
with. Mode `?2031` asks to be sent the same report unasked, and `Emulator::set_palette` queues
it into the replies whenever a new palette moves the background across that middle, and only
then: a program that hears picks a dark or a light theme of its own on it, and Claude Code asks
OSC 11 for the colour again. `Terminal::set_palette` sends those replies the way `feed` does,
through the writer's queue in the app, so a theme switch or the desktop's light and dark pair
reaches every pane whose program asked. `vte` has no hook for the question, and `Term` answers
DECRQM about `?2031` with 0, not recognised, so the OSC watcher keeps the mode itself, stops on
both questions to answer them in place among `Term`'s own answers, and drops `Term`'s `0`. A
synchronized update holds an OSC 11 past `Term` until it ends, and answers the OSC 11 query
behind it then, from the colour it set: so the watcher stops on what sets the background,
where whether an update holds it is read off `vte`, a question about the scheme behind one
ends the update first, as one nobody ended is ended, and a theme switch that lands while one
is held is weighed once it is let go of: once `vte` has played it, even when another opens
behind its end in the same read. An update that leaves the background alone stays whole
either way. A mirror answers neither and reports nothing.

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
`platform_insets::LayoutInsets` divides one answer between two elements — and, for as long as
`crookui_core` had no scrollable element, a ceiling: the list was `Clipped`, roughly nine tabs
fitted a 640px window, and the rest were drawn, clipped away and unclickable. The settings
page needed a `Scrollable` anyway, so the panel got one too and the ceiling is gone. Auto-scroll
came after it: every row records where it was drawn, and selecting a tab by any means scrolls
the panel until that row is in view — except under a hand that is carrying one, where the
row's slot is wherever the last step of the drag put it.

**The rest of shell integration.** Crook now installs the four OSC 133 marks into the zsh,
bash and fish it starts (`app/src/shell_integration`, and "Blocks" in §7), which is the half
that says where a command starts and ends. Warp's channel does more than that, and two
things beyond the marks are worth naming rather than discovering — one Crook's channel has
since grown, and one it closed but for a corner:

- **Completion is in**, and it took the second channel this paragraph used to ask for. OSC 133
  is an announcement — the shell says where a prompt began and how a command ended — and
  completion is a *question*, so there is a second protocol beside it, in `app/src/completion.rs`
  and the three snippets.

  The question is a **file**: Crook writes the line up to the caret into the session's own
  scratch — private to the user, and renamed into place whole (see "Where the scratch lives"
  in §7) — and sends `ESC [ 6339 ~`, a key the snippet has bound. The answer is a file too, and
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
generated JSON Schema — exists to serve roughly 800 settings. Crook has eleven, in two `serde`
structs in one JSON file, read once at startup and written back whole. That is correct at eleven
and the migration to something larger is a day; doing it in the other order is a month.

What the page took from Warp is the *presentation* and the *shape*, not the plumbing. The
presentation: a rail of pages, category headings with a rule between them,
label-left/control-right rows with a description line, apply-on-click with no Save button, a
reset button that doubles as the modified indicator, and inert rows drawn greyed rather than
dropped. The shape changed once. Settings were a **pane** at first — Warp's design, the same
thing a shell lives in, opened in a tab of their own and closed with the same close chord —
and they are a **sidebar section** now. The sidebar is Telegram's shape: one column, with a
short row of buttons at its foot — Sessions, Settings, Plugins, Store — choosing what fills
it. They are not tabs and are not drawn as tabs, because what they switch is the whole
window, which a tab does not do. The rail of pages sits where the tab list was, and the page
where the panes were. `crook/window` declares the `sidebar.section` slot; `crook/settings`
fills one, and so do `crook/plugins` and `crook/store`. `--section <name>` shows one by the
name on its button, the way `--settings` shows a page.

Being a pane had a price, and paying it back is the other reason it is gone. A settings pane
was a pane with no shell, so `PaneContent` became an enum and everything that reads a pane —
the two row renderers, the session file, and `Workspace::open_panes`, which the terminal
model syncs against and which had to keep a shell off the one that drew no grid — grew a
branch asking whether a pane was really a pane, with `Pane::session` and `Pane::status`
returning an `Option` for the answer. A section is not a pane, so the whole bill came back:
`Pane::session` is an `&AgentSession` again, `Pane::status` an `AgentStatus`, and every one
of those branches is gone.

Search is in, and it closed the one gap this list used to name. It is a single field at the
top of the rail, filtering the rail and the content together — the query matched against
each row's own keywords and its category's, so `theme` finds the theme row inside the
category the word names and `keys` finds every binding. It took the text-field *element*
Crook had been missing: the model was always there — `app/src/editor` draws nothing, touches
no clipboard and knows no keystroke — but the only thing that drew it was `CommandInput`,
which measures in terminal cells against a `CellFont`. `app/src/workspace/text_field.rs` is
that element now; the settings search is the first text input in Crook that is not a pane's,
and the tab-list search box and the worktree dialog draw the same one. `crookui_core` still
has no text field of its own — the one that was missing turned out to belong beside the
workspace's views rather than in the toolkit.

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
nothing at all: the chain ends at the first tick that finds the panel gone. (`keybindings.json`
is re-read the same way while the settings section is showing; see §7.) The re-read happens
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
to prescribe: snapshot types entirely separate from the live ones, holding a title, a directory,
a share of a split and the name of the agent a pane ran, and nothing else — a
`Vec<TabSnapshot>` plus an active index and the window's size, place and state, `serde_json` to
a file beside the settings.

Three decisions in it are worth naming. It is written **on every change rather than on the way
out**, because there is no reliable way out: a window closed by the window manager, a process
killed, a machine that lost power — none of them runs a shutdown path, and a file written only
at exit is missing exactly when somebody wanted it. Every id in a restored strip is **minted
fresh**, so a restored window is indistinguishable from one somebody opened by hand and nothing
keyed by pane id can collide with a previous process's. And a file this build did not write is
**bounded rather than validated**: sixty-four tabs and sixteen panes, because there is no
correct number and a truncated or hand-edited file must not be able to open ten thousand ptys
before the first frame.

A fourth followed the same rule the settings file already keeps: a file this build cannot read
*whole* is not a file it cannot read. `Session::salvage` reads the tabs one at a time and drops
the one that does not read, naming its position in the log, and reads the other keys each on
their own; `settings.json` does the same per key, so a `"login_shell": "yes"` costs that switch
and not the font size beside it. Opening a fresh, empty window over one bad value was the one
outcome both files exist to prevent. What the salvage drops is not lost either: before it runs,
the file is copied as it was into `session-backups/` beside it — the three newest kept — since
the next save writes a file that parses over the only evidence of what could not be read.

A fifth came from a second window. `crookui` runs one window per process, so two windows are two
processes sharing one file. When each one read the file at startup and wrote it on every change,
the second window came back with a copy of the first one's tabs, each agent's resume line typed
in where that agent was still running, and from then on whichever window saved last decided
which tabs the next launch got back. Now the first ordinary window takes an advisory lock
(`session::claim`, through std's `File::try_lock`: `flock` on macOS and Linux, `LockFileEx` on
Windows), and only that window restores and saves. A later ordinary window opens fresh, does not
save, and says so in a line under its header (`workspace/session_note.rs`, in the crash note's
shape). A launcher's window does neither and says nothing, as before. The lock is on
`session.lock` rather than on the session file, because a save renames a new session file into
place and the lock would stay on the old one. The lock file is never deleted, because deleting it
races the next window opening it. The system releases the lock when the process ends, crash
included. **New window** (`crook/window/new-window`) starts the binary again with
`--working-directory` set to the focused pane's directory (`app/src/new_window.rs`), or the home
directory when the new window could not read that one off its command line, which Crook reads as
text, or this user could not enter it. That makes it a launcher's window: it never takes the
session, so the window it was opened from keeps its tabs, and like every launcher's window it
says nothing about the session, even when restoring is turned on in it. On Windows the new
window does not attach to the console the first one did, so closing the shell that started the
first window does not end the second. A later ordinary window does say something when restoring
is turned on in it, even after its line was dismissed, because before the lock the switch wrote
that window's tabs at once and now it writes nothing. Every window takes the others' settings as
they are saved (below), so the switch reaches the keeper within a second and the keeper's tabs
are the ones remembered. If the keeper has closed by
then, the second window takes the lock instead and writes its own tabs over whatever restoring
last left in the file. That is the one time a second window takes over: when the keeper merely
closes, nobody has said which window's tabs should win. One file per window, all of them
restored at the next launch, is the fuller answer.

The settings are the other file every window shares, and there the answer is the opposite: no
window owns `settings.json`, so every window has to agree about it. Each used to write its whole
copy back on every change, which made the last window to save the one that decided every key —
a plugin revoked in one window was granted again by the next theme chosen in another, and went
on running with the grant there until it was restarted. Now a save writes only what that
window changed (`Settings::take_changes`: the keys this build owns, compared with what the
window last agreed with the file about, and one entry at a time under `plugin_grants` and
`resume_lines` and one name at a time in `disabled_plugins`), laid over the file as it is when
the save lands, with the read and the write one step under a lock on `settings.lock` beside it
(`Settings::write_changes`). The lock is waited for in the system's own queue, on a thread of
its own, for at most two seconds; past that the save is never written without it — the change
waits, the page says it is not being saved, and it is tried again. Saves run on threads of
their own rather than the pool, since they can wait. Only a file system that keeps no locks is
written without one, reading the file again just before the rename. A file that does not read is
copied into `settings-backups/` and not written over at all until it reads.

**A revoke always lands, and a grant only over the answer it was given over.** Every answer a
Crook writes for a plugin counts that plugin's entry in `plugin_grant_revisions` up by one, and a
grant lands only on the revision its window last saw, and only while the plugin's grant — read
the way a load reads it, so `[]` is "nothing allowed" — still says what it said then. The
revision catches a grant and a revoke made elsewhere in between, which leave the value as it
was; the value catches a hand edit, which counts nothing. A grant set aside leaves the file's
answer standing and the card asks again; a grant that finds the file already saying exactly it
has landed. Older builds carry `plugin_grant_revisions` through their saves as a key they do not
know. A window closing with revokes it could not write — the lock held, the file not reading —
parks them in a file of its own, `settings.pending.<pid>-<nonce>.json`, written once and never
rewritten, so windows closing together at a logout cannot lose each other's; every save, every
look at the file and the next launch lay them over it, and a save that lands removes exactly the
parked files it read under the lock. A parked file that does not read is kept and named in the
log. The lock is waited for by one thread per lock file in a process, which a retry waits on
rather than starting another.
`--uninstall-plugin` reads the settings strictly and removes nothing when they do not read, since
a revoke worked out against every default is a revoke of nothing.

Every window looks at the file every three quarters of a second, on a thread of its own rather
than a parked worker (`settings::watch`), and takes what it says: a grant revoked anywhere
rebuilds the plugin here under the new answer, and whatever it had asked for under the old one
goes with the old build. A look stamped while one of the window's own saves was on its way is set
aside for the next, and the save's changes stay visible to a look until they are on disk. A file
gone for three seconds of looks is "do not know" for the plugins — nothing switched back on,
nothing revoked. The type size, the tabs panel's width and whether it shows stay each window's
own while it runs (`settings::WINDOW_KEYS`), though the last one written is what the next window
opens with. `keybindings.json` keeps its own poll, but every save of it carries every edit not
yet on disk and makes them again, in order, on the file as it is, under `keybindings.lock`.

A sixth is where the window was. At first only its size came back, read off the frame, so a
window kept maximised on a second monitor reopened as an ordinary window on the first, at the
work area's size. `crookui` now follows a `WindowPlacement` — the corner of the window, the size
it goes back to, and whether it is maximised or fullscreen — once at the end of each batch of
events that moved or resized it and once more as the event loop stops, and tells the application
through `WindowDelegate::placed` only when it changed. At the end of the batch and not at each
event, because Windows sends the move of a maximise, with the frame already the work area's,
before the resize that tells winit the window is maximised. The file keeps it as `window`,
`window_position`, `maximized` and `fullscreen`, every one optional, so a file from before reads
as it always did. A maximised or fullscreen window keeps the corner and size it had before,
since those are what it goes back to, and a minimised one changes nothing: Windows reports it at
(-32000, -32000). The corner is in the desktop's own coordinates — pixels on Windows and X11,
points on macOS, where winit's physical pixels change with the screen under the window — and
Wayland reports none. Before a window opens at a remembered corner, `placement::onto_a_monitor`
checks it against the monitors there are now: it stays while enough of its top edge is on one to
drag it by, and otherwise moves wholly onto the monitor it overlaps most, or the nearest, so a
window whose monitor was unplugged comes back on screen. Fullscreen is asked for once the window
exists rather than as an attribute, because macOS restores a window opened fullscreen to the
size of the screen. macOS refuses fullscreen while it animates into a new Space — a window
opened from a fullscreen VS Code — and winit asks again only for the attribute, so `crookui`
asks again itself, every half second for the window's first four seconds
(`placement::FullscreenAsked`). It stops sooner only on AppKit's word that the window has
entered fullscreen, `NSWindowDidEnterFullScreenNotification` (`fullscreen_entered`): winit takes
the window as fullscreen from the moment it asks, and one opened maximised is already as wide
as its monitor, so neither says a refusal is not still to come. On X11 winit sees only the
fullscreen it asked for, and goes on seeing one the window manager has ended, so there a window
that covers a monitor exactly keeps the size it goes back to, as a fullscreen one does. A lone
tiled window with no bar beside it covers a monitor too, so the guess is never kept as
fullscreen — the session file is read on Wayland too, which would open that window over
everything — and no window is asked to open fullscreen on X11: one closed fullscreen comes
back as it was before. Only the keeper writes any
of it, as with the tabs, and a save still on its way as the window closes is written then, on
the main thread (`Workspace::finish_saving_the_session`), since the pool it would run on ends
with the process.

What is *not* remembered is the point: no scrollback, no output, no process. A window that
redrew yesterday's output over a shell that had never run any of it would be lying about the
state of the machine. The settings section is left out too — it is something somebody opened to
change a setting, not work in progress.

Which is why a close asks before it ends anything still working. A process is exactly what does
not come back, and every agent is a child of the window — a pane's pty hangs its child up when it
goes — so the window's close, a tab's, a group's and a pane's stop when a pane they would take
has an agent running or waiting on a person, or a shell running a command, and put a question up
whose default is Cancel (`app/src/workspace/closing.rs`). The window's close includes the
desktop's: winit's `CloseRequested` goes to the application through the window delegate rather
than ending the event loop there. What cannot ask is anything that ends the process from outside
it: a `SIGTERM` or a kill, a logout that kills rather than closes, a power cut. A window manager's
close is not one of them: it arrives as `CloseRequested` and asks like the rest. Nor is macOS's
Quit — the menu item, `cmd-q`, the Dock's, a polite logout's — although winit turns it into
`applicationWillTerminate:` with no `applicationShouldTerminate:` to say no in: Crook adds that
method to winit's delegate (`crates/crookui/src/windowing/quit.rs`), answers it from what the
window said when the event loop last went idle, and refuses a quit that would ask, which then
asks as the window's close. Because it can come while somebody is typing
elsewhere, the window asks the desktop for attention rather than taking the keyboard, and the
card's way to End opens only after the keyboard has been still for a second — since the card
came up, since the window last took the keyboard, and since the last key it had no use for.

What *is* remembered of a process is one word: the program name of a coding agent a pane was
running — `claude`, never the prompt typed after it, and only for a program in `agent.rs`'s
table — so that the restored pane can offer that agent's resume line (`claude --continue`,
`codex resume --last`) in its composer, unsent. The directory is the session key: those lines
resume the most recent conversation *in the directory they run in*, and Crook's worktrees give
each agent a directory of its own, so no session id, hook or wire change is needed. Two panes of
one agent in one directory are offered the agent's picker instead, or nothing where it has none,
and a CLI whose "most
recent" is the repository's rather than the directory's is offered its picker or nothing. One
fact more for an agent that was not started by its name: the package runner and package, or
the version manager, that found it (`npx @google/gemini-cli`, `mise x node@22 -- claude`),
which the resume line is run through, since an `npx` user has no `gemini` on `PATH`. It is
rebuilt from the line by `agent::launch_of` rather than copied, so a `VAR=value` (an API key's
usual place), `env`, `nohup` and the runner's own options never reach the file, and a hand edit
that is not that shape comes back as the agent alone. Every assignment is dropped, a `PATH=` that
found the agent too, so that agent is offered its bare line. A line that ran the agent in another
directory (`env -C` in any spelling getopt takes, `mise x -C`, `npx --workspace`) or some other
way no kept line could repeat (`mise x` with a sandbox's `--deny-*` and `--allow-*`, with `-E`,
or with an option of `env`'s, a runner's or `mise`'s that `launch_of` does not know) is no
launch, and the file keeps no agent for it, because any line it could be offered would resume
another directory's conversation or run the agent unconfined. It is still the agent the pane
runs, by `agent::program_of`, which is what its title is read with: codex's spinner counts only
in a pane that runs codex. Aliases are not seen through: the shell reports the line as typed, and only
that shell could expand it. The
rule above still holds: nothing old is redrawn, and a new process starts only when a person
presses Enter, or "Resume every agent" for every line nobody has touched.

**Telemetry, crash reports that are sent, automatic updates.** All absent. What exists is
local and asked for: a panic writes a report into a folder on the machine and the next window
says so (`app/src/diagnostics`), and the updater runs when a person presses it — nothing is
sent by either. Worth noting that adding Sentry on macOS is not a `Cargo.toml` line: Warp's
build script downloads an `xcframework` and its bundler wires an rpath for it, which is
precisely the class of thing §3 was written to avoid. If sending crash reports ever becomes
necessary, prefer something that is a pure-Rust crate on all three platforms, and treat a
build script as the cost it is.

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
| Settings | ~800, with a macro DSL and cloud sync | 11, two `serde` structs and a name in one JSON file |
| Themes | 21 built in, gradients, images, a creator, OS sync, hot reload | 13 built in, the same file format, a creator without the image, no OS sync |
| Theme chooser | a 240px docked panel with search and virtualisation | a 248px docked panel, no search, every row built |
| Icons | its own `WarpIcon` and `UiIcon` sets, rendered from SVG | Lucide, vendored as path commands, one distance-field rasterizer — and one drawn mark, filled (§4) |
| Settings UI | a pane, 16 pages, search over ~800 widgets | a sidebar section, 4 pages, search over 30 (§4) |
| Git worktrees | none | a right-click menu on a tab, after herdr's model (§7) |

The through-line: Crook keeps every *architectural* idea from Warp and rejects almost every
*build-system* one. The architecture is what makes a GPU terminal tractable in Rust. The build
system is what a decade of platform-specific product requirements does to a repository, and
Crook has not earned any of those requirements yet.
