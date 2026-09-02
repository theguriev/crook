//! Executable specification for the semantics the rest of the app assumes.
//!
//! These are the cases that are easy to get subtly wrong when reimplementing
//! an entity system: the order callbacks run in, what happens to a
//! subscription whose owner is busy, and — the subtle one — exactly when a
//! weak handle stops upgrading.

use std::cell::RefCell;
use std::rc::Rc;

use super::*;
use crate::element::Element;
use crate::elements::Empty;

/// A view that renders nothing, for tests about everything except rendering.
macro_rules! trivial_view {
    ($name:ident) => {
        impl View for $name {
            fn ui_name() -> &'static str {
                stringify!($name)
            }

            fn render(&self, _: &AppContext) -> Box<dyn Element> {
                Empty::new().finish()
            }
        }

        impl TypedActionView for $name {
            type Action = ();
        }
    };
}

#[test]
fn subscribers_see_events_in_order_and_new_subscriptions_go_first() {
    #[derive(Default)]
    struct Model {
        events: Vec<usize>,
    }

    impl Entity for Model {
        type Event = usize;
    }

    App::test(|mut app| async move {
        let app = &mut app;
        let subscriber = app.add_model(|_| Model::default());
        let emitter = app.add_model(|_| Model::default());
        let emitter_again = emitter.clone();

        subscriber.update(app, |_, ctx| {
            ctx.subscribe_to_model(&emitter, move |model: &mut Model, _, event, ctx| {
                model.events.push(*event);

                // Subscribing from inside a callback is legal, and the new
                // subscription starts firing with the *next* event.
                ctx.subscribe_to_model(&emitter_again, |model, _, event, _| {
                    model.events.push(*event * 2);
                });
            });
        });

        emitter.update(app, |_, ctx| ctx.emit(7));
        subscriber.read(app, |model, _| assert_eq!(model.events, [7]));

        emitter.update(app, |_, ctx| ctx.emit(5));
        subscriber.read(app, |model, _| assert_eq!(model.events, [7, 10, 5]));
    })
}

#[test]
fn observers_run_on_notify_and_read_the_new_value() {
    #[derive(Default)]
    struct Model {
        count: usize,
        observed: Vec<usize>,
    }

    impl Entity for Model {
        type Event = ();
    }

    App::test(|mut app| async move {
        let app = &mut app;
        let observer = app.add_model(|_| Model::default());
        let observed = app.add_model(|_| Model::default());

        observer.update(app, |_, ctx| {
            ctx.observe(&observed, |model: &mut Model, observed, ctx| {
                model.observed.push(observed.as_ref(ctx).count);
            });
        });

        observed.update(app, |model, ctx| {
            model.count = 7;
            ctx.notify();
        });

        observer.read(app, |model, _| assert_eq!(model.observed, [7]));
    })
}

#[test]
fn a_view_observing_a_model_sees_its_updates() {
    #[derive(Default)]
    struct Chip {
        observed: Vec<usize>,
    }

    impl Entity for Chip {
        type Event = ();
    }

    trivial_view!(Chip);

    #[derive(Default)]
    struct Usage {
        tokens: usize,
    }

    impl Entity for Usage {
        type Event = ();
    }

    App::test(|mut app| async move {
        let app = &mut app;
        let (_, chip) = app.add_window(|_| Chip::default());
        let usage = app.add_model(|_| Usage::default());

        chip.update(app, |_, ctx| {
            ctx.observe(&usage, |chip: &mut Chip, usage, ctx| {
                chip.observed.push(usage.as_ref(ctx).tokens);
                ctx.notify();
            });
        });

        usage.update(app, |usage, ctx| {
            usage.tokens = 11;
            ctx.notify();
        });

        chip.read(app, |chip, _| assert_eq!(chip.observed, [11]));
    })
}

#[test]
fn dropping_the_last_handle_removes_the_view_and_its_subscriptions() {
    struct Tab {
        other: Option<ViewHandle<Tab>>,
        events: Vec<usize>,
    }

    impl Entity for Tab {
        type Event = usize;
    }

    trivial_view!(Tab);

    impl Tab {
        fn new(other: Option<ViewHandle<Tab>>, ctx: &mut ViewContext<Self>) -> Self {
            if let Some(other) = other.as_ref() {
                ctx.subscribe_to_view(other, |tab: &mut Tab, _, event, _| {
                    tab.events.push(*event);
                });
            }
            Self {
                other,
                events: Vec::new(),
            }
        }
    }

    App::test(|mut app| async move {
        let (window_id, _root) = app.add_window(|ctx| Tab::new(None, ctx));
        let emitter = app.add_view(window_id, |ctx| Tab::new(None, ctx));
        let subscriber = app.add_view(window_id, |ctx| Tab::new(Some(emitter.clone()), ctx));

        app.read(|ctx| assert_eq!(ctx.windows[&window_id].views.len(), 3));

        emitter.update(&mut app, |_, ctx| {
            ctx.emit(1);
            ctx.emit(2);
        });
        subscriber.read(&app, |tab, _| assert_eq!(tab.events, [1, 2]));

        // The subscriber owns the only other handle, so clearing it inside its
        // own update drops the emitter for good.
        subscriber.update(&mut app, |tab, _| {
            drop(emitter);
            tab.other.take();
        });

        app.read(|ctx| {
            assert_eq!(ctx.windows[&window_id].views.len(), 2);
            assert!(ctx.subscriptions.is_empty());
        });
    })
}

#[test]
fn a_weak_handle_stops_upgrading_the_moment_the_last_strong_one_drops() {
    struct Trigger;

    impl Entity for Trigger {
        type Event = ();
    }

    struct Target;

    impl Entity for Target {
        type Event = ();
    }

    struct Owner {
        target: Option<ModelHandle<Target>>,
        weak_target: Option<WeakModelHandle<Target>>,
    }

    impl Entity for Owner {
        type Event = ();
    }

    App::test(|mut app| async move {
        let app = &mut app;
        let trigger = app.add_model(|_| Trigger);
        let owner = app.add_model(|_| Owner {
            target: None,
            weak_target: None,
        });

        {
            let target = app.add_model(|_| Target);
            let weak_target = target.downgrade();

            owner.update(app, |owner, ctx| {
                owner.target = Some(target);
                owner.weak_target = Some(weak_target);

                ctx.subscribe_to_model(&trigger, |owner: &mut Owner, _, _, ctx| {
                    owner.target.take();

                    // The model is still in the app's map here — removal is
                    // deferred to the next flush — so upgrading must consult
                    // the refcounts, not the map.
                    assert!(
                        owner.weak_target.as_ref().unwrap().upgrade(ctx).is_none(),
                        "a weak handle upgraded after its last strong handle was dropped"
                    );
                });
            });
        }

        trigger.update(app, |_, ctx| ctx.emit(()));

        owner.read(app, |owner, ctx| {
            assert!(owner.weak_target.as_ref().unwrap().upgrade(ctx).is_none());
        });
    })
}

#[test]
fn a_weak_view_handle_stops_upgrading_the_moment_the_last_strong_one_drops() {
    struct Target;

    impl Entity for Target {
        type Event = ();
    }

    trivial_view!(Target);

    struct Trigger;

    impl Entity for Trigger {
        type Event = ();
    }

    struct Owner {
        target: Option<ViewHandle<Target>>,
        weak_target: Option<WeakViewHandle<Target>>,
    }

    impl Entity for Owner {
        type Event = ();
    }

    App::test(|mut app| async move {
        let trigger = app.add_model(|_| Trigger);
        let owner = app.add_model(|_| Owner {
            target: None,
            weak_target: None,
        });

        // A plain view rather than the root, so the window does not hold a
        // handle of its own.
        let (window_id, _root) = app.add_window(|_| Target);
        {
            let target = app.add_view(window_id, |_| Target);
            let weak_target = target.downgrade();
            let trigger_for_callback = trigger.clone();

            owner.update(&mut app, |owner, ctx| {
                owner.target = Some(target);
                owner.weak_target = Some(weak_target);

                ctx.subscribe_to_model(&trigger_for_callback, |owner: &mut Owner, _, _, ctx| {
                    owner.target.take();
                    assert!(
                        owner.weak_target.as_ref().unwrap().upgrade(ctx).is_none(),
                        "a weak view handle upgraded after its last strong handle was dropped"
                    );
                });
            });
        }

        trigger.update(&mut app, |_, ctx| ctx.emit(()));

        owner.read(&app, |owner, ctx| {
            assert!(owner.weak_target.as_ref().unwrap().upgrade(ctx).is_none());
        });
    })
}

#[test]
fn focus_moves_and_ancestors_hear_about_it() {
    struct Pane {
        name: &'static str,
        children: Vec<ViewHandle<Pane>>,
        events: Rc<RefCell<Vec<String>>>,
    }

    impl Entity for Pane {
        type Event = ();
    }

    impl View for Pane {
        fn ui_name() -> &'static str {
            "Pane"
        }

        fn render(&self, _: &AppContext) -> Box<dyn Element> {
            Empty::new().finish()
        }

        fn on_focus(&mut self, focus_ctx: &FocusContext, _: &mut ViewContext<Self>) {
            let what = if focus_ctx.is_self_focused() {
                "self focused"
            } else {
                "child focused"
            };
            self.events
                .borrow_mut()
                .push(format!("{} {what}", self.name));
        }

        fn on_blur(&mut self, blur_ctx: &BlurContext, _: &mut ViewContext<Self>) {
            let what = if blur_ctx.is_self_blurred() {
                "self blurred"
            } else {
                "child blurred"
            };
            self.events
                .borrow_mut()
                .push(format!("{} {what}", self.name));
        }
    }

    impl TypedActionView for Pane {
        type Action = ();
    }

    App::test(|mut app| async move {
        let events: Rc<RefCell<Vec<String>>> = Rc::default();

        let (_, root) = app.add_window(|ctx| {
            // Created through the view's own context, so the parent link
            // exists before anything is ever laid out.
            let child = ctx.add_view(|_| Pane {
                name: "child",
                children: Vec::new(),
                events: events.clone(),
            });
            Pane {
                name: "root",
                children: vec![child],
                events: events.clone(),
            }
        });

        assert_eq!(events.take(), ["root self focused"]);

        root.update(&mut app, |root, ctx| ctx.focus(&root.children[0]));
        root.update(&mut app, |_, ctx| ctx.focus_self());

        assert_eq!(
            events.take(),
            [
                "root self blurred",
                "child self focused",
                "root child focused",
                "child self blurred",
                "root child blurred",
                "root self focused",
            ]
        );
    })
}

#[test]
fn focus_returns_to_the_root_when_the_focused_view_is_dropped() {
    #[derive(Default)]
    struct Pane;

    impl Entity for Pane {
        type Event = ();
    }

    trivial_view!(Pane);

    App::test(|mut app| async move {
        let (window_id, root) = app.add_window(|_| Pane);
        let child = app.add_view(window_id, |_| Pane);

        root.update(&mut app, |_, ctx| ctx.focus(&child));
        assert_eq!(app.focused_view_id(window_id), Some(child.id()));

        app.update(|_| drop(child));
        assert_eq!(app.focused_view_id(window_id), Some(root.id()));
    })
}

#[test]
fn a_typed_action_stops_at_the_deepest_view_that_handles_its_type() {
    #[derive(Debug)]
    struct Select(&'static str);

    #[derive(Debug)]
    struct Close(usize);

    #[derive(Default)]
    struct Workspace {
        handled: Vec<String>,
    }

    impl Entity for Workspace {
        type Event = ();
    }

    impl View for Workspace {
        fn ui_name() -> &'static str {
            "Workspace"
        }

        fn render(&self, _: &AppContext) -> Box<dyn Element> {
            Empty::new().finish()
        }
    }

    impl TypedActionView for Workspace {
        type Action = Select;

        fn handle_action(&mut self, action: &Select, _: &mut ViewContext<Self>) {
            self.handled.push(action.0.to_owned());
        }
    }

    #[derive(Default)]
    struct Tab {
        handled: Vec<usize>,
    }

    impl Entity for Tab {
        type Event = ();
    }

    impl View for Tab {
        fn ui_name() -> &'static str {
            "Tab"
        }

        fn render(&self, _: &AppContext) -> Box<dyn Element> {
            Empty::new().finish()
        }
    }

    impl TypedActionView for Tab {
        type Action = Close;

        fn handle_action(&mut self, action: &Close, _: &mut ViewContext<Self>) {
            self.handled.push(action.0);
        }
    }

    App::test(|mut app| async move {
        let (window_id, root) = app.add_window(|_| Workspace::default());
        let outer = app.add_view(window_id, |_| Workspace::default());
        let tab = app.add_view(window_id, |_| Tab::default());
        let chain = [root.id(), outer.id(), tab.id()];

        // A workspace action fired from the tab passes straight through it and
        // lands on the nearest workspace, which is the bubbling mechanism.
        assert!(app.dispatch_typed_action(window_id, &chain, &Select("first")));
        outer.read(&app, |view, _| assert_eq!(view.handled, ["first"]));
        root.read(&app, |view, _| assert!(view.handled.is_empty()));

        assert!(app.dispatch_typed_action(window_id, &chain, &Close(3)));
        tab.read(&app, |view, _| assert_eq!(view.handled, [3]));

        // Without the tab in the chain, the same action reaches nobody.
        assert!(!app.dispatch_typed_action(window_id, &chain[..2], &Close(4)));
    })
}

#[test]
fn a_view_created_through_a_context_inherits_its_responder_chain() {
    #[derive(Default)]
    struct Pane;

    impl Entity for Pane {
        type Event = ();
    }

    trivial_view!(Pane);

    App::test(|mut app| async move {
        let (window_id, root) = app.add_window(|_| Pane);

        let child = root.update(&mut app, |_, ctx| ctx.add_view(|_| Pane));
        let grandchild = child.update(&mut app, |_, ctx| ctx.add_view(|_| Pane));

        app.read(|ctx| {
            assert_eq!(
                ctx.view_ancestors(window_id, grandchild.id()),
                [root.id(), child.id(), grandchild.id()],
                "ancestors run root-first, and exist before any layout pass"
            );
        });
    })
}

#[test]
fn notifying_a_view_marks_it_for_redraw() {
    #[derive(Default)]
    struct Pane;

    impl Entity for Pane {
        type Event = ();
    }

    trivial_view!(Pane);

    App::test(|mut app| async move {
        let (window_id, root) = app.add_window(|_| Pane);

        // Creating the window already invalidated the root; take that first.
        app.update(|ctx| ctx.take_all_invalidations_for_window(window_id));
        assert!(!app.read(|ctx| ctx.has_window_invalidations(window_id)));

        root.update(&mut app, |_, ctx| ctx.notify());

        let invalidation = app.update(|ctx| ctx.take_all_invalidations_for_window(window_id));
        assert!(invalidation.updated.contains(&root.id()));
    })
}

#[test]
fn spawned_work_lands_back_on_its_entity() {
    #[derive(Default)]
    struct Usage {
        tokens: u64,
    }

    impl Entity for Usage {
        type Event = ();
    }

    App::test(|mut app| async move {
        let usage = app.add_model(|_| Usage::default());

        let counted = usage.update(&mut app, |_, ctx| {
            let counting = ctx.background().spawn(async { 40 + 2 });
            ctx.spawn(counting, |usage: &mut Usage, tokens, ctx| {
                usage.tokens = tokens;
                ctx.notify();
            })
        });
        counted.await;

        usage.read(&app, |usage, _| assert_eq!(usage.tokens, 42));
    })
}
