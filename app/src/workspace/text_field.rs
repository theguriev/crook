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
use crookui_core::event::{DispatchedEvent, Event, Ime, Keystroke, MouseButton};
use crookui_core::fonts::{LineStyle, Properties, StyleAndFont, TextStyle};
use crookui_core::geometry::Point;
use crookui_core::icons::IconKey;
use crookui_core::prelude::*;
use crookui_core::presenter::{LayoutContext, PaintContext};
use crookui_core::scene::PointerShape;
use crookui_core::text_layout::Line;

use crate::clipboard::Clipboard;
use crate::editor::{Editor, Motion};
use crate::input_keys::{self, Intent, Platform};
use crate::pane_selection::PaneSelection;
use crate::selection::Cells;
use crate::text_input::TextInput;
use crate::theme::theme;

use super::action::WorkspaceAction;
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
    ///
    /// Owned rather than borrowed for the sake of one caller: a sandboxed
    /// plugin's picker names its own placeholder, and a string that arrived
    /// over a wire cannot be `'static` without being leaked once a frame.
    placeholder: String,
    /// The mark at its left edge, where it has one. A search box says what it
    /// is with a magnifier; a field whose label is written above it says it
    /// with the label.
    icon: Option<Lucide>,
    /// What to dispatch when the field is pressed.
    ///
    /// The whole of this field's part in deciding who has the keyboard, and it
    /// is deliberately this small: the field says *it was clicked*, and
    /// whatever holds the fields decides what that means. A field that set
    /// `has_keys` on itself would be a field that could not take the keyboard
    /// away from the one that had it.
    focus: Option<WorkspaceAction>,
    /// What to dispatch when a keystroke changed the text.
    ///
    /// For a holder that has to answer the text as it is typed rather than
    /// read it when a button is pressed — the worktree creator's branch
    /// follows its prompt. Said after the change, and only for one: a caret
    /// moved or a selection made is not an edit.
    edited: Option<WorkspaceAction>,
    /// The selection in the output this field floats over, for the one field
    /// that does: the find bar's. See [`Self::over_output`].
    output: Option<PaneSelection>,
    /// Whether the window this field is in has the desktop's keyboard. See
    /// [`Self::with_window_focused`].
    window_focused: bool,
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
        placeholder: impl Into<String>,
    ) -> Self {
        Self {
            input,
            clipboard,
            fonts,
            hover,
            placeholder: placeholder.into(),
            icon: None,
            focus: None,
            edited: None,
            output: None,
            // In front until told otherwise, as the workspace's own bit is.
            window_focused: true,
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

    /// What to dispatch when it is pressed.
    ///
    /// For a surface with more than one field on it, where a press has to move
    /// the keyboard. A field with none is a field that is either the only one
    /// or has the keyboard by some other rule.
    pub fn with_focus(mut self, action: WorkspaceAction) -> Self {
        self.focus = Some(action);
        self
    }

    /// What to dispatch when a keystroke changes what the field holds.
    pub fn with_edit(mut self, action: WorkspaceAction) -> Self {
        self.edited = Some(action);
        self
    }

    /// Lets the copy chord through to a selection in the output under the
    /// field, while the field has nothing selected of its own.
    ///
    /// For the find bar, which floats over a pane's list and so is handed
    /// every keystroke before the list is. Claiming the chord with nothing to
    /// copy took it from the drag a person had just made over the output, and
    /// the clipboard kept whatever it held before. Rule 1 of
    /// [`input_keys::route`] gives an output selection the chord over the
    /// composer's; this is that rule for a field drawn on top of the output
    /// rather than under it, stopping short where the field has text selected
    /// itself — clicking into this field lets go of no highlight, as clicking
    /// into the composer does, so an output selection here can be one nobody
    /// is aiming at any more.
    pub fn over_output(mut self, selection: PaneSelection) -> Self {
        self.output = Some(selection);
        self
    }

    /// Says whether the window has the desktop's keyboard, which the field is
    /// drawn as listening — the accent ring and the caret — only while it does.
    ///
    /// The drawing's business and not the keyboard's, as it is for a pane's
    /// composer: a find bar or a search box with the keys in a window behind
    /// another application is one nothing typed reaches, and drawn listening
    /// it made every Crook window on a desktop look like the one being typed
    /// into. A keystroke still asks [`TextInput::has_keys`], so the window
    /// that comes back types into the field it left.
    pub fn with_window_focused(mut self, focused: bool) -> Self {
        self.window_focused = focused;
        self
    }

    /// Whether the copy chord belongs to the output under the field rather
    /// than to the field. See [`Self::over_output`].
    fn yields_copy(&self) -> bool {
        self.input.editor().selected_text().is_empty()
            && self
                .output
                .as_ref()
                .is_some_and(|output| output.has_selection_in(Cells::List))
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
    ///
    /// And the copy chord is declined while nothing in the field is selected,
    /// so that it reaches what is under the field. The find bar floats over a
    /// pane's output, and with the bar holding the keyboard a line dragged out
    /// of that output is the only thing the copy chord can mean: claimed here,
    /// it copied nothing and the output that had the selection never heard it.
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
            self.say_edited(ctx);
            ctx.notify();
            return true;
        }

        let Some(intent) = input_keys::intent(keystroke, chars, Platform::current()) else {
            return false;
        };
        if intent == Intent::Copy && self.yields_copy() {
            return false;
        }

        let intent = match intent {
            Intent::Submit | Intent::Newline => return true,
            Intent::HistoryUp => Intent::Move(Motion::LineStart),
            Intent::HistoryDown => Intent::Move(Motion::LineEnd),
            Intent::Copy if self.input.editor().selection().is_empty() => return false,
            other => other,
        };

        // Compared rather than inferred from the intent: a paste of nothing,
        // a delete at the start and an undo with nothing to undo are all
        // edits by kind and change nothing.
        let before = self
            .edited
            .is_some()
            .then(|| self.input.editor().text().to_owned());
        match intent {
            // One line in, as one line. Enter is refused above, but a paste
            // went straight to the editor, which keeps the newlines a
            // multi-line command needs: `foo` and `bar` pasted on two lines
            // became a tab name with a line break in it — saved to the
            // session that way — and a branch name git would refuse.
            Intent::Paste => {
                if let Some(pasted) = self.clipboard.read() {
                    let line = one_line(&pasted);
                    self.input.edit(|editor| editor.paste(&line));
                }
            }
            intent => {
                self.input.apply(intent, &self.clipboard);
            }
        }
        if before.is_some_and(|before| before != self.input.editor().text()) {
            self.say_edited(ctx);
        }
        ctx.notify();
        true
    }

    /// Takes what an input method composed, if this field is the one
    /// listening: a dead key's accent, a word of Japanese, inserted as typed
    /// text is and on one line, as a paste is.
    ///
    /// Claimed rather than left to whatever is under the field, which may be
    /// a full-screen program that would be handed the word instead — a find
    /// bar over Claude Code searching `caf` while `é` went into the prompt
    /// below it. The composition on its way is not drawn: the field shows
    /// what it holds, and the word arrives in it whole.
    fn commit(&self, text: &str, ctx: &mut EventContext) -> bool {
        if !self.input.has_keys() {
            return false;
        }
        let line = one_line(text);
        if !line.is_empty() {
            self.input.edit(|editor| editor.insert(&line));
            self.say_edited(ctx);
            ctx.notify();
        }
        true
    }

    /// Tells whoever asked that the text changed.
    fn say_edited(&self, ctx: &mut EventContext) {
        if let Some(action) = self.edited {
            ctx.dispatch_typed_action(action);
        }
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

        let listening = self.input.has_keys() && self.window_focused;
        let hovered = self.hover.lock().is_hovered();
        let border = if listening {
            theme().accent
        } else if hovered {
            theme().overlay_3
        } else {
            theme().border
        };

        // Hit-recorded: a press on the box is a press on the field, and the
        // rail row under it must not get it. The I-beam is asked for after
        // it, or the box would cover its own request.
        ctx.scene
            .draw_rect_with_hit_recording(RectF::new(origin, size))
            .with_background(theme().overlay_1)
            .with_border(Border::all(1.).with_border_color(border))
            .with_corner_radius(CornerRadius::with_all(Radius::Pixels(6.)));
        ctx.scene
            .request_pointer(RectF::new(origin, size), PointerShape::Text);

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
            Event::KeyDown {
                keystroke, chars, ..
            } => self.type_key(keystroke, chars, ctx),
            Event::Ime(Ime::Commit(text)) => self.commit(text, ctx),
            Event::MouseDown {
                position,
                button: MouseButton::Left,
                click_count,
                ..
            } if bounds.contains_point(*position) => {
                // Before the caret moves, because taking the keyboard is what
                // makes the caret worth moving.
                if let Some(action) = self.focus {
                    ctx.dispatch_typed_action(action);
                }
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

/// `text` with each line break — `\r\n`, `\n` or `\r` — and the run of them
/// it may be part of turned into a single space, and the breaks at either end
/// dropped: what a paste of several lines is in a field that holds one.
fn one_line(text: &str) -> String {
    text.split(['\r', '\n'])
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::one_line;

    #[test]
    fn a_paste_of_several_lines_is_one_line_with_spaces_between() {
        assert_eq!(one_line("foo\nbar"), "foo bar");
        assert_eq!(one_line("foo\r\nbar\r\n"), "foo bar", "Windows line ends");
        assert_eq!(
            one_line("\nfoo\n\n\nbar\n"),
            "foo bar",
            "blank lines and ends"
        );
        assert_eq!(one_line("one line"), "one line", "nothing to do");
    }
}
