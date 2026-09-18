//! Reference counts, kept outside the entities they count.
//!
//! A handle is not a pointer to its entity; it is an id plus a share in this
//! table. Cloning a handle increments here, dropping one decrements here, and
//! an entity whose count reaches zero is recorded as *dropped* — but not
//! removed. Removal happens later, in `AppContext::remove_dropped_items`, when
//! no callback is mid-flight.
//!
//! That gap is why [`RefCounts::is_model_dropped`] exists: between the last
//! handle dropping and the entity actually being removed, the entity is still
//! in the app's maps, and a weak handle that upgraded against those maps alone
//! would resurrect something the app has already decided is dead.

use std::mem;

use crate::core::{EntityId, EntityIdMap, EntityIdSet};

/// The share count of every live entity, plus the ones that just hit zero.
#[derive(Default)]
pub(crate) struct RefCounts {
    entity_counts: EntityIdMap<usize>,
    dropped: DroppedItems,
}

/// Entities whose last handle went away and that have not been removed yet.
#[derive(Default)]
pub(crate) struct DroppedItems {
    pub(crate) models: EntityIdSet,
    pub(crate) views: EntityIdSet,
}

impl RefCounts {
    /// Records one more handle to `entity_id`.
    ///
    /// A handle taken to an entity that had dropped to zero takes it back out
    /// of the dropped set: between the last handle dropping and the app
    /// removing the entity, an observer can mint a fresh strong handle to it
    /// (it is still in the app's maps), and that handle means something holds
    /// it again. Without this the entity would stay marked for removal and be
    /// freed while a live handle still points at it — see
    /// [`is_model_dropped`](Self::is_model_dropped) for the window this lives
    /// in.
    pub(crate) fn inc_entity(&mut self, entity_id: EntityId) {
        *self.entity_counts.entry(entity_id).or_insert(0) += 1;
        // An id is only ever a model or a view, so one of these is a no-op.
        self.dropped.models.remove(&entity_id);
        self.dropped.views.remove(&entity_id);
    }

    /// Records one fewer handle to a model.
    pub(crate) fn dec_model(&mut self, model_id: EntityId) {
        if self.dec(model_id) {
            self.dropped.models.insert(model_id);
        }
    }

    /// Records one fewer handle to a view.
    pub(crate) fn dec_view(&mut self, view_id: EntityId) {
        if self.dec(view_id) {
            self.dropped.views.insert(view_id);
        }
    }

    /// Whether this model has been dropped but not yet removed.
    pub(crate) fn is_model_dropped(&self, model_id: EntityId) -> bool {
        self.dropped.models.contains(&model_id)
    }

    /// Whether this view has been dropped but not yet removed.
    pub(crate) fn is_view_dropped(&self, view_id: EntityId) -> bool {
        self.dropped.views.contains(&view_id)
    }

    /// Takes the pending drops, leaving the set empty.
    pub(crate) fn take_dropped(&mut self) -> DroppedItems {
        mem::take(&mut self.dropped)
    }

    fn dec(&mut self, entity_id: EntityId) -> bool {
        let count = self
            .entity_counts
            .get_mut(&entity_id)
            .expect("a handle was dropped for an entity with no outstanding handles");
        *count -= 1;
        if *count == 0 {
            self.entity_counts.remove(&entity_id);
            true
        } else {
            false
        }
    }
}

impl DroppedItems {
    /// Whether nothing is waiting to be removed.
    pub(crate) fn is_empty(&self) -> bool {
        self.models.is_empty() && self.views.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_re_acquired_handle_takes_an_entity_back_out_of_the_dropped_set() {
        // Between an entity's last handle dropping and its removal, an observer
        // can mint a fresh strong handle to it. That handle means something
        // holds it again, so it must no longer be waiting to be removed — or
        // the app would free an entity a live handle still points at.
        let id = EntityId::new();
        let mut counts = RefCounts::default();

        counts.inc_entity(id);
        counts.dec_model(id);
        assert!(counts.is_model_dropped(id), "its last handle went away");

        counts.inc_entity(id);
        assert!(
            !counts.is_model_dropped(id),
            "a new handle takes it back out of the dropped set"
        );
        assert!(
            counts.take_dropped().models.is_empty(),
            "so nothing removes an entity a handle still points at"
        );
    }
}
