//! [`Workspace`]: the state behind the window, and the one place it changes.

use std::collections::HashMap;
use std::rc::Rc;

use crookui_core::elements::MouseStateHandle;
use crookui_core::event::{Keystroke, Modifiers};
use crookui_core::fonts::FamilyId;
use crookui_core::prelude::*;

use crate::tab::{AgentSession, Tab, TabAction, TabEffect, TabId, TabStrip};
use crate::theme::THEME;
use crate::usage_model::UsageModel;

use super::usage_chip::UsageChip;
use super::{body, header_toolbar};

/// The two font families the interface is set in, resolved once at startup.
///
/// Passed down rather than looked up, because resolving a family is a search
/// through the system font database and a view renders many times a second.
#[derive(Copy, Clone)]
pub struct Fonts {
    /// Labels, buttons, the chip.
    pub ui: FamilyId,
    /// Anything that stands in for terminal output.
    pub monospace: FamilyId,
}

/// What the workspace calls when the last tab is closed.
///
/// A callback rather than a window handle, so nothing in this module names the
/// windowing layer: the headless snapshot path passes a no-op and gets the
/// same code path a real window gets.
pub type QuitRequest = Rc<dyn Fn()>;

/// What the mouse is doing to one tab, kept across renders.
///
/// The element tree is thrown away every time the view re-renders, so hover
/// and press state cannot live in it. It lives here, keyed by identity, and is
/// handed back to the elements each frame.
pub(super) struct TabInteraction {
    /// The tab itself.
    pub(super) tab: MouseStateHandle,
    /// Its close button.
    pub(super) close: MouseStateHandle,
}

/// The window's root view.
pub struct Workspace {
    tabs: TabStrip,
    fonts: Fonts,
    usage: ModelHandle<UsageModel>,
    chip: ViewHandle<UsageChip>,
    interactions: HashMap<TabId, TabInteraction>,
    new_tab: MouseStateHandle,
    quit: QuitRequest,
}

impl Workspace {
    /// Builds the workspace, its usage chip, and one tab to start in.
    pub fn new(fonts: Fonts, quit: QuitRequest, ctx: &mut ViewContext<Self>) -> Self {
        let usage = UsageModel::handle(ctx);

        // The canonical model-changed-so-repaint bridge. The chip observes the
        // same model for itself; this is what keeps the header honest when the
        // reading changes the chip's width and the row around it has to be
        // laid out again.
        ctx.observe(&usage, |_, _, ctx| ctx.notify());

        let chip = ctx.add_view(|ctx| UsageChip::new(fonts, ctx));

        let mut workspace = Self {
            tabs: TabStrip::new(),
            fonts,
            usage,
            chip,
            interactions: HashMap::new(),
            new_tab: MouseStateHandle::default(),
            quit,
        };
        workspace.sync_interactions();
        workspace
    }

    /// The tabs, read-only. Changing them goes through [`Self::apply`].
    pub fn tabs(&self) -> &TabStrip {
        &self.tabs
    }

    /// The families the interface is set in.
    pub fn fonts(&self) -> Fonts {
        self.fonts
    }

    /// Starts the usage poll chain. Call once, after the window exists.
    pub fn start_usage_poll(&self, ctx: &mut ViewContext<Self>) {
        self.usage.update(ctx, |model, ctx| model.start(ctx));
    }

    /// Reports the agent's progress into one session, and repaints the tab
    /// that shows it. Returns whether the tab is still open.
    ///
    /// A closure rather than a returned `&mut AgentSession`, because the
    /// repaint has to be part of the same call: a caller that renamed a
    /// session and did not notify would leave the old title on screen with
    /// nothing reporting an error, until some unrelated click happened to
    /// rebuild the frame.
    pub fn update_session(
        &mut self,
        id: TabId,
        ctx: &mut ViewContext<Self>,
        report: impl FnOnce(&mut AgentSession),
    ) -> bool {
        let Some(tab) = self.tabs.get_mut(id) else {
            return false;
        };

        report(tab.session_mut());
        ctx.notify();
        true
    }

    /// Applies a tab action, brings the per-tab mouse state back in step, and
    /// repaints if the strip actually moved.
    ///
    /// Every mutation of the strip goes through here, which is what lets the
    /// interaction map be keyed by identity without ever leaking an entry for
    /// a tab that has been closed — and what makes "the strip changed" and
    /// "the window is dirty" the same statement rather than two.
    pub fn apply(&mut self, action: TabAction, ctx: &mut ViewContext<Self>) -> TabEffect {
        let effect = self.tabs.apply(action);
        self.sync_interactions();

        // An action that changed nothing repaints nothing: holding down
        // cmd-alt-left on the leftmost tab must not put the window on a
        // key-repeat render loop.
        if effect == TabEffect::Changed {
            ctx.notify();
        }
        effect
    }

    /// The tab action a keystroke means, if it means one.
    ///
    /// Bindings produce exactly the values the mouse produces — there is no
    /// second code path for the keyboard, which is what stops a shortcut from
    /// drifting away from the button beside it.
    pub fn action_for(&self, keystroke: &Keystroke) -> Option<TabAction> {
        if !is_platform_chord(keystroke.modifiers) {
            return None;
        }

        let shift = keystroke.modifiers.shift;
        let alt = keystroke.modifiers.alt;
        match (keystroke.key.as_str(), shift, alt) {
            ("t", false, false) => Some(TabAction::New),
            ("w", false, false) => Some(TabAction::Close(self.tabs.active_id())),
            ("left", true, false) => self.neighbour(-1).map(TabAction::Select),
            ("right", true, false) => self.neighbour(1).map(TabAction::Select),
            ("left", false, true) => Some(TabAction::MoveLeft),
            ("right", false, true) => Some(TabAction::MoveRight),
            _ => None,
        }
    }

    pub(super) fn chip(&self) -> &ViewHandle<UsageChip> {
        &self.chip
    }

    pub(super) fn new_tab_state(&self) -> MouseStateHandle {
        self.new_tab.clone()
    }

    pub(super) fn interaction(&self, id: TabId) -> Option<&TabInteraction> {
        self.interactions.get(&id)
    }

    /// The tab `offset` slots away from the active one, wrapping at both ends.
    fn neighbour(&self, offset: isize) -> Option<TabId> {
        let count = self.tabs.len() as isize;
        let index = self.tabs.index_of(self.tabs.active_id())? as isize;
        let wrapped = (index + offset).rem_euclid(count) as usize;
        self.tabs.iter().nth(wrapped).map(Tab::id)
    }

    fn sync_interactions(&mut self) {
        for tab in self.tabs.iter() {
            self.interactions
                .entry(tab.id())
                .or_insert_with(|| TabInteraction {
                    tab: MouseStateHandle::default(),
                    close: MouseStateHandle::default(),
                });
        }
        self.interactions
            .retain(|id, _| self.tabs.get(*id).is_some());
    }
}

impl Entity for Workspace {
    type Event = ();
}

impl View for Workspace {
    fn ui_name() -> &'static str {
        "Workspace"
    }

    fn render(&self, _: &AppContext) -> Box<dyn Element> {
        Container::new(
            Flex::column()
                .with_main_axis_size(MainAxisSize::Max)
                .with_child(header_toolbar::render(self))
                .with_child(Expanded::new(1., body::render(self)).finish())
                .finish(),
        )
        .with_background_color(THEME.ground)
        .finish()
    }
}

impl TypedActionView for Workspace {
    type Action = TabAction;

    fn handle_action(&mut self, action: &TabAction, ctx: &mut ViewContext<Self>) {
        if self.apply(*action, ctx) == TabEffect::CloseWindow {
            (self.quit)();
        }
    }
}

/// Whether these modifiers are the platform's "this is an application command"
/// chord: Command on macOS, Control everywhere else.
fn is_platform_chord(modifiers: Modifiers) -> bool {
    if cfg!(target_os = "macos") {
        modifiers.cmd && !modifiers.ctrl
    } else {
        modifiers.ctrl && !modifiers.cmd
    }
}
