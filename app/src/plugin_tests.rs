//! What the plugins a release binary carries promise each other.

use std::cell::Cell;
use std::collections::BTreeMap;
use std::rc::Rc;
use std::sync::Arc;

use crookui_core::App;
use crookui_core::executor::{Background, LocalQueue};
use crookui_core::fonts::FamilyId;

use crate::Channel;
use crate::plugins;
use crate::settings::Settings;
use crate::terminal_font::{CELL_FONT_SIZE, CellFont};
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
        let mut host = load(plugins::defaults(), disabled, BTreeMap::new(), fonts, ctx);
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
fn unloading_a_plugin_takes_its_contribution_out_of_the_slot_it_filled() {
    // What disabling one will do, and the whole reason a registration is a
    // guard: the slot goes back to being what it was before the plugin loaded.
    // Asked of the palette and the window's overlay because that is the one
    // pair a release binary still has — `header.right` is declared by a plugin
    // in the box and filled only by one installed from a file.
    with_host(|host| {
        let palette = PluginId::parse("crook/palette").expect("a literal that parses");
        assert!(
            !host
                .slots()
                .is_empty(crate::plugins::window::WINDOW_OVERLAY)
        );

        host.unload(&palette);

        assert!(
            host.slots()
                .is_empty(crate::plugins::window::WINDOW_OVERLAY),
            "the palette outlived the plugin that contributed it"
        );
        assert!(!host.loaded().iter().any(|manifest| manifest.id == palette));
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
    // A plugin cannot take a chord the window owns. What it asks for is the
    // weakest layer of the keybindings — see `crate::keybindings` — so
    // `crook/palette` gets the palette chord because nothing else wants it,
    // and a plugin asking for the one that opens a tab would be under it.
    with_host(|host| {
        let palette = ActionName::parse("crook/palette/open").expect("a literal");
        let chord = if cfg!(target_os = "macos") {
            "shift+cmd+p"
        } else {
            "ctrl+shift+p"
        };

        let rules = host.suggested_rules();
        let asked = rules
            .iter()
            .find(|rule| rule.command == palette)
            .expect("the palette asked for a chord");

        assert_eq!(asked.chord(), chord);
        assert_eq!(asked.source, crate::keybindings::Source::Plugin);
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

        assert_eq!(
            titles,
            ["Appearance", "Shell", "Keyboard Shortcuts", "About"]
        );
    });
}

#[test]
fn disabling_a_plugin_takes_its_settings_page_off_the_rail() {
    // The whole point of the page belonging to the feature: somebody who turns
    // the shell plugin off loses the page that configures it, rather than
    // being left with a page whose switches control nothing.
    with_host(|host| {
        let shell = PluginId::parse("crook/shell").expect("a literal that parses");
        let key = "crook/shell/page";
        assert!(host.settings_page_id(key).is_some());

        host.unload(&shell);

        assert!(
            host.settings_page_id(key).is_none(),
            "the page outlived the plugin that added it"
        );
        let titles: Vec<String> = host
            .settings_pages()
            .into_iter()
            .map(|(_, title)| title)
            .collect();
        assert_eq!(titles, ["Appearance", "Keyboard Shortcuts", "About"]);
    });
}

#[test]
fn a_plugin_switched_off_can_be_switched_back_on() {
    // The other half of `unload`, and what makes the switch a switch rather
    // than a door. Building again is not resuming: nothing of the previous
    // life survives, which is correct — a plugin that was off saw nothing
    // happen while it was off.
    with_host_and_context(|host, ctx| {
        let palette = PluginId::parse("crook/palette").expect("a literal that parses");
        let open = ActionName::parse("crook/palette/open").expect("a literal");
        assert!(host.is_loaded(&palette));

        host.unload(&palette);
        assert!(!host.is_loaded(&palette));
        assert!(
            host.slots()
                .is_empty(crate::plugins::window::WINDOW_OVERLAY)
        );

        host.enable(&palette, ctx);

        assert!(host.is_loaded(&palette));
        assert!(
            !host
                .slots()
                .is_empty(crate::plugins::window::WINDOW_OVERLAY),
            "the palette did not come back"
        );
        assert!(
            host.action(&open).is_some(),
            "the action did not come back with it"
        );
        assert!(host.audit().is_empty(), "{:?}", host.audit());
    });
}

#[test]
fn a_plugin_the_settings_switched_off_is_carried_and_not_built() {
    // What "off" means: `build` never runs, so the plugin registers nothing
    // and makes nothing — but it is still in the list, because a switch you
    // cannot see is a switch you cannot turn back on.
    let disabled = vec!["crook/palette".to_owned()];
    with_disabled(&disabled, |host| {
        let palette = PluginId::parse("crook/palette").expect("a literal that parses");
        let open = ActionName::parse("crook/palette/open").expect("a literal");

        assert!(!host.is_loaded(&palette));
        assert!(
            host.available()
                .iter()
                .any(|manifest| manifest.id == palette),
            "a switched-off plugin has to stay visible"
        );
        assert!(
            host.slots()
                .is_empty(crate::plugins::window::WINDOW_OVERLAY)
        );
        assert!(host.action(&open).is_none());
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

#[test]
fn the_marks_on_a_tab_row_are_slots_and_a_release_binary_leaves_them_empty() {
    // Declared by `crook/tabs` and filled by nobody in the box, which is the
    // shape `header.right` has: the disc a row draws is the *host's* answer to
    // an empty slot rather than a contribution competing with a plugin's. See
    // `plugins::tabs` for why that difference matters.
    with_host(|host| {
        let mark = crate::plugins::tabs::TAB_ROW_MARK;
        let badge = crate::plugins::tabs::TAB_ROW_BADGE;

        assert_eq!(host.row_slot_named("tab.row.mark"), Some(mark));
        assert_eq!(host.row_slot_named("tab.row.badge"), Some(badge));
        assert!(host.rows().is_empty(mark));
        assert!(host.rows().is_empty(badge));
        // And they are not in the other registry, so a plugin that contributed
        // an ordinary element to one is told the slot does not exist there
        // rather than drawing something with no way to ask which row it is on.
        assert_eq!(host.slot_named("tab.row.mark"), None);
        assert!(host.audit().is_empty(), "{:?}", host.audit());
    });
}

#[test]
fn a_row_contribution_goes_back_out_with_the_plugin_that_made_it() {
    // The guard, for the registry the tab rows use. Every other registration
    // in this file is proven the same way, and a second registry is a second
    // place for one to be left behind.
    with_host(|host| {
        let mark = crate::plugins::tabs::TAB_ROW_MARK;
        let who = PluginId::parse("crook/host").expect("a literal that parses");

        host.contribute_row(mark, "mark", 0, |_, _, _| None);
        assert_eq!(
            host.rows().contributors(mark),
            vec![(who.clone(), EntryId::new("mark"))]
        );

        host.unload(&who);

        assert!(host.rows().is_empty(mark), "the contribution outlived it");
    });
}
