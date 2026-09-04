//! Where a contribution lives, and what takes it back out.
//!
//! Two registries and one guard. Both registries are generic over what a
//! contribution *is*, because this crate must not know what an element or a
//! context is: the application instantiates [`Slots`] with a closure that
//! builds elements and [`Actions`] with a closure that reaches its own state.
//! What is shared between them — and what is worth having in one place — is the
//! rule that registering hands back something whose [`Drop`] undoes it.
//!
//! # Why `Rc<RefCell<…>>`
//!
//! The same reason `MouseStateHandle` is one: this is state a *view* holds
//! between frames, on the one thread that draws, handed by clone to whatever
//! needs it. The core's foreground executor is deliberately `!Send`, so there
//! is no second thread for a lock to protect against, and an `Arc<Mutex>` here
//! would be a lie about where this runs.

use std::cell::RefCell;
use std::rc::{Rc, Weak};

use crate::id::{ActionName, EntryId, PluginId, SlotId};

/// Undoes one registration when it is dropped.
///
/// Everything a plugin contributes comes back as one of these, and a plugin
/// that is unloaded is a plugin whose guards were dropped. There is no
/// `unregister(id)` anywhere, and that is the point: an unregister call is a
/// thing a caller can forget, and a guard is a thing the compiler will not let
/// them keep by accident.
#[must_use = "dropping a Registration immediately undoes what was just registered"]
pub struct Registration(Option<Box<dyn FnOnce()>>);

impl Registration {
    /// A guard that runs `undo` when it is dropped.
    pub fn new(undo: impl FnOnce() + 'static) -> Self {
        Self(Some(Box::new(undo)))
    }

    /// A guard over nothing, for a registration that was refused.
    ///
    /// A refusal is reported as a [`Complaint`] and still hands back a guard,
    /// so that a caller keeping guards in a list never has to branch on
    /// whether one arrived.
    pub fn none() -> Self {
        Self(None)
    }

    /// Keeps the contribution for the life of the process.
    ///
    /// For a registration made by something that is never unloaded. Named
    /// rather than implicit, so that leaking is a decision somebody wrote down.
    pub fn keep_forever(mut self) {
        self.0 = None;
    }
}

impl Drop for Registration {
    fn drop(&mut self) {
        if let Some(undo) = self.0.take() {
            undo();
        }
    }
}

impl std::fmt::Debug for Registration {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Registration")
            .field("live", &self.0.is_some())
            .finish()
    }
}

/// How many contributions a slot renders.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Cardinality {
    /// One: the entry with the lowest `order` wins and the rest are not drawn.
    /// For a place where two of something makes no sense — the one item pinned
    /// to the right of the header, the pane's content.
    Single,
    /// All of them, in `order`, then in the order they were registered. For a
    /// row of chips, a list of menu entries, a column of settings sections.
    List,
}

/// Something a registry was asked to do that it could not, or should not.
///
/// Collected rather than returned, because the answer to "a plugin contributed
/// to a slot nobody declares" is not to fail its `build` — it is to draw
/// everything else and say so, by name, in the log and on the plugin's page.
/// The house rule is written in `settings.rs` and applies here without change:
/// nothing may cost a person their window.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Complaint {
    /// Two owners declared the same slot. The first one keeps it.
    SlotDeclaredTwice {
        /// The slot.
        slot: SlotId,
        /// Who declared it first.
        owner: PluginId,
        /// Who tried second, and was refused.
        second: PluginId,
    },
    /// A contribution named a slot nothing declares. It is kept, and drawn if
    /// that slot is ever declared, but somebody has probably misspelled it.
    SlotNotDeclared {
        /// The slot that was named.
        slot: SlotId,
        /// Who named it.
        by: PluginId,
    },
    /// More than one entry in a [`Cardinality::Single`] slot. The lowest
    /// `order` is drawn; the others are not.
    SlotIsSingle {
        /// The slot.
        slot: SlotId,
        /// How many wanted it.
        entries: usize,
    },
    /// Two plugins registered the same action name. The first one keeps it.
    ActionTaken {
        /// The name.
        action: ActionName,
        /// Who has it.
        owner: PluginId,
        /// Who tried second, and was refused.
        second: PluginId,
    },
}

impl Complaint {
    /// The slot this is about, where it is about one.
    ///
    /// For a surface that has to decide whose problem a complaint is:
    /// [`Self::SlotIsSingle`] names no plugin because it is about a slot with
    /// too many things in it, and the plugins it concerns are the ones that
    /// put them there.
    pub fn slot(&self) -> Option<SlotId> {
        match self {
            Self::SlotDeclaredTwice { slot, .. }
            | Self::SlotNotDeclared { slot, .. }
            | Self::SlotIsSingle { slot, .. } => Some(*slot),
            Self::ActionTaken { .. } => None,
        }
    }

    /// Whether this names `plugin`.
    ///
    /// The plugin that was refused *and* the one that was already there: both
    /// of them are in a collision, and a person looking at either card wants
    /// to know about it.
    pub fn names(&self, plugin: &PluginId) -> bool {
        match self {
            Self::SlotDeclaredTwice { owner, second, .. }
            | Self::ActionTaken { owner, second, .. } => owner == plugin || second == plugin,
            Self::SlotNotDeclared { by, .. } => by == plugin,
            Self::SlotIsSingle { .. } => false,
        }
    }
}

impl std::fmt::Display for Complaint {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::SlotDeclaredTwice {
                slot,
                owner,
                second,
            } => write!(
                formatter,
                "{second} declared the slot {slot}, which {owner} already owns"
            ),
            Self::SlotNotDeclared { slot, by } => write!(
                formatter,
                "{by} contributed to the slot {slot}, which nothing declares"
            ),
            Self::SlotIsSingle { slot, entries } => write!(
                formatter,
                "the slot {slot} holds one entry and {entries} were offered; the rest are not drawn"
            ),
            Self::ActionTaken {
                action,
                owner,
                second,
            } => write!(
                formatter,
                "{second} registered the action {action}, which {owner} already owns"
            ),
        }
    }
}

/// One thing contributed to a slot.
struct Entry<C> {
    slot: SlotId,
    by: PluginId,
    id: EntryId,
    order: i32,
    /// Rising, so that two entries with one `order` keep the order they were
    /// registered in — which for a list of plugins is the order they loaded.
    seq: u64,
    payload: C,
}

struct SlotTable<C> {
    declared: Vec<(SlotId, Cardinality, PluginId)>,
    entries: Vec<Entry<C>>,
    complaints: Vec<Complaint>,
    seq: u64,
}

/// Every place the interface will render something a plugin gave it.
///
/// `C` is what a contribution is. The application uses a closure that builds an
/// element from its own state; a test uses a string.
pub struct Slots<C>(Rc<RefCell<SlotTable<C>>>);

impl<C> Clone for Slots<C> {
    fn clone(&self) -> Self {
        Self(self.0.clone())
    }
}

impl<C: 'static> Default for Slots<C> {
    fn default() -> Self {
        Self::new()
    }
}

impl<C: 'static> Slots<C> {
    /// An empty registry.
    pub fn new() -> Self {
        Self(Rc::new(RefCell::new(SlotTable {
            declared: Vec::new(),
            entries: Vec::new(),
            complaints: Vec::new(),
            seq: 0,
        })))
    }

    /// Declares a slot, which is what makes it renderable.
    ///
    /// Only the declaring plugin renders it. Declaring the same slot twice is a
    /// [`Complaint`] and the first declaration stands — a slot whose owner
    /// could be replaced by whatever loaded last would let one plugin quietly
    /// take another's surface.
    pub fn declare(&self, by: &PluginId, slot: SlotId, cardinality: Cardinality) -> Registration {
        let mut table = self.0.borrow_mut();
        if let Some((_, _, owner)) = table.declared.iter().find(|(id, _, _)| *id == slot) {
            let complaint = Complaint::SlotDeclaredTwice {
                slot,
                owner: owner.clone(),
                second: by.clone(),
            };
            table.complaints.push(complaint);
            return Registration::none();
        }

        table.declared.push((slot, cardinality, by.clone()));
        drop(table);

        let weak = Rc::downgrade(&self.0);
        Registration::new(move || {
            if let Some(table) = weak.upgrade() {
                table.borrow_mut().declared.retain(|(id, _, _)| *id != slot);
            }
        })
    }

    /// Contributes one entry to a slot.
    ///
    /// A contribution to a slot nothing has declared is kept — load order is
    /// not something a contributor should have to know — and complained about
    /// once, when [`Self::audit`] runs.
    pub fn contribute(
        &self,
        by: &PluginId,
        slot: SlotId,
        id: EntryId,
        order: i32,
        payload: C,
    ) -> Registration {
        let mut table = self.0.borrow_mut();
        table.seq += 1;
        let seq = table.seq;
        table.entries.push(Entry {
            slot,
            by: by.clone(),
            id,
            order,
            seq,
            payload,
        });
        drop(table);

        let weak = Rc::downgrade(&self.0);
        Registration::new(move || {
            if let Some(table) = weak.upgrade() {
                table.borrow_mut().entries.retain(|entry| entry.seq != seq);
            }
        })
    }

    /// Builds one thing per entry in `slot`, in the order they will be drawn.
    ///
    /// The visitor runs while the registry is borrowed, so a contribution that
    /// registers another contribution while being rendered would panic rather
    /// than quietly change what is being iterated. That is the right failure:
    /// rendering is not the place to change what there is to render.
    pub fn map<T>(&self, slot: SlotId, mut build: impl FnMut(&C) -> T) -> Vec<T> {
        let table = self.0.borrow();
        let mut entries: Vec<&Entry<C>> = table
            .entries
            .iter()
            .filter(|entry| entry.slot == slot)
            .collect();
        entries.sort_by_key(|entry| (entry.order, entry.seq));
        entries.iter().map(|entry| build(&entry.payload)).collect()
    }

    /// Builds the one entry at `index` in the order the slot draws, if the
    /// slot has that many.
    ///
    /// For a surface that shows one of its entries at a time — a rail of
    /// settings pages shows one page — where building every entry to reach one
    /// of them would be building four pages nobody is looking at. The order is
    /// [`map`](Self::map)'s, so an index taken from
    /// [`contributors`](Self::contributors) names the same entry here.
    pub fn at<T>(&self, slot: SlotId, index: usize, use_it: impl FnOnce(&C) -> T) -> Option<T> {
        let table = self.0.borrow();
        let mut entries: Vec<&Entry<C>> = table
            .entries
            .iter()
            .filter(|entry| entry.slot == slot)
            .collect();
        entries.sort_by_key(|entry| (entry.order, entry.seq));
        entries.get(index).map(|entry| use_it(&entry.payload))
    }

    /// Builds the one thing a [`Cardinality::Single`] slot draws, if anything
    /// was contributed to it.
    pub fn one<T>(&self, slot: SlotId, build: impl FnOnce(&C) -> T) -> Option<T> {
        let table = self.0.borrow();
        table
            .entries
            .iter()
            .filter(|entry| entry.slot == slot)
            .min_by_key(|entry| (entry.order, entry.seq))
            .map(|entry| build(&entry.payload))
    }

    /// Whether anything has been contributed to a slot.
    ///
    /// For a renderer that must not draw its surrounding chrome — a gutter, a
    /// divider, a header — when the slot inside it is empty.
    pub fn is_empty(&self, slot: SlotId) -> bool {
        !self
            .0
            .borrow()
            .entries
            .iter()
            .any(|entry| entry.slot == slot)
    }

    /// Every slot that has been declared, in the order they were.
    ///
    /// For a host that has to resolve a *string* against the slots that exist
    /// — which is what a sandboxed plugin hands it. A native plugin names a
    /// [`SlotId`] and cannot name one that does not exist; a plugin outside
    /// the binary can name anything, and the answer to a name nothing
    /// declares is "no", not a new slot.
    pub fn declared(&self) -> Vec<SlotId> {
        self.0
            .borrow()
            .declared
            .iter()
            .map(|(slot, _, _)| *slot)
            .collect()
    }

    /// Which plugin contributed each entry to a slot, in drawing order.
    ///
    /// For the plugins page, which has to be able to say who put a thing on
    /// screen, and for a host that must attribute a panic to somebody.
    pub fn contributors(&self, slot: SlotId) -> Vec<(PluginId, EntryId)> {
        let table = self.0.borrow();
        let mut entries: Vec<&Entry<C>> = table
            .entries
            .iter()
            .filter(|entry| entry.slot == slot)
            .collect();
        entries.sort_by_key(|entry| (entry.order, entry.seq));
        entries
            .iter()
            .map(|entry| (entry.by.clone(), entry.id.clone()))
            .collect()
    }

    /// Everything wrong with what is registered.
    ///
    /// A question, not a drain: asking twice gives the same answer, and asking
    /// after a plugin is disabled gives a shorter one. The host asks once after
    /// every plugin has built — so that a contribution arriving before its slot
    /// was declared is not an error — and the plugins page asks again whenever
    /// it draws, which it could not do if the first ask emptied the list.
    ///
    /// Two kinds of answer come back together. A *refusal* — a slot declared
    /// twice, a name already taken — is a thing that happened, and is
    /// remembered; everything else is derived from what is registered now, and
    /// stops being said the moment it stops being true.
    pub fn audit(&self) -> Vec<Complaint> {
        let table = self.0.borrow();
        let mut complaints = table.complaints.clone();

        let declared: Vec<(SlotId, Cardinality)> = table
            .declared
            .iter()
            .map(|(slot, cardinality, _)| (*slot, *cardinality))
            .collect();

        for (slot, cardinality) in &declared {
            if *cardinality != Cardinality::Single {
                continue;
            }
            let entries = table
                .entries
                .iter()
                .filter(|entry| entry.slot == *slot)
                .count();
            if entries > 1 {
                complaints.push(Complaint::SlotIsSingle {
                    slot: *slot,
                    entries,
                });
            }
        }

        let mut orphans: Vec<(SlotId, PluginId)> = Vec::new();
        for entry in &table.entries {
            let known = declared.iter().any(|(slot, _)| *slot == entry.slot);
            let said = orphans
                .iter()
                .any(|(slot, by)| *slot == entry.slot && *by == entry.by);
            if !known && !said {
                orphans.push((entry.slot, entry.by.clone()));
            }
        }
        complaints.extend(
            orphans
                .into_iter()
                .map(|(slot, by)| Complaint::SlotNotDeclared { slot, by }),
        );

        complaints
    }
}

struct ActionTable<H> {
    /// The handler is an `Option` because [`Actions::with`] takes it out for
    /// the duration of the call; see there.
    actions: Vec<(ActionName, PluginId, Option<H>)>,
    complaints: Vec<Complaint>,
}

/// Every named thing the application can be asked to do.
///
/// `H` is what a handler is; the application uses a closure over its own state.
/// The point of a *name* is that three things can reach it that an enum variant
/// could not: a chord in the user's keymap file, an entry in a menu or a
/// palette, and another plugin.
pub struct Actions<H>(Rc<RefCell<ActionTable<H>>>);

impl<H> Clone for Actions<H> {
    fn clone(&self) -> Self {
        Self(self.0.clone())
    }
}

impl<H: 'static> Default for Actions<H> {
    fn default() -> Self {
        Self::new()
    }
}

impl<H: 'static> Actions<H> {
    /// An empty registry.
    pub fn new() -> Self {
        Self(Rc::new(RefCell::new(ActionTable {
            actions: Vec::new(),
            complaints: Vec::new(),
        })))
    }

    /// Registers a handler under a name.
    ///
    /// The name carries its owner, so a collision is two registrations of one
    /// plugin's own action rather than two plugins fighting — but it is still
    /// refused, and the first one stands.
    pub fn register(&self, by: &PluginId, action: ActionName, handler: H) -> Registration {
        let mut table = self.0.borrow_mut();
        if let Some((_, owner, _)) = table.actions.iter().find(|(name, _, _)| *name == action) {
            let complaint = Complaint::ActionTaken {
                action,
                owner: owner.clone(),
                second: by.clone(),
            };
            table.complaints.push(complaint);
            return Registration::none();
        }

        table
            .actions
            .push((action.clone(), by.clone(), Some(handler)));
        drop(table);

        let weak: Weak<RefCell<ActionTable<H>>> = Rc::downgrade(&self.0);
        Registration::new(move || {
            if let Some(table) = weak.upgrade() {
                table
                    .borrow_mut()
                    .actions
                    .retain(|(name, _, _)| *name != action);
            }
        })
    }

    /// Runs `use_it` against the handler for `action`, if there is one.
    ///
    /// A visitor rather than a returned handler, because `H` is not `Clone`
    /// and a registry that handed one out could not take it back.
    ///
    /// **The registry is not borrowed while the handler runs.** The handler is
    /// lifted out of the table, the borrow is dropped, and it is put back
    /// afterwards — which is what makes the obvious thing work: an action whose
    /// handler disables the plugin that owns it, or registers another action,
    /// or opens a palette that reads [`names`](Self::names). Holding the borrow
    /// across the call would have made every one of those a panic in somebody
    /// else's plugin.
    ///
    /// Two consequences, both wanted. The action is *unreachable while it is
    /// running*, so an action that invokes itself does nothing the second time
    /// rather than recursing until the stack ends; and if the handler removed
    /// its own registration, the handler is dropped at the end of the call
    /// instead of being put back on top of it.
    pub fn with<T>(&self, action: &ActionName, use_it: impl FnOnce(&H) -> T) -> Option<T> {
        let handler = {
            let mut table = self.0.borrow_mut();
            let entry = table
                .actions
                .iter_mut()
                .find(|(name, _, _)| name == action)?;
            entry.2.take()?
        };

        let outcome = use_it(&handler);

        if let Some(entry) = self
            .0
            .borrow_mut()
            .actions
            .iter_mut()
            .find(|(name, _, _)| name == action)
        {
            entry.2 = Some(handler);
        }

        Some(outcome)
    }

    /// Whether anything answers to this name.
    ///
    /// True while the handler is running, because it is still registered: what
    /// [`with`](Self::with) lifts out is an implementation detail of the call,
    /// not a change to what exists.
    pub fn contains(&self, action: &ActionName) -> bool {
        self.0
            .borrow()
            .actions
            .iter()
            .any(|(name, _, _)| name == action)
    }

    /// Every action there is, and who owns it, sorted by name.
    ///
    /// What a command palette lists and what the settings page's keys page
    /// prints instead of the hard-coded strings it prints today.
    pub fn names(&self) -> Vec<(ActionName, PluginId)> {
        let table = self.0.borrow();
        let mut names: Vec<(ActionName, PluginId)> = table
            .actions
            .iter()
            .map(|(name, owner, _)| (name.clone(), owner.clone()))
            .collect();
        names.sort();
        names
    }

    /// Everything wrong with what is registered. A question, like
    /// [`Slots::audit`], and answerable as many times as it is asked.
    pub fn audit(&self) -> Vec<Complaint> {
        self.0.borrow().complaints.clone()
    }
}
