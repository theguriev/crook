//! What the plugins a release binary carries promise each other.

use std::cell::Cell;
use std::rc::Rc;
use std::sync::Arc;

use crookui_core::executor::{Background, LocalQueue};
use crookui_core::fonts::FamilyId;
use crookui_core::{AddSingletonModel as _, App};

use crate::Channel;
use crate::plugins;
use crate::settings::Settings;
use crate::terminal_font::{CELL_FONT_SIZE, CellFont};
use crate::usage_model::UsageModel;
use crate::window_controls::Recorder;
use crate::workspace::{Fonts, Workspace};

use super::*;

/// Loads every plugin in the box the way the application does, and hands the
/// host to the test.
///
/// A real app and a real window, because `Plugin::build` takes a context and a
/// plugin is allowed to make an entity with it — a host built without one
/// would be a host no plugin could have been written against. The workspace
/// this loads *into* is the window's own; the host built here is a second one,
/// which is what makes these tests about the plugins rather than about
/// whichever host the workspace happens to be holding.
fn with_host(test: impl FnOnce(&mut Host)) {
    let queue = LocalQueue::new();
    let mut app = App::new(queue.foreground(), Arc::new(Background::new(2)));
    app.update(|ctx| ctx.add_singleton_model(UsageModel::new));

    let quits = Rc::new(Cell::new(0));
    let quit: crate::workspace::QuitRequest = Rc::new(move || quits.set(quits.get() + 1));
    let fonts = Fonts {
        ui: FamilyId(0),
        monospace: FamilyId(0),
    };

    let (_, workspace) = app.add_window(|ctx| {
        Workspace::new(
            fonts,
            CellFont::headless(CELL_FONT_SIZE),
            Settings::ephemeral(),
            Channel::Dev,
            quit,
            Rc::new(Recorder::default()),
            ctx,
        )
    });

    workspace.update(&mut app, |_, ctx| {
        let mut host = load(plugins::defaults(), fonts, ctx);
        test(&mut host);
    });
}

#[test]
fn every_plugin_in_the_box_loads() {
    // The one test that would catch a plugin added to `defaults` and never
    // tried: a `build` that returns an error, or panics, is a feature missing
    // from the window with only a log line to say so.
    with_host(|host| {
        assert!(
            host.refused().is_empty(),
            "a plugin the binary ships did not load: {:?}",
            host.refused()
        );
        assert_eq!(host.loaded().len(), plugins::defaults().len());
    });
}

#[test]
fn the_plugins_in_the_box_have_nothing_to_complain_about() {
    // A misspelled slot name is the likeliest mistake in a plugin, and it is
    // invisible: the contribution is kept, nothing draws it, and the window
    // comes up one feature short. The audit is what turns that into a failing
    // test rather than a bug report.
    with_host(|host| {
        let complaints: Vec<String> = host
            .audit()
            .into_iter()
            .map(|complaint| complaint.to_string())
            .collect();

        assert!(complaints.is_empty(), "{complaints:#?}");
    });
}

#[test]
fn every_plugin_says_who_it_is() {
    with_host(|host| {
        for manifest in host.loaded() {
            assert_eq!(manifest.schema, Manifest::SCHEMA);
            assert_eq!(manifest.tier, Tier::Native, "{} is in the box", manifest.id);
            assert!(!manifest.name.is_empty());
            // The store lists this and nothing else until somebody clicks, so
            // a plugin without one is a row a person cannot choose between.
            assert!(
                !manifest.description.is_empty(),
                "{} says nothing about itself",
                manifest.id
            );
        }
    });
}

#[test]
fn two_plugins_cannot_have_one_name() {
    with_host(|host| {
        let mut names: Vec<&str> = host
            .loaded()
            .iter()
            .map(|manifest| manifest.id.as_str())
            .collect();
        let all = names.len();
        names.sort_unstable();
        names.dedup();

        assert_eq!(names.len(), all, "two plugins in the box share an id");
    });
}

#[test]
fn unloading_a_plugin_takes_its_contribution_off_the_header() {
    // What disabling one will do, and the whole reason a registration is a
    // guard: the slot goes back to being what it was before the plugin loaded.
    with_host(|host| {
        let usage = PluginId::parse("crook/usage").expect("a literal that parses");
        assert!(!host.slots().is_empty(crate::plugins::header::HEADER_RIGHT));

        host.unload(&usage);

        assert!(
            host.slots().is_empty(crate::plugins::header::HEADER_RIGHT),
            "the chip outlived the plugin that contributed it"
        );
        assert!(!host.loaded().iter().any(|manifest| manifest.id == usage));
    });
}

#[test]
fn disabling_a_plugin_takes_its_commands_and_its_chord_with_it() {
    // Everything a plugin registered goes back out together, or a palette
    // keeps offering a row that does nothing and a chord keeps reaching an
    // action nobody answers to.
    with_host(|host| {
        let window = PluginId::parse("crook/window").expect("a literal that parses");
        let open_a_tab = ActionName::parse("crook/window/new-tab").expect("a literal");
        assert!(
            host.commands()
                .iter()
                .any(|(_, name, _)| *name == open_a_tab)
        );

        host.unload(&window);

        assert!(
            !host.commands().iter().any(|(by, _, _)| *by == window),
            "a disabled plugin is still offering commands"
        );
        assert!(
            host.action(&open_a_tab).is_none(),
            "a disabled plugin's action still answers"
        );
    });
}

#[test]
fn a_surface_that_is_down_claims_nothing() {
    // The palette claims Escape while it is up. Nothing is up in a host
    // nobody has opened anything on, so Escape belongs to whatever is under
    // it — which is the difference between a modal and a keyboard grab.
    with_host(|host| {
        assert!(!host.a_surface_is_up());
        assert!(
            host.keys_for(&crookui_core::event::Keystroke::new(
                "escape",
                crookui_core::event::Modifiers::default()
            ))
            .is_none()
        );
    });
}

#[test]
fn a_plugins_chord_is_a_suggestion_and_not_a_claim() {
    // A plugin cannot take a chord the window owns. `crook/palette` asks for
    // the palette chord and gets it because nothing else wants it; a plugin
    // asking for the one that opens a tab would be ignored.
    with_host(|host| {
        let palette = ActionName::parse("crook/palette/open").expect("a literal");
        let chord = if cfg!(target_os = "macos") {
            "cmd-shift-p"
        } else {
            "ctrl-shift-p"
        };
        let keystroke = crate::keymap::parse_chord(chord).expect("a chord");

        assert_eq!(host.suggested_for(&keystroke), host.action(&palette));
    });
}
