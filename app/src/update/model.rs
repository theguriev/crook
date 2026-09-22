//! What a check for a newer Crook found, and the install it can start.
//!
//! A model for the reason the store has one: asking a server and unpacking a
//! twenty-megabyte archive are both blocking work, and neither may happen
//! inside `render`. What arrives has to be able to repaint the window, which
//! is the other half of what a model is for.
//!
//! # Nothing here starts by itself
//!
//! [`UpdateModel::check`] is reached by the button on the About page and by
//! nothing else — no timer, no launch, no poll. The same sentence
//! `store::model` opens with, and the same reason: the request is Crook's own
//! rather than a plugin's, so the only thing that makes it acceptable is that
//! a person asked for it.
//!
//! # What it keeps
//!
//! The answer to the last check, and what the last install came to. Not on
//! disk: a version this machine heard about yesterday is not a fact about this
//! machine, and a settings file that remembered it would be a file rewritten
//! by a button that changed no setting. So the page says what was found in
//! *this* window, and a window that has not asked is a window that says so.

use std::time::SystemTime;

use crookui_core::prelude::*;

use crate::Channel;

use super::Published;

/// Whether a newer Crook is out, as far as this window has been told.
pub struct UpdateModel {
    /// Whether a check is in flight.
    checking: bool,
    /// Whether an install is in flight.
    installing: bool,
    /// What the last check found, and when.
    found: Option<(Published, SystemTime)>,
    /// What the last check or install went wrong with.
    problem: Option<String>,
    /// What the last install came to, for the line that says to restart.
    installed: Option<String>,
}

impl Entity for UpdateModel {
    type Event = ();
}

impl UpdateModel {
    /// A window that has asked nothing.
    pub fn new() -> Self {
        Self {
            checking: false,
            installing: false,
            found: None,
            problem: None,
            installed: None,
        }
    }

    /// Whether a check is in flight.
    pub fn checking(&self) -> bool {
        self.checking
    }

    /// Whether an install is in flight.
    pub fn installing(&self) -> bool {
        self.installing
    }

    /// What the last check found, and when it was made.
    pub fn found(&self) -> Option<(&Published, SystemTime)> {
        self.found.as_ref().map(|(found, at)| (found, *at))
    }

    /// The release this build is behind, if the last check found one.
    pub fn newer(&self) -> Option<&Published> {
        self.found
            .as_ref()
            .map(|(found, _)| found)
            .filter(|found| found.is_newer())
    }

    /// What went wrong, if the last thing that happened went wrong.
    pub fn problem(&self) -> Option<&str> {
        self.problem.as_deref()
    }

    /// The version the last install put in place.
    pub fn installed(&self) -> Option<&str> {
        self.installed.as_deref()
    }

    /// Asks the releases page what the newest release is.
    pub fn check(&mut self, ctx: &mut ModelContext<Self>) {
        if self.checking || self.installing {
            return;
        }
        self.checking = true;
        self.problem = None;
        self.installed = None;
        ctx.notify();

        let background = ctx.background().clone();
        ctx.spawn(
            async move {
                background
                    .spawn(async move {
                        let agent = crate::plugins::store::fetch::agent();
                        super::published(&agent)
                    })
                    .await
            },
            |model, outcome: Result<Published, String>, ctx| {
                model.checking = false;
                match outcome {
                    Ok(found) => model.found = Some((found, SystemTime::now())),
                    Err(why) => model.problem = Some(why),
                }
                ctx.notify();
            },
        )
        .detach();
    }

    /// Installs what the last check found, over this binary.
    ///
    /// Nothing happens without a check first: the tag to fetch is the one the
    /// releases page named, and a version typed in from somewhere else is not
    /// something a button offers.
    ///
    /// The channel is handed in rather than kept, because the one thing it
    /// decides — whether this build may replace itself at all — is also drawn
    /// on the page beside the button, and the page has the workspace that
    /// knows it.
    pub fn install(&mut self, channel: Channel, ctx: &mut ModelContext<Self>) {
        if self.checking || self.installing {
            return;
        }
        let Some(found) = self.newer() else {
            return;
        };
        let tag = found.tag.clone();
        let version = found.version.clone();
        let binary = match super::replaceable(channel) {
            Ok(binary) => binary,
            Err(refusal) => {
                self.problem = Some(refusal.to_string());
                ctx.notify();
                return;
            }
        };

        self.installing = true;
        self.problem = None;
        ctx.notify();

        let background = ctx.background().clone();
        ctx.spawn(
            async move {
                background
                    .spawn(async move {
                        let agent = crate::plugins::store::fetch::agent();
                        super::install(&tag, &binary, &agent).map(|()| version)
                    })
                    .await
            },
            |model, outcome: Result<String, String>, ctx| {
                model.installing = false;
                match outcome {
                    Ok(version) => model.installed = Some(version),
                    Err(why) => model.problem = Some(why),
                }
                ctx.notify();
            },
        )
        .detach();
    }
}

impl Default for UpdateModel {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_window_that_has_asked_nothing_says_nothing() {
        let model = UpdateModel::new();

        assert!(!model.checking());
        assert!(!model.installing());
        assert!(model.found().is_none());
        assert!(model.newer().is_none(), "a version it never asked for");
        assert!(model.problem().is_none());
        assert!(model.installed().is_none());
    }

    #[test]
    fn a_dev_build_says_why_it_cannot_replace_itself_before_anything_is_pressed() {
        // The page draws this beside the version rather than drawing a button
        // that would refuse when pressed, and the answer is the channel's
        // rather than the model's — which is why the model does not keep one.
        assert_eq!(
            crate::update::replaceable(Channel::Dev).err(),
            Some(crate::update::Refusal::Dev)
        );
    }
}
