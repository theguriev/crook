//! The element library: eleven primitives that a tab bar, a chip and a menu
//! are built from.
//!
//! Each is either pure layout ([`Flex`], [`Align`], [`ConstrainedBox`],
//! [`Empty`], [`Clipped`], [`Stack`]), pure content ([`Text`]), pure
//! interaction ([`Hoverable`], [`Dismiss`]), pure delegation ([`ChildView`]) —
//! or the one that draws a box, [`Container`].
//! Styling is composed by nesting rather than cascaded through a style tree,
//! which is why there are so few of them.

mod align;
mod child_view;
mod clipped;
mod constrained_box;
mod container;
mod dismiss;
mod empty;
pub mod flex;
mod hoverable;
pub mod stack;
mod text;

#[cfg(test)]
mod tests;

pub use align::Align;
pub use child_view::ChildView;
pub use clipped::Clipped;
pub use constrained_box::ConstrainedBox;
pub use container::{Container, Margin, Padding};
pub use dismiss::Dismiss;
pub use empty::Empty;
pub use flex::{
    CrossAxisAlignment, Expanded, Flex, FlexFit, FlexParentData, MainAxisAlignment, MainAxisSize,
    Shrinkable,
};
pub use hoverable::{Hoverable, MouseState, MouseStateHandle};
pub use stack::{AnchorTo, Corner, Stack};
pub use text::Text;
