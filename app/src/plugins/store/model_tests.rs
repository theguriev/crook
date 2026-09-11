//! What the store knows before it has asked anybody anything, and what it
//! does with what it is handed.
//!
//! Reading an index over the network is not tested here and is not testable
//! here: it is a request to a host that is not this machine's. What is
//! tested is everything either of the slow things lands in, which is where a
//! mistake would be silent: a list read off the disk, a download taken
//! exactly once, the queue behind a download, the pictures out of a module —
//! answered by a fetcher that is a closure, so the module is one the test
//! assembled and no socket is opened — and what the page is told.

use std::sync::Arc;
use std::time::{Duration, Instant};

use crookui_core::App;
use crookui_core::executor::{Background, LocalQueue};

use super::*;
use crate::picture::tests::{icon_png, preview_png};
use crate::plugins::wasm::tests::{Scratch, manifest, wasm_carrying};

const INDEX: &[u8] = br#"{"schema": 1, "plugins": [
    {"id": "eugen/probe", "name": "Probe", "description": "d",
     "versions": [{"version": "1.0.0", "abi": 8, "url": "https://x.invalid/p.wasm", "sha256": "aa"}]}]}"#;

/// How long a test waits for the pool to answer before it gives up.
const DELIVERY_TIMEOUT: Duration = Duration::from_secs(10);

/// An app whose local queue the test can drive by hand.
fn app() -> (Arc<LocalQueue>, App) {
    let queue = LocalQueue::new();
    let app = App::new(queue.foreground(), Arc::new(Background::new(1)));
    (queue, app)
}

/// Pumps the queue until `settled` answers yes, or fails.
fn deliver(queue: &LocalQueue, app: &App, what: &str, mut settled: impl FnMut(&App) -> bool) {
    let deadline = Instant::now() + DELIVERY_TIMEOUT;
    loop {
        queue.run_until_parked();
        if settled(app) {
            return;
        }
        assert!(Instant::now() < deadline, "{what}");
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn probe() -> PluginId {
    PluginId::parse("eugen/probe").expect("a literal that parses")
}

/// A release of `version` at `url`, promising `bytes`.
fn release(version: &str, bytes: &[u8]) -> Release {
    Release {
        version: version.to_owned(),
        abi: 8,
        url: format!("{}p-{version}.wasm", fetch::assets_prefix()),
        sha256: fetch::sha256_hex(bytes),
        bytes: bytes.len() as u64,
        capabilities: Vec::new(),
        asks: Vec::new(),
        yanked: None,
        previews: Vec::new(),
    }
}

/// A fetcher answering every release with `bytes`, checked the way the real
/// one checks, and counting how often it was asked.
fn answering(bytes: Vec<u8>, asked: Arc<std::sync::atomic::AtomicUsize>) -> Fetcher {
    Arc::new(move |release| {
        asked.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        fetch::checked(bytes.clone(), release)
    })
}

#[test]
fn a_store_with_a_cache_knows_what_is_in_it_without_asking_anybody() {
    // The whole of what "offline shows the cache" means: the list is there,
    // on a machine that has never reached the registry in this session.
    let scratch = Scratch::new("store-model");
    let cache = Cache::at(scratch.path());
    cache.write(INDEX, Some("\"abc\"")).expect("it should keep");

    let model = StoreModel::new(Some(cache));

    assert_eq!(model.offers().len(), 1);
    assert_eq!(model.offers()[0].id.as_str(), "eugen/probe");
    assert!(model.fetched().is_some(), "and how old it is");
    assert!(!model.looking());
}

#[test]
fn a_store_with_nothing_cached_offers_nothing_and_is_not_a_failure() {
    let scratch = Scratch::new("store-model-empty");

    let model = StoreModel::new(Some(Cache::at(scratch.path())));

    assert!(model.offers().is_empty());
    assert_eq!(model.fetched(), None);
    assert_eq!(model.problem(), None);
}

#[test]
fn a_machine_with_nowhere_to_keep_a_list_still_has_a_store() {
    // `Cache::user()` answers `None` where there is no data directory, and a
    // store that could not be built there would be a section that is missing
    // rather than one that is empty.
    let model = StoreModel::new(None);

    assert!(model.offers().is_empty());
}

#[test]
fn a_second_download_waits_its_turn_and_starts_when_the_first_lands() {
    // One fetch at a time, and the second press is queued rather than
    // refused: what the page hears is one downloading and one waiting, and
    // finishing the first is what starts the second.
    let (_queue, mut app) = app();
    let never = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let model = app.update(|ctx| {
        ctx.add_model(|_| {
            let mut model = StoreModel::new(None);
            model.fetch_with(answering(b"module".to_vec(), never.clone()));
            model
        })
    });
    let other = PluginId::parse("eugen/other").expect("a literal that parses");
    let first = release("1.0.0", b"module");
    let second = release("2.0.0", b"module");

    app.update(|ctx| {
        model.update(ctx, |model, ctx| {
            model.download(&probe(), &first, ctx);
            model.download(&other, &second, ctx);
            // A third press for a plugin already on its way changes nothing.
            model.download(&other, &second, ctx);
        });
    });
    app.read(|ctx| {
        let model = model.as_ref(ctx);
        assert_eq!(model.downloading(), Some(&probe()));
        assert_eq!(model.queued(), vec![other.clone()]);
        let heard = model.heard();
        assert_eq!(heard.busy(&probe()), Some(Busy::Downloading));
        assert_eq!(heard.busy(&other), Some(Busy::Waiting));
    });

    app.update(|ctx| {
        model.update(ctx, |model, ctx| {
            model.arrived(probe(), first.clone(), Ok(b"module".to_vec()), ctx);
        });
    });
    app.read(|ctx| {
        let model = model.as_ref(ctx);
        assert_eq!(model.downloading(), Some(&other), "the queue did not move");
        assert!(model.queued().is_empty());
    });
    app.update(|ctx| {
        model.update(ctx, |model, _| {
            let landed = model.landed();
            assert_eq!(landed.len(), 1);
            assert_eq!(landed[0].0, probe());
            assert!(model.landed().is_empty(), "taken once");
        });
    });
}

#[test]
fn update_all_queues_each_plugin_once() {
    let (_queue, mut app) = app();
    let never = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let model = app.update(|ctx| {
        ctx.add_model(|_| {
            let mut model = StoreModel::new(None);
            model.fetch_with(answering(b"module".to_vec(), never.clone()));
            model
        })
    });
    let other = PluginId::parse("eugen/other").expect("a literal that parses");
    let third = PluginId::parse("eugen/third").expect("a literal that parses");
    let wanted = release("2.0.0", b"module");

    app.update(|ctx| {
        model.update(ctx, |model, ctx| {
            // One already waiting, which the sweep must not queue twice.
            model.download(&probe(), &wanted, ctx);
            model.download(&other, &wanted, ctx);
            model.update_all(
                vec![
                    (other.clone(), wanted.clone()),
                    (third.clone(), wanted.clone()),
                    (probe(), wanted.clone()),
                ],
                ctx,
            );
        });
    });

    app.read(|ctx| {
        let model = model.as_ref(ctx);
        assert_eq!(model.downloading(), Some(&probe()));
        assert_eq!(model.queued(), vec![other, third]);
    });
}

#[test]
fn a_module_somebody_looked_inside_is_not_fetched_again_to_install() {
    // The whole point of keeping the bytes: seeing the pictures is
    // downloading the module, and a person who then presses Install has
    // already paid for it. The fetcher is asked once, and the install lands
    // without the download slot ever being taken.
    let (queue, mut app) = app();
    let module = wasm_carrying(
        &manifest("eugen/probe"),
        "header.right",
        10,
        &[
            ("crook.preview.1", &preview_png(640, 128)),
            ("crook.caption.1", b"The chip in the header"),
            ("crook.preview.2", &preview_png(400, 300)),
        ],
    );
    let asked = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let fetching = answering(module.clone(), asked.clone());
    let model = app.update(|ctx| {
        ctx.add_model(|_| {
            let mut model = StoreModel::new(None);
            model.fetch_with(fetching);
            model
        })
    });
    let offered = release("1.0.0", &module);

    app.update(|ctx| {
        model.update(ctx, |model, ctx| {
            model.look_inside(&probe(), Some(&offered), None, ctx);
        });
    });
    app.read(|ctx| {
        assert_eq!(model.as_ref(ctx).looking_inside(), Some(&probe()));
    });
    deliver(&queue, &app, "the pictures never landed", |app| {
        app.read(|ctx| model.as_ref(ctx).looked_inside().is_some())
    });

    app.read(|ctx| {
        let model = model.as_ref(ctx);
        assert_eq!(model.looking_inside(), None);
        let looked = model.looked_inside().expect("the pictures landed");
        assert_eq!(looked.plugin, probe());
        assert_eq!(looked.bytes, module, "the module is kept");
        assert_eq!(looked.pictures.len(), 2);
        assert_eq!(
            (looked.pictures[0].width, looked.pictures[0].height),
            (640, 128)
        );
        assert_eq!(
            looked.pictures[0].caption.as_deref(),
            Some("The chip in the header")
        );
        assert_eq!(looked.pictures[1].caption, None);
        assert_eq!(model.problem(), None);
    });
    assert_eq!(asked.load(std::sync::atomic::Ordering::SeqCst), 1);

    // A second look at the same module draws the pictures again from the
    // bytes held, and asks the network nothing.
    app.update(|ctx| {
        model.update(ctx, |model, ctx| {
            model.look_inside(&probe(), Some(&offered), None, ctx);
        });
    });
    deliver(&queue, &app, "the second look never landed", |app| {
        app.read(|ctx| model.as_ref(ctx).looking_inside().is_none())
    });
    assert_eq!(asked.load(std::sync::atomic::Ordering::SeqCst), 1);

    app.update(|ctx| {
        model.update(ctx, |model, ctx| {
            model.download(&probe(), &offered, ctx);
            assert_eq!(model.downloading(), None, "nothing to download");
            let landed = model.landed();
            assert_eq!(landed.len(), 1);
            assert_eq!(landed[0].2.as_deref(), Ok(module.as_slice()));
        });
    });
    assert_eq!(asked.load(std::sync::atomic::Ordering::SeqCst), 1);

    // And a different release of the same plugin is a different module,
    // which the held bytes are no answer to.
    let mut newer = offered.clone();
    newer.sha256 = fetch::sha256_hex(b"something else");
    app.update(|ctx| {
        model.update(ctx, |model, ctx| {
            model.download(&probe(), &newer, ctx);
            assert_eq!(model.downloading(), Some(&probe()));
        });
    });
}

#[test]
fn a_look_inside_that_brings_nothing_says_so_the_way_a_download_does() {
    // The same GET a download is, refused the same way: the sentence is
    // the one the observer lands for a module that did not arrive, about
    // the plugin it was asked for, and the look is over.
    let (queue, mut app) = app();
    let model = app.update(|ctx| {
        ctx.add_model(|_| {
            let mut model = StoreModel::new(None);
            model.fetch_with(Arc::new(|_| Err(String::from("connection refused"))));
            model
        })
    });
    let offered = release("1.0.0", b"never fetched");

    app.update(|ctx| {
        model.update(ctx, |model, ctx| {
            model.look_inside(&probe(), Some(&offered), None, ctx);
        });
    });
    deliver(&queue, &app, "the refusal never landed", |app| {
        app.read(|ctx| model.as_ref(ctx).looking_inside().is_none())
    });

    app.read(|ctx| {
        let model = model.as_ref(ctx);
        assert_eq!(
            model.problem(),
            Some((Some(&probe()), "did not arrive: connection refused"))
        );
        assert!(model.looked_inside().is_none());
    });
}

#[test]
fn the_pictures_of_the_module_already_here_need_no_fetch() {
    // An installed plugin's previews come out of the module on this machine:
    // the fetcher is never asked, and no bytes are kept, since there is
    // nothing Install could reuse.
    let (queue, mut app) = app();
    let asked = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let fetching = answering(Vec::new(), asked.clone());
    let model = app.update(|ctx| {
        ctx.add_model(|_| {
            let mut model = StoreModel::new(None);
            model.fetch_with(fetching);
            model
        })
    });
    let installed = vec![Preview {
        png: Arc::new(preview_png(200, 100)),
        width: 200,
        height: 100,
        caption: Some(String::from("A caption")),
    }];

    app.update(|ctx| {
        model.update(ctx, |model, ctx| {
            model.look_inside(&probe(), None, Some(installed), ctx);
        });
    });
    deliver(&queue, &app, "the pictures never landed", |app| {
        app.read(|ctx| model.as_ref(ctx).looked_inside().is_some())
    });

    app.read(|ctx| {
        let looked = model.as_ref(ctx).looked_inside().expect("landed");
        assert!(looked.bytes.is_empty());
        assert_eq!(looked.pictures.len(), 1);
        assert_eq!(looked.pictures[0].caption.as_deref(), Some("A caption"));
    });
    assert_eq!(asked.load(std::sync::atomic::Ordering::SeqCst), 0);

    // Nothing to look inside is a sentence on the card, not a task.
    app.update(|ctx| {
        model.update(ctx, |model, ctx| {
            model.look_inside(&probe(), None, None, ctx);
            assert_eq!(model.looking_inside(), None);
            assert!(
                model
                    .problem()
                    .is_some_and(|(about, _)| about == Some(&probe()))
            );
        });
    });
}

#[test]
fn the_icons_the_list_carries_are_decoded_on_the_pool_and_not_before() {
    // A face beside every name costs the one request the list already is —
    // and none of the foreground's time: `new` decodes nothing, and the
    // pictures land after the pool has run.
    let scratch = Scratch::new("store-icons");
    let cache = Cache::at(scratch.path());
    let icon = base64::engine::general_purpose::STANDARD.encode(icon_png(32));
    let index = format!(
        r#"{{"schema": 1, "plugins": [
            {{"id": "eugen/probe", "name": "Probe", "description": "d", "icon": "{icon}",
             "versions": []}},
            {{"id": "eugen/faceless", "name": "Faceless", "description": "d", "versions": []}},
            {{"id": "eugen/broken", "name": "Broken", "description": "d", "icon": "bm90IGEgcG5n",
             "versions": []}}]}}"#
    );
    cache.write(index.as_bytes(), None).expect("it should keep");

    let (queue, mut app) = app();
    let model = app.update(|ctx| ctx.add_model(|_| StoreModel::new(Some(cache))));
    app.read(|ctx| assert!(model.as_ref(ctx).icons().is_empty()));

    app.update(|ctx| {
        model.update(ctx, |model, ctx| model.remember_icons(ctx));
    });
    deliver(&queue, &app, "the icons never landed", |app| {
        app.read(|ctx| !model.as_ref(ctx).icons().is_empty())
    });

    app.read(|ctx| {
        let icons = model.as_ref(ctx).icons();
        let face = icons.get("eugen/probe").expect("the probe's face");
        assert_eq!(face.size(), (32, 32));
        assert!(!icons.contains_key("eugen/faceless"));
        // One that is not a PNG is a line in the log and no face, not a
        // list with a hole in it.
        assert!(!icons.contains_key("eugen/broken"));
    });
}

#[test]
fn a_face_is_held_small_and_a_list_of_faces_is_held_to_a_budget() {
    // What keeps a hostile list from costing a gigabyte for the session: a
    // face decoded at the largest size the rule allows is shrunk to the
    // largest size anything draws, a string past the registry's own cap is
    // never decoded, and past the budget the rest of the rows go faceless.
    let encoded = |png: Vec<u8>| base64::engine::general_purpose::STANDARD.encode(png);
    let large = encoded(icon_png(256));
    let small = encoded(icon_png(32));
    let too_long = "A".repeat(MOST_ICON_CHARS + 1);

    let icons = decoded_icons(
        &[
            (String::from("eugen/large"), large),
            (String::from("eugen/long"), too_long),
        ],
        ICONS_BUDGET,
    );
    assert_eq!(
        icons["eugen/large"].size(),
        (ICON_HELD_EDGE, ICON_HELD_EDGE),
        "a large face is held at the size it is drawn"
    );
    assert!(
        !icons.contains_key("eugen/long"),
        "a string past the cap was decoded"
    );

    // Five faces of four kilobytes each, and room for three.
    let listed: Vec<(String, String)> = (0..5)
        .map(|n| (format!("eugen/face-{n}"), small.clone()))
        .collect();
    let icons = decoded_icons(&listed, 3 * 32 * 32 * 4);
    assert_eq!(icons.len(), 3, "the budget was not the budget: {icons:?}");
    assert!(icons.contains_key("eugen/face-0"));
    assert!(icons.contains_key("eugen/face-2"));
    assert!(!icons.contains_key("eugen/face-3"));
}

#[test]
fn a_module_whose_picture_is_past_the_rule_is_still_the_module_install_would_fetch() {
    // The reader refuses a non-square icon, and the look must not throw the
    // module away with the picture: the bytes were fetched and checked, the
    // card promised Install would need no second download, and the host
    // runs such a module faceless rather than refusing it. So the look
    // lands with no pictures, and the download after it asks the network
    // nothing.
    let (queue, mut app) = app();
    let module = wasm_carrying(
        &manifest("eugen/probe"),
        "header.right",
        10,
        &[
            ("crook.icon", &preview_png(300, 200)),
            ("crook.preview.1", &preview_png(400, 300)),
        ],
    );
    let asked = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let model = app.update(|ctx| {
        ctx.add_model(|_| {
            let mut model = StoreModel::new(None);
            model.fetch_with(answering(module.clone(), asked.clone()));
            model
        })
    });
    let offered = release("1.0.0", &module);

    app.update(|ctx| {
        model.update(ctx, |model, ctx| {
            model.look_inside(&probe(), Some(&offered), None, ctx);
        });
    });
    deliver(&queue, &app, "the look never landed", |app| {
        app.read(|ctx| model.as_ref(ctx).looking_inside().is_none())
    });

    app.read(|ctx| {
        let model = model.as_ref(ctx);
        assert_eq!(
            model.problem(),
            None,
            "a picture past the rule is not a failure"
        );
        let looked = model.looked_inside().expect("the module landed");
        assert_eq!(
            looked.bytes, module,
            "the module was thrown away with its picture"
        );
        assert!(looked.pictures.is_empty());
    });
    app.update(|ctx| {
        model.update(ctx, |model, ctx| {
            model.download(&probe(), &offered, ctx);
            assert_eq!(model.downloading(), None, "the module was fetched again");
            assert_eq!(model.landed().len(), 1);
        });
    });
    assert_eq!(asked.load(std::sync::atomic::Ordering::SeqCst), 1);
}
