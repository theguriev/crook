//! Turning what a sandboxed plugin described into what the window draws.
//!
//! The whole of the second tier's UI contract is here, and it is deliberately
//! one file: a plugin outside the binary can produce exactly the shapes below
//! and nothing else, so "what can a store plugin put on screen" is a question
//! with an answer somebody can read in one sitting.
//!
//! # Every tone is resolved here, not there
//!
//! A [`Node`] names *what a thing is* and this decides what that looks like.
//! So a plugin written before a theme existed is drawn correctly in it, a
//! plugin cannot produce grey-on-grey by accident, and changing what "warning"
//! looks like changes it everywhere at once — including in plugins nobody can
//! rebuild.
//!
//! # Every measurement is the host's too
//!
//! A bar's thickness, a panel's width, the corner on either of them: a plugin
//! names none of these and could not name them usefully if it tried, because
//! the window it will be drawn in is not the window it was written against.
//! The numbers below are the ones Crook's own chrome carries, so a panel a
//! plugin hangs off its chip is the panel the usage chip used to hang off its
//! own — down to the hairline.

use std::cell::{Cell, RefCell};

use crookui_core::elements::{Margin, Padding, Paragraph};
use crookui_core::fonts::{FamilyId, Properties, Weight};
use crookui_core::geometry::Color;
use crookui_core::icons::{Art, Chomp};
use crookui_core::prelude::*;

use crook_plugin_api::{Gap, Node, Size, Tone};

use crate::plugin::ActionId;
use crate::theme::theme;
use crate::workspace::WorkspaceAction;

/// The three sizes, in the interface's own numbers.
const SMALL: f32 = 10.5;
const BODY: f32 = 12.;
const LARGE: f32 = 16.;

/// How big the words and the marks are in the place this contribution is
/// drawn.
///
/// A plugin names three sizes and no numbers, and until there was a slot
/// smaller than a row there was only one set of numbers to resolve them to.
/// There is more than one now: a mark at the head of a tab row has 24 pixels
/// to be seen in and the badge on its corner has eleven, and a plugin whose
/// [`Size::Body`] came out at twelve in both would be illegible in one and
/// overflowing the other. So the *place* carries the numbers, which is the
/// same division of labour every other measurement here follows — the plugin
/// says what a thing is and the host says how big that is here.
#[derive(Copy, Clone, Debug, PartialEq)]
pub(super) struct Scale {
    small: f32,
    body: f32,
    large: f32,
    /// What an [`Icon`](Node::Icon) is drawn at, which is one number rather
    /// than three: a mark has no size of its own to name.
    icon: f32,
    /// What Crook's own drawn artwork is drawn at, which is not the icon size.
    /// See [`PIRATE_SIZE`].
    art: f32,
}

impl Scale {
    /// A contribution in a row of ordinary chrome — the header, a panel.
    pub(super) const ROW: Self = Self {
        small: SMALL,
        body: BODY,
        large: LARGE,
        icon: BODY,
        art: PIRATE_SIZE,
    };

    /// The 24px mark at the head of a tab row.
    ///
    /// Sixteen for a body, which is what makes a glyph in that box read as the
    /// disc it replaced rather than as a caption where a mark should be: the
    /// disc it stands in for is 18 pixels across, and a 12pt glyph in a 24pt
    /// box looks like something that failed to load.
    pub(super) const MARK: Self = Self {
        small: 13.,
        body: 16.,
        large: 19.,
        icon: 16.,
        // The disc it stands in for, near enough: artwork fills its box edge
        // to edge where an icon is a stroke inside a margin.
        art: 20.,
    };

    /// The badge on the corner of that mark, which has about eight pixels
    /// inside its ring and can hold one glyph.
    pub(super) const BADGE: Self = Self {
        small: 7.,
        body: 8.,
        large: 9.,
        icon: 8.5,
        art: 8.5,
    };

    /// What a size is here, in points.
    fn points(self, size: Size) -> f32 {
        match size {
            Size::Small => self.small,
            Size::Body => self.body,
            Size::Large => self.large,
        }
    }
}

/// The corner on something a plugin made pressable.
///
/// The same six pixels every other control in Crook takes, and deliberately
/// not a fully rounded one. A stadium corner says "badge" — a thing that
/// reports a state — and the moment a chip is wide enough to hold a mark and a
/// number beside it, the two semicircular ends read as slack rather than as
/// shape. Six is what a button, a menu and a panel already use here.
const PRESS_RADIUS: f32 = 6.;

/// What a pressable puts between its content and its own edge.
///
/// Even on both sides, which is worth saying because the chip this replaced
/// was not: it reserved room for the widest percentage it could ever print, so
/// that the mark beside the number never moved. That reservation is what a
/// person sees as a gap after a short reading, and it is the wrong trade for
/// something sitting at the end of a row — the chip is the last thing in the
/// header, so what a wider number costs is a few pixels of empty header, and
/// what the reservation cost was visible on every frame.
const PRESS_PADDING: Padding = Padding {
    top: 3.,
    bottom: 3.,
    left: 7.,
    right: 7.,
};

/// How thick a meter is, and — halved — its corner.
///
/// The usage panel's own bars, which is where this number comes from: five
/// pixels is thin enough that a row of them reads as a measurement rather than
/// as a stack of buttons, and a corner of half the height is what makes the
/// track a lozenge instead of a rectangle with rounded ends.
const METER_HEIGHT: f32 = 5.;

/// How tall the tallest column of a [`Node::Bars`] is drawn.
const BARS_HEIGHT: f32 = 34.;

/// How wide one column is.
///
/// Fixed, and much narrower than the share of the row each column is given: a
/// column as wide as it is tall is a block, and seven blocks in a row read as
/// a chart of nothing. The space around them is what makes the shape legible.
const BARS_WIDTH: f32 = 13.;

/// What a column of nothing still gets.
///
/// Two pixels, because a day with no work has to be a column of no height
/// rather than a gap — a chart with a hole in it says the week was shorter
/// than it was.
const BARS_FLOOR: f32 = 2.;

/// How thick the hairline is. One pixel, before the display scale, because a
/// seam that is two is a border.
const RULE_HEIGHT: f32 = 1.;

/// How wide a panel is. Wider than the options menu's 200, for the reason the
/// usage panel is: this one carries a figure and a label on one line, and a
/// meter across another.
const PANEL_WIDTH: f32 = 280.;

/// Its corner, matching the options menu's.
const PANEL_RADIUS: f32 = 6.;

/// Above the first row and below the last.
const PANEL_PADDING: f32 = 10.;

/// And at each side of every row in it.
///
/// Crook's own panel insets each *row* by fourteen and lets the rule between
/// two sections run the full width — a shape this vocabulary cannot say,
/// because a plugin describes rows and not their margins. So the panel insets
/// all of its content and [`rule`] steps back out by exactly this much, which
/// puts the decision where it belongs: a plugin says "these two things are not
/// the same measurement" and the host draws that seam the way Crook draws
/// seams. Leaving it to the plugin — a [`Node::Gap`] at each end of every row
/// — is the same decision made twenty times, and gets it wrong once.
const PANEL_INSET: f32 = 14.;

/// Between the thing a panel hangs off and the panel itself, so the two do not
/// read as one box.
const PANEL_OFFSET: f32 = 6.;

/// How big the pirate is drawn beside a row's own text, a little over the
/// 12pt label beside him — [`Scale`] carries the number for everywhere else.
/// The
/// artwork is a disc that fills its box edge to edge, where an icon of the
/// same nominal size is a stroke inside two units of margin, so matching the
/// numbers would draw a pirate that towered over everything else in the row.
const PIRATE_SIZE: f32 = 15.;

/// The pirate's yellow, and the black of the patch and the strap.
///
/// Constants rather than theme roles: this is a piece of artwork, like a logo,
/// and a pirate whose face took the palette's cast would stop being the mark
/// people recognise. The one thing the theme decides is what a *stale* pirate
/// looks like — see [`pirate`].
const PIRATE_FACE: Color = Color::hex(0xf9d949);
const PIRATE_INK: Color = Color::hex(0x151515);

/// The mouse state a plugin's controls keep between frames.
///
/// A `MouseStateHandle` has to be keyed by something that outlives the element
/// tree, and a node has no identity: a plugin describes a fresh tree every
/// render, and two chips drawn from one entry are indistinguishable from one
/// chip drawn twice. What *does* outlive the tree is the contribution — one
/// entry in one slot, built once — so the handles live there and are handed
/// out in the order the tree asks for them. Two frames of a control that has
/// not changed shape ask in the same order and get the same handle, which is
/// the whole of what makes a hover survive the frame it started in.
///
/// A tree that changes shape between frames — a second button appearing before
/// the first — shifts everything after it by one, and what that costs is a
/// hover drawn on the wrong control for exactly one frame. The alternative is
/// to make a plugin name its controls, which is asking a stranger to get right
/// something the host can be approximately right about for free.
#[derive(Default)]
pub(super) struct Hovers {
    /// One per control the tree has ever asked for, in the order it asked.
    kept: RefCell<Vec<MouseStateHandle>>,
    /// How far through that order this render has got.
    next: Cell<usize>,
}

impl Hovers {
    /// Starts a render at the beginning of the order.
    fn rewind(&self) {
        self.next.set(0);
    }

    /// The handle for the next control in the tree.
    fn take(&self) -> MouseStateHandle {
        let index = self.next.get();
        self.next.set(index + 1);

        let mut kept = self.kept.borrow_mut();
        if index >= kept.len() {
            kept.resize_with(index + 1, MouseStateHandle::default);
        }
        kept[index].clone()
    }
}

/// The font and the sizes every node in one contribution is drawn with.
///
/// One value rather than two parameters because they travel together down the
/// whole tree and always have: a node names neither, and both are answers to
/// the same question — what does a plugin's description look like *here*.
#[derive(Copy, Clone)]
struct Chrome {
    /// The interface's own family. Nothing in this tier chooses a font.
    ui: FamilyId,
    /// How big the words and the marks are in this place.
    scale: Scale,
}

/// Builds the element a node describes.
///
/// `action` resolves one of the plugin's action names to something the window
/// can dispatch; a button whose action answers to nothing is drawn inert
/// rather than left out, because a control that vanishes is harder to explain
/// than one that does not respond. Every other node that names an action —
/// [`Node::Pressable`], [`Node::Anchored`] — follows the same rule.
pub(super) fn element(
    node: &Node,
    ui: FamilyId,
    scale: Scale,
    action: &dyn Fn(&str) -> Option<ActionId>,
    hovers: &Hovers,
) -> Box<dyn Element> {
    hovers.rewind();
    let chrome = Chrome { ui, scale };
    // A contribution is measured against whatever its slot offers, and a slot
    // offers no width: `header.right` hands its entry an infinite main axis,
    // because the row it sits in has already given its surplus to a filler.
    // So a contribution starts unbounded, and the one place that changes is a
    // panel — see [`BOUNDED`].
    element_in(node, chrome, action, hovers, UNBOUNDED)
}

/// Whether the subtree being built has a width to take a share of.
///
/// Two of the shapes here — [`Node::Fill`] and [`Node::Meter`] — are a *share
/// of an axis*, and an axis nobody bounded cannot be shared. `Flex` says so
/// itself, loudly: a debug build asserts and a release build logs and lays out
/// something degenerate. Neither is a thing a stranger's plugin gets to cause,
/// so the fact travels down the tree and a share asked for where there is
/// nothing to share draws nothing and says why, once.
///
/// The host bounds exactly one thing, and it is the thing that needs it: a
/// panel is [`PANEL_WIDTH`] wide because the host made it so.
const BOUNDED: bool = true;
/// See [`BOUNDED`].
const UNBOUNDED: bool = false;

/// Builds the element a node describes, knowing whether it has room to divide.
fn element_in(
    node: &Node,
    chrome: Chrome,
    action: &dyn Fn(&str) -> Option<ActionId>,
    hovers: &Hovers,
    bounded: bool,
) -> Box<dyn Element> {
    match node {
        Node::Empty => Empty::new().finish(),
        Node::Text { text, size, tone } => {
            Text::new(text.clone(), chrome.ui, chrome.scale.points(*size))
                .with_color(colour(*tone))
                .finish()
        }
        Node::Badge { text, tone } => badge(text, *tone, chrome.ui),
        Node::Icon { name, tone } => icon(name, *tone, chrome.scale),
        Node::Row(children) => {
            let mut row = Flex::row()
                .with_main_axis_size(main_axis_size(children, bounded))
                .with_cross_axis_alignment(CrossAxisAlignment::Center);
            for child in children {
                row.add_child(element_in(child, chrome, action, hovers, bounded));
            }
            row.finish()
        }
        Node::Column(children) => {
            let mut column = Flex::column()
                .with_main_axis_size(main_axis_size(children, bounded))
                .with_cross_axis_alignment(CrossAxisAlignment::Start);
            for child in children {
                column.add_child(element_in(child, chrome, action, hovers, bounded));
            }
            column.finish()
        }
        Node::Gap(gap) => ConstrainedBox::new(Empty::new().finish())
            .with_width(pixels(*gap))
            .with_height(pixels(*gap))
            .finish(),
        Node::Button {
            label,
            action: name,
            tone,
        } => button(label, action(name), *tone, chrome.ui, hovers.take()),
        Node::Meter { fraction, tone } if bounded => meter(*fraction, *tone),
        Node::Rule => rule(),
        Node::Bars { values, tone } if bounded => bars(values, *tone),
        // The idiomatic spacer, and the only thing in this vocabulary that
        // asks its parent for room rather than reporting a size of its own.
        Node::Fill if bounded => Expanded::new(1., Empty::new().finish()).finish(),
        // A share of an axis nobody bounded. See [`BOUNDED`].
        Node::Fill | Node::Meter { .. } | Node::Bars { .. } => unbounded_share(),
        Node::Note { text, tone } => note(text, *tone, chrome.ui),
        Node::Pressable {
            content,
            action: name,
        } => pressable(
            element_in(content, chrome, action, hovers, bounded),
            action(name),
            hovers.take(),
        ),
        Node::Anchored {
            content,
            panel,
            dismiss,
        } => anchored(
            content,
            panel.as_deref(),
            action(dismiss),
            chrome,
            action,
            hovers,
            bounded,
        ),
    }
}

/// How much room a row or a column takes along its own axis.
///
/// A flex holding a [`Node::Fill`] takes everything it is offered, and one
/// without holds only what its children need. Both halves matter. A row that
/// stayed at `Min` would still be stretched by the tight [`Expanded`] inside
/// it — the arithmetic works out the same — but it would be saying the
/// opposite of what it does, and the day a spacer stops filling its whole
/// share the row would silently collapse around it. And a flex that expanded
/// *without* a `Fill` would push a chip in the header out to the width of the
/// header.
///
/// What neither can do is divide an axis nobody bounded: a `Fill` inside a row
/// that is itself inside a row is being asked to take its share of infinity.
/// That is [`Flex`]'s own rule rather than this tier's — it says so, loudly,
/// in a debug build — and the shape to describe instead is a `Fill` in a
/// column's row, where the width comes from the panel around it.
fn main_axis_size(children: &[Node], bounded: bool) -> MainAxisSize {
    if bounded && children.iter().any(|child| matches!(child, Node::Fill)) {
        MainAxisSize::Max
    } else {
        MainAxisSize::Min
    }
}

/// What a share of an axis nobody bounded comes to.
///
/// Nothing, and one line in the log for whoever wrote the plugin — once,
/// because this is on the frame path and a plugin drawing a meter in the
/// header would otherwise write a line sixty times a second. The alternative
/// is what `Flex` does when it is asked to divide infinity, which is to assert
/// in a debug build; a plugin from a store may not do that to a window.
fn unbounded_share() -> Box<dyn Element> {
    static SAID: std::sync::Once = std::sync::Once::new();
    SAID.call_once(|| {
        log::warn!(
            "a plugin asked for a share of a width nothing gave it \
             \u{2014} a Fill or a Meter outside a panel draws nothing"
        );
    });
    Empty::new().finish()
}

/// A pill, the shape the usage chip is drawn in.
fn badge(text: &str, tone: Tone, ui: FamilyId) -> Box<dyn Element> {
    Container::new(
        Text::new(text.to_owned(), ui, SMALL)
            .with_color(theme().ground)
            .with_style(Properties {
                weight: Weight::Semibold,
                ..Properties::default()
            })
            .finish(),
    )
    .with_background_color(colour(tone))
    .with_corner_radius(CornerRadius::with_all(Radius::Percentage(50.)))
    .with_padding(Padding {
        top: 3.,
        bottom: 3.,
        left: 8.,
        right: 8.,
    })
    .finish()
}

/// A mark, by whatever a plugin called it.
///
/// Two vocabularies, looked up in this order because only one of them can
/// grow: Crook's own drawn marks are a short list this file writes down, and
/// everything else is Lucide, which is thousands of names nobody here chose.
fn icon(name: &str, tone: Tone, scale: Scale) -> Box<dyn Element> {
    if let Some(chomp) = chomp_named(name) {
        return pirate(chomp, tone, scale);
    }

    match Lucide::named(name) {
        Some(icon) => Icon::new(icon, scale.icon)
            .with_color(colour(tone))
            .finish(),
        // A name this build has no icon for draws nothing. A plugin
        // written against a newer Crook should be missing a glyph, not
        // refused.
        None => Empty::new().finish(),
    }
}

/// Which frame of the bite a name asks for, if it asks for the pirate at all.
///
/// Three names rather than one name and a frame number, because a plugin does
/// not describe an animation here: **the host runs no timer for this**. A
/// plugin that wants a chomping pirate holds its own clock, names a different
/// frame on each render and asks to be drawn again — which is the same deal
/// every other piece of plugin state gets, and the reason a plugin nobody is
/// waiting on costs the window nothing. Anyone tempted to add a frame timer to
/// this file is about to animate a mark on behalf of a plugin that did not ask
/// for it, sixty times a second, in every window.
fn chomp_named(name: &str) -> Option<Chomp> {
    match name {
        "pirate" => Some(Chomp::Shut),
        "pirate-open" => Some(Chomp::Open),
        "pirate-wide" => Some(Chomp::Wide),
        _ => None,
    }
}

/// The pirate, at one frame of his bite.
///
/// Two layers rather than one: a rasterized mark is a coverage mask and a mask
/// has one colour, so the yellow head and the black on it are drawn one over
/// the other. Neither is meaningful alone.
///
/// [`Tone::Muted`] greys the **face** and leaves the ink alone; every other
/// tone leaves both the colours they were drawn in. That is the one thing a
/// plugin gets to say about a mark, and greying the face is what says a
/// reading is stale — a picture that stayed bright beside a greyed-out number
/// would be the loudest thing in the row insisting it is current.
///
/// The ink stays dark, and the reason is what a two-layer mask is. The ink is
/// the eyepatch, the strap and the grin, drawn *on* the face; painting both in
/// one colour does not produce a grey pirate, it produces a plain disc with
/// nothing on it, because there is nothing left to tell the layers apart. That
/// shipped, and what it looked like on somebody's screen was a grey circle.
fn pirate(chomp: Chomp, tone: Tone, scale: Scale) -> Box<dyn Element> {
    let face = match tone {
        Tone::Muted => theme().text_muted,
        _ => PIRATE_FACE,
    };
    let ink = PIRATE_INK;

    Stack::new()
        .with_child(
            Icon::new(Art::PirateFace(chomp), scale.art)
                .with_color(face)
                .finish(),
        )
        .with_child(
            Icon::new(Art::PirateInk(chomp), scale.art)
                .with_color(ink)
                .finish(),
        )
        .finish()
}

/// Something to press, drawn as one.
///
/// Unlike a [`pressable`] this has a ground at rest, because a labelled
/// control that only appears when reached for is a control nobody finds. Its
/// mouse state comes from the contribution's own [`Hovers`], which is what
/// lets it light up under the pointer at all — a handle made here would be
/// thrown away with the frame and a button would never look hovered.
fn button(
    label: &str,
    action: Option<ActionId>,
    tone: Tone,
    ui: FamilyId,
    mouse: MouseStateHandle,
) -> Box<dyn Element> {
    let (text, enabled) = match action {
        Some(_) => (colour(tone), true),
        None => (theme().text_muted, false),
    };

    let face = Container::new(
        Text::new(label.to_owned(), ui, SMALL)
            .with_color(text)
            .finish(),
    )
    .with_background_color(theme().overlay_1)
    .with_corner_radius(CornerRadius::with_all(Radius::Pixels(6.)))
    .with_padding(Padding {
        top: 4.,
        bottom: 4.,
        left: 8.,
        right: 8.,
    })
    .finish();

    match (action, enabled) {
        (Some(id), true) => Hoverable::new(mouse, move |_| face)
            .on_click(move |_, ctx, _| ctx.dispatch_typed_action(WorkspaceAction::Run(id)))
            .finish(),
        _ => face,
    }
}

/// Anything at all, made to answer a click.
///
/// # It is nothing until somebody reaches for it
///
/// At rest there is no ground under a pressable: what is on screen is exactly
/// the subtree the plugin described, sitting on whatever surface it was
/// contributed to. A ground appears under the pointer and darkens on the
/// press, and that is the whole of the affordance — which is the rule the
/// usage chip arrived at when it was still in the box, and the reason is the
/// same. A control drawn as a control at all times is a box in the chrome
/// competing with the thing inside it; a mark that lights up when reached for
/// is a mark until it is needed.
///
/// The padding is there at rest as well as under the pointer, because a
/// control that grew by fourteen pixels when the mouse arrived would shove the
/// row along with it.
///
/// # What it still cannot say
///
/// That it is inert. A [`button`] says so by going grey; what a pressable
/// draws is the plugin's own subtree, and greying that would be the host
/// deciding for the plugin what a dimmed chip looks like. So a pressable whose
/// action answers to nothing is drawn exactly like a live one and does
/// nothing when pressed — and it is still drawn, for the reason a button is: a
/// chip that vanished because its plugin was switched off mid-session is
/// harder to explain than one that does not respond.
fn pressable(
    content: Box<dyn Element>,
    action: Option<ActionId>,
    mouse: MouseStateHandle,
) -> Box<dyn Element> {
    let face = Hoverable::new(mouse, move |state| {
        let ground = if state.is_clicked() {
            theme().ground
        } else if state.is_hovered() {
            theme().tab_active
        } else {
            // Not the surface it happens to sit on: a pressable is contributed
            // to a slot the host chooses, and a colour guessed here would be
            // the one place a plugin looks wrong in a theme nobody tested.
            Color::TRANSPARENT
        };

        Container::new(content)
            .with_background_color(ground)
            .with_corner_radius(CornerRadius::with_all(Radius::Pixels(PRESS_RADIUS)))
            .with_padding(PRESS_PADDING)
            .finish()
    });

    match action {
        Some(id) => face
            .on_click(move |_, ctx, _| ctx.dispatch_typed_action(WorkspaceAction::Run(id)))
            .finish(),
        None => face.finish(),
    }
}

/// A contribution with a panel hung under it.
///
/// The panel is up exactly while the plugin says it is: `panel` is `None` on
/// every frame it is shut, and this draws the contribution alone then rather
/// than a stack with nothing in it. Whether it is open is the plugin's state
/// because the plugin is the only thing that can answer a click on the chip —
/// the host would otherwise be keeping a flag per contribution that nothing is
/// allowed to read.
///
/// Anchored to the *right* edges, offset down by [`PANEL_OFFSET`], and kept on
/// screen: the slot a chip is contributed to is at the end of the header, so a
/// panel hung off its left corner would open past the window. Modal, like the
/// options menu, which is what makes clicking the chip again one toggle rather
/// than two — the press that dismisses is swallowed rather than delivered to
/// the chip underneath it.
fn anchored(
    content: &Node,
    panel: Option<&Node>,
    dismiss: Option<ActionId>,
    chrome: Chrome,
    action: &dyn Fn(&str) -> Option<ActionId>,
    hovers: &Hovers,
    bounded: bool,
) -> Box<dyn Element> {
    let content = element_in(content, chrome, action, hovers, bounded);
    let Some(panel) = panel else {
        return content;
    };

    let mut stack = Stack::new().with_child(content);
    stack.add_anchored_overlay_child(
        // The panel, and only the panel, has a width: the host gave it one.
        // A panel hangs *below* whatever opened it and is a surface of its
        // own, so it is drawn at the ordinary size whatever the thing it hangs
        // off was drawn at: a panel under a mark on a tab row is a panel, not
        // a mark.
        Dismiss::new(panel_chrome(element_in(
            panel,
            Chrome {
                scale: Scale::ROW,
                ..chrome
            },
            action,
            hovers,
            BOUNDED,
        )))
        .modal()
        .on_dismiss(move |ctx, _| {
            // A dismissal that answers to nothing takes nothing down: the
            // panel is the plugin's state, so a plugin that named an
            // action it never registered has hung a panel it cannot shut.
            // Inert rather than absent, for the reason a button is.
            if let Some(id) = dismiss {
                ctx.dispatch_typed_action(WorkspaceAction::Run(id));
            }
        })
        .finish(),
        AnchorTo {
            parent: Corner::BottomRight,
            child: Corner::TopRight,
            offset: vec2f(0., PANEL_OFFSET),
            keep_on_screen: true,
            keep_clear_of_parent: false,
        },
    );
    stack.finish()
}

/// The ground a panel's content sits on.
///
/// Supplied by the host and not by the plugin, which is the whole reason
/// [`Node::Anchored`] carries a panel rather than leaving a plugin to build
/// one: there is no shadow in the shader, so an opaque ground and a hairline
/// are what separate a floating panel from the header behind it, and a plugin
/// that had to know that would be a plugin that gets it wrong. What a plugin
/// describes is the rows; what it cannot do is draw a panel that does not look
/// like Crook's.
fn panel_chrome(content: Box<dyn Element>) -> Box<dyn Element> {
    ConstrainedBox::new(
        Container::new(content)
            .with_padding(Padding {
                top: PANEL_PADDING,
                bottom: PANEL_PADDING,
                left: PANEL_INSET,
                right: PANEL_INSET,
            })
            .with_background_color(theme().surface_raised)
            .with_border(Border::all(1.).with_border_color(theme().overlay_1))
            .with_corner_radius(CornerRadius::with_all(Radius::Pixels(PANEL_RADIUS)))
            .finish(),
    )
    .with_width(PANEL_WIDTH)
    .finish()
}

/// A track with part of it filled.
///
/// As long as the room it is given rather than a width of its own, which is
/// what lets one meter sit in a 280-pixel panel and another in a slot half
/// that: the fill is a *share* of the track, handed out by the flex, so
/// nothing here has to know how wide the track came out. Which also means a
/// meter wants a bounded axis, exactly as [`main_axis_size`] describes — it is
/// a thing that spans, and the shape it spans is a panel's column rather than
/// a row in the header.
///
/// The fraction is clamped rather than refused, which is what the ABI promises
/// — a reading that briefly exceeds its own limit is a thing that happens and
/// is not worth an empty frame. A share of nothing is left out altogether
/// rather than added with a flex factor of zero: a flex hands each child the
/// room that is left divided by the flex that is left, so a full bar followed
/// by a zero-factor sibling would be dividing nothing by nothing — and a NaN
/// that reaches a layout takes every size beside it with it.
fn meter(fraction: f32, tone: Tone) -> Box<dyn Element> {
    // A fraction that is not a number is not a reading. It lands on an empty
    // bar rather than being carried into the layout, where it would poison
    // every size computed beside it.
    let fraction = if fraction.is_nan() {
        0.
    } else {
        fraction.clamp(0., 1.)
    };

    let mut track = Flex::row().with_main_axis_size(MainAxisSize::Max);
    if fraction > 0. {
        track.add_child(
            Expanded::new(
                fraction,
                Container::new(Empty::new().finish())
                    .with_background_color(colour(tone))
                    .with_corner_radius(CornerRadius::with_all(Radius::Pixels(METER_HEIGHT / 2.)))
                    .finish(),
            )
            .finish(),
        );
    }
    if fraction < 1. {
        track.add_child(Expanded::new(1. - fraction, Empty::new().finish()).finish());
    }

    ConstrainedBox::new(
        Container::new(track.finish())
            .with_background_color(theme().overlay_2)
            .with_corner_radius(CornerRadius::with_all(Radius::Pixels(METER_HEIGHT / 2.)))
            .finish(),
    )
    .with_height(METER_HEIGHT)
    .finish()
}

/// A sentence, wrapped, for the things a plugin has to say in words.
///
/// The quiet size the usage panel writes its own sentences at, because that is
/// what a note *is*: the state that could not be a figure — "reading…", "no
/// transcripts yet", the name of whatever went wrong — and it is not supposed
/// to outweigh the numbers it is explaining. A plugin that wants prose at the
/// interface's own size is describing a [`Node::Text`] that happens to be
/// long, and a `Text` is the thing that does not wrap.
///
/// Copied from that panel without its insets and without its bottom margin:
/// those belong to the panel's rows rather than to the sentence, and a note in
/// a header row would otherwise arrive shoved fourteen pixels in from a panel
/// edge that is not there. Room around a note is a [`Node::Gap`] the plugin
/// asks for.
fn note(text: &str, tone: Tone, ui: FamilyId) -> Box<dyn Element> {
    Paragraph::new(text.to_owned(), ui, SMALL)
        .with_color(colour(tone))
        .finish()
}

/// A hairline across whatever holds it, full-bleed like the options menu's.
///
/// Two pixels above it and ten below, which is the asymmetry the usage panel's
/// divider carries: a rule belongs to the section it opens rather than to the
/// one it closed, so the room goes underneath.
fn rule() -> Box<dyn Element> {
    Container::new(
        ConstrainedBox::new(
            Container::new(Empty::new().finish())
                .with_background_color(theme().overlay_2)
                .finish(),
        )
        .with_height(RULE_HEIGHT)
        .finish(),
    )
    .with_margin(Margin {
        top: 2.,
        bottom: 10.,
        // Back out of the panel's own inset, so the seam runs the full width
        // the way every other hairline in Crook does. A rule that stopped
        // where the text stops would read as an underline for the row above
        // it rather than as a division between two things.
        left: -PANEL_INSET,
        right: -PANEL_INSET,
    })
    .finish()
}

/// A row of columns, each as tall as its share of the tallest.
///
/// The tallest is found here rather than asked of the plugin, because a plugin
/// that had to normalise its own numbers would be a plugin that divides by
/// zero on a quiet week. Every column takes an equal share of the width and
/// draws [`BARS_WIDTH`] of it, which is what puts the air between them.
fn bars(values: &[f32], tone: Tone) -> Box<dyn Element> {
    let tallest = values
        .iter()
        .copied()
        .filter(|value| value.is_finite())
        .fold(0., f32::max);

    // Spread evenly, with the gap before the first column and after the last
    // as wide as the ones between: seven columns pushed against the two ends
    // of a panel read as two groups rather than as a week.
    let mut row = Flex::row()
        .with_main_axis_size(MainAxisSize::Max)
        .with_main_axis_alignment(MainAxisAlignment::SpaceEvenly)
        .with_cross_axis_alignment(CrossAxisAlignment::End);

    for value in values {
        let share = if tallest > 0. && value.is_finite() {
            (value / tallest).clamp(0., 1.)
        } else {
            0.
        };
        let height = (share * BARS_HEIGHT).max(BARS_FLOOR);

        row.add_child(
            ConstrainedBox::new(
                Container::new(Empty::new().finish())
                    .with_background_color(colour(tone))
                    .with_corner_radius(CornerRadius::with_all(Radius::Pixels(1.5)))
                    .finish(),
            )
            .with_width(BARS_WIDTH)
            .with_height(height)
            .finish(),
        );
    }

    ConstrainedBox::new(row.finish())
        .with_height(BARS_HEIGHT)
        .finish()
}

/// What a tone is, in the theme in force.
fn colour(tone: Tone) -> Color {
    match tone {
        Tone::Primary => theme().text_primary,
        Tone::Muted => theme().text_muted,
        Tone::Accent => theme().accent,
        Tone::Warning => theme().usage_high,
        Tone::Danger => theme().usage_critical,
        Tone::Success => theme().usage_normal,
    }
}

/// What a gap is, in pixels.
fn pixels(gap: Gap) -> f32 {
    match gap {
        Gap::Small => 4.,
        Gap::Medium => 8.,
        Gap::Large => 16.,
    }
}
