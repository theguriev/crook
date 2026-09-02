//! The development channel binary.
//!
//! Everything is in the library; this exists to name a channel and to carry
//! the Windows subsystem attribute, which has to be a crate-level attribute in
//! a binary and cannot live in a library.

#![cfg_attr(feature = "release_bundle", windows_subsystem = "windows")]

use std::process::ExitCode;

fn main() -> ExitCode {
    match crook::run(crook::Channel::Dev) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("crook: {error:#}");
            ExitCode::FAILURE
        }
    }
}
