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
use crate::workspace::{Fonts, Opening, Workspace};

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
    with_host_and_context(|host, _| test(host));
}

/// The same, with some of the plugins switched off the way a settings file
/// switches them off.
fn with_disabled(disabled: &[String], test: impl FnOnce(&mut Host)) {
    with_context(disabled, |host, _| test(host));
}

/// The same, for a test that needs the context too — switching a plugin back
/// on runs its `build`, which needs one.
fn with_host_and_context(test: impl FnOnce(&mut Host, &mut ViewContext<Workspace>)) {
    with_context(&[], test);
}

/// The whole of it: an app, a window, and a host built inside a real context.
fn with_context(disabled: &[String], test: impl FnOnce(&mut Host, &mut ViewContext<Workspace>)) {
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
            Opening {
                settings: Settings::ephemeral(),
                channel: Channel::Dev,
                plugins: plugins::defaults(),
            },
            quit,
            Rc::new(Recorder::default()),
            ctx,
        )
    });

    workspace.update(&mut app, |_, ctx| {
        let mut host = load(plugins::defaults(), disabled, fonts, ctx);
        test(&mut host, ctx);
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

#[test]
fn the_settings_rail_is_what_the_plugins_put_in_it() {
    // Nothing enumerates these. The rail's order is the `order` each page
    // asked for, so a plugin that adds one lands where it asked to and not
    // where a list in the settings module happens to put it.
    with_host(|host| {
        let titles: Vec<String> = host
            .settings_pages()
            .into_iter()
            .map(|(_, title)| title)
            .collect();

        assert_eq!(titles, ["Appearance", "Shell", "Usage", "Keys", "About"]);
    });
}

#[test]
fn disabling_a_plugin_takes_its_settings_page_off_the_rail() {
    // The whole point of the page belonging to the feature: somebody who turns
    // the usage plugin off loses the chip *and* the page that configures it,
    // rather than being left with a page whose switch controls nothing.
    with_host(|host| {
        let usage = PluginId::parse("crook/usage").expect("a literal that parses");
        let key = "crook/usage/page";
        assert!(host.settings_page_id(key).is_some());

        host.unload(&usage);

        assert!(
            host.settings_page_id(key).is_none(),
            "the page outlived the plugin that added it"
        );
        let titles: Vec<String> = host
            .settings_pages()
            .into_iter()
            .map(|(_, title)| title)
            .collect();
        assert_eq!(titles, ["Appearance", "Shell", "Keys", "About"]);
    });
}

#[test]
fn a_plugin_switched_off_can_be_switched_back_on() {
    // The other half of `unload`, and what makes the switch a switch rather
    // than a door. Building again is not resuming: nothing of the previous
    // life survives, which is correct — a plugin that was off saw nothing
    // happen while it was off.
    with_host_and_context(|host, ctx| {
        let usage = PluginId::parse("crook/usage").expect("a literal that parses");
        assert!(host.is_loaded(&usage));

        host.unload(&usage);
        assert!(!host.is_loaded(&usage));
        assert!(host.slots().is_empty(crate::plugins::header::HEADER_RIGHT));

        host.enable(&usage, ctx);

        assert!(host.is_loaded(&usage));
        assert!(
            !host.slots().is_empty(crate::plugins::header::HEADER_RIGHT),
            "the chip did not come back"
        );
        assert!(host.settings_page_id("crook/usage/page").is_some());
        assert!(host.audit().is_empty(), "{:?}", host.audit());
    });
}

#[test]
fn a_plugin_the_settings_switched_off_is_carried_and_not_built() {
    // What "off" means: `build` never runs, so the plugin registers nothing
    // and makes nothing — but it is still in the list, because a switch you
    // cannot see is a switch you cannot turn back on.
    let disabled = vec!["crook/usage".to_owned()];
    with_disabled(&disabled, |host| {
        let usage = PluginId::parse("crook/usage").expect("a literal that parses");

        assert!(!host.is_loaded(&usage));
        assert!(
            host.available().iter().any(|manifest| manifest.id == usage),
            "a switched-off plugin has to stay visible"
        );
        assert!(host.slots().is_empty(crate::plugins::header::HEADER_RIGHT));
        assert!(host.settings_page_id("crook/usage/page").is_none());
        // And nothing else noticed: a plugin that is off is not a plugin that
        // failed.
        assert!(host.refused().is_empty());
        assert!(host.audit().is_empty(), "{:?}", host.audit());
    });
}

#[test]
fn a_name_in_the_disabled_list_that_answers_to_nothing_costs_nothing() {
    // A plugin somebody uninstalled, or one from a build they no longer run.
    // The name is kept in their file — dropping it would switch the feature
    // back on the day they reinstall it — and it switches nothing off here.
    let disabled = vec!["eugen/never-installed".to_owned()];
    with_disabled(&disabled, |host| {
        assert_eq!(host.loaded().len(), host.available().len());
        assert!(host.refused().is_empty());
    });
}
