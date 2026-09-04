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

use crookui_core::elements::Padding;
use crookui_core::fonts::{FamilyId, Properties, Weight};
use crookui_core::geometry::Color;
use crookui_core::prelude::*;

use crook_plugin_api::{Gap, Node, Size, Tone};

use crate::plugin::ActionId;
use crate::theme::theme;
use crate::workspace::WorkspaceAction;

/// The three sizes, in the interface's own numbers.
const SMALL: f32 = 10.5;
const BODY: f32 = 12.;
const LARGE: f32 = 16.;

/// Builds the element a node describes.
///
/// `action` resolves one of the plugin's action names to something the window
/// can dispatch; a button whose action answers to nothing is drawn inert
/// rather than left out, because a control that vanishes is harder to explain
/// than one that does not respond.
pub(super) fn element(
    node: &Node,
    ui: FamilyId,
    action: &dyn Fn(&str) -> Option<ActionId>,
) -> Box<dyn Element> {
    match node {
        Node::Empty => Empty::new().finish(),
        Node::Text { text, size, tone } => Text::new(text.clone(), ui, points(*size))
            .with_color(colour(*tone))
            .finish(),
        Node::Badge { text, tone } => badge(text, *tone, ui),
        Node::Icon { name, tone } => match Lucide::named(name) {
            Some(icon) => Icon::new(icon, BODY).with_color(colour(*tone)).finish(),
            // A name this build has no icon for draws nothing. A plugin
            // written against a newer Crook should be missing a glyph, not
            // refused.
            None => Empty::new().finish(),
        },
        Node::Row(children) => {
            let mut row = Flex::row()
                .with_main_axis_size(MainAxisSize::Min)
                .with_cross_axis_alignment(CrossAxisAlignment::Center);
            for child in children {
                row.add_child(element(child, ui, action));
            }
            row.finish()
        }
        Node::Column(children) => {
            let mut column = Flex::column()
                .with_main_axis_size(MainAxisSize::Min)
                .with_cross_axis_alignment(CrossAxisAlignment::Start);
            for child in children {
                column.add_child(element(child, ui, action));
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
        } => button(label, action(name), *tone, ui),
    }
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

/// Something to press.
///
/// Not `Hoverable`: a plugin's button holds no state of its own between
/// frames, and a mouse-state handle keyed by nothing would be a handle every
/// button on screen shared. What it loses is the hover highlight, which is the
/// honest cost of a control described rather than owned — and the day that
/// matters, the state goes in the plugin's own entry rather than here.
fn button(label: &str, action: Option<ActionId>, tone: Tone, ui: FamilyId) -> Box<dyn Element> {
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
        (Some(id), true) => Hoverable::new(MouseStateHandle::default(), move |_| face)
            .on_click(move |_, ctx, _| ctx.dispatch_typed_action(WorkspaceAction::Run(id)))
            .finish(),
        _ => face,
    }
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

/// What a size is, in points.
fn points(size: Size) -> f32 {
    match size {
        Size::Small => SMALL,
        Size::Body => BODY,
        Size::Large => LARGE,
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
