//! Dimension 8, measured rather than asserted: a plugin's hook is observed
//! NOT to run.
//!
//! `d02528d3` built `pluginconf` and the `enabledPlugins` block from a table
//! measured by hand at a terminal. Nothing committed observed any of it, and
//! the trap this repository keeps falling into is exactly that shape — a grep
//! for a key in a document nobody executes, and the criterion reads as met.
//! `mcp_launch_live.rs` is the same argument for dimension 7 and says it at
//! length; this is its counterpart for the content dimension 7 cannot reach.
//!
//! A plugin's `hooks/hooks.json` is spawned by the agent itself before the
//! session has done anything, and no MCP configuration of any kind is
//! involved. So the proof is a real enabled plugin whose `SessionStart` hook
//! appends to a file, started through the document APEX actually writes:
//!
//!   * `pluginconf::read` over a fixture HOME,
//!   * `pluginconf::curate` at the policy under test,
//!   * `hook::settings_json`, which is the document `apex-agentd` hands the
//!     agent, hooks and all, and
//!   * the adapter's own `--settings` argv.
//!
//! Not a hand-written settings file. That would measure Claude's flag, which
//! is not in doubt; what is in doubt is whether APEX's document uses it.
//!
//! ## The positive runs are what make the negative ones a measurement
//!
//! Every policy is run, not only the removing ones. `as_configured` HAS to
//! leave the mark, and `curated` with the plugin's own name on the list has to
//! leave it too. Without those, a fixture with a broken marketplace, an
//! unregistered plugin or a sentinel script that never ran would report a
//! removal that APEX had nothing to do with — and the fixture spent an
//! afternoon in exactly that state: a plugin can be installed and enabled and
//! still not load, because its marketplace is not in `known_marketplaces.json`.
//!
//! The positive runs also calibrate [`WINDOW`]. A negative run that simply did
//! not wait long enough is indistinguishable from a removal, so the window is
//! only credible because the positive runs produce their mark inside it — in
//! the same test, on the same machine, in the same second.
//!
//! ## Why `#[ignore]`, which is not the same as skipped
//!
//! This drives the real `claude`, and `claude` is not installed on the CI
//! runners — `mcp_launch_live.rs` can refuse to skip because `bwrap` IS
//! installed there, and refusing here would only turn CI red for something it
//! cannot measure. So it is opt-in (`cargo test -p apex --test
//! plugin_removal_live -- --ignored`) and it FAILS, loudly, if `claude` is
//! missing when it is asked to run. `tests/test-apex-skill.sh --with-binary`
//! runs it when the binary is there and prints a could-not-run line when it is
//! not, so the difference between "held" and "never checked" stays visible.
//!
//! Nothing here touches the real home: `env_clear`, a fixture `HOME`, a
//! fixture `XDG_RUNTIME_DIR` so the hook bridge cannot reach the live
//! `apex-agentd`, and `ANTHROPIC_BASE_URL` on a dead local port so no request
//! leaves the machine.

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use apex_agent_core::hook;
use apex_agent_core::pluginconf;
use apex_agent_core::policy::PluginPolicy;

/// The `apex` this test drives: the build under test, not whatever is on PATH.
const APEX: &str = env!("CARGO_BIN_EXE_apex");

/// The plugin's key, in the `name@marketplace` spelling `enabledPlugins` uses.
const PLUGIN: &str = "sentinel@apex-test-market";

/// How long a run is given to produce its mark before it is killed.
///
/// A session started against a dead endpoint never exits on its own — it
/// retries until something stops it — so every run here is killed rather than
/// waited for. `SessionStart` fires long before the first request, which is
/// the whole reason this dimension exists, and the positive runs prove the
/// window is long enough on the machine the test is actually on.
const WINDOW: Duration = Duration::from_secs(30);

/// A fixture root under `/var/tmp`, unique per run.
fn fixture() -> PathBuf {
    let dir = PathBuf::from("/var/tmp").join(format!(
        "apex-plugin-removal-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    std::fs::create_dir_all(&dir).expect("fixture root");
    dir
}

fn write(path: &Path, body: &str) {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).expect("fixture directory");
    }
    std::fs::write(path, body).unwrap_or_else(|e| panic!("writing {}: {e}", path.display()));
}

/// A home with one enabled plugin whose `SessionStart` hook appends to a file.
///
/// Every file here is one Claude actually reads, and the set is not
/// negotiable: `settings.json` enables the plugin, `installed_plugins.json`
/// says where the live copy is (a version-2 ARRAY, which is what a current
/// install writes), and `known_marketplaces.json` plus the checkout's
/// `.claude-plugin/marketplace.json` are what make it load at all. Leave the
/// marketplace out and the plugin is enabled, installed, and silently inert —
/// which would have read here as a removal.
fn build_home(root: &Path) -> (PathBuf, PathBuf) {
    let home = root.join("home");
    let sentinel = root.join("sentinel.log");
    let install = home
        .join(".claude/plugins/cache/apex-test-market/sentinel/1.0.0");
    let market = home.join(".claude/plugins/marketplaces/apex-test-market");

    write(
        &install.join(".claude-plugin/plugin.json"),
        r#"{"name":"sentinel","version":"1.0.0","description":"leaves a mark when it is loaded"}"#,
    );
    let script = install.join("hooks/sentinel.sh");
    write(
        &script,
        &format!("#!/bin/sh\nprintf 'ran\\n' >> {:?}\nexit 0\n", sentinel),
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755))
            .expect("the hook script has to be executable");
    }
    write(
        &install.join("hooks/hooks.json"),
        &format!(
            r#"{{"hooks":{{"SessionStart":[{{"hooks":[{{"type":"command","command":{:?},"timeout":10}}]}}]}}}}"#,
            script.display().to_string()
        ),
    );

    // The marketplace checkout. Claude resolves the plugin through this, so a
    // fixture without it measures nothing.
    write(
        &market.join(".claude-plugin/marketplace.json"),
        r#"{"name":"apex-test-market","owner":{"name":"APEX test fixture"},
            "metadata":{"description":"exists only inside this test","version":"1.0.0"},
            "plugins":[{"name":"sentinel","description":"leaves a mark when it is loaded",
                        "version":"1.0.0","source":"./plugins/sentinel"}]}"#,
    );
    write(
        &market.join("plugins/sentinel/.claude-plugin/plugin.json"),
        r#"{"name":"sentinel","version":"1.0.0","description":"leaves a mark when it is loaded"}"#,
    );

    write(
        &home.join(".claude/settings.json"),
        &format!(r#"{{"enabledPlugins":{{"{PLUGIN}":true}}}}"#),
    );
    write(
        &home.join(".claude/plugins/installed_plugins.json"),
        &format!(
            r#"{{"version":2,"plugins":{{"{PLUGIN}":[{{"scope":"user","version":"1.0.0","installPath":{:?}}}]}}}}"#,
            install.display().to_string()
        ),
    );
    write(
        &home.join(".claude/plugins/known_marketplaces.json"),
        &format!(
            r#"{{"apex-test-market":{{"source":{{"source":"local","path":{0:?}}},"installLocation":{0:?}}}}}"#,
            market.display().to_string()
        ),
    );
    // Onboarding, so the run reaches a session rather than a prompt.
    write(
        &home.join(".claude.json"),
        r#"{"hasCompletedOnboarding":true,"numStartups":5,"theme":"dark"}"#,
    );

    (home, sentinel)
}

/// Start one session and give it [`WINDOW`] to leave its mark.
///
/// Returns whether the mark appeared. The child is killed either way: it is
/// talking to a dead port and will never stop on its own.
fn run_session(root: &Path, home: &Path, sentinel: &Path, settings: Option<&Path>) -> bool {
    let _ = std::fs::remove_file(sentinel);
    let cwd = root.join("proj");
    std::fs::create_dir_all(&cwd).expect("project directory");

    let mut cmd = Command::new("claude");
    cmd.current_dir(&cwd)
        .arg("-p")
        .arg("say nothing");
    if let Some(path) = settings {
        // The adapter's own argv, not a hand-rolled `--settings`: if the
        // daemon ever stops passing the document, this stops passing it too.
        let adapter = apex_agent_core::adapter::by_id("claude")
            .expect("the claude adapter is the one this dimension applies to");
        cmd.args(adapter.hook_settings_args(path));
    }
    cmd.env_clear()
        .env("HOME", home)
        .env("PATH", std::env::var("PATH").unwrap_or_default())
        // The hook bridge in the document runs the real `apex`. Pointing the
        // runtime directory at the fixture means it cannot find — and so
        // cannot talk to — the live `apex-agentd` on this machine.
        .env("XDG_RUNTIME_DIR", root.join("run"))
        // Nothing leaves the machine: port 1 has nothing on it.
        .env("ANTHROPIC_BASE_URL", "http://127.0.0.1:1")
        .env("ANTHROPIC_API_KEY", "sk-ant-this-key-is-not-real")
        .env("TERM", "dumb")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());

    let mut child: Child = cmd.spawn().unwrap_or_else(|e| {
        panic!(
            "could not start `claude` ({e}). This test MEASURES a plugin hook \
             not running; with no agent to start it measures nothing, and a \
             silent pass here is the failure mode it exists to prevent. \
             Install Claude Code or do not ask for --ignored tests."
        )
    });

    let deadline = Instant::now() + WINDOW;
    let mut seen = false;
    while Instant::now() < deadline {
        if sentinel.exists() {
            seen = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    let _ = child.kill();
    let _ = child.wait();
    seen
}

/// The document `apex-agentd` writes for one policy, through the real code.
fn settings_for(root: &Path, home: &Path, policy: PluginPolicy, allow: &[String]) -> Option<PathBuf> {
    let installed = pluginconf::read(home);
    assert_eq!(
        installed.len(),
        1,
        "the fixture home should hold exactly one enabled plugin, not {installed:?}"
    );
    assert!(
        installed[0].content.hooks,
        "the fixture plugin has to SHIP a hook, or nothing here is being measured: {:?}",
        installed[0]
    );
    assert!(
        installed[0].content.runs_outside_mcp(),
        "a hook is content no MCP confinement reaches; that is the premise"
    );

    let curated = pluginconf::curate(&installed, policy, allow);
    let document = hook::settings_json(Path::new(APEX), None, curated.as_ref());
    let path = root.join(format!("settings-{}.json", policy.as_str()));
    write(&path, &document.to_string());
    Some(path)
}

#[test]
#[ignore = "drives the real `claude`, which CI does not install; run with --ignored"]
fn a_removed_plugins_hook_is_observed_not_to_run() {
    let root = fixture();
    let (home, sentinel) = build_home(&root);

    // CONTROL, and the most important run in the file. No document at all:
    // this is a session exactly as the machine would start it, and the hook
    // has to fire. If it does not, every removal below is meaningless.
    assert!(
        run_session(&root, &home, &sentinel, None),
        "the fixture plugin's SessionStart hook did not run even with NOTHING \
         removing it, so this fixture cannot measure a removal. Check that the \
         marketplace is registered in known_marketplaces.json — an enabled, \
         installed plugin whose marketplace is unknown loads silently as \
         nothing, which reads here exactly like a removal."
    );

    // The default policy removes nothing and `curate` returns None, so the
    // document carries no `enabledPlugins` key at all. The hook still runs —
    // which is the statement that APEX's own document is not what stops it.
    let as_configured = settings_for(&root, &home, PluginPolicy::AsConfigured, &[]);
    assert!(
        !std::fs::read_to_string(as_configured.as_ref().expect("a document"))
            .expect("the document")
            .contains("enabledPlugins"),
        "as_configured must not write the key at all; replacing the user's own \
         object for no reason is the behaviour that breaks first"
    );
    assert!(
        run_session(&root, &home, &sentinel, as_configured.as_deref()),
        "APEX's own settings document stopped a plugin the policy KEPT"
    );

    // `curated` with the plugin's own name: kept, and observed kept. This is
    // the run that separates "the policy removed it" from "the --settings flag
    // breaks plugins".
    let kept = settings_for(
        &root,
        &home,
        PluginPolicy::Curated,
        &[PLUGIN.to_string()],
    );
    let kept_doc = std::fs::read_to_string(kept.as_ref().expect("a document")).expect("the doc");
    assert!(
        kept_doc.contains(&format!("\"{PLUGIN}\":true")),
        "a curated keep has to be named TRUE, not omitted: {kept_doc}"
    );
    assert!(
        run_session(&root, &home, &sentinel, kept.as_deref()),
        "a plugin on the allowlist did not run, so the document removes what it \
         was told to keep"
    );

    // And the two removals. These are the assertions the file exists for.
    let none = settings_for(&root, &home, PluginPolicy::NoPlugins, &[]);
    assert!(
        !run_session(&root, &home, &sentinel, none.as_deref()),
        "`--plugins none` left the plugin's SessionStart hook RUNNING. The \
         hook is code no MCP confinement reaches, so a dimension that does not \
         remove it removes nothing."
    );

    let other = settings_for(
        &root,
        &home,
        PluginPolicy::Curated,
        &["something-else@elsewhere".to_string()],
    );
    assert!(
        !run_session(&root, &home, &sentinel, other.as_deref()),
        "`--plugins curated` kept a plugin that is not on the allowlist"
    );

    let _ = std::fs::remove_dir_all(&root);
}
