//! Handles: identity plus a reference count, never a pointer.
//!
//! A `ModelHandle<T>` or `ViewHandle<T>` carries an [`EntityId`], a
//! `PhantomData<T>` and a weak share in the app's refcount table. It grants no
//! access on its own: to reach the entity you must also present a context, and
//! the context hands out the borrow. That is the reason a view graph never
//! fights the borrow checker — there is no long-lived `&mut` into an entity
//! anywhere, only ids waiting to be resolved.
//!
//! Handles are deliberately neither `Send` nor `Sync`. They are useless off the
//! main thread (every access needs a context that lives there), and refusing to
//! send them is what keeps that fact visible at compile time. Background work
//! that needs an entity awaits its result on the foreground instead.

use std::any::{TypeId, type_name};
use std::fmt::{self, Debug};
use std::hash::{Hash, Hasher};
use std::marker::PhantomData;
use std::sync::{Arc, Weak};

use parking_lot::Mutex;

use crate::core::refcount::RefCounts;
use crate::core::{AppContext, Entity, EntityId, ModelContext, ViewContext, WindowId};

/// A strong reference to a model.
///
/// While one exists the model stays alive; when the last one drops, the model
/// is removed at the next effect flush.
pub struct ModelHandle<T> {
    model_id: EntityId,
    model_type: PhantomData<T>,
    ref_counts: Weak<Mutex<RefCounts>>,
}

impl<T: Entity> ModelHandle<T> {
    pub(in crate::core) fn new(model_id: EntityId, ref_counts: &Arc<Mutex<RefCounts>>) -> Self {
        ref_counts.lock().inc_entity(model_id);
        Self {
            model_id,
            model_type: PhantomData,
            ref_counts: Arc::downgrade(ref_counts),
        }
    }

    /// The model's id.
    pub fn id(&self) -> EntityId {
        self.model_id
    }

    /// A weak handle to the same model.
    pub fn downgrade(&self) -> WeakModelHandle<T> {
        WeakModelHandle::new(self.model_id)
    }

    /// Borrows the model.
    ///
    /// Panics if the model is currently checked out by an update — including
    /// the case where a model reads itself from inside its own update.
    pub fn as_ref<'a, A: ModelAsRef>(&self, app: &'a A) -> &'a T {
        app.model(self)
    }

    /// Reads the model with access to the whole app.
    pub fn read<A, F, S>(&self, app: &A, read: F) -> S
    where
        A: ReadModel,
        F: FnOnce(&T, &AppContext) -> S,
    {
        app.read_model(self, read)
    }

    /// Mutates the model with a context of its own.
    ///
    /// The model is checked out of the app for the duration, so it may reach
    /// anything except itself; re-entering panics rather than aliasing.
    pub fn update<A, F, S>(&self, app: &mut A, update: F) -> S
    where
        A: UpdateModel,
        F: FnOnce(&mut T, &mut ModelContext<T>) -> S,
    {
        app.update_model(self, update)
    }
}

impl<T> Clone for ModelHandle<T> {
    fn clone(&self) -> Self {
        if let Some(ref_counts) = self.ref_counts.upgrade() {
            ref_counts.lock().inc_entity(self.model_id);
        }
        Self {
            model_id: self.model_id,
            model_type: PhantomData,
            ref_counts: self.ref_counts.clone(),
        }
    }
}

impl<T> Drop for ModelHandle<T> {
    fn drop(&mut self) {
        if let Some(ref_counts) = self.ref_counts.upgrade() {
            ref_counts.lock().dec_model(self.model_id);
        }
    }
}

impl<T> PartialEq for ModelHandle<T> {
    fn eq(&self, other: &Self) -> bool {
        self.model_id == other.model_id
    }
}

impl<T> Eq for ModelHandle<T> {}

impl<T> Hash for ModelHandle<T> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.model_id.hash(state);
    }
}

impl<T> std::borrow::Borrow<EntityId> for ModelHandle<T> {
    fn borrow(&self) -> &EntityId {
        &self.model_id
    }
}

impl<T> Debug for ModelHandle<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple(&format!("ModelHandle<{}>", type_name::<T>()))
            .field(&self.model_id)
            .finish()
    }
}

/// A weak reference to a model.
///
/// This is what a model or view holds when it wants to reach *itself* later —
/// a strong self-handle is a cycle the app can never break.
pub struct WeakModelHandle<T> {
    model_id: EntityId,
    model_type: PhantomData<T>,
}

impl<T: Entity> WeakModelHandle<T> {
    pub(in crate::core) fn new(model_id: EntityId) -> Self {
        Self {
            model_id,
            model_type: PhantomData,
        }
    }

    /// The model's id, whether or not it still exists.
    pub fn id(&self) -> EntityId {
        self.model_id
    }

    /// A strong handle, or `None` if the model is gone.
    ///
    /// "Gone" includes the window between the last strong handle dropping and
    /// the model actually being removed: the refcount table is consulted, not
    /// just the model map.
    pub fn upgrade(&self, app: &AppContext) -> Option<ModelHandle<T>> {
        if app.models.contains_key(&self.model_id)
            && !app.ref_counts.lock().is_model_dropped(self.model_id)
        {
            Some(ModelHandle::new(self.model_id, &app.ref_counts))
        } else {
            None
        }
    }
}

impl<T> Clone for WeakModelHandle<T> {
    fn clone(&self) -> Self {
        Self {
            model_id: self.model_id,
            model_type: PhantomData,
        }
    }
}

impl<T> Debug for WeakModelHandle<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple(&format!("WeakModelHandle<{}>", type_name::<T>()))
            .field(&self.model_id)
            .finish()
    }
}

/// A type-erased strong reference to a model.
///
/// Used where a strong reference must be stored without infecting the
/// container with a type parameter — the singleton table, for instance.
pub struct AnyModelHandle {
    model_id: EntityId,
    model_type: TypeId,
    ref_counts: Weak<Mutex<RefCounts>>,
}

impl AnyModelHandle {
    /// The model's id.
    pub fn id(&self) -> EntityId {
        self.model_id
    }

    /// Whether the erased model is a `T`.
    pub fn is<T: 'static>(&self) -> bool {
        TypeId::of::<T>() == self.model_type
    }

    /// Recovers a typed handle, or `None` on a type mismatch.
    pub fn downcast<T: Entity>(self) -> Option<ModelHandle<T>> {
        if self.is::<T>()
            && let Some(ref_counts) = self.ref_counts.upgrade()
        {
            return Some(ModelHandle::new(self.model_id, &ref_counts));
        }
        None
    }

    /// Borrows the erased model as a `T`, or `None` on a type mismatch.
    pub fn downcast_ref<'a, T: Entity>(&'a self, ctx: &'a AppContext) -> Option<&'a T> {
        if self.is::<T>() {
            return ctx.models.get(&self.model_id)?.as_any().downcast_ref();
        }
        None
    }
}

impl Clone for AnyModelHandle {
    fn clone(&self) -> Self {
        if let Some(ref_counts) = self.ref_counts.upgrade() {
            ref_counts.lock().inc_entity(self.model_id);
        }
        Self {
            model_id: self.model_id,
            model_type: self.model_type,
            ref_counts: self.ref_counts.clone(),
        }
    }
}

impl Drop for AnyModelHandle {
    fn drop(&mut self) {
        if let Some(ref_counts) = self.ref_counts.upgrade() {
            ref_counts.lock().dec_model(self.model_id);
        }
    }
}

impl<T: Entity> From<ModelHandle<T>> for AnyModelHandle {
    fn from(handle: ModelHandle<T>) -> Self {
        if let Some(ref_counts) = handle.ref_counts.upgrade() {
            ref_counts.lock().inc_entity(handle.model_id);
        }
        Self {
            model_id: handle.model_id,
            model_type: TypeId::of::<T>(),
            ref_counts: handle.ref_counts.clone(),
        }
    }
}

/// A strong reference to a view.
pub struct ViewHandle<T> {
    window_id: WindowId,
    view_id: EntityId,
    view_type: PhantomData<T>,
    ref_counts: Weak<Mutex<RefCounts>>,
}

impl<T: Entity> ViewHandle<T> {
    pub(in crate::core) fn new(
        window_id: WindowId,
        view_id: EntityId,
        ref_counts: &Arc<Mutex<RefCounts>>,
    ) -> Self {
        ref_counts.lock().inc_entity(view_id);
        Self {
            window_id,
            view_id,
            view_type: PhantomData,
            ref_counts: Arc::downgrade(ref_counts),
        }
    }

    /// A weak handle to the same view.
    pub fn downgrade(&self) -> WeakViewHandle<T> {
        WeakViewHandle::new(self.window_id, self.view_id)
    }

    /// Borrows the view.
    ///
    /// Panics if the view is currently checked out by an update. A child that
    /// reads its parent while the parent is rendering will hit this; use
    /// [`Self::try_as_ref`] where that is possible.
    pub fn as_ref<'a, A: ViewAsRef>(&self, app: &'a A) -> &'a T {
        app.view(self)
    }

    /// Borrows the view, or `None` if it is checked out or gone.
    pub fn try_as_ref<'a, A: ViewAsRef>(&self, app: &'a A) -> Option<&'a T> {
        app.try_view(self)
    }

    /// Reads the view with access to the whole app.
    pub fn read<A, F, S>(&self, app: &A, read: F) -> S
    where
        A: ReadView,
        F: FnOnce(&T, &AppContext) -> S,
    {
        app.read_view(self, read)
    }

    /// Mutates the view with a context of its own.
    pub fn update<A, F, S>(&self, app: &mut A, update: F) -> S
    where
        A: UpdateView,
        F: FnOnce(&mut T, &mut ViewContext<T>) -> S,
    {
        app.update_view(self, update)
    }

    /// Whether this view currently holds focus.
    pub fn is_focused(&self, app: &AppContext) -> bool {
        app.focused_view_id(self.window_id) == Some(self.view_id)
    }
}

// These accessors do not depend on `T`, so they live in an unbounded block:
// the read path only knows its handles as `T: 'static`, not as entities.
impl<T> ViewHandle<T> {
    /// The view's id.
    pub fn id(&self) -> EntityId {
        self.view_id
    }

    /// The window this view belongs to.
    pub fn window_id(&self) -> WindowId {
        self.window_id
    }
}

impl<T> Clone for ViewHandle<T> {
    fn clone(&self) -> Self {
        if let Some(ref_counts) = self.ref_counts.upgrade() {
            ref_counts.lock().inc_entity(self.view_id);
        }
        Self {
            window_id: self.window_id,
            view_id: self.view_id,
            view_type: PhantomData,
            ref_counts: self.ref_counts.clone(),
        }
    }
}

impl<T> Drop for ViewHandle<T> {
    fn drop(&mut self) {
        if let Some(ref_counts) = self.ref_counts.upgrade() {
            ref_counts.lock().dec_view(self.view_id);
        }
    }
}

impl<T> PartialEq for ViewHandle<T> {
    fn eq(&self, other: &Self) -> bool {
        self.window_id == other.window_id && self.view_id == other.view_id
    }
}

impl<T> Eq for ViewHandle<T> {}

impl<T> Hash for ViewHandle<T> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.view_id.hash(state);
    }
}

impl<T> Debug for ViewHandle<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct(&format!("ViewHandle<{}>", type_name::<T>()))
            .field("window_id", &self.window_id)
            .field("view_id", &self.view_id)
            .finish()
    }
}

/// A weak reference to a view.
pub struct WeakViewHandle<T> {
    window_id: WindowId,
    view_id: EntityId,
    view_type: PhantomData<T>,
}

impl<T: Entity> WeakViewHandle<T> {
    pub(in crate::core) fn new(window_id: WindowId, view_id: EntityId) -> Self {
        Self {
            window_id,
            view_id,
            view_type: PhantomData,
        }
    }

    /// The view's id, whether or not it still exists.
    pub fn id(&self) -> EntityId {
        self.view_id
    }

    /// The window the view belonged to.
    pub fn window_id(&self) -> WindowId {
        self.window_id
    }

    /// A strong handle, or `None` if the view is gone.
    pub fn upgrade(&self, app: &AppContext) -> Option<ViewHandle<T>> {
        if app.stored_view(self.window_id, self.view_id).is_some()
            && !app.ref_counts.lock().is_view_dropped(self.view_id)
        {
            Some(ViewHandle::new(
                self.window_id,
                self.view_id,
                &app.ref_counts,
            ))
        } else {
            None
        }
    }
}

impl<T> Clone for WeakViewHandle<T> {
    fn clone(&self) -> Self {
        Self {
            window_id: self.window_id,
            view_id: self.view_id,
            view_type: PhantomData,
        }
    }
}

impl<T> Debug for WeakViewHandle<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct(&format!("WeakViewHandle<{}>", type_name::<T>()))
            .field("window_id", &self.window_id)
            .field("view_id", &self.view_id)
            .finish()
    }
}

/// A type-erased strong reference to a view.
///
/// A window holds one of these for its root view; that single reference is
/// what keeps the whole tree below it alive.
pub(crate) struct AnyViewHandle {
    window_id: WindowId,
    view_id: EntityId,
    view_type: TypeId,
    ref_counts: Weak<Mutex<RefCounts>>,
}

impl AnyViewHandle {
    /// The view's id.
    pub(crate) fn id(&self) -> EntityId {
        self.view_id
    }

    /// Whether the erased view is a `T`.
    pub(crate) fn is<T: 'static>(&self) -> bool {
        TypeId::of::<T>() == self.view_type
    }

    /// Recovers a typed handle, or `None` on a type mismatch.
    pub(crate) fn downcast<T: Entity>(self) -> Option<ViewHandle<T>> {
        if self.is::<T>()
            && let Some(ref_counts) = self.ref_counts.upgrade()
        {
            return Some(ViewHandle::new(self.window_id, self.view_id, &ref_counts));
        }
        None
    }
}

impl Clone for AnyViewHandle {
    fn clone(&self) -> Self {
        if let Some(ref_counts) = self.ref_counts.upgrade() {
            ref_counts.lock().inc_entity(self.view_id);
        }
        Self {
            window_id: self.window_id,
            view_id: self.view_id,
            view_type: self.view_type,
            ref_counts: self.ref_counts.clone(),
        }
    }
}

impl Drop for AnyViewHandle {
    fn drop(&mut self) {
        if let Some(ref_counts) = self.ref_counts.upgrade() {
            ref_counts.lock().dec_view(self.view_id);
        }
    }
}

impl<T: Entity> From<&ViewHandle<T>> for AnyViewHandle {
    fn from(handle: &ViewHandle<T>) -> Self {
        if let Some(ref_counts) = handle.ref_counts.upgrade() {
            ref_counts.lock().inc_entity(handle.view_id);
        }
        Self {
            window_id: handle.window_id,
            view_id: handle.view_id,
            view_type: TypeId::of::<T>(),
            ref_counts: handle.ref_counts.clone(),
        }
    }
}

impl<T: Entity> From<ViewHandle<T>> for AnyViewHandle {
    fn from(handle: ViewHandle<T>) -> Self {
        (&handle).into()
    }
}

/// Borrowing a model.
///
/// This trait and the five below are the structural trick that makes contexts
/// compose: `AppContext`, `ViewContext`, `ModelContext` and `App` all implement
/// them, so `handle.update(ctx, ..)` compiles the same no matter which context
/// is in hand.
pub trait ModelAsRef {
    /// Borrows the model behind `handle`.
    fn model<T: Entity>(&self, handle: &ModelHandle<T>) -> &T;
}

/// Reading a model together with the app around it.
pub trait ReadModel: ModelAsRef {
    /// Runs `read` against the model and the app.
    fn read_model<T, F, S>(&self, handle: &ModelHandle<T>, read: F) -> S
    where
        T: Entity,
        F: FnOnce(&T, &AppContext) -> S;
}

/// Mutating a model.
pub trait UpdateModel: ReadModel {
    /// Checks the model out, runs `update`, and checks it back in.
    fn update_model<T, F, S>(&mut self, handle: &ModelHandle<T>, update: F) -> S
    where
        T: Entity,
        F: FnOnce(&mut T, &mut ModelContext<T>) -> S;
}

/// Borrowing a view.
///
/// Reads need only `T: 'static`: the view-ness is already carried by
/// `ViewHandle<T>`, which can only be minted for a view type.
pub trait ViewAsRef {
    /// Borrows the view behind `handle`, panicking if it is checked out.
    fn view<T: 'static>(&self, handle: &ViewHandle<T>) -> &T;

    /// Borrows the view behind `handle`, or `None` if it is checked out.
    fn try_view<T: 'static>(&self, handle: &ViewHandle<T>) -> Option<&T>;
}

/// Reading a view together with the app around it.
pub trait ReadView: ViewAsRef {
    /// Runs `read` against the view and the app.
    fn read_view<T, F, S>(&self, handle: &ViewHandle<T>, read: F) -> S
    where
        T: 'static,
        F: FnOnce(&T, &AppContext) -> S;
}

/// Mutating a view.
pub trait UpdateView: ReadView {
    /// Checks the view out, runs `update`, and checks it back in.
    ///
    /// Unlike the read path this needs `T: Entity`, because the closure gets a
    /// `ViewContext<T>`, which can `emit(T::Event)`.
    fn update_view<T, F, S>(&mut self, handle: &ViewHandle<T>, update: F) -> S
    where
        T: Entity,
        F: FnOnce(&mut T, &mut ViewContext<T>) -> S;
}
