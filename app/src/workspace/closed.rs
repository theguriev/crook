//! What "Reopen the closed tab" keeps of each close, and how it puts it back.
//!
//! # A close is kept whole
//!
//! One press brings back one close, so what is kept is what one close took,
//! in one piece: a tab, a whole group, or one pane of a split whose tab stayed
//! open. Kept as tabs and nothing else, a group closed from its heading was
//! never written down at all — the press brought back an older tab, or
//! nothing — and `cmd-w` on one pane of a split brought back some other tab
//! and left the pane gone. A close that takes the last member of a group takes
//! the group with it, so that is a group too, of one: bringing back the tab
//! without its heading would be bringing back less than went.
//!
//! A close of several — the others, the tabs below, the tabs above — is
//! still several closes, written bottom first so that the presses bring them
//! back top first, each at its own place with everything above it already
//! back. A group every member of which went with it is one of them, written
//! where its first member was.
//!
//! Everything is kept as the session file's snapshots — see
//! [`crate::session`] — so what comes back comes back the way a restored
//! window does: names, colours, splits and shares, each shell in the
//! directory it was in, and each agent that was running offered its resume
//! line, unsent.
//!
//! # Putting it back
//!
//! Where it was, as near as the strip that is left allows, and through the
//! same clamps a drop goes through — see [`TabStrip::reopen`] — because the
//! strip has moved since, and an index that was between two loose tabs then
//! can be between two members of a group now.
//!
//! A tab or a pane names what it went out of by id, and ids are never reused:
//! a group that comes back is a new group and a tab a new tab. So bringing one
//! back hands its new id on to whatever is still kept naming the old one. A
//! tab closed out of a group before the rest of the group went goes back into
//! the group once that is back, and a pane closed out of a tab before the tab
//! went goes back into the tab.

use crate::session::{PaneSnapshot, TabSnapshot};
use crate::tab::{GroupId, Pane, PaneId, SplitAxis, Tab, TabAction, TabEffect, TabId, TabStrip};

/// How many closes "Reopen the closed tab" can bring back, newest first.
///
/// A browser's own number, near enough. Past it the oldest is forgotten: a
/// tab closed twenty closes ago is one nobody is reaching back for with a
/// chord, and each snapshot holds its panes' names and directories. A group
/// is one of them, however many tabs it held, since it went in one close and
/// comes back in one press.
const CLOSED_KEPT: usize = 10;

/// What one close took.
enum Taken {
    /// A tab, from a group that is still there or from none.
    Tab {
        /// The id it had, which a pane closed out of it before it went names.
        id: TabId,
        snapshot: TabSnapshot,
        /// Its place in the strip.
        index: usize,
        /// The group it was in, which it goes back into if that is still here.
        group: Option<GroupId>,
    },
    /// A group, with every tab it held.
    Group {
        /// The id it had, which a tab closed out of it before it went names.
        id: GroupId,
        /// What its heading said.
        name: String,
        /// Whether its tabs were folded away.
        collapsed: bool,
        /// Its tabs in strip order, each with the id it had.
        tabs: Vec<(TabId, TabSnapshot)>,
        /// The place in the strip of its first tab.
        index: usize,
        /// Which of `tabs` was used most recently, which is the one selected
        /// when they come back.
        selected: usize,
    },
    /// One pane of a split, out of a tab that stayed open.
    Pane {
        /// The id it had, to tell whether the close took it.
        id: PaneId,
        /// The tab it was in.
        tab: TabId,
        snapshot: PaneSnapshot,
        /// Its place in the split.
        index: usize,
        /// Which way the split went, for a tab left with one pane since.
        axis: SplitAxis,
        /// Its tab's place in the strip, where it comes back as a tab of its
        /// own if that tab is not here any more.
        place: usize,
    },
}

/// What a close is about to take, written down before it takes it: after
/// it, there is nothing left to write down.
pub(super) struct Taking(Vec<Taken>);

impl Taking {
    /// What `action` would take from `strip`, bottom first. Nothing, for an
    /// action that closes nothing.
    pub(super) fn by(strip: &TabStrip, action: TabAction) -> Self {
        let tabs = match action {
            TabAction::ClosePane(pane) => {
                let Some(tab) = strip.tab_of(pane) else {
                    return Self(Vec::new());
                };
                // A pane of several is a pane. The last one is its tab, and
                // is written down as the tab.
                if let Some(taken) = pane_taken(strip, tab, pane) {
                    return Self(vec![taken]);
                }
                vec![tab]
            }
            _ => strip.closed_by(action),
        };

        let mut taken = Vec::with_capacity(tabs.len());
        let mut groups = Vec::new();
        for id in &tabs {
            let (Some(tab), Some(index)) = (strip.get(*id), strip.index_of(*id)) else {
                continue;
            };
            match tab.group() {
                Some(group)
                    if strip
                        .members(group)
                        .all(|member| tabs.contains(&member.id())) =>
                {
                    // Members are contiguous and `tabs` is in strip order, so
                    // the first one met is where the group starts.
                    if !groups.contains(&group) {
                        groups.push(group);
                        taken.extend(group_taken(strip, group, index));
                    }
                }
                group => {
                    if let Some(snapshot) = TabSnapshot::of(tab) {
                        taken.push(Taken::Tab {
                            id: *id,
                            snapshot,
                            index,
                            group,
                        });
                    }
                }
            }
        }
        // Bottom first: the presses take the newest first, so the top one
        // comes back first and the ones under it land below it.
        taken.reverse();
        Self(taken)
    }
}

/// A pane closed out of a split, or `None` for a tab's last pane.
fn pane_taken(strip: &TabStrip, tab: TabId, pane: PaneId) -> Option<Taken> {
    let holder = strip.get(tab)?;
    let panes = holder.panes();
    if !panes.is_split() {
        return None;
    }
    Some(Taken::Pane {
        id: pane,
        tab,
        snapshot: PaneSnapshot::of(panes.get(pane)?),
        index: panes.index_of(pane)?,
        axis: panes.axis(),
        place: strip.index_of(tab)?,
    })
}

/// A whole group, from `index`, where its first tab is.
fn group_taken(strip: &TabStrip, group: GroupId, index: usize) -> Option<Taken> {
    let heading = strip.group(group)?;
    let tabs: Vec<(TabId, TabSnapshot)> = strip
        .members(group)
        .filter_map(|tab| Some((tab.id(), TabSnapshot::of(tab)?)))
        .collect();
    let selected = strip
        .mru()
        .iter()
        .find_map(|used| tabs.iter().position(|(id, _)| id == used))
        .unwrap_or(0);
    Some(Taken::Group {
        id: group,
        name: heading.name().to_owned(),
        collapsed: heading.is_collapsed(),
        tabs,
        index,
        selected,
    })
}

/// The closes a window has kept, newest last.
#[derive(Default)]
pub(super) struct Closed(Vec<Taken>);

impl Closed {
    /// Whether there is nothing to bring back.
    pub(super) fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Keeps what a close took, once it has been applied to `strip`.
    ///
    /// Only what it did take: a close of every tab there is closes the window
    /// instead and leaves the strip as it was, and a tab still here is not
    /// one to bring back.
    pub(super) fn keep(&mut self, taking: Taking, strip: &TabStrip) {
        for taken in taking.0 {
            let went = match &taken {
                Taken::Tab { id, .. } => strip.get(*id).is_none(),
                Taken::Group { tabs, .. } => tabs.iter().all(|(id, _)| strip.get(*id).is_none()),
                Taken::Pane { id, .. } => strip.pane(*id).is_none(),
            };
            if !went {
                continue;
            }
            if self.0.len() == CLOSED_KEPT {
                self.0.remove(0);
            }
            self.0.push(taken);
        }
    }

    /// Brings back the newest close into `strip`, and answers what that did
    /// to the strip and the panes that came back. `None` when there is
    /// nothing kept to bring back.
    pub(super) fn reopen(&mut self, strip: &mut TabStrip) -> Option<(TabEffect, Vec<PaneId>)> {
        match self.0.pop()? {
            Taken::Tab {
                id,
                snapshot,
                index,
                group,
            } => {
                let tab = snapshot.restore()?;
                let back = (tab.id(), panes_of(&tab));
                let effect = strip.reopen(tab, index, group);
                self.renamed_tab(id, back.0);
                Some((effect, back.1))
            }
            Taken::Group {
                id,
                name,
                collapsed,
                tabs,
                index,
                selected,
            } => {
                let (was, tabs): (Vec<TabId>, Vec<Tab>) = tabs
                    .iter()
                    .filter_map(|(id, snapshot)| Some((*id, snapshot.restore()?)))
                    .unzip();
                let is: Vec<TabId> = tabs.iter().map(Tab::id).collect();
                let panes = tabs.iter().flat_map(panes_of).collect();
                let group = strip.reopen_group(name, collapsed, tabs, index, selected)?;
                for (was, is) in was.into_iter().zip(is) {
                    self.renamed_tab(was, is);
                }
                self.renamed_group(id, group);
                Some((TabEffect::Changed, panes))
            }
            Taken::Pane {
                tab,
                snapshot,
                index,
                axis,
                place,
                ..
            } => {
                if strip.get(tab).is_some() {
                    let pane = snapshot.restore();
                    let back = vec![pane.id()];
                    Some((strip.reopen_pane(tab, pane, index, axis), back))
                } else {
                    // Its tab went without being kept, so there is no tab to
                    // put it back into: it comes back a tab of its own, where
                    // that one was.
                    let alone = TabSnapshot {
                        panes: vec![snapshot],
                        ..TabSnapshot::default()
                    };
                    let tab = alone.restore()?;
                    let back = panes_of(&tab);
                    Some((strip.reopen(tab, place, None), back))
                }
            }
        }
    }

    /// Hands a tab's new id to the panes kept that went out of it.
    fn renamed_tab(&mut self, was: TabId, is: TabId) {
        for taken in &mut self.0 {
            if let Taken::Pane { tab, .. } = taken
                && *tab == was
            {
                *tab = is;
            }
        }
    }

    /// Hands a group's new id to the tabs kept that went out of it.
    fn renamed_group(&mut self, was: GroupId, is: GroupId) {
        for taken in &mut self.0 {
            if let Taken::Tab {
                group: Some(group), ..
            } = taken
                && *group == was
            {
                *group = is;
            }
        }
    }
}

/// The panes of a tab about to go back into the strip.
fn panes_of(tab: &Tab) -> Vec<PaneId> {
    tab.panes().iter().map(Pane::id).collect()
}
