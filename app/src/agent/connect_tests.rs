use std::ffi::OsStr;
use std::path::{Path, PathBuf};

use super::*;

/// What `claude plugin list --json` printed, Claude Code 2.1.280, with the
/// plugin installed from a marketplace that is a git checkout.
const CLAUDE_LISTED: &str = r#"[
  {
    "id": "crook@crook",
    "version": "6b2a55f742d2",
    "scope": "user",
    "enabled": true,
    "installPath": "/home/someone/.claude/plugins/cache/crook/crook/6b2a55f742d2",
    "mcpServers": { "crook": { "type": "stdio", "command": "sh" } }
  }
]"#;

/// What `codex plugin list --json` printed, codex-cli 0.149.1.
const CODEX_LISTED: &str = r#"{
  "installed": [
    {
      "pluginId": "crook@crook",
      "name": "crook",
      "marketplaceName": "crook",
      "version": "local",
      "installed": true,
      "enabled": true
    }
  ],
  "available": []
}"#;

/// What `copilot plugin list --json` printed, Copilot CLI 1.0.89.
const COPILOT_LISTED: &str = r#"[
  { "name": "crook", "marketplace": "crook", "enabled": true, "source": "live" }
]"#;

#[test]
fn each_agents_list_is_read_by_the_shape_it_prints() {
    assert_eq!(
        read_list(CLAUDE_LISTED),
        Some(Listed::Installed {
            version: Some("6b2a55f742d2".into()),
            enabled: true
        })
    );
    assert_eq!(
        read_list(CODEX_LISTED),
        Some(Listed::Installed {
            version: Some("local".into()),
            enabled: true
        })
    );
    assert_eq!(
        read_list(COPILOT_LISTED),
        Some(Listed::Installed {
            version: None,
            enabled: true
        })
    );
}

#[test]
fn an_empty_list_another_plugin_or_an_offered_one_is_missing() {
    assert_eq!(read_list("[]"), Some(Listed::Missing));
    assert_eq!(
        read_list(r#"{ "installed": [], "available": [] }"#),
        Some(Listed::Missing)
    );
    assert_eq!(
        read_list(r#"[{ "id": "crook@someone-else", "enabled": true }]"#),
        Some(Listed::Missing)
    );
    assert_eq!(
        read_list(r#"{ "installed": [{ "pluginId": "crook@crook", "installed": false }] }"#),
        Some(Listed::Missing)
    );
    assert_eq!(
        read_list(r#"[{ "id": "crook@crook", "enabled": false }]"#),
        Some(Listed::Installed {
            version: None,
            enabled: false
        })
    );
}

#[test]
fn what_is_not_a_list_is_not_read_as_one() {
    for said in ["", "No plugins installed.", r#"{ "plugins": [] }"#, "42"] {
        assert_eq!(read_list(said), None, "{said:?}");
    }
}

#[test]
fn a_plugin_older_than_this_crook_or_named_by_a_commit_is_behind() {
    let installed = |version: &str| Listed::Installed {
        version: Some(version.into()),
        enabled: true,
    };
    assert_eq!(
        standing(&installed("0.1.30"), "0.1.39"),
        Standing::Behind {
            version: "0.1.30".into()
        }
    );
    assert_eq!(
        standing(&installed("6b2a55f742d2"), "0.1.39"),
        Standing::Behind {
            version: "6b2a55f742d2".into()
        }
    );
    for current in ["0.1.39", "0.1.40", "v0.2.0"] {
        assert_eq!(
            standing(&installed(current), "0.1.39"),
            Standing::Connected {
                version: Some(current.into())
            },
            "{current}"
        );
    }
    // Loaded in place from a checkout, which is as new as the checkout.
    for in_place in ["local", "unknown"] {
        assert!(
            matches!(
                standing(&installed(in_place), "0.1.39"),
                Standing::Connected { .. }
            ),
            "{in_place}"
        );
    }
    assert_eq!(standing(&Listed::Missing, "0.1.39"), Standing::Missing);
    assert_eq!(
        standing(
            &Listed::Installed {
                version: Some("0.1.39".into()),
                enabled: false
            },
            "0.1.39"
        ),
        Standing::Off
    );
}

#[test]
fn a_line_runs_every_command_and_names_the_program_as_found() {
    let bash = OsStr::new("/bin/bash");
    assert_eq!(
        line("claude", Step::Connect, "claude", bash).as_deref(),
        Some("claude plugin marketplace add theguriev/crook; claude plugin install crook@crook")
    );
    assert_eq!(
        line("claude", Step::Update, "/opt/agents/claude", bash).as_deref(),
        Some(
            "/opt/agents/claude plugin marketplace update crook; \
             /opt/agents/claude plugin update crook@crook"
        )
    );
    assert_eq!(
        line(
            "copilot",
            Step::Connect,
            "copilot",
            OsStr::new(r"C:\Windows\System32\cmd.exe")
        )
        .as_deref(),
        Some("copilot plugin marketplace add theguriev/crook & copilot plugin install crook@crook")
    );
    assert_eq!(line("gemini", Step::Connect, "gemini", bash), None);
    assert_eq!(line("aider", Step::Update, "aider", bash), None);
}

#[test]
fn every_agent_with_a_plugin_can_be_brought_up_to_date() {
    for agent in AGENTS {
        assert_eq!(
            agent.plugin.is_empty(),
            agent.plugin_update.is_empty(),
            "{}",
            agent.name
        );
        for command in agent.plugin.iter().chain(agent.plugin_update) {
            assert!(
                command.starts_with(&format!("{} plugin ", agent.name)),
                "{command}"
            );
        }
    }
    assert_eq!(
        connectable().collect::<Vec<_>>(),
        ["claude", "codex", "copilot"]
    );
}

/// The manifests every agent reads Crook's plugin by, and the version each
/// says, which `script/release` sets.
fn manifests() -> Vec<PathBuf> {
    let plugin = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("the workspace root")
        .join("packaging")
        .join("claude-code");
    vec![
        plugin.join(".claude-plugin").join("plugin.json"),
        plugin.join(".codex-plugin").join("plugin.json"),
        plugin.join(".github").join("plugin").join("plugin.json"),
    ]
}

#[test]
fn every_plugin_manifest_says_this_crooks_version() {
    // Claude Code names the copy it installs by this, and updates it only
    // when it changes: a manifest left behind would keep every person on the
    // copy they have, and the Agents page would call it current.
    for manifest in manifests() {
        let text = std::fs::read_to_string(&manifest).expect("the manifest is there");
        let value: serde_json::Value = serde_json::from_str(&text).expect("the manifest is JSON");
        assert_eq!(
            value.get("version").and_then(serde_json::Value::as_str),
            Some(env!("CARGO_PKG_VERSION")),
            "{}",
            manifest.display()
        );
    }
}

#[test]
fn claude_says_it_updates_by_itself_only_where_its_files_turn_that_on() {
    let scratch = Scratch::new("auto-update");
    let config = scratch.path();
    assert!(!claude_updates_by_itself(config), "with no files at all");

    std::fs::create_dir_all(config.join("plugins")).unwrap();
    std::fs::write(
        config.join("plugins").join("known_marketplaces.json"),
        r#"{ "crook": { "source": { "source": "github" }, "autoUpdate": true } }"#,
    )
    .unwrap();
    assert!(
        claude_updates_by_itself(config),
        "the /plugin toggle's file"
    );

    // A settings entry comes first, as it does for Claude Code.
    std::fs::write(
        config.join("settings.json"),
        r#"{ "extraKnownMarketplaces": { "crook": { "autoUpdate": false } } }"#,
    )
    .unwrap();
    assert!(
        !claude_updates_by_itself(config),
        "the settings entry said no"
    );
}

#[test]
fn the_auto_update_entry_crook_prints_is_json_claude_reads() {
    let value: serde_json::Value =
        serde_json::from_str(&format!("{{ {CLAUDE_AUTO_UPDATE} }}")).expect("it parses");
    assert_eq!(
        value.pointer("/extraKnownMarketplaces/crook/autoUpdate"),
        Some(&serde_json::Value::Bool(true))
    );
    assert_eq!(
        value.pointer("/extraKnownMarketplaces/crook/source/repo"),
        Some(&serde_json::Value::from("theguriev/crook"))
    );
}

#[cfg(unix)]
#[test]
fn the_look_runs_the_agents_own_list_and_reads_it() {
    let scratch = Scratch::new("look");
    let agent = scratch.path().join("claude");
    let asked = scratch.path().join("asked");
    crate::process::write_program(
        &agent,
        &format!(
            "#!/bin/sh\nprintf '%s ' \"$@\" > '{}'\nprintf '%s' \"$LISTED\"\n",
            asked.display()
        ),
    );

    let listed = look(
        &agent,
        &[(String::from("LISTED"), CLAUDE_LISTED.to_owned())],
    );
    assert_eq!(
        listed,
        Ok(Listed::Installed {
            version: Some("6b2a55f742d2".into()),
            enabled: true
        })
    );
    assert_eq!(
        std::fs::read_to_string(&asked).unwrap(),
        "plugin list --json "
    );

    let unreadable = look(&agent, &[(String::from("LISTED"), String::from("nope"))]);
    assert!(unreadable.is_err(), "{unreadable:?}");
}

#[cfg(unix)]
#[test]
fn a_list_that_fails_is_no_answer() {
    let scratch = Scratch::new("look-fails");
    let agent = scratch.path().join("claude");
    crate::process::write_program(&agent, "#!/bin/sh\necho '[]'\nexit 3\n");
    assert_eq!(
        look(&agent, &[]),
        Err(String::from("its plugin list ended with status 3"))
    );
}

#[cfg(unix)]
#[test]
fn an_agent_is_typed_by_its_name_on_path_and_by_its_path_elsewhere() {
    let scratch = Scratch::new("located");
    let on_path = scratch.path().join("bin");
    let installer = scratch.path().join("installer");
    std::fs::create_dir_all(&on_path).unwrap();
    std::fs::create_dir_all(&installer).unwrap();
    crate::process::write_program(&on_path.join("claude"), "#!/bin/sh\n");
    crate::process::write_program(&installer.join("copilot"), "#!/bin/sh\n");
    // Found, and with no plugin to connect.
    crate::process::write_program(&on_path.join("gemini"), "#!/bin/sh\n");

    let found = located(&SearchPath {
        by_name: vec![on_path.clone()],
        installers: vec![installer.clone()],
    });
    assert_eq!(
        found,
        [
            Located {
                agent: "claude",
                file: on_path.join("claude"),
                typed: "claude".into(),
            },
            Located {
                agent: "copilot",
                file: installer.join("copilot"),
                typed: installer.join("copilot").to_string_lossy().into_owned(),
            },
        ]
    );
}

/// A directory of the test's own, removed when it goes.
struct Scratch(PathBuf);

impl Scratch {
    fn new(label: &str) -> Self {
        static NEXT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        let path = std::env::temp_dir().join(format!(
            "crook-connect-{label}-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).expect("the temporary directory is writable");
        Self(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
