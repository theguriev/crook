//! The sizes the chrome's words are set in, and the corners its boxes are cut
//! to.
//!
//! The sibling of [`theme`](crate::theme), for the numbers a palette does not
//! hold. A view asks for a role — `text::CAPTION`, `radius::MEDIUM` — rather
//! than writing `10.5` or `Radius::Pixels(5.)` beside the thing it is drawing,
//! for the reason the palette asks for `theme().surface` rather than a hex:
//! a number written where it is used is a number that drifts. Before this
//! module every menu, panel and page declared its own, and the same job came
//! out at three sizes of secondary line and five radii of corner — `10`, `10.5`
//! and `11` for the line under a title, `4`, `5`, `6`, `7` and `8` for a
//! corner — none of them chosen against the others, all of them reading as
//! "close enough" one at a time and as noise side by side.
//!
//! What is *not* here is as deliberate as what is. The terminal's own type is
//! a person's setting and lives in [`settings`](crate::settings) and
//! [`terminal_font`](crate::terminal_font). An icon's size is the size of a
//! drawing, not of a word, and stays beside the drawing it sizes — a chevron
//! at 16 next to a 12pt label is a shape matched to that label by eye, and
//! moving it onto a type scale would only invite someone to "fix" it to 12.
//! A pill or a dot is a circle, `Radius::Percentage(50.)`, and has no radius
//! to name. And a corner that is a measurement of something else — half a
//! thumb's width, the focus ring's reach past the box it rings — is computed
//! where that something is.

/// The steps of the chrome's type, in points.
///
/// Three steps that do work and no step between them. A step is a role a
/// person can name — the line under a title, the title — and a size that is
/// only "a little smaller than body" is not one: it is a size nobody chose
/// against the others, and at 10 or 10.5 it was also judged in review to be
/// too small to read at arm's length on an ordinary display.
pub mod text {
    /// The secondary line: the branch under a tab's title, a group's heading,
    /// the chord printed beside a command, a setting's description, a chip's
    /// count, the names under the sidebar's section buttons.
    ///
    /// Eleven, one step under [`BODY`], which is enough to read as "a fact
    /// about the thing above" without being so small the fact is not read.
    pub const CAPTION: f32 = 11.;

    /// The interface's own size: a row's title, a menu entry, a setting's
    /// label, a button's word, the text in a field.
    pub const BODY: f32 = 12.;

    /// The name at the head of a page or a panel: a settings page's title, the
    /// changes panel's file, the theme panel's heading, the title of a pane
    /// that has exited.
    pub const TITLE: f32 = 16.;
}

/// The steps of the chrome's corners, in pixels.
///
/// Corners nest: a box inside a rounded box at an inset reads as belonging
/// to it when its radius is the outer one less the inset, and as a sticker
/// on top of it when it is not. So the steps are chosen as pairs as much as
/// one at a time — a [`SMALL`](radius::SMALL) row inside a
/// [`MEDIUM`](radius::MEDIUM) menu, a `MEDIUM` row inside a
/// [`LARGE`](radius::LARGE) card.
pub mod radius {
    /// A key's cap, a chip, a swatch, and a row that sits inside a rounded
    /// box rather than across a flat one: the picker's rows inside its menu,
    /// a choice's hover inside the settings page's card.
    ///
    /// Also the tab list's rows and the group cards they sit in, which are a
    /// dense column of rows rather than controls standing on their own — and
    /// which the close button inside a row matches, so the button reads as a
    /// part of the row and not as a second card laid on it.
    pub const SMALL: f32 = 4.;

    /// Anything a person presses or reads as one thing standing on its own: a
    /// button, a field, a menu, a popover, a card, the find bar, the plate
    /// behind a block's controls, a row in a list that is not nested in a
    /// rounded box.
    ///
    /// The one most of the chrome takes, which is the point of it: a menu and
    /// the button that opened it and the panel beside them all agreeing is
    /// what makes the window read as one piece of work.
    pub const MEDIUM: f32 = 6.;

    /// A card that holds rounded rows of its own: the palette, the settings
    /// page's current-theme card and the box a question's answer sits in.
    ///
    /// Only a box with [`MEDIUM`] rows inside it needs it. A card that is a
    /// card and nothing nested in it takes `MEDIUM`, and is not made rounder
    /// for being bigger.
    pub const LARGE: f32 = 10.;
}
