//! The sandbox a store plugin runs in.
//!
//! One module, one [`Sandbox`], and everything that crosses between them is
//! bytes described by [`crook_plugin_api`]. The host knows nothing about the
//! plugin's language and the plugin knows nothing about the host's types,
//! which is the whole point: a stranger's code runs in everybody's terminal
//! and the only thing it can reach is what is written down here.
//!
//! # What a sandboxed plugin cannot do
//!
//! Everything, by default. It has no imports but the handful in [`imports`],
//! no filesystem, no clock, no network, no random numbers and no way to reach
//! the host's memory — wasm's linear memory is its own, and the only thing
//! crossing the boundary is a length and an offset into it, checked on every
//! read. What it *may* do is what its manifest asked for and a person granted;
//! nothing here grants anything.
//!
//! # Nothing it does may cost a person their window
//!
//! The rule the rest of Crook follows, and the one that decides the shape of
//! this file. A plugin can loop forever, allocate until the machine gives up,
//! return nonsense, or trap — and each of those has to end as a plugin that is
//! switched off with a line saying which, on a frame that was drawn on time.
//!
//! * **Forever** is [`Fuel`]: wasmi counts instructions and the call ends when
//!   the count runs out.
//! * **Until the machine gives up** is a memory limit, set when the store is
//!   made rather than checked afterwards.
//! * **Nonsense** is a decode that fails, which is one contribution that draws
//!   nothing.
//! * **A trap** is an `Err` from the call, which is the same.
//!
//! A plugin that runs out of fuel or traps is *disabled by the caller*, not
//! retried: a plugin that loops on frame one will loop on frame two, and a
//! terminal that spends a whole frame's budget discovering that every frame is
//! a terminal that has stopped working.
//!
//! # One thing the guarantee needed from outside this file
//!
//! `wasmi` ships two dispatch backends, and its default picks the one that
//! dispatches by tail call — faster, and reliant on the optimiser to turn the
//! recursion into a jump. Unoptimised, it grows the **host** stack per
//! instruction, and a guest loop of about ten thousand iterations overflows
//! it: an abort, not a trap, which no amount of fuel can catch. The workspace
//! turns on `portable-dispatch`, which dispatches in a loop and cannot grow
//! the stack whatever it is compiled at.
//!
//! Measured rather than assumed, and asserted in
//! `a_plugin_may_do_real_work_without_taking_the_host_stack_with_it`.

mod host;
mod sandbox;

pub use host::Registry;
pub use sandbox::{Fuel, Problem, Sandbox};

/// The names a guest may import, and what each is for.
///
/// A guest that imports anything else does not instantiate, which is a plugin
/// that is refused with a line naming what it asked for — rather than one that
/// runs until it calls the thing that is not there.
pub mod imports {
    /// The module every host function lives in.
    pub const MODULE: &str = "crook";

    /// `contribute(slot_ptr, slot_len, entry_ptr, entry_len, order)`.
    pub const CONTRIBUTE: &str = "contribute";

    /// `register_action(name_ptr, name_len, title_ptr, title_len)`, where a
    /// zero-length title means an action that is reachable but not offered.
    pub const REGISTER_ACTION: &str = "register_action";

    /// `log(level, ptr, len)`, where the level is
    /// 1 error, 2 warn, 3 info, 4 debug — anything else is `info`.
    pub const LOG: &str = "log";
}

/// The names a guest must export, and what each is for.
pub mod exports {
    /// The guest's linear memory, which is the only place a string can be.
    pub const MEMORY: &str = "memory";

    /// `crook_abi_version() -> i32`. Called first, and a mismatch is a refusal
    /// before anything else runs.
    pub const ABI_VERSION: &str = "crook_abi_version";

    /// `crook_alloc(len: i32) -> i32`, for the host to put a string somewhere
    /// the guest owns.
    pub const ALLOC: &str = "crook_alloc";

    /// `crook_manifest() -> i64`, packed as `(ptr << 32) | len`.
    pub const MANIFEST: &str = "crook_manifest";

    /// `crook_build() -> i32`, zero for "loaded". Everything it registers, it
    /// registers by calling the imports above.
    pub const BUILD: &str = "crook_build";

    /// `crook_render(slot_ptr, slot_len) -> i64`, packed like the manifest.
    pub const RENDER: &str = "crook_render";

    /// `crook_run(name_ptr, name_len) -> i32`, zero for "done".
    pub const RUN: &str = "crook_run";
}

/// Splits the `(ptr << 32) | len` a guest returns a slice as.
///
/// One `i64` rather than an out-parameter, because a guest writing to a
/// pointer the host passed in is a guest the host has to trust about where it
/// wrote. This way the host does the checking, on its own side, every time.
pub(crate) fn unpack(packed: i64) -> (u32, u32) {
    let packed = packed as u64;
    ((packed >> 32) as u32, packed as u32)
}

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
