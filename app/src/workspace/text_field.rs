//! A one-line text field: the text input that is not a pane's.
//!
//! Two things use it — the settings rail's search box and the worktree
//! creator's branch name — and they differ by a placeholder and an icon.
//!
//! # Why not [`CommandInput`]
//!
//! Because that element is a *composer*. It draws in the terminal's cell grid
//! so that it lines up character-for-character with the output above it, it
//! draws its first row on the shell's own prompt row at a negative offset, it
//! takes its colours from the pty's palette, its Enter sends a line, and its
//! keyboard policy is [`input_keys::route`] — whose first three rules are
//! about a selection, an alternate screen and a signal reaching a shell. A
//! search box in a settings sidebar wants none of that and would have to
//! switch every one of them off.
//!
//! What it shares instead is the two things worth sharing. The state is a
//! [`TextInput`] — the same editor, the same undo stack, the same caret blink,
//! the same `has_keys` arbitration the composer uses — and the keymap is
//! [`input_keys::intent`], which is `route` with the pane taken out of it. So
//! the field gets word movement, the line ends, the clipboard chords and the
//! emacs bindings macOS puts in every text field, and it gets them from the
//! same table that gives them to the composer, which is what stops the two
//! drifting.
//!
//! [`CommandInput`]: super::input_element::CommandInput
//!
//! # Shaped, not celled
//!
//! The glyphs go through the shaper, like every label on the page beside it,
//! rather than through the monospace cell path the composer uses. A search box
//! that came out in the terminal font would read as a terminal in the middle
//! of a settings sidebar. That means the caret's position comes from the
//! shaped line — [`Line::x_for_index`] — rather than from a column times a
//! cell width, and a click is resolved by walking the grapheme boundaries and
//! asking the same question of each, which at the length of a search query is
//! a dozen comparisons.

use crookui_core::elements::MouseStateHandle;
use crookui_core::event::{DispatchedEvent, Event, Keystroke, MouseButton};
use crookui_core::fonts::{LineStyle, Properties, StyleAndFont, TextStyle};
use crookui_core::geometry::Point;
use crookui_core::icons::IconKey;
use crookui_core::prelude::*;
use crookui_core::presenter::{LayoutContext, PaintContext};
use crookui_core::text_layout::Line;

use crate::clipboard::Clipboard;
use crate::editor::{Editor, Motion};
use crate::input_keys::{self, Intent, Platform};
use crate::text_input::TextInput;
use crate::theme::theme;

use super::view::Fonts;

/// The field's height, which is the rail's row height plus its padding.
pub(crate) const HEIGHT: f32 = 26.;

/// The type size, matching the rail's own rows.
const TEXT_SIZE: f32 = 12.;

/// The magnifier, and the gap after it.
const ICON_SIZE: f32 = 12.;
const ICON_GAP: f32 = 6.;

/// The inset inside the box, left and right.
const PADDING: f32 = 7.;

/// The caret's width.
const CARET_WIDTH: f32 = 1.5;

/// One field.
pub struct TextField {
    input: TextInput,
    clipboard: Clipboard,
    fonts: Fonts,
    hover: MouseStateHandle,
    /// What it says while nothing has been typed.
    placeholder: &'static str,
    /// The mark at its left edge, where it has one. A search box says what it
    /// is with a magnifier; a field whose label is written above it says it
    /// with the label.
    icon: Option<Lucide>,
    /// The shaped text, kept from layout for paint and for hit testing.
    line: Option<Line>,
    /// Whether `line` is the placeholder rather than what was typed.
    showing_placeholder: bool,
    size: Option<Vector2F>,
    origin: Option<Point>,
}

impl TextField {
    /// A field over `input`, which is where everything typed into it lives.
    pub fn new(
        input: TextInput,
        clipboard: Clipboard,
        fonts: Fonts,
        hover: MouseStateHandle,
        placeholder: &'static str,
    ) -> Self {
        Self {
            input,
            clipboard,
            fonts,
            hover,
            placeholder,
            icon: None,
            line: None,
            showing_placeholder: false,
            size: None,
            origin: None,
        }
    }

    /// Puts a mark at the field's left edge.
    pub fn with_icon(mut self, icon: Lucide) -> Self {
        self.icon = Some(icon);
        self
    }

    /// Where the glyphs begin, relative to the field's own origin.
    fn text_origin(&self, origin: Vector2F) -> Vector2F {
        let icon = if self.icon.is_some() {
            ICON_SIZE + ICON_GAP
        } else {
            0.
        };
        origin + vec2f(PADDING + icon, 0.)
    }

    /// Handles a keystroke, if this field is the one listening.
    ///
    /// Three keys are answered here rather than by the keymap, because all
    /// three mean "a line, and somewhere to send it" and this field has
    /// neither. Enter and Shift-Enter do nothing — the filter has already run,
    /// on the keystroke that changed the text — and the arrows that would walk
    /// a shell's history move the caret instead, which in a single-line field
    /// means the two ends of it.
    fn type_key(&self, keystroke: &Keystroke, chars: &str, ctx: &mut EventContext) -> bool {
        if !self.input.has_keys() {
            return false;
        }

        // Escape empties the box, and empties it *before* it closes anything:
        // a filter left in force behind a panel nobody can see is a settings
        // page that has silently lost most of its rows.
        if keystroke.key == "escape" && keystroke.modifiers.is_empty() {
            if self.input.editor().is_empty() {
                return false;
            }
            self.input.edit(Editor::clear);
            ctx.notify();
            return true;
        }

        let Some(intent) = input_keys::intent(keystroke, chars, Platform::current()) else {
            return false;
        };

        let intent = match intent {
            Intent::Submit | Intent::Newline => return true,
            Intent::HistoryUp => Intent::Move(Motion::LineStart),
            Intent::HistoryDown => Intent::Move(Motion::LineEnd),
            other => other,
        };

        self.input.apply(intent, &self.clipboard);
        ctx.notify();
        true
    }

    /// The byte offset a press at `x` lands on.
    ///
    /// The nearest grapheme boundary, measured through the same shaped line
    /// the glyphs were drawn from — so a click lands where it looks like it
    /// lands even in text the shaper reordered or ligated.
    fn offset_at(&self, x: f32) -> usize {
        let Some(line) = self.line.as_ref() else {
            return 0;
        };
        let text = self.input.editor().text().to_owned();

        let mut best = 0;
        let mut best_distance = f32::INFINITY;
        for offset in boundaries(&text) {
            let distance = (line.x_for_index(offset) - x).abs();
            if distance < best_distance {
                best_distance = distance;
                best = offset;
            }
        }
        best
    }
}

/// Every grapheme boundary of `text`, including both ends.
fn boundaries(text: &str) -> impl Iterator<Item = usize> + '_ {
    use unicode_segmentation::UnicodeSegmentation;

    text.grapheme_indices(true)
        .map(|(at, _)| at)
        .chain(std::iter::once(text.len()))
}

impl Element for TextField {
    fn layout(
        &mut self,
        constraint: SizeConstraint,
        ctx: &mut LayoutContext,
        _: &AppContext,
    ) -> Vector2F {
        let typed = self.input.editor().text().to_owned();
        self.showing_placeholder = typed.is_empty();
        let text = if self.showing_placeholder {
            self.placeholder.to_owned()
        } else {
            typed
        };

        let style = StyleAndFont::new(self.fonts.ui, Properties::default(), TextStyle::default());
        self.line = Some(ctx.text_layout.layout_line(
            &text,
            LineStyle {
                font_size: TEXT_SIZE,
                ..Default::default()
            },
            &[(0..text.len(), style)],
            f32::INFINITY,
        ));

        let size = constraint.apply(vec2f(constraint.max.x(), HEIGHT));
        self.size = Some(size);
        size
    }

    fn paint(&mut self, origin: Vector2F, ctx: &mut PaintContext, _: &AppContext) {
        let size = self
            .size
            .expect("the field was painted before it was laid out");
        let line = self
            .line
            .as_ref()
            .expect("the field was painted before it was laid out");
        self.origin = Some(Point::from_vec2f(origin, ctx.scene.z_index()));

        let listening = self.input.has_keys();
        let hovered = self.hover.lock().is_hovered();
        let border = if listening {
            theme().accent
        } else if hovered {
            theme().overlay_3
        } else {
            theme().border
        };

        // Hit-recorded: a press on the box is a press on the field, and the
        // rail row under it must not get it.
        ctx.scene
            .draw_rect_with_hit_recording(RectF::new(origin, size))
            .with_background(theme().overlay_1)
            .with_border(Border::all(1.).with_border_color(border))
            .with_corner_radius(CornerRadius::with_all(Radius::Pixels(6.)));

        if let Some(icon) = self.icon {
            ctx.scene.draw_icon(
                IconKey::new(icon, ICON_SIZE),
                RectF::new(
                    origin + vec2f(PADDING, (size.y() - ICON_SIZE) / 2.),
                    Vector2F::splat(ICON_SIZE),
                ),
                if self.showing_placeholder {
                    theme().text_muted
                } else {
                    theme().text_primary
                },
            );
        }

        let text_origin = self.text_origin(origin);
        let width = size.x() - (text_origin.x() - origin.x()) - PADDING;
        let box_top = text_origin.y() + (size.y() - line.height()) / 2.;
        let text_box = RectF::new(vec2f(text_origin.x(), box_top), vec2f(width, line.height()));

        // The selection goes under the glyphs, the caret over them: the same
        // order the composer paints in, and the only order in which a caret
        // inside a selection is visible.
        let selection = self.input.editor().selection();
        if !self.showing_placeholder && !selection.is_empty() {
            let start = line.x_for_index(selection.start());
            let end = line.x_for_index(selection.end());
            ctx.scene
                .draw_rect_without_hit_recording(RectF::new(
                    vec2f(text_origin.x() + start, box_top),
                    vec2f((end - start).min(width - start), line.height()),
                ))
                .with_background(theme().selection);
        }

        let ink = if self.showing_placeholder {
            theme().text_muted
        } else {
            theme().text_primary
        };
        line.paint(text_box, ink, ctx.scene);

        if listening && self.input.caret_is_visible() {
            let caret = line.x_for_index(self.input.editor().caret()).min(width);
            ctx.scene
                .draw_rect_without_hit_recording(RectF::new(
                    vec2f(text_origin.x() + caret, box_top + 1.),
                    vec2f(CARET_WIDTH, line.height() - 2.),
                ))
                .with_background(theme().accent);
        }
    }

    fn dispatch_event(
        &mut self,
        event: &DispatchedEvent,
        ctx: &mut EventContext,
        _: &AppContext,
    ) -> bool {
        let Some(bounds) = self.bounds() else {
            return false;
        };
        let Some(z_index) = self.z_index() else {
            return false;
        };
        // A keystroke is never filtered by what is painted over the field; a
        // press is, so that a menu open over the page is not clicked through.
        let Some(event) = event.at_z_index(z_index, ctx) else {
            return false;
        };

        match event {
            Event::KeyDown { keystroke, chars } => self.type_key(keystroke, chars, ctx),
            Event::MouseDown {
                position,
                button: MouseButton::Left,
                click_count,
                ..
            } if bounds.contains_point(*position) => {
                let offset = self.offset_at(position.x() - self.text_origin(bounds.origin()).x());
                self.input.press(offset, *click_count);
                ctx.notify();
                true
            }
            Event::MouseDragged {
                button: MouseButton::Left,
                position,
                ..
            } => {
                let offset = self.offset_at(position.x() - self.text_origin(bounds.origin()).x());
                if self.input.drag_to(offset) {
                    ctx.notify();
                    return true;
                }
                false
            }
            Event::MouseUp {
                button: MouseButton::Left,
                ..
            } => self.input.release(),
            _ => false,
        }
    }

    fn size(&self) -> Option<Vector2F> {
        self.size
    }

    fn origin(&self) -> Option<Point> {
        self.origin
    }
}
