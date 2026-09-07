//! The registry, and what this machine knows about it.
//!
//! A plugin arrives as a file, and until now finding that file was somebody
//! else's problem: a README, a release page, a link in a chat. The registry is
//! the list — one JSON index, published as a static file, listing every plugin
//! it has built from source with the ABI it speaks, where the artifact is and
//! what it hashes to.
//!
//! # What is here
//!
//! The three halves that have no window in them: [`index`], what a registry
//! publishes and which of it this build can run; [`cache`], the copy on disk
//! that makes the list openable offline and is the only record of what has
//! been withdrawn; and [`fetch`], the one request Crook makes of its own,
//! written under the rule the README states about telemetry.
//!
//! Nothing here draws anything, and nothing here installs anything: what a
//! plugin may do is decided against the module that was downloaded, by the
//! same code that decides it for a module somebody copied in by hand.

pub mod cache;
pub mod fetch;
pub mod index;
