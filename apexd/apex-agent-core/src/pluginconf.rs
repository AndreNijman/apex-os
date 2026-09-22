//! Which plugins a session is started with, rather than the ones it finds —
//! P1-026's second criterion, for the half `mcpconf` cannot reach.
//!
//! [`crate::mcpconf`] confines the executable content a plugin declares in its
//! `.mcp.json`: `--strict-mcp-config` drops the plugin's own definition and the
//! curated document starts the same program through `apex mcp run`, so
//! bubblewrap holds it. That is one of the three kinds of code a plugin ships.
//! The other two are its **hooks** and the **scripts** those hooks run, and
//! neither goes through MCP at all — Claude spawns a hook itself, from the
//! plugin's `hooks/hooks.json`, before the session has done anything.
//!
//! So a session could have every plugin MCP server wrapped and still run a
//! plugin's `SessionStart` command unconfined by anything APEX added. This
//! module is the answer to that, and the answer is **removal, not
//! confinement**: APEX does not own the agent's process, so it cannot put a
//! sandbox around a hook the agent spawns. It can decide which plugins are
//! loaded at all.
//!
//! ## What was measured before any of this was designed
//!
//! Claude Code 2.1.260, on this machine, 2026-09-12. Fixture `HOME`,
//! `ANTHROPIC_BASE_URL` pointed at a dead local port so nothing left the
//! machine, a real enabled plugin installed from a real marketplace checkout,
//! and one sentinel per observable that appends its name to a log when it runs.
//! Four observables per run: the **plugin's** `SessionStart` hook, the
//! **plugin's** MCP server, **APEX's own** `--settings` hook (§6.1's state
//! bridge), and the server in the curated `--mcp-config` document.
//!
//! | started with | plugin hook | plugin MCP | APEX hook | curated MCP |
//! |---|---|---|---|---|
//! | nothing (control) | ran | ran | ran | — |
//! | `--strict-mcp-config --mcp-config` | **ran** | dropped | ran | ran |
//! | `--safe-mode` | dropped | dropped | **dropped** | **dropped** |
//! | `--bare` | dropped | dropped | **dropped** | ran |
//! | `--settings {"disableAllHooks":true}` | dropped | dropped | **dropped** | ran |
//! | `--settings {"enabledPlugins":{"p":false}}` | dropped | dropped | ran | ran |
//!
//! Three things in that table decided the design, and none of them could be
//! taken from the flags' help text:
//!
//! 1. **Row two is the shortfall, measured.** Round 1's curated document was
//!    already in place and the plugin's `SessionStart` hook still ran. The gap
//!    P1-026 had left open was not a reasoned possibility; it is an observed
//!    process.
//! 2. **`--safe-mode` and `--bare` both take APEX's own hooks with them**, and
//!    `--safe-mode` takes the curated MCP document too. A hardened profile
//!    built on either would silently turn off §6.1's session-state bridge and,
//!    for `--safe-mode`, the very confinement round 1 built. `--bare` also
//!    sets `CLAUDE_CODE_SIMPLE=1`, drops `CLAUDE.md` discovery and restricts
//!    authentication to `ANTHROPIC_API_KEY`: a different product, not a
//!    hardening switch.
//! 3. **`enabledPlugins` in the `--settings` file is surgical.** It is the one
//!    mechanism that removes the plugin's code and leaves both of APEX's own
//!    instruments running. It is also a file APEX already writes and already
//!    passes — [`crate::hook::settings_json`] — so this costs no new argument
//!    and no new flag on the agent's command line.
//!
//! ## Why the document names every plugin, including the kept ones
//!
//! `enabledPlugins` is an object key, and `--settings` outranks the user's own
//! settings for object keys rather than merging into them —
//! [`crate::hook::settings_json`]'s `statusLine` comment records the same fact
//! from the other side. Relying on that replacement to remove a plugin would
//! make the report depend on a merge rule, so [`curate`] writes the **full
//! map**: every plugin this machine has enabled, each with an explicit `true`
//! or `false`. The removal is then true under either merge rule, and
//! [`Curated::removed`] is a list of names APEX can print rather than a claim
//! about precedence.
//!
//! ## What this cannot do, stated rather than implied
//!
//! The same limit [`crate::mcpconf`] has, and it has to keep being said: **this
//! document only exists on the path that writes it.** A person who types
//! `claude` in a terminal is not running through `apex agent`, no `--settings`
//! is passed, and every enabled plugin's hooks run. Removing a plugin from a
//! session is a property of APEX-started sessions, not of the machine. The verb
//! that changes the machine is `claude plugin disable`, and it is the user's to
//! type.
//!
//! And removal is not confinement. A kept plugin's hooks still run inside
//! whatever the SESSION is confined to — a real boundary under
//! `--sandbox project` or `strict`, and none at all under `unrestricted`.
//! [`PluginPolicy::AsConfigured`] is honest about that rather than quiet about
//! it: it is the default, it removes nothing, and the readouts say so.

use std::path::{Path, PathBuf};

use serde_json::{Map, Value};

use crate::policy::PluginPolicy;

/// The kinds of executable content one installed plugin ships.
///
/// Read off the tree rather than guessed from the manifest: a plugin's
/// `plugin.json` in Claude 2.1 carries a name, a version and a description and
/// says nothing about what is beside it, so a reader that trusted the manifest
/// would report no hooks for a plugin whose `hooks/hooks.json` is right there.
///
/// Separate booleans and not one `runs_code`, because the three are removed by
/// different things and a readout that merged them would be unable to say which
/// half of a plugin a curated MCP document had already dealt with.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Content {
    /// `hooks/hooks.json`: commands Claude spawns itself, outside the tool
    /// permission system and before the session has done anything.
    pub hooks: bool,
    /// `.mcp.json`: servers, which [`crate::mcpconf`] wraps rather than
    /// removes when the session keeps them.
    pub mcp: bool,
    /// Anything under `scripts/`. Reached by a hook, by a command or by a
    /// skill, so it is code on the machine whether or not the hooks file
    /// names it.
    pub scripts: bool,
    /// `commands/`, `agents/`, `skills/`: content that only runs when the
    /// agent chooses to run it, and therefore through the session's own tools
    /// and the session's own sandbox.
    pub prompts: bool,
}

impl Content {
    /// Read what is actually in the plugin's install directory.
    pub fn read(root: &Path) -> Content {
        Content {
            hooks: root.join("hooks/hooks.json").is_file(),
            mcp: root.join(".mcp.json").is_file(),
            scripts: root.join("scripts").is_dir(),
            prompts: ["commands", "agents", "skills"]
                .iter()
                .any(|d| root.join(d).is_dir()),
        }
    }

    /// Whether this plugin runs code that APEX's MCP confinement never sees.
    ///
    /// Hooks and the scripts beside them. Deliberately **not** `mcp`: a plugin
    /// whose only executable content is an MCP server is one
    /// [`crate::mcpconf::curate`] already wraps, and counting it here would
    /// make a hardened profile look necessary for a plugin already confined.
    pub fn runs_outside_mcp(&self) -> bool {
        self.hooks || self.scripts
    }
}

/// One plugin this machine has enabled, as the two files that decide it say.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Installed {
    /// `name@marketplace`, which is the key `enabledPlugins` uses and the
    /// spelling a person types. Kept verbatim for the reason
    /// [`crate::mcpconf`] keeps a server's: two spellings of one name is how a
    /// document APEX writes stops matching a policy a human wrote.
    pub name: String,
    /// Where the live copy is — `installed_plugins.json`'s `installPath`, not
    /// the marketplace checkout beside it. The same distinction
    /// [`crate::mcpconf`] measured for `.mcp.json`, and it applies to
    /// `hooks/hooks.json` for the same reason.
    pub path: Option<PathBuf>,
    /// What is in that directory.
    pub content: Content,
}

impl Installed {
    /// The marketplace half of `name@marketplace`, when there is one.
    pub fn marketplace(&self) -> Option<&str> {
        self.name.rsplit_once('@').map(|(_, m)| m)
    }
}

fn read_json(path: &Path) -> Option<Value> {
    serde_json::from_slice(&std::fs::read(path).ok()?).ok()
}

/// Every plugin `settings.json` has switched on, with what each one ships.
///
/// The enabled set comes from `~/.claude/settings.json`'s `enabledPlugins` and
/// the install path from `~/.claude/plugins/installed_plugins.json`, which is
/// the same pair [`crate::profile::doctor`] reads and deliberately not a second
/// walk of them.
///
/// A name switched on with no install entry keeps its row with `path: None`.
/// It is a plugin that does not load, and dropping it here would make
/// [`curate`] write a document that silently stopped naming it — the removal
/// would then read as APEX's doing rather than as a broken install.
pub fn read(home: &Path) -> Vec<Installed> {
    let root = home.join(".claude");
    let settings = read_json(&root.join("settings.json"));
    let installed = read_json(&root.join("plugins/installed_plugins.json"));

    let mut out: Vec<Installed> = enabled_names(settings.as_ref())
        .into_iter()
        .map(|name| {
            let path = installed
                .as_ref()
                .and_then(|d| d.get("plugins"))
                .and_then(|p| p.get(&name))
                .and_then(first_install_path);
            let content = path.as_deref().map(Content::read).unwrap_or_default();
            Installed {
                name,
                path,
                content,
            }
        })
        .collect();
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

/// The names `enabledPlugins` switches ON, in either shape Claude has written.
///
/// Claude 2.1 writes an object — `{"name@market": true}` — and older builds
/// wrote a list of strings. A `false` value is a name that is present and
/// switched off, so it is not one of these. The same rule
/// [`crate::profile`]'s `names_in` applies, restated here rather than shared,
/// because that one is private to a module whose subject is a different file.
fn enabled_names(settings: Option<&Value>) -> Vec<String> {
    match settings.and_then(|s| s.get("enabledPlugins")) {
        Some(Value::Object(map)) => map
            .iter()
            .filter(|(_, v)| *v != &Value::Bool(false))
            .map(|(k, _)| k.clone())
            .collect(),
        Some(Value::Array(items)) => items
            .iter()
            .filter_map(Value::as_str)
            .map(str::to_string)
            .collect(),
        _ => Vec::new(),
    }
}

/// The `installPath` of the first entry for one plugin.
///
/// `installed_plugins.json` version 2 keys each name to an ARRAY, because one
/// plugin can be installed at user scope and project scope at once. The first
/// entry is the one Claude loads; a reader that expected an object finds
/// nothing at all on a current install.
fn first_install_path(entries: &Value) -> Option<PathBuf> {
    let list = entries.as_array()?;
    let first = list.first()?;
    let dir = first.get("installPath").and_then(Value::as_str)?;
    Some(PathBuf::from(dir))
}

/// The `enabledPlugins` block a session is started with, and what it removed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Curated {
    /// The object that goes into the `--settings` document. Every enabled
    /// plugin, each with an explicit boolean — see the module docs for why the
    /// kept ones are named too.
    pub block: Map<String, Value>,
    /// The plugins this session keeps, in the order [`read`] returned them.
    pub kept: Vec<String>,
    /// The plugins this session does not load.
    pub removed: Vec<String>,
}

impl Curated {
    /// How many removed plugins ship code that no MCP confinement would have
    /// caught — hooks, or scripts beside them.
    ///
    /// The number that says what the policy bought, as distinct from how many
    /// names it struck out. A profile that removed three prompt-only plugins
    /// removed nothing that runs on its own, and a report that printed `3`
    /// either way would be describing the list rather than the machine.
    pub fn removed_code(&self, installed: &[Installed]) -> usize {
        self.removed
            .iter()
            .filter(|name| {
                installed
                    .iter()
                    .any(|p| &&p.name == name && p.content.runs_outside_mcp())
            })
            .count()
    }
}

/// Decide which plugins reach the session.
///
/// `None` when the policy removes nothing, which is [`PluginPolicy::AsConfigured`]
/// and also the machine with no plugins enabled at all. Writing an
/// `enabledPlugins` block in that case would replace the user's own object with
/// a copy of itself — harmless today and a trap the first time Claude changes
/// how the key merges, for no benefit either way.
///
/// `allow` is the runtime's configuration, never the request, for the reason
/// [`crate::config::Config::connector_allow`] gives: a list the confined thing
/// gets to write is not a boundary. [`crate::policy::AgentPolicy::validate_for`]
/// refuses [`PluginPolicy::Curated`] with an empty one rather than starting a
/// session that removes everything while reporting a curated list.
pub fn curate(installed: &[Installed], policy: PluginPolicy, allow: &[String]) -> Option<Curated> {
    if !policy.reduces() || installed.is_empty() {
        return None;
    }

    let mut block = Map::new();
    let mut kept = Vec::new();
    let mut removed = Vec::new();

    for plugin in installed {
        let keep = match policy {
            PluginPolicy::AsConfigured => true,
            PluginPolicy::NoPlugins => false,
            PluginPolicy::Curated => allow.iter().any(|a| a == &plugin.name),
        };
        block.insert(plugin.name.clone(), Value::Bool(keep));
        if keep {
            kept.push(plugin.name.clone());
        } else {
            removed.push(plugin.name.clone());
        }
    }

    Some(Curated {
        block,
        kept,
        removed,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn fixture(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "apex-pluginconf-{}-{tag}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        std::fs::create_dir_all(&dir).expect("fixture");
        dir
    }

    /// Build a home with two plugins: `hooky@mp` ships a hook and a script,
    /// `promptly@mp` ships only commands.
    fn two_plugin_home(dir: &Path) -> PathBuf {
        let home = dir.join("home");
        let hooky = home.join(".claude/plugins/cache/mp/hooky/1.0.0");
        let promptly = home.join(".claude/plugins/cache/mp/promptly/1.0.0");
        std::fs::create_dir_all(hooky.join("hooks")).expect("hooky");
        std::fs::create_dir_all(hooky.join("scripts")).expect("scripts");
        std::fs::create_dir_all(promptly.join("commands")).expect("promptly");
        std::fs::create_dir_all(home.join(".claude/plugins")).expect("plugins");
        std::fs::write(hooky.join("hooks/hooks.json"), b"{}").expect("hooks");
        std::fs::write(
            home.join(".claude/settings.json"),
            serde_json::to_vec(&json!({
                "enabledPlugins": {"hooky@mp": true, "promptly@mp": true, "off@mp": false},
            }))
            .expect("json"),
        )
        .expect("settings");
        std::fs::write(
            home.join(".claude/plugins/installed_plugins.json"),
            serde_json::to_vec(&json!({
                "version": 2,
                "plugins": {
                    "hooky@mp": [{"scope": "user", "installPath": hooky.to_string_lossy()}],
                    "promptly@mp": [{"scope": "user", "installPath": promptly.to_string_lossy()}],
                },
            }))
            .expect("json"),
        )
        .expect("installed");
        home
    }

    #[test]
    fn a_plugin_switched_off_in_settings_is_not_one_of_the_enabled_ones() {
        let dir = fixture("off");
        let home = two_plugin_home(&dir);
        let found = read(&home);
        let names: Vec<&str> = found.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, vec!["hooky@mp", "promptly@mp"]);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn the_content_of_a_plugin_is_read_off_the_tree_and_not_the_manifest() {
        let dir = fixture("content");
        let home = two_plugin_home(&dir);
        let found = read(&home);

        let hooky = found.iter().find(|p| p.name == "hooky@mp").expect("hooky");
        assert!(hooky.content.hooks, "hooks/hooks.json is there");
        assert!(hooky.content.scripts, "scripts/ is there");
        assert!(!hooky.content.mcp, "no .mcp.json was written");
        assert!(
            hooky.content.runs_outside_mcp(),
            "a hook is exactly the content no MCP document reaches"
        );

        let promptly = found
            .iter()
            .find(|p| p.name == "promptly@mp")
            .expect("promptly");
        assert!(promptly.content.prompts, "commands/ is there");
        assert!(
            !promptly.content.runs_outside_mcp(),
            "commands run through the agent's own tools, so through its sandbox"
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn an_install_path_is_read_from_the_array_version_2_actually_writes() {
        let dir = fixture("v2");
        let home = two_plugin_home(&dir);
        let found = read(&home);
        let hooky = found.iter().find(|p| p.name == "hooky@mp").expect("hooky");
        assert!(
            hooky.path.as_deref().is_some_and(|p| p.ends_with("1.0.0")),
            "installPath came out of the array: {:?}",
            hooky.path
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_plugin_enabled_with_no_install_entry_keeps_its_row() {
        let dir = fixture("ghost");
        let home = dir.join("home");
        std::fs::create_dir_all(home.join(".claude")).expect("claude");
        std::fs::write(
            home.join(".claude/settings.json"),
            br#"{"enabledPlugins": {"ghost@nowhere": true}}"#,
        )
        .expect("settings");

        let found = read(&home);
        assert_eq!(found.len(), 1, "the row survives a missing install entry");
        assert_eq!(found[0].path, None);
        assert_eq!(found[0].content, Content::default());
        assert_eq!(found[0].marketplace(), Some("nowhere"));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn as_configured_writes_no_block_at_all() {
        let dir = fixture("asconf");
        let home = two_plugin_home(&dir);
        let found = read(&home);
        assert_eq!(curate(&found, PluginPolicy::AsConfigured, &[]), None);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn none_names_every_enabled_plugin_with_an_explicit_false() {
        let dir = fixture("none");
        let home = two_plugin_home(&dir);
        let found = read(&home);
        let curated = curate(&found, PluginPolicy::NoPlugins, &[]).expect("a block");

        // The kept ones are named too — see the module docs. A block that
        // listed only the removals would depend on how `--settings` merges an
        // object key.
        assert_eq!(curated.block.len(), 2);
        assert_eq!(curated.block["hooky@mp"], Value::Bool(false));
        assert_eq!(curated.block["promptly@mp"], Value::Bool(false));
        assert!(curated.kept.is_empty());
        assert_eq!(curated.removed, vec!["hooky@mp", "promptly@mp"]);

        // One of the two ships a hook; the other only ships prompts. The
        // number that says what the policy bought is the first one.
        assert_eq!(curated.removed_code(&found), 1);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn curated_keeps_the_names_the_runtime_configured_and_no_others() {
        let dir = fixture("curated");
        let home = two_plugin_home(&dir);
        let found = read(&home);
        let allow = vec!["promptly@mp".to_string()];
        let curated = curate(&found, PluginPolicy::Curated, &allow).expect("a block");

        assert_eq!(curated.kept, vec!["promptly@mp"]);
        assert_eq!(curated.removed, vec!["hooky@mp"]);
        assert_eq!(curated.block["promptly@mp"], Value::Bool(true));
        assert_eq!(curated.block["hooky@mp"], Value::Bool(false));
        // The removed one is the one that ran code of its own.
        assert_eq!(curated.removed_code(&found), 1);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_name_on_the_allow_list_that_is_not_installed_adds_nothing() {
        let dir = fixture("ghostallow");
        let home = two_plugin_home(&dir);
        let found = read(&home);
        let allow = vec!["promptly@mp".to_string(), "absent@mp".to_string()];
        let curated = curate(&found, PluginPolicy::Curated, &allow).expect("a block");
        assert!(
            !curated.block.contains_key("absent@mp"),
            "the document describes what is installed, not what a list wished for"
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_machine_with_no_plugins_gets_no_block_whatever_the_policy() {
        let dir = fixture("empty");
        let home = dir.join("home");
        std::fs::create_dir_all(home.join(".claude")).expect("claude");
        std::fs::write(home.join(".claude/settings.json"), b"{}").expect("settings");
        let found = read(&home);
        assert!(found.is_empty());
        assert_eq!(curate(&found, PluginPolicy::NoPlugins, &[]), None);
        std::fs::remove_dir_all(&dir).ok();
    }
}
