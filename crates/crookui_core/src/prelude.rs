//! What a file that writes a view needs in scope.
//!
//! ```
//! use crookui_core::prelude::*;
//! ```

pub use crate::core::{
    AppContext, Entity, GetSingletonModelHandle as _, ModelContext, ModelHandle, SingletonEntity,
    TypedActionView, View, ViewContext, ViewHandle,
};
pub use crate::element::{Element, ParentElement as _, SizeConstraint};
pub use crate::elements::{
    Align, AnchorTo, ChildView, Clipped, ConstrainedBox, Container, Corner, CrossAxisAlignment,
    Dismiss, Empty, Expanded, Flex, Hoverable, MainAxisAlignment, MainAxisSize, MouseStateHandle,
    Shrinkable, Stack, Text,
};
pub use crate::geometry::{Color, RectF, Vector2F, vec2f};
pub use crate::presenter::EventContext;
pub use crate::scene::{Border, CornerRadius, Fill, Radius};
