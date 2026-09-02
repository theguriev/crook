//! The release channel binary.
//!
//! Identical to the development one but for the channel it names. See
//! `src/bin/dev.rs` for why the shim exists at all.

#![cfg_attr(feature = "release_bundle", windows_subsystem = "windows")]

use std::process::ExitCode;

fn main() -> ExitCode {
    match crook::run(crook::Channel::Stable) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("crook: {error:#}");
            ExitCode::FAILURE
        }
    }
}
