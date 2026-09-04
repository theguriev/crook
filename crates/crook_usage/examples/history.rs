//! Prints the week the transcripts on this machine describe, and exits.
//!
//! [`usage`](../usage.rs)'s companion: that one asks Claude what is left of the
//! limits, this one reads what Claude Code wrote down locally. Together they
//! are everything the usage popover draws, minus pixels — which is what makes
//! this the place to check a scan against a real home directory, where the
//! files are hundreds of megabytes and two in five turns are duplicates.

use std::time::Instant;

use chrono::Utc;
use crook_usage::{HISTORY_DAYS, ModelUsage, read_history};

fn main() -> std::process::ExitCode {
    let started = Instant::now();
    let history = match read_history(Utc::now()) {
        Ok(history) => history,
        Err(err) => {
            eprintln!("Could not read the transcripts: {err:#}");
            return std::process::ExitCode::FAILURE;
        }
    };
    let elapsed = started.elapsed();

    println!(
        "last {HISTORY_DAYS} days — {} turns in {} sessions, read in {elapsed:.1?}",
        history.requests, history.sessions
    );

    for model in &history.by_model {
        println!(
            "  {:<12} {:>10} out {:>12} cache read {:>7} turns",
            model.display_name(),
            model.output,
            model.cache_read,
            model.requests
        );
    }

    println!("  by day:");
    for day in &history.by_day {
        println!("    {} {:>14}", day.date, day.tokens);
    }

    println!("  projects:");
    for project in &history.projects {
        let branch = project.branch.as_deref().unwrap_or("—");
        println!(
            "    {:<24} {branch:<20} {:>14}",
            project.name, project.tokens
        );
    }

    let total: u64 = history.by_model.iter().map(ModelUsage::tokens).sum();
    println!("  {total} tokens in all");

    std::process::ExitCode::SUCCESS
}
