//! `pane.close`: a pane the caller opened, closed when it is done with it.
//!
//! What a lead does with a worker it has read the answer of: `crook pane
//! close 7` closes pane 7 — its tab with it, when it was the tab's last
//! pane — so that the panel is not left a row of finished workers for a
//! person to clear away by hand.
//!
//! # What may be closed
//!
//! Only a pane the caller opened, with `tab.new` or `pane.split`, or one
//! those opened in turn: the panes [`watch`] lets it watch,
//! less its own. Never the caller's own pane, which ends by its shell
//! exiting, and never one a person opened, nor another agent's workers: a
//! pane is a person's, or the agent's that asked for it.
//!
//! # Nothing working, and nothing asked
//!
//! A close that would end something still working is one Crook asks a person
//! about — see `workspace::closing` — and a request is never that person. So
//! a pane whose agent is running or waiting on someone, whose shell has a
//! command open, or whose agent's hook is waiting on a question this window
//! holds, is refused as `working`, with what it is doing, and the card is
//! never put up for it: a question nobody at the keyboard asked for is one
//! they learn to press through. A pane that is the one a person is working
//! in — the active tab's focused pane — is refused as `focused`, since a
//! close under somebody's hands takes the rest of what they type with it.
//! Everything else closes at once, as a shell at its prompt closes for a
//! person with no question.

use crookui_core::prelude::*;
use serde_json::Value;

use super::protocol::{ClosePane, PaneClosed, Refusal, code};
use super::watch;
use crate::workspace::Workspace;

/// Answers one `pane.close`.
pub fn close(
    workspace: &mut Workspace,
    asked: &ClosePane,
    token: Option<&str>,
    ctx: &mut ViewContext<Workspace>,
) -> Result<Value, Refusal> {
    let number = asked.pane;
    let Some(caller) = token.and_then(|token| workspace.pane_with_token(token, ctx)) else {
        return Err(Refusal::new(
            code::UNAUTHORIZED,
            "only a pane of this window can close a pane it opened, and the request carries no \
             pane's CROOK_TOKEN; run this inside a Crook pane",
        ));
    };
    let strip = workspace.tabs();
    let Some(pane) = strip
        .panes()
        .map(|(_, pane)| pane.id())
        .find(|pane| pane.as_u64() == number)
    else {
        return Err(watch::no_such_pane(number));
    };
    if pane == caller {
        return Err(Refusal::new(
            code::OWN_PANE,
            format!(
                "pane {number} is this pane, and a pane is never closed by a request from inside \
                 it; it closes when its shell exits"
            ),
        ));
    }
    if !workspace.watches().observes(strip, caller, pane) {
        return Err(Refusal::new(
            code::NOT_OPENED,
            format!(
                "pane {number} is not one this pane opened, and only a person closes a pane \
                 they or another agent opened; close the tabs and panes you opened with `crook \
                 tab new` or `crook pane split`"
            ),
        ));
    }
    let session = strip
        .pane(pane)
        .map(|open| open.session())
        .expect("the pane was just found open");
    let asking = workspace.asks().asking(pane) || session.asking.is_some();
    if session.is_working() || asking {
        let doing = if asking {
            "its agent is asking a person a question".to_owned()
        } else if let Some(command) = &session.running_command {
            format!("its shell is running {}", printable(command))
        } else {
            format!("its agent says it is {}", super::word(session.status))
        };
        return Err(Refusal::new(
            code::WORKING,
            format!(
                "pane {number} is still working — {doing} — and closing it would end that; Crook \
                 asks a person before a close ends work, and never on a request's behalf. Wait \
                 until it is done (`crook pane wait {number} --until finished` or `--until \
                 idle`), or ask the person to close it"
            ),
        ));
    }
    if strip.focused_pane_id() == Some(pane) {
        return Err(Refusal::new(
            code::FOCUSED,
            format!(
                "pane {number} is the one a person is working in, and closing it would take \
                 what they type with it; ask them to close it, or wait until they have moved on"
            ),
        ));
    }
    let tab = strip.tab_of(pane);
    if !workspace.close_worker_pane(pane, ctx) {
        return Err(Refusal::new(
            code::FAILED,
            format!("pane {number} could not be closed"),
        ));
    }
    log::info!(
        "pane {} closed pane {number} through the control socket",
        caller.as_u64()
    );
    let tab_closed = tab.is_some_and(|tab| workspace.tabs().get(tab).is_none());
    Ok(serde_json::to_value(PaneClosed {
        pane_id: number,
        tab_closed,
    })
    .expect("a closed pane is numbers, which encode"))
}

/// `text` with its control characters written out, for a refusal that
/// names a command a program in the pane chose.
fn printable(text: &str) -> String {
    text.chars()
        .flat_map(|character| match character.is_control() {
            true => character.escape_default().collect::<Vec<_>>(),
            false => vec![character],
        })
        .collect()
}
