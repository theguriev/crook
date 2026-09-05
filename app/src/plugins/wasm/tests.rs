//! A plugin that is not in the binary, loaded into one that is.
//!
//! The module below is written in wasm's text format and assembled here, so
//! these run with no wasm toolchain installed — and it is a *real* module, so
//! what is being tested is the boundary rather than a mock of it.
//!
//! # Two halves, and why the second one is not a wasm module
//!
//! Loading is one question and drawing is another. The tests at the top ask
//! what happens to a file in the plugins directory; the ones below ask what a
//! [`Node`] becomes, and they hand the node to [`render::element`] directly
//! rather than round-tripping it through a guest. Nothing in between would be
//! exercised by the trip — the vocabulary is the same type on both sides of
//! the wire, and `crook_plugin_api`'s own tests cover the encoding — so a
//! module here would only mean that a test about a pirate needs a wasm
//! toolchain's worth of scaffolding to say which colour he is.
//!
//! What is drawn is read back out of a real [`Scene`], through a real
//! presenter, because that is the only place the answer actually exists: an
//! element tree has no opinion about its own colours until something paints
//! it.

use std::collections::BTreeMap;
use std::fs;
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;

use crookui_core::event::{Event, Modifiers, MouseButton};
use crookui_core::executor::{Background, LocalQueue};
use crookui_core::fonts::{FamilyId, FontId, LineStyle, StyleAndFont};
use crookui_core::icons::{Art, Chomp, Mark};
use crookui_core::platform::TextLayoutSystem;
use crookui_core::text_layout::{Glyph, Line, Run};
use crookui_core::{Action, App, Presenter, Scene, WindowId};

use crook_plugin_api::{Capability, Gap, Manifest, Node, Row, Size, Tone, to_bytes};

use crate::clipboard::Clipboard;
use crate::plugin::{ActionId, Voice};
use crate::theme::theme;
use crate::workspace::{Fonts, WorkspaceAction};

use super::*;

/// A directory of this test's own.
pub(crate) struct Scratch {
    path: PathBuf,
}

impl Scratch {
    pub(crate) fn new(name: &str) -> Self {
        static SERIAL: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let serial = SERIAL.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let path =
            std::env::temp_dir().join(format!("crook-wasm-{name}-{}-{serial}", std::process::id()));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).expect("the scratch directory should be creatable");
        Self { path }
    }

    pub(crate) fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

/// What the test plugin says about itself.
pub(crate) fn manifest(id: &str) -> Manifest {
    Manifest {
        abi: crook_plugin_api::ABI_VERSION,
        id: id.to_owned(),
        name: "Probe".to_owned(),
        description: "A plugin that exists to be looked at.".to_owned(),
        version: "0.1.0".to_owned(),
        capabilities: vec![Capability::ReadTabs],
    }
}

/// What it asks to have drawn.
pub(crate) fn tree() -> Node {
    Node::Row(vec![
        Node::Icon {
            name: "git-branch".to_owned(),
            tone: Tone::Muted,
        },
        Node::Gap(Gap::Small),
        Node::Text {
            text: "from a sandbox".to_owned(),
            size: Size::Small,
            tone: Tone::Muted,
        },
        Node::Badge {
            text: "probed".to_owned(),
            tone: Tone::Accent,
        },
        Node::Button {
            label: "Poke".to_owned(),
            action: "poke".to_owned(),
            tone: Tone::Accent,
        },
    ])
}

/// `bytes` as a wasm data-segment string.
fn escaped(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("\\{byte:02x}")).collect()
}

/// A module that contributes `tree()` to `slot` at `order`, and offers one
/// action.
pub(crate) fn wasm(id: &str, slot: &str, order: i32) -> Vec<u8> {
    let manifest = to_bytes(&manifest(id)).expect("a manifest should encode");
    let tree = to_bytes(&tree()).expect("a tree should encode");
    let tree_at = 16 + manifest.len() as u32;
    let strings_at = 4096;
    let entry_at = strings_at + slot.len() as u32;
    let action_at = entry_at + 4;
    let title_at = action_at + 4;

    let text = format!(
        r#"(module
            (import "crook" "contribute"
              (func $contribute (param i32 i32 i32 i32 i32)))
            (import "crook" "register_action"
              (func $register_action (param i32 i32 i32 i32)))
            (memory (export "memory") 1)
            (global $next (mut i32) (i32.const 8192))
            (data (i32.const 16) "{manifest_bytes}")
            (data (i32.const {tree_at}) "{tree_bytes}")
            (data (i32.const {strings_at}) "{slot}")
            (data (i32.const {entry_at}) "here")
            (data (i32.const {action_at}) "poke")
            (data (i32.const {title_at}) "Poke the probe")
            (func (export "crook_abi_version") (result i32) (i32.const {abi}))
            (func (export "crook_alloc") (param $len i32) (result i32)
              (local $at i32)
              (local.set $at (global.get $next))
              (global.set $next (i32.add (global.get $next) (local.get $len)))
              (local.get $at))
            (func (export "crook_manifest") (result i64)
              (i64.or (i64.shl (i64.const 16) (i64.const 32)) (i64.const {manifest_len})))
            (func (export "crook_build") (result i32)
              (call $contribute
                (i32.const {strings_at}) (i32.const {slot_len})
                (i32.const {entry_at}) (i32.const 4)
                (i32.const {order}))
              (call $register_action
                (i32.const {action_at}) (i32.const 4)
                (i32.const {title_at}) (i32.const 14))
              (i32.const 0))
            (func (export "crook_render") (param i32 i32 i32 i32) (result i64)
              (i64.or (i64.shl (i64.const {tree_at}) (i64.const 32)) (i64.const {tree_len})))
            (func (export "crook_run") (param i32 i32 i32 i32) (result i32) (i32.const 0)))"#,
        manifest_bytes = escaped(&manifest),
        manifest_len = manifest.len(),
        tree_bytes = escaped(&tree),
        tree_len = tree.len(),
        slot_len = slot.len(),
        abi = crook_plugin_api::ABI_VERSION,
    );

    wat::parse_str(&text).expect("the test module should assemble")
}

/// Installs one into `directory` under its own folder.
pub(crate) fn install(directory: &Path, folder: &str, wasm: &[u8]) {
    let home = directory.join(folder);
    fs::create_dir_all(&home).expect("the plugin directory should be creatable");
    fs::write(home.join(MODULE_FILE), wasm).expect("the module should be writable");
}

#[test]
fn a_module_in_the_plugins_directory_is_found_and_read() {
    let scratch = Scratch::new("found");
    install(
        scratch.path(),
        "probe",
        &wasm("eugen/probe", "header.right", 0),
    );

    let found = installed(scratch.path());

    assert_eq!(found.len(), 1);
    let manifest = found[0].manifest();
    assert_eq!(manifest.id.to_string(), "eugen/probe");
    assert_eq!(manifest.name, "Probe");
    assert_eq!(manifest.tier, Tier::Wasm);
}

#[test]
fn a_directory_that_is_not_there_is_no_plugins_and_not_a_failure() {
    // The ordinary state of a fresh install, and not worth a line in the log
    // let alone a refusal to start.
    assert!(installed(Path::new("/nowhere/at/all/really")).is_empty());
}

#[test]
fn something_in_the_directory_that_is_not_a_plugin_costs_only_itself() {
    let scratch = Scratch::new("rubbish");
    install(scratch.path(), "rubbish", b"not a wasm module");
    install(
        scratch.path(),
        "probe",
        &wasm("eugen/probe", "header.right", 0),
    );
    // And a directory with nothing in it at all.
    fs::create_dir_all(scratch.path().join("empty")).expect("creatable");

    let found = installed(scratch.path());

    assert_eq!(
        found.len(),
        1,
        "the good plugin did not survive the bad one"
    );
}

#[test]
fn plugins_are_loaded_in_a_order_that_is_the_same_on_every_machine() {
    // A slot settles two entries of equal `order` by load order, so a load
    // order that came from whatever `read_dir` happened to answer would put
    // two plugins in a different place on two machines.
    let scratch = Scratch::new("order");
    for (folder, id) in [("zeta", "eugen/zeta"), ("alpha", "eugen/alpha")] {
        install(scratch.path(), folder, &wasm(id, "header.right", 0));
    }

    let ids: Vec<String> = installed(scratch.path())
        .iter()
        .map(|plugin| plugin.manifest().id.to_string())
        .collect();

    assert_eq!(ids, ["eugen/alpha", "eugen/zeta"]);
}

#[test]
fn a_manifest_naming_an_id_that_is_not_one_is_refused() {
    let scratch = Scratch::new("bad-id");
    install(
        scratch.path(),
        "probe",
        &wasm("Eugen/Probe", "header.right", 0),
    );

    assert!(installed(scratch.path()).is_empty());
}

/// The window a node is drawn in.
///
/// Four hundred wide so that a share of the room is a round number of pixels
/// and an assertion about a meter is about the fraction rather than about
/// rounding; tall enough that a panel hung under a chip has somewhere to go.
const WINDOW: Vector2F = vec2f(400., 200.);

/// A shaper with no fonts: every character is a square half its font size.
///
/// The same stand-in the element layer's own tests use. Real metrics come from
/// a real backend, and what a test about a mark's colour needs from a shaper is
/// only that a label beside it takes up some room.
struct StubShaper;

impl TextLayoutSystem for StubShaper {
    fn layout_line(
        &self,
        text: &str,
        line_style: LineStyle,
        style_runs: &[(Range<usize>, StyleAndFont)],
        _: f32,
    ) -> Line {
        let advance = line_style.font_size * 0.5;
        let glyphs: Vec<_> = text
            .char_indices()
            .enumerate()
            .map(|(position, (index, character))| Glyph {
                id: character as u32,
                position_along_baseline: vec2f(position as f32 * advance, 0.),
                index,
                width: advance,
            })
            .collect();
        let width = glyphs.len() as f32 * advance;

        Line {
            width,
            runs: vec![Run {
                font_id: FontId(0),
                glyphs,
                styles: style_runs
                    .first()
                    .map(|(_, style_and_font)| style_and_font.style)
                    .unwrap_or_default(),
                width,
            }],
            font_size: line_style.font_size,
            line_height_ratio: line_style.line_height_ratio,
            baseline_ratio: line_style.baseline_ratio,
            ascent: line_style.font_size * 0.8,
            descent: line_style.font_size * 0.2,
        }
    }
}

/// `owner/name/<what>`, for the four actions a picker's keys resolve to.
fn name(what: &str) -> ActionName {
    ActionName::parse(&format!("someone/theirs/{what}")).expect("a name built from a literal")
}

/// A view whose whole tree is one plugin's node.
///
/// `answers` stands in for the host's action table, and it is one value rather
/// than a map because one value is the whole distinction this tier draws: a
/// name is registered or it is not.
struct Drawn {
    node: Node,
    answers: Option<ActionId>,
    /// The mouse state a contribution keeps between frames. Held here for the
    /// reason the real one is held on the contribution: the tests draw the
    /// same node twice — once to find a control and once after pressing it —
    /// and a handle made per render would forget the press in between.
    hovers: render::Hovers,
    /// What the host holds on the plugin's behalf, for the same reason.
    ///
    /// The real one is made once per plugin in `build`; these tests have no
    /// plugin, so this stands in for one — and it has to survive between two
    /// draws for exactly the reason `hovers` does, since what a picker is
    /// showing is what a key pressed after it was drawn acts on.
    chrome: Rc<picker::Chrome>,
    /// What a field in a picker would paste from. Never used by these tests
    /// and required to build one, which is the ordinary state of a clipboard
    /// in a headless window.
    clipboard: Clipboard,
}

impl Entity for Drawn {
    type Event = ();
}

impl TypedActionView for Drawn {
    /// Nothing, because nothing here answers to anything.
    ///
    /// A root view has to name an action type, and what these tests read is
    /// what an element *asked* the window to do rather than what came of it:
    /// the asking is this tier's whole side of the bargain, and the workspace
    /// that would carry it out is not in this window.
    type Action = ();
}

impl View for Drawn {
    fn ui_name() -> &'static str {
        "Drawn"
    }

    fn render(&self, _: &AppContext) -> Box<dyn Element> {
        let answers = self.answers;
        render::element(
            &self.node,
            &render::Surroundings {
                fonts: Fonts {
                    ui: FamilyId(0),
                    monospace: FamilyId(0),
                },
                placement: render::Placement::Below,
                action: &move |_| answers,
                hovers: &self.hovers,
                chrome: &self.chrome,
                clipboard: &self.clipboard,
            },
        )
    }
}

/// One node, in a window, painted.
struct Frame {
    app: App,
    presenter: Presenter,
    window_id: WindowId,
}

impl Frame {
    /// A frame whose node names no action anything answers to — the state
    /// every plugin's first draw is in, before its actions are registered.
    fn new(node: Node) -> Self {
        Self::answering(node, None)
    }

    /// The same, with every name in the node resolving to `answers`.
    fn answering(node: Node, answers: Option<ActionId>) -> Self {
        let queue = LocalQueue::new();
        // One worker: nothing drawn here waits on anything.
        let mut app = App::new(queue.foreground(), Arc::new(Background::new(1)));
        let (window_id, _) = app.add_window(|_| Drawn {
            node,
            answers,
            hovers: render::Hovers::default(),
            chrome: Rc::new(picker::Chrome::new(
                picker::Keys {
                    next: name("next"),
                    previous: name("previous"),
                    choose: name("choose"),
                    close: name("close"),
                },
                Voice::default(),
            )),
            clipboard: Clipboard::new(),
        });
        let presenter = Presenter::new(window_id, Arc::new(StubShaper));

        Self {
            app,
            presenter,
            window_id,
        }
    }

    /// Renders and paints, which is where the answers below are read from.
    fn scene(&mut self) -> Rc<Scene> {
        let window_id = self.window_id;
        let presenter = &mut self.presenter;
        self.app.update(|ctx| {
            let invalidation = ctx.take_all_invalidations_for_window(window_id);
            presenter.invalidate(invalidation, ctx);
            presenter.build_scene(WINDOW, 1., ctx)
        })
    }

    /// Moves the pointer to `at`, so that whatever is under it knows.
    ///
    /// A separate step from [`Frame::click`] because hover is the thing worth
    /// asserting on its own: a control that only lights up while a button is
    /// held is a control nobody can find.
    fn hover(&mut self, at: Vector2F) {
        // Through the window rather than straight at the presenter, which is
        // what [`Frame::click`] does: a click is read for what it *asked* the
        // window to do, and a move is read for what it left behind, so this
        // one has to go the way the platform's own moves go.
        let window_id = self.window_id;
        let presenter = &mut self.presenter;
        self.app.update(|ctx| {
            ctx.dispatch_window_event(
                window_id,
                Event::MouseMoved {
                    position: at,
                    modifiers: Modifiers::default(),
                    is_synthetic: false,
                },
                presenter,
            )
        });
    }

    /// Presses and releases at `at`, and hands back what the window was asked
    /// to do.
    ///
    /// Taken from the presenter rather than dispatched into the app, because
    /// there is no workspace here to answer a [`WorkspaceAction`]: what is
    /// being tested is that the element asked, not what a handler would then
    /// have done.
    fn click(&mut self, at: Vector2F) -> Vec<WorkspaceAction> {
        let mut asked = Vec::new();
        for event in [
            Event::MouseDown {
                button: MouseButton::Left,
                position: at,
                modifiers: Modifiers::default(),
                click_count: 1,
            },
            Event::MouseUp {
                button: MouseButton::Left,
                position: at,
                modifiers: Modifiers::default(),
            },
        ] {
            let presenter = &mut self.presenter;
            let result = self.app.update(|ctx| presenter.dispatch_event(event, ctx));
            asked.extend(result.actions.iter().filter_map(|dispatched| {
                // Through the trait object on purpose. `Box<dyn Action>` is
                // itself an `Action`, so calling this on the box answers with
                // the box's own type and downcasts to nothing at all.
                let action: &dyn Action = &*dispatched.action;
                action.as_any().downcast_ref::<WorkspaceAction>()
            }));
        }
        asked
    }
}

/// Every mark the frame draws, in paint order.
fn marks(scene: &Scene) -> Vec<Mark> {
    scene
        .layers()
        .flat_map(|layer| layer.icons.iter())
        .map(|drawn| drawn.icon_key.mark)
        .collect()
}

/// What each of them is painted in, in the same order.
fn mark_colors(scene: &Scene) -> Vec<Color> {
    scene
        .layers()
        .flat_map(|layer| layer.icons.iter())
        .map(|drawn| drawn.color)
        .collect()
}

/// Where the first mark is, which is the only way to click a node that is a
/// picture rather than a word.
fn mark_center(scene: &Scene) -> Vector2F {
    let bounds = scene
        .layers()
        .flat_map(|layer| layer.icons.iter())
        .map(|drawn| drawn.bounds)
        .next()
        .expect("a mark should have been drawn to click on");
    bounds.origin() + bounds.size() / 2.
}

/// Where the first mark is and how big, for a test about what is drawn around
/// one.
fn mark_box(scene: &Scene) -> Option<RectF> {
    scene
        .layers()
        .flat_map(|layer| layer.icons.iter())
        .map(|drawn| drawn.bounds)
        .next()
}

/// Every rectangle filled with `color`, in paint order.
fn rects_of(scene: &Scene, color: Color) -> Vec<RectF> {
    scene
        .layers()
        .flat_map(|layer| layer.rects.iter())
        .filter(|rect| rect.background == Fill::Solid(color))
        .map(|rect| rect.bounds)
        .collect()
}

/// The corner every rectangle filled with `color` was given, in paint order.
fn rounding_of(scene: &Scene, color: Color) -> Vec<Radius> {
    scene
        .layers()
        .flat_map(|layer| layer.rects.iter())
        .filter(|rect| rect.background == Fill::Solid(color))
        .map(|rect| rect.corner_radius.get_top_left())
        .collect()
}

/// One action, registered the way the host registers a plugin's.
fn an_action() -> ActionId {
    let mut host = Host::new(
        Fonts {
            ui: FamilyId(0),
            monospace: FamilyId(0),
        },
        BTreeMap::new(),
    );
    host.register_action(
        ActionName::parse("eugen/probe/poke").expect("a literal that parses"),
        |_, _| {},
    )
}

#[test]
fn a_pirate_is_drawn_by_name_at_the_frame_the_name_asks_for() {
    // The mark lives in the host and the *animation* does not: a plugin cycles
    // these three names on a clock of its own, so all three have to be
    // reachable by name and each has to be the frame it says it is.
    for (name, chomp) in [
        ("pirate", Chomp::Shut),
        ("pirate-open", Chomp::Open),
        ("pirate-wide", Chomp::Wide),
    ] {
        let mut frame = Frame::new(Node::Icon {
            name: name.to_owned(),
            tone: Tone::Primary,
        });

        assert_eq!(
            marks(&frame.scene()),
            [
                Mark::Art(Art::PirateFace(chomp)),
                Mark::Art(Art::PirateInk(chomp))
            ],
            "{name:?} did not draw both layers of its own frame"
        );
    }
}

#[test]
fn a_name_this_build_has_no_icon_for_draws_nothing() {
    // A plugin written against a newer Crook should be missing a glyph rather
    // than being refused, and a name that is nearly the pirate's is not the
    // pirate's.
    for name in ["pirate-grinning", "gitbranch", ""] {
        let mut frame = Frame::new(Node::Icon {
            name: name.to_owned(),
            tone: Tone::Primary,
        });

        assert!(
            marks(&frame.scene()).is_empty(),
            "{name:?} drew something this build has no mark for"
        );
    }

    // And a name it does have still draws, so the test above is not passing
    // because nothing is drawn at all.
    let mut known = Frame::new(Node::Icon {
        name: "git-branch".to_owned(),
        tone: Tone::Primary,
    });
    assert_eq!(marks(&known.scene()), [Mark::Icon(Lucide::GitBranch)]);
}

#[test]
fn a_stale_reading_greys_the_whole_mark_and_not_half_of_it() {
    // `Muted` is what a plugin says when the figure beside the mark is not
    // current. Both layers take it: a yellow face wearing a grey eyepatch
    // would read as a rendering fault rather than as a stale reading.
    let mut stale = Frame::new(Node::Icon {
        name: "pirate".to_owned(),
        tone: Tone::Muted,
    });

    assert_eq!(
        mark_colors(&stale.scene()),
        [theme().text_muted, theme().text_muted]
    );

    // Every other tone leaves the artwork the colours it was drawn in, which
    // is the whole reason a mark is not an icon: a pirate that took the
    // palette's cast would stop being the mark people recognise.
    let mut current = Frame::new(Node::Icon {
        name: "pirate".to_owned(),
        tone: Tone::Accent,
    });

    assert_eq!(
        mark_colors(&current.scene()),
        [Color::hex(0xf9d949), Color::hex(0x151515)]
    );
}

/// One node, in the only place the host gives a width to divide.
///
/// A meter is a share of an axis, and the axis a contribution is measured
/// against is infinite — `header.right` hands its entry whatever is left of a
/// row that has already given its surplus away. So every assertion about a
/// share is an assertion about one inside a panel, which is the only shape a
/// plugin can honestly draw one in.
fn in_a_panel(node: Node) -> Node {
    Node::Anchored {
        content: Box::new(Node::Empty),
        panel: Some(Box::new(node)),
        dismiss: "shut".to_owned(),
    }
}

#[test]
fn a_share_of_a_width_nobody_gave_draws_nothing_rather_than_asserting() {
    // `Flex` asserts in a debug build when it is asked to divide an infinite
    // axis, and a plugin from a store may not do that to a window. So a meter
    // or a spacer outside a panel is dropped, and the plugin's author is told
    // once rather than sixty times a second.
    let mut loose = Frame::new(Node::Meter {
        fraction: 0.5,
        tone: Tone::Accent,
    });
    let scene = loose.scene();

    assert!(rects_of(&scene, theme().overlay_2).is_empty());
    assert!(rects_of(&scene, theme().accent).is_empty());

    // And the same node inside a panel is drawn, which is what says the rule
    // is about the room and not about the node.
    let mut held = Frame::new(in_a_panel(Node::Meter {
        fraction: 0.5,
        tone: Tone::Accent,
    }));

    assert_eq!(rects_of(&held.scene(), theme().overlay_2).len(), 1);
}

#[test]
fn a_reading_past_its_own_limit_fills_the_bar_and_stops_there() {
    // The ABI promises the host clamps rather than refuses: a session that
    // briefly reports more than its own limit is a thing that happens, and it
    // is not worth an empty frame. The fill is a share of the track, so what
    // says it was clamped is that the two are the same width.
    let mut over = Frame::new(in_a_panel(Node::Meter {
        fraction: 1.7,
        tone: Tone::Accent,
    }));
    let scene = over.scene();
    let track = rects_of(&scene, theme().overlay_2);
    let filled = rects_of(&scene, theme().accent);

    assert_eq!(track.len(), 1, "a meter draws one track");
    assert_eq!(filled.len(), 1, "and one fill");
    assert_eq!(filled[0].width(), track[0].width());

    // The other end of the clamp draws no fill at all rather than a sliver of
    // one at a negative width.
    let mut under = Frame::new(in_a_panel(Node::Meter {
        fraction: -0.5,
        tone: Tone::Accent,
    }));
    let scene = under.scene();

    assert_eq!(rects_of(&scene, theme().overlay_2).len(), 1);
    assert!(rects_of(&scene, theme().accent).is_empty());

    // And a fraction inside its range takes exactly that share of whatever
    // room the meter was given, which is what makes it a fraction rather than
    // a width. Measured against the track rather than against a number: the
    // panel's width is the host's business and a test that hard-coded it
    // would fail the day the host changed its mind.
    let mut quarter = Frame::new(in_a_panel(Node::Meter {
        fraction: 0.25,
        tone: Tone::Accent,
    }));
    let scene = quarter.scene();
    let track = rects_of(&scene, theme().overlay_2)[0].width();

    assert_eq!(rects_of(&scene, theme().accent)[0].width(), track * 0.25);
}

#[test]
fn a_pressable_whose_action_answers_to_nothing_is_drawn_and_does_nothing() {
    let chip = Node::Pressable {
        content: Box::new(Node::Icon {
            name: "pirate".to_owned(),
            tone: Tone::Primary,
        }),
        action: "poke".to_owned(),
    };

    // Drawn, not left out: a chip that vanished because its plugin was
    // switched off mid-session is harder to explain than one that does not
    // respond.
    let mut inert = Frame::new(chip.clone());
    let scene = inert.scene();
    let at = mark_center(&scene);
    assert_eq!(marks(&scene).len(), 2);
    assert!(
        inert.click(at).is_empty(),
        "a name nothing answers to ran something"
    );

    // And the same node, with the same click, once the host has an action of
    // that name — so the test above is about the resolution and not about the
    // click landing somewhere else.
    let action = an_action();
    let mut live = Frame::answering(chip, Some(action));
    let at = mark_center(&live.scene());

    assert_eq!(live.click(at), [WorkspaceAction::Run(action)]);
}

#[test]
fn a_panel_the_plugin_says_is_shut_is_not_drawn_at_all() {
    // Whether the panel is up is the plugin's state, and `None` is how it says
    // "shut". The contribution is then the whole of what the slot holds —
    // no ground, no hairline, nothing hanging under it.
    let chip = Node::Icon {
        name: "pirate".to_owned(),
        tone: Tone::Primary,
    };
    let mut shut = Frame::new(Node::Anchored {
        content: Box::new(chip.clone()),
        panel: None,
        dismiss: "shut".to_owned(),
    });
    let scene = shut.scene();

    assert_eq!(marks(&scene).len(), 2, "the chip itself is still drawn");
    assert!(
        rects_of(&scene, theme().surface_raised).is_empty(),
        "a shut panel left its ground on screen"
    );

    // Open, the host supplies the ground: a plugin cannot draw a panel that
    // does not look like Crook's, and it does not get to choose how wide one
    // is either.
    let mut open = Frame::new(Node::Anchored {
        content: Box::new(chip),
        panel: Some(Box::new(Node::Rule)),
        dismiss: "shut".to_owned(),
    });
    let scene = open.scene();
    let ground = rects_of(&scene, theme().surface_raised);

    assert_eq!(ground.len(), 1);
    assert_eq!(ground[0].width(), 280.);
}

#[test]
fn a_pressable_is_the_thing_itself_until_somebody_reaches_for_it() {
    // The rule the usage chip arrived at when it was still in the box: a
    // control drawn as a control at all times is a box in the chrome competing
    // with what is inside it, and a mark that lights up when reached for is a
    // mark until it is needed.
    let chip = Node::Pressable {
        content: Box::new(Node::Icon {
            name: "pirate".to_owned(),
            tone: Tone::Primary,
        }),
        action: "panel".to_owned(),
    };
    let mut frame = Frame::answering(chip, Some(an_action()));

    let scene = frame.scene();
    assert!(
        rects_of(&scene, theme().tab_active).is_empty(),
        "a pressable nobody is pointing at drew a ground"
    );

    frame.hover(mark_center(&scene));
    let scene = frame.scene();

    let ground = rects_of(&scene, theme().tab_active);
    assert_eq!(ground.len(), 1, "reaching for it lit nothing up");
    assert_eq!(
        rounding_of(&scene, theme().tab_active),
        vec![Radius::Pixels(6.)],
        "a fully rounded corner reads as a badge rather than as something to press"
    );
}

#[test]
fn a_pressable_is_its_content_and_an_even_edge_and_nothing_else() {
    // What a person sees as a gap after a short reading is room reserved for a
    // longer one. The chip is the last thing in the header, so what a wider
    // number costs is a few pixels of empty header — and what reserving them
    // costs is visible on every frame.
    let mark = Node::Icon {
        name: "pirate".to_owned(),
        tone: Tone::Primary,
    };
    let mut bare = Frame::new(mark.clone());
    let alone = mark_box(&bare.scene()).expect("the mark is drawn");

    let mut pressed = Frame::answering(
        Node::Pressable {
            content: Box::new(mark),
            action: "panel".to_owned(),
        },
        Some(an_action()),
    );
    let scene = pressed.scene();
    let at = mark_center(&scene);
    pressed.hover(at);
    let scene = pressed.scene();
    let ground = rects_of(&scene, theme().tab_active)[0];

    assert_eq!(ground.width(), alone.width() + 14., "seven a side, evenly");
    assert_eq!(ground.height(), alone.height() + 6.);
}

#[test]
fn what_a_plugin_types_is_the_template_it_was_granted_with_its_hole_filled() {
    // The shape of the command is the person's — they allowed that string —
    // and only the hole is the plugin's.
    assert_eq!(
        fill("cd {}", "/home/eugen/Work").as_deref(),
        Some("cd '/home/eugen/Work'")
    );
    assert_eq!(
        fill("git switch {}", "main").as_deref(),
        Some("git switch 'main'")
    );
}

#[test]
fn an_argument_is_one_word_however_it_is_spelled() {
    // The whole reason the host fills the hole rather than the plugin: a
    // branch called `; rm -rf ~` has to stay a branch name.
    let line = fill("git switch {}", "; rm -rf ~").expect("a template with a hole");

    assert_eq!(line, "git switch '; rm -rf ~'");

    // And a quote inside it does not end the quoting. How it is spelled is
    // per platform — POSIX escapes the quote, PowerShell doubles it — so what
    // is asserted is what it is: one word, opened and closed by this function
    // and by nothing in the middle.
    let quoted = fill("cd {}", "it's here").expect("a template with a hole");
    assert!(quoted.starts_with("cd '") && quoted.ends_with('\''));
    assert_ne!(quoted, "cd 'it's here'", "the quote has to be dealt with");
}

#[test]
fn a_control_character_is_refused_rather_than_escaped() {
    // A newline is the character that ends a command line. A directory whose
    // name holds one is not worth the reasoning it would take to be sure.
    assert_eq!(fill("cd {}", "one\ntwo"), None);
    assert_eq!(fill("cd {}", "one\rtwo"), None);
}

#[test]
fn a_template_with_nowhere_to_put_the_argument_types_nothing() {
    // A grant is text, and text somebody wrote by hand can be wrong. What it
    // must not be is a command run with the argument silently dropped.
    assert_eq!(fill("git status", "main"), None);
}

#[test]
fn a_chip_is_a_quiet_pill_and_a_badge_is_a_loud_one() {
    // The one thing that tells the two apart, and the reason there are two: a
    // chip's ground is the theme's own raised surface, so a row of them beside
    // a prompt does not read as a row of readings.
    let mut frame = Frame::new(Node::Chip {
        icon: String::new(),
        text: String::from("~/Work/crook"),
        tone: Tone::Primary,
    });
    let scene = frame.scene();

    assert!(
        !rects_of(&scene, theme().surface_raised).is_empty(),
        "a chip sits on the theme's raised ground"
    );
    assert!(
        rects_of(&scene, theme().accent).is_empty(),
        "and never on a tone: that is what a badge is for"
    );
}

#[test]
fn a_picker_outside_a_panel_draws_nothing_rather_than_dividing_infinity() {
    // The rule `Fill` and `Meter` follow, for the same reason: a picker is a
    // column in a field's width, and a slot offers no width. See `BOUNDED`.
    let picker = Node::Picker {
        placeholder: String::from("Search directories…"),
        rows: vec![Row {
            key: String::from("/home/eugen"),
            label: String::from("home"),
            icon: String::new(),
            tone: Tone::Primary,
        }],
        choose: String::from("choose"),
    };

    let mut loose = Frame::new(picker.clone());
    let bare = loose.scene();
    let outside: usize = bare.layers().flat_map(|layer| layer.glyphs.iter()).count();

    let mut panelled = Frame::new(in_a_panel(picker));
    let drawn = panelled.scene();
    let inside: usize = drawn.layers().flat_map(|layer| layer.glyphs.iter()).count();

    assert_eq!(outside, 0, "a picker with no width to divide draws nothing");
    assert!(
        inside > 0,
        "and one in a panel draws its field and its rows"
    );
}
