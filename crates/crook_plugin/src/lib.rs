//! The vocabulary every plugin is written in.
//!
//! Crook's own features are plugins. That is not a slogan about extensibility —
//! it is the only way to know the plugin API is enough, because an API that
//! the application itself does not use is an API nobody has tried to build
//! anything with. The usage chip, the Themes panel, the worktree menu and the
//! settings pages are meant to end up on the same registries a stranger's
//! plugin uses, and each one that moves is a proof.
//!
//! # What is here, and what is deliberately not
//!
//! Identities ([`PluginId`], [`ActionName`], [`SlotId`]), a [`Manifest`], and
//! the two registries a contribution lives in — [`Slots`] and [`Actions`] —
//! plus the [`Registration`] guard that takes a contribution back out again.
//! That is all. There is no `Plugin` trait here and no host: both of those name
//! Crook's own types, so they live in the application beside the state they
//! reach. This crate is what a plugin compiles against, which is why it has
//! **no dependencies at all** — every one it took would be a dependency it
//! imposed on the whole ecosystem, and a version somebody else's plugin would
//! have to agree with.
//!
//! # Registration returns a guard
//!
//! Everything a plugin contributes is undone by dropping something. That is
//! the one idea taken wholesale from DeepSeek Harness, whose Cordis kernel
//! spends a fibre state machine reimplementing it in a language without
//! destructors; in Rust it is [`Drop`], and it costs nothing. A plugin that is
//! disabled drops its [`Registration`]s and the surfaces it was contributing to
//! are the surfaces it was contributing to before it loaded.
//!
//! # Slots, and the rule that keeps them honest
//!
//! A *slot* is a place in the interface that will render whatever has been
//! contributed to it. Its owner **declares** it — with a [`Cardinality`], once —
//! and anybody may **contribute** an entry with an id and an order. The owner
//! is the only thing that renders it. That split is what stops "plugins can
//! change everything" from meaning "plugins can draw anywhere": the way to make
//! more of Crook changeable is to declare another slot, never to widen what a
//! contribution may be.
//!
//! The registries are generic over what a contribution *is* ([`Slots<C>`],
//! [`Actions<H>`]) because this crate must not know about elements, views or
//! contexts. The application instantiates them with its own closure types.

#![deny(missing_docs)]

mod id;
mod manifest;
mod registry;

#[cfg(test)]
mod tests;

pub use id::{ActionName, EntryId, IdError, PluginId, SlotId};
pub use manifest::{Manifest, Tier};
pub use registry::{Actions, Cardinality, Complaint, Registration, Slots};
