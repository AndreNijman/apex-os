//! The MCP configuration a session is launched with, rather than the one it
//! finds — P1-026's second criterion and P1-028's second.
//!
//! Both items had the same remainder, and `mcp/sidecar.rs` and `connector.rs`
//! both named it in the same words: `--strict-mcp-config` plus a configuration
//! file the runtime wrote. This is that file.
//!
//! ## What was measured before any of this was designed
//!
//! Claude Code 2.1.260, probed on this machine with `ANTHROPIC_BASE_URL`
//! pointed at a dead local port so nothing left the machine, and one sentinel
//! script per server that records the fact that it was started:
//!
//! 1. With no flag, **four** surfaces each start a server: `~/.claude.json`'s
//!    `mcpServers`, `~/.claude.json`'s `projects.<dir>.mcpServers`, the working
//!    directory's own `.mcp.json`, and an enabled plugin's `.mcp.json`.
//! 2. With `--strict-mcp-config --mcp-config <file>`, **only the file's servers
//!    start**. The plugin-defined one is dropped with the rest — which is the
//!    fact the whole design rests on, and the one that could not be taken from
//!    the flag's help text, since "all other MCP configurations" does not say
//!    whether a plugin counts as a configuration.
//! 3. A server key containing `:` is accepted verbatim in such a file. So the
//!    curated document can keep the agent's own spelling —
//!    `plugin:github:github` stays `plugin:github:github` — and the
//!    `mcp__<server>__<tool>` names that permission rules already refer to do
//!    not move underneath them.
//! 4. A plugin's live `.mcp.json` is the copy under `installed_plugins.json`'s
//!    `installPath`, not the marketplace checkout beside it.
//!
//! ## Two things it does, and they are not the same thing
//!
//! **Confinement (P1-026).** A kept program whose definition came from
//! somebody else's tree is rewritten to start through `apex mcp run <name> --
//! …`, so bubblewrap confines it. That covers a plugin's `.mcp.json` and a
//! repository's, which is exactly the executable content P1-026 is about: code
//! that arrives with a checkout or a marketplace update and that nobody read.
//!
//! The user's own definitions in `~/.claude.json` are **not** silently wrapped.
//! That is the same line [`crate::profile`] and `mcp/servers.rs`'s
//! `Surface::is_writable_here` already draw, from the other side: the user's
//! own file is the user's own decision, `apex mcp confine` is the verb for it,
//! and closing `npx -y @modelcontextprotocol/server-memory` off the network
//! without being asked would break a working machine quietly — the worst way to
//! deliver a security improvement.
//!
//! **Selection (P1-028).** [`ConnectorPolicy`] decides which connectors reach
//! the session at all. `curated` is the per-connector switch P1-028's second
//! criterion asks for and that APEX did not have: the names come from the
//! runtime's own configuration, not from the request, for the reason
//! `Config::network_allow` gives — a list the confined thing gets to write is
//! not a boundary.
//!
//! ## The approval the agent already applies, applied here too
//!
//! A `.mcp.json` in a working directory is approved per directory: Claude
//! records the answer in `~/.claude.json` as `enabledMcpjsonServers` and
//! `disabledMcpjsonServers`, and a server nobody has approved is shown as
//! pending and not connected to. A curated document that listed one would be
//! APEX approving, on the user's behalf, a server defined by a repository they
//! cloned. So an unapproved repository server is dropped and says why, and a
//! disabled one is dropped whatever else is true of it.
//!
//! ## What this deliberately takes away
//!
//! Under a strict configuration the session's own edits to `~/.claude.json`'s
//! `mcpServers` do not reach the agent for the lifetime of that session. That
//! is not a side effect: it is the hole `sidecar.rs` named — `~/.claude.json`
//! has to be bound writable, so a session could rewrite a definition to drop
//! its own wrapper — and closing it is the point. A person adding a server for
//! real adds it and starts a session; an agent adding one mid-session does not
//! get it.
//!
//! ## What it cannot do, stated rather than implied
//!
//! A cloud endpoint has no local process, so nothing here confines one; it is
//! kept or it is not. And an adapter that has no such flag gets no curated
//! document at all — [`crate::adapter::Adapter::strict_mcp`] is a per-adapter
//! fact, and a runtime that pretended otherwise would report a confinement it
//! had not applied.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use serde_json::{json, Map, Value};

use crate::policy::ConnectorPolicy;

/// Where a server definition was read from.
///
/// The same four surfaces `apex mcp list` reports, named from the runtime's
/// side. `mcp/servers.rs`'s `Surface` is built from this rather than from a
/// second walk of the same four files.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Origin {
    /// `~/.claude.json` → `mcpServers`. The user's own, every directory.
    User,
    /// `~/.claude.json` → `projects.<dir>.mcpServers`. One directory.
    Directory(String),
    /// The working directory's own `.mcp.json`, shared with everyone who
    /// clones it, and approved per directory by the agent.
    Repository(PathBuf),
    /// An enabled plugin's `.mcp.json`, replaced on every plugin update.
    Plugin { plugin: String, file: PathBuf },
}

impl Origin {
    /// Whether this definition came from somebody else's tree.
    ///
    /// The inverse of `Surface::is_writable_here`, and deliberately the same
    /// line: a definition APEX may rewrite in place is one the user owns, and a
    /// definition APEX confines at launch is one they do not.
    pub fn is_third_party(&self) -> bool {
        matches!(self, Origin::Repository(_) | Origin::Plugin { .. })
    }

    pub fn tag(&self) -> &'static str {
        match self {
            Origin::User => "user",
            Origin::Directory(_) => "directory",
            Origin::Repository(_) => "repository",
            Origin::Plugin { .. } => "plugin",
        }
    }

    pub fn describe(&self) -> String {
        match self {
            Origin::User => "~/.claude.json (every directory)".to_string(),
            Origin::Directory(dir) => format!("~/.claude.json, only in {dir}"),
            Origin::Repository(file) => format!("{} (in the repository)", file.display()),
            Origin::Plugin { plugin, .. } => {
                format!("the '{plugin}' plugin, replaced on every update")
            }
        }
    }
}

/// One MCP server definition, as the file that defines it writes it.
///
/// `def` is the object verbatim. Everything a curated document keeps is kept
/// from here — headers, `env`, `type`, whatever a newer agent grows — because
/// a document rebuilt from fields this build happens to understand would drop
/// the ones it does not and blame the server.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Definition {
    /// The name the agent addresses it by. A plugin's is `plugin:<p>:<key>`.
    pub name: String,
    /// The key inside its own file, which is what an approval list names.
    pub key: String,
    pub origin: Origin,
    pub def: Value,
}

/// How the agent reaches a server, as far as this build can tell.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Wire {
    /// A program on this machine. The only thing bubblewrap can confine.
    Program { command: String, args: Vec<String> },
    /// An address off this machine. No local process, so no local sandbox.
    Endpoint { url: String },
    /// Neither, and the definition is quoted rather than assumed. Not "local":
    /// calling it that would say something specific and false about what
    /// confines it.
    Unplaceable(String),
}

impl Definition {
    pub fn wire(&self) -> Wire {
        let Some(def) = self.def.as_object() else {
            return Wire::Unplaceable("not an object".to_string());
        };
        let kind = def.get("type").and_then(Value::as_str).unwrap_or("");
        if let Some(url) = def.get("url").and_then(Value::as_str) {
            return Wire::Endpoint {
                url: url.to_string(),
            };
        }
        if let Some(command) = def.get("command").and_then(Value::as_str) {
            let args = def
                .get("args")
                .and_then(Value::as_array)
                .map(|a| {
                    a.iter()
                        .filter_map(Value::as_str)
                        .map(str::to_string)
                        .collect()
                })
                .unwrap_or_default();
            return Wire::Program {
                command: command.to_string(),
                args,
            };
        }
        Wire::Unplaceable(if kind.is_empty() {
            "no transport".to_string()
        } else {
            kind.to_string()
        })
    }
}

/// Which `.mcp.json` servers the user has answered for, in one directory.
///
/// Three states rather than two, and the third is the one that matters: a
/// server nobody has answered for is *pending*, not denied and not allowed,
/// and the agent does not connect to it. A curated document must not turn
/// pending into allowed.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Approval {
    pub enabled: BTreeSet<String>,
    pub disabled: BTreeSet<String>,
}

impl Approval {
    /// Whether a repository-defined server may be started here.
    pub fn approved(&self, key: &str) -> bool {
        self.enabled.contains(key) && !self.disabled.contains(key)
    }
}

/// Read a JSON document, or nothing.
///
/// A document that does not parse is not an error: a half-written
/// `~/.claude.json` must not stop the plugin definitions from being read.
fn read_json(path: &Path) -> Option<Value> {
    serde_json::from_slice(&std::fs::read(path).ok()?).ok()
}

fn collect(
    node: Option<&Value>,
    origin: &Origin,
    name_of: impl Fn(&str) -> String,
    out: &mut Vec<Definition>,
) {
    let Some(servers) = node.and_then(Value::as_object) else {
        return;
    };
    for (key, def) in servers {
        out.push(Definition {
            name: name_of(key),
            key: key.clone(),
            origin: origin.clone(),
            def: def.clone(),
        });
    }
}

/// Every MCP server definition a session started in `cwd` would load.
///
/// The four surfaces measured above, in the order the agent applies them, and
/// then sorted by name so two runs of the same machine produce the same
/// document — a curated file that reordered itself every launch would show up
/// as a change in every diff anyone took of it.
pub fn read(home: &Path, cwd: Option<&Path>) -> Vec<Definition> {
    let mut out = Vec::new();

    if let Some(doc) = read_json(&home.join(".claude.json")) {
        collect(doc.get("mcpServers"), &Origin::User, |k| k.to_string(), &mut out);
        if let Some(cwd) = cwd {
            let dir = cwd.to_string_lossy().into_owned();
            let scoped = doc
                .get("projects")
                .and_then(|p| p.get(&dir))
                .and_then(|p| p.get("mcpServers"));
            collect(scoped, &Origin::Directory(dir), |k| k.to_string(), &mut out);
        }
    }

    if let Some(cwd) = cwd {
        let file = cwd.join(".mcp.json");
        if let Some(doc) = read_json(&file) {
            collect(
                doc.get("mcpServers").or(Some(&doc)),
                &Origin::Repository(file),
                |k| k.to_string(),
                &mut out,
            );
        }
    }

    for (plugin, file) in enabled_plugin_configs(home) {
        let Some(doc) = read_json(&file) else { continue };
        let named = plugin.clone();
        collect(
            doc.get("mcpServers").or(Some(&doc)),
            &Origin::Plugin { plugin, file },
            move |k| format!("plugin:{named}:{k}"),
            &mut out,
        );
    }

    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

/// The per-directory answers to the `.mcp.json` approval prompt.
pub fn approvals(home: &Path, cwd: Option<&Path>) -> Approval {
    let mut approval = Approval::default();
    let (Some(doc), Some(cwd)) = (read_json(&home.join(".claude.json")), cwd) else {
        return approval;
    };
    let dir = cwd.to_string_lossy().into_owned();
    let Some(entry) = doc.get("projects").and_then(|p| p.get(&dir)) else {
        return approval;
    };
    for (key, into) in [
        ("enabledMcpjsonServers", &mut approval.enabled),
        ("disabledMcpjsonServers", &mut approval.disabled),
    ] {
        if let Some(list) = entry.get(key).and_then(Value::as_array) {
            into.extend(list.iter().filter_map(Value::as_str).map(str::to_string));
        }
    }
    approval
}

/// Each enabled plugin's `.mcp.json`, when it has one.
///
/// Enabled is read from `settings.json`, and the file is the one under
/// `installed_plugins.json`'s `installPath` — measurement 4 above: the
/// marketplace checkout beside it can be a different, staler file, and a
/// reader that took that one would confine a definition the agent never runs.
pub fn enabled_plugin_configs(home: &Path) -> Vec<(String, PathBuf)> {
    let Some(settings) = read_json(&home.join(".claude/settings.json")) else {
        return Vec::new();
    };
    let enabled: Vec<String> = settings
        .get("enabledPlugins")
        .and_then(Value::as_object)
        .map(|m| {
            m.iter()
                .filter(|(_, v)| v.as_bool() == Some(true))
                .map(|(k, _)| k.clone())
                .collect()
        })
        .unwrap_or_default();
    let Some(installed) = read_json(&home.join(".claude/plugins/installed_plugins.json")) else {
        return Vec::new();
    };
    let Some(plugins) = installed.get("plugins").and_then(Value::as_object) else {
        return Vec::new();
    };

    let mut out = Vec::new();
    for full in enabled {
        let Some(entries) = plugins.get(&full).and_then(Value::as_array) else {
            continue;
        };
        let short = full.split('@').next().unwrap_or(&full).to_string();
        for entry in entries {
            let Some(dir) = entry.get("installPath").and_then(Value::as_str) else {
                continue;
            };
            let file = Path::new(dir).join(".mcp.json");
            if file.is_file() {
                out.push((short.clone(), file));
            }
        }
    }
    out
}

// ═════════════════════════════════════════════════════════════════════════════
//  the wrapper
// ═════════════════════════════════════════════════════════════════════════════

/// Whether a server name can be used as one path component.
///
/// `apex mcp run <name>` derives two paths from the name — the policy file
/// `<dir>/<name>.toml` and the private home `…/mcp/<name>` — so a name with a
/// separator in it is a name that escapes both. `:` is fine and is what a
/// plugin server's name already contains; `/`, `..` and an empty name are not.
/// Checked here rather than at the two path joins, so the wrapper and the
/// runner agree about which names exist.
pub fn usable_as_server_name(name: &str) -> bool {
    !name.is_empty()
        && name != "."
        && name != ".."
        && !name.contains('/')
        && !name.contains('\\')
        && !name.contains('\0')
}

/// Whether a string is a name an MCP server could actually have.
///
/// Weaker than [`usable_as_server_name`] on purpose, and the difference is the
/// point: this is what `connector_allow` entries are checked against, and an
/// entry that is merely un-confinable — a name with a `/` in it — is still a
/// name the agent might use, so refusing it from the allow list would silently
/// drop a connector somebody asked to keep. Only a name nothing can ever
/// match is dropped.
pub fn usable_as_connector_name(name: &str) -> bool {
    !name.trim().is_empty() && !name.contains('\0')
}

/// `apex mcp run <name> -- <command> [args…]`, as command and args.
///
/// The one place this argv is built. `mcp/confine.rs` writes the same shape
/// into a definition and `mcp/servers.rs` recognises it; a second spelling
/// anywhere would be a wrapper one of them did not see.
pub fn wrap(apex: &Path, name: &str, command: &str, args: &[String]) -> (String, Vec<String>) {
    let mut wrapped = vec![
        "mcp".to_string(),
        "run".to_string(),
        name.to_string(),
        "--".to_string(),
        command.to_string(),
    ];
    wrapped.extend(args.iter().cloned());
    (apex.to_string_lossy().into_owned(), wrapped)
}

/// The server an `apex mcp run <name> -- …` definition confines, and the
/// command it confines.
///
/// Matched on the argument vector rather than on the program's name, so a
/// program that merely has `apex` in its name is not mistaken for the wrapper.
pub fn unwrap_wrapped(command: &str, args: &[String]) -> Option<(String, Vec<String>)> {
    let program = Path::new(command).file_name()?.to_str()?;
    if program != "apex" {
        return None;
    }
    let mut rest = args.iter();
    if rest.next().map(String::as_str) != Some("mcp") {
        return None;
    }
    if rest.next().map(String::as_str) != Some("run") {
        return None;
    }
    let name = rest.next()?.clone();
    if rest.next().map(String::as_str) != Some("--") {
        return None;
    }
    let inner: Vec<String> = rest.cloned().collect();
    if inner.is_empty() {
        return None;
    }
    Some((name, inner))
}

// ═════════════════════════════════════════════════════════════════════════════
//  the curated document
// ═════════════════════════════════════════════════════════════════════════════

/// Whether a connector reached the session, and what happened to it.
///
/// `confined` is three-valued on purpose. `Some(true)` is bubblewrap running;
/// `Some(false)` is a program that runs with everything the session has;
/// `None` is "there is no local process here to confine", which is an endpoint
/// and is not the same statement as either.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Decision {
    pub name: String,
    pub origin: Origin,
    pub kept: bool,
    pub confined: Option<bool>,
    pub why: String,
}

impl Decision {
    pub fn to_json(&self) -> Value {
        json!({
            "name": self.name,
            "origin": self.origin.tag(),
            "definedIn": self.origin.describe(),
            "kept": self.kept,
            "sandboxed": self.confined,
            "why": self.why,
        })
    }
}

/// The document the runtime hands the agent, and the account of how it was
/// arrived at.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Curated {
    pub document: Value,
    pub decisions: Vec<Decision>,
}

impl Curated {
    pub fn kept(&self) -> usize {
        self.decisions.iter().filter(|d| d.kept).count()
    }

    pub fn dropped(&self) -> usize {
        self.decisions.iter().filter(|d| !d.kept).count()
    }

    pub fn confined(&self) -> usize {
        self.decisions
            .iter()
            .filter(|d| d.kept && d.confined == Some(true))
            .count()
    }

    /// Whether anything kept is an executable this did not confine.
    ///
    /// The field a reader should look at before believing "everything is
    /// sandboxed", for the reason `provenance.rs` gives about
    /// `everythingExecutableIsSandboxed` being vacuously true with nothing to
    /// confine.
    pub fn has_unconfined_program(&self) -> bool {
        self.decisions
            .iter()
            .any(|d| d.kept && d.confined == Some(false))
    }

    pub fn to_json(&self) -> Value {
        json!({
            "kept": self.kept(),
            "dropped": self.dropped(),
            "sandboxed": self.confined(),
            "unconfinedProgramKept": self.has_unconfined_program(),
            "connectors": self.decisions.iter().map(Decision::to_json).collect::<Vec<_>>(),
        })
    }

    /// One line per connector, for `apex agent connectors` and the session
    /// report.
    pub fn lines(&self) -> Vec<String> {
        self.decisions
            .iter()
            .map(|d| {
                format!(
                    "{:<32}  {:<8}  {}",
                    d.name,
                    if d.kept { "kept" } else { "REMOVED" },
                    d.why
                )
            })
            .collect()
    }
}

/// Build the launch configuration.
///
/// Pure: every filesystem question — where `apex` is, what the definitions
/// are, what the user approved — is the caller's, so the document can be
/// asserted exhaustively the way the sandbox argv is.
///
/// `apex` is `None` when the runtime could not find its own binary. That is a
/// could-not-run, not an absence: the programs are kept and reported
/// **unconfined with the reason**, because dropping them would break the
/// session and calling them sandboxed would be a lie.
pub fn curate(
    defs: &[Definition],
    approval: &Approval,
    policy: ConnectorPolicy,
    allow: &[String],
    apex: Option<&Path>,
) -> Curated {
    let allow: BTreeSet<&str> = allow.iter().map(String::as_str).collect();
    let mut servers = Map::new();
    let mut decisions = Vec::new();

    for def in defs {
        let wire = def.wire();
        let (keep, why) = decide(def, &wire, approval, policy, &allow);
        if !keep {
            decisions.push(Decision {
                name: def.name.clone(),
                origin: def.origin.clone(),
                kept: false,
                confined: None,
                why,
            });
            continue;
        }

        let (value, confined, note) = match &wire {
            Wire::Program { command, args } => {
                if let Some(inner) = unwrap_wrapped(command, args) {
                    (
                        def.def.clone(),
                        Some(true),
                        format!(
                            "already wrapped: it starts through `apex mcp run {}`, so bubblewrap \
                             confines it",
                            inner.0
                        ),
                    )
                } else if !def.origin.is_third_party() {
                    (
                        def.def.clone(),
                        Some(false),
                        "NOT sandboxed: your own definition, left exactly as you wrote it — \
                         `apex mcp confine` is the verb that changes that"
                            .to_string(),
                    )
                } else if !usable_as_server_name(&def.name) {
                    (
                        def.def.clone(),
                        Some(false),
                        "NOT sandboxed: its name cannot be a directory component, so \
                         `apex mcp run` has nowhere to put its policy or its private home"
                            .to_string(),
                    )
                } else {
                    match apex {
                        Some(apex) => {
                            let (command, args) = wrap(apex, &def.name, command, args);
                            (
                                rewrite(&def.def, &command, &args),
                                Some(true),
                                "sandboxed: the launch configuration starts it through \
                                 `apex mcp run`, so bubblewrap confines it"
                                    .to_string(),
                            )
                        }
                        None => (
                            def.def.clone(),
                            Some(false),
                            "NOT sandboxed, and this is a could-not-run rather than a choice: \
                             the runtime could not find its own `apex` binary to wrap it with"
                                .to_string(),
                        ),
                    }
                }
            }
            Wire::Endpoint { url } => (
                def.def.clone(),
                None,
                format!("cloud endpoint {url} — no local process, so no local sandbox applies"),
            ),
            Wire::Unplaceable(what) => (
                def.def.clone(),
                None,
                format!(
                    "kept as defined, and it is defined as {what} — which side of this machine \
                     it is on cannot be said, so nothing here claims to confine it"
                ),
            ),
        };

        servers.insert(def.name.clone(), value);
        decisions.push(Decision {
            name: def.name.clone(),
            origin: def.origin.clone(),
            kept: true,
            confined,
            why: note,
        });
    }

    Curated {
        document: json!({ "mcpServers": Value::Object(servers) }),
        decisions,
    }
}

/// Whether one definition reaches the session, and why not when it does not.
fn decide(
    def: &Definition,
    wire: &Wire,
    approval: &Approval,
    policy: ConnectorPolicy,
    allow: &BTreeSet<&str>,
) -> (bool, String) {
    // The agent's own answer first, and it is the user's answer: a server they
    // switched off must not come back through a file APEX wrote.
    if approval.disabled.contains(&def.key) {
        return (
            false,
            "removed: you disabled it for this directory".to_string(),
        );
    }
    if matches!(def.origin, Origin::Repository(_)) && !approval.approved(&def.key) {
        return (
            false,
            "removed: a .mcp.json server is approved per directory and this one has not been \
             approved here. APEX will not approve, on your behalf, a server the repository you \
             cloned defines"
                .to_string(),
        );
    }

    match policy {
        ConnectorPolicy::AsConfigured => (true, String::new()),
        ConnectorPolicy::NoConnectors => (
            false,
            "removed: this session was started with no connectors at all".to_string(),
        ),
        ConnectorPolicy::LocalOnly => match wire {
            Wire::Program { .. } => (true, String::new()),
            Wire::Endpoint { .. } => (
                false,
                "removed: the cloud plane is off for this session".to_string(),
            ),
            // Not kept, and the reason is the whole of the three-state
            // discipline: it cannot be *shown* to be on the plane that was
            // kept, and "probably local" is not a measurement.
            Wire::Unplaceable(what) => (
                false,
                format!(
                    "removed: defined as {what}, so it cannot be shown to be on this machine, \
                     and this session keeps only what can"
                ),
            ),
        },
        ConnectorPolicy::Curated => {
            if allow.contains(def.name.as_str()) {
                (true, String::new())
            } else {
                (
                    false,
                    "removed: not named in the runtime's connector list".to_string(),
                )
            }
        }
    }
}

/// The same definition with a different command and arguments.
///
/// Every other key is carried through untouched — `env`, `headers`, `type`,
/// and whatever a newer agent grows — because this is a wrapper, not a rewrite.
fn rewrite(def: &Value, command: &str, args: &[String]) -> Value {
    let mut out = def.as_object().cloned().unwrap_or_default();
    out.insert("command".to_string(), Value::String(command.to_string()));
    out.insert(
        "args".to_string(),
        Value::Array(args.iter().map(|a| Value::String(a.clone())).collect()),
    );
    Value::Object(out)
}

/// What the curated file is called inside the session scratch.
///
/// Not `.mcp.json` and not `mcp.json`: the scratch is bound writable and
/// visible, and a file whose name is one the agent also reads from the project
/// is how somebody debugging this ends up reading the wrong one. The same
/// reasoning as `REDACTED_SETTINGS_FILE`.
pub const CONFIG_FILE: &str = "apex-mcp-launch.json";

/// Where the curated configuration goes for a session.
pub fn config_path(scratch: &Path) -> PathBuf {
    scratch.join(CONFIG_FILE)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn def(name: &str, origin: Origin, body: Value) -> Definition {
        Definition {
            name: name.to_string(),
            key: name.rsplit(':').next().unwrap_or(name).to_string(),
            origin,
            def: body,
        }
    }

    fn plugin(name: &str) -> Origin {
        Origin::Plugin {
            plugin: name.to_string(),
            file: PathBuf::from("/tmp/plugin/.mcp.json"),
        }
    }

    fn apex() -> PathBuf {
        PathBuf::from("/usr/bin/apex")
    }

    fn servers(c: &Curated) -> Vec<String> {
        c.document["mcpServers"]
            .as_object()
            .expect("mcpServers")
            .keys()
            .cloned()
            .collect()
    }

    #[test]
    fn a_plugins_program_is_started_through_the_wrapper_and_the_users_own_is_not() {
        // P1-026's second criterion, in the one place it is decided. The
        // plugin's command is replaced; the user's is byte-identical to what
        // they wrote.
        let defs = vec![
            def(
                "plugin:p:srv",
                plugin("p"),
                json!({"command": "node", "args": ["server.js"], "env": {"A": "1"}}),
            ),
            def(
                "mine",
                Origin::User,
                json!({"command": "npx", "args": ["-y", "thing"]}),
            ),
        ];
        let c = curate(
            &defs,
            &Approval::default(),
            ConnectorPolicy::AsConfigured,
            &[],
            Some(&apex()),
        );
        let doc = &c.document["mcpServers"];
        assert_eq!(doc["plugin:p:srv"]["command"], json!("/usr/bin/apex"));
        assert_eq!(
            doc["plugin:p:srv"]["args"],
            json!(["mcp", "run", "plugin:p:srv", "--", "node", "server.js"])
        );
        // A wrapper, not a rewrite: everything else survives.
        assert_eq!(doc["plugin:p:srv"]["env"], json!({"A": "1"}));
        assert_eq!(doc["mine"], defs[1].def);

        assert_eq!(c.confined(), 1);
        assert!(c.has_unconfined_program(), "the user's own is not confined");
        let mine = c.decisions.iter().find(|d| d.name == "mine").expect("mine");
        assert!(mine.why.contains("NOT sandboxed"), "{}", mine.why);
        assert!(mine.why.contains("apex mcp confine"), "{}", mine.why);
    }

    #[test]
    fn an_already_wrapped_definition_is_not_wrapped_twice() {
        // The regression a second wrap would be: `apex mcp run x -- apex mcp
        // run x -- …`, which starts a sandbox inside a sandbox to run the same
        // program and reports it as confined either way, so nothing would ever
        // notice.
        let defs = vec![def(
            "plugin:p:srv",
            plugin("p"),
            json!({"command": "apex", "args": ["mcp", "run", "plugin:p:srv", "--", "node", "s.js"]}),
        )];
        let c = curate(
            &defs,
            &Approval::default(),
            ConnectorPolicy::AsConfigured,
            &[],
            Some(&apex()),
        );
        assert_eq!(c.document["mcpServers"]["plugin:p:srv"], defs[0].def);
        assert_eq!(c.confined(), 1);
        assert!(!c.has_unconfined_program());
    }

    #[test]
    fn a_runtime_that_cannot_find_its_own_binary_says_so_instead_of_claiming_a_sandbox() {
        // "Permission denied is not absence", applied to the wrapper: the
        // server still runs, and the report says it is unconfined and why.
        let defs = vec![def(
            "plugin:p:srv",
            plugin("p"),
            json!({"command": "node", "args": []}),
        )];
        let c = curate(
            &defs,
            &Approval::default(),
            ConnectorPolicy::AsConfigured,
            &[],
            None,
        );
        assert_eq!(c.kept(), 1);
        assert_eq!(c.confined(), 0);
        assert!(c.has_unconfined_program());
        assert!(
            c.decisions[0].why.contains("could-not-run"),
            "{}",
            c.decisions[0].why
        );
    }

    #[test]
    fn an_endpoint_is_neither_sandboxed_nor_reported_as_unsandboxed() {
        // Three-valued, and this is the value that only exists because the
        // other two would both be wrong: there is no local process here.
        let defs = vec![def(
            "cloudy",
            Origin::User,
            json!({"type": "http", "url": "https://example.com/mcp"}),
        )];
        let c = curate(
            &defs,
            &Approval::default(),
            ConnectorPolicy::AsConfigured,
            &[],
            Some(&apex()),
        );
        assert_eq!(c.decisions[0].confined, None);
        assert!(!c.has_unconfined_program());
        assert_eq!(c.to_json()["connectors"][0]["sandboxed"], Value::Null);
    }

    #[test]
    fn an_unapproved_repository_server_is_not_approved_on_the_users_behalf() {
        // The agent shows an unapproved `.mcp.json` server as pending and does
        // not connect to it. Writing it into a curated file would answer that
        // prompt for them, in a file they never see, for a definition that
        // arrived with a repository they cloned.
        let repo = Origin::Repository(PathBuf::from("/p/.mcp.json"));
        let defs = vec![def("fromrepo", repo.clone(), json!({"command": "node"}))];
        let c = curate(
            &defs,
            &Approval::default(),
            ConnectorPolicy::AsConfigured,
            &[],
            Some(&apex()),
        );
        assert_eq!(c.kept(), 0);
        assert!(c.decisions[0].why.contains("approved per directory"));

        // Approved, and it is both kept and wrapped — a repository is somebody
        // else's tree too.
        let approval = Approval {
            enabled: ["fromrepo".to_string()].into_iter().collect(),
            disabled: BTreeSet::new(),
        };
        let c = curate(
            &defs,
            &approval,
            ConnectorPolicy::AsConfigured,
            &[],
            Some(&apex()),
        );
        assert_eq!(c.kept(), 1);
        assert_eq!(c.confined(), 1);

        // And an explicit disable beats an enable, whichever order they are in.
        let approval = Approval {
            enabled: ["fromrepo".to_string()].into_iter().collect(),
            disabled: ["fromrepo".to_string()].into_iter().collect(),
        };
        let c = curate(
            &defs,
            &approval,
            ConnectorPolicy::AsConfigured,
            &[],
            Some(&apex()),
        );
        assert_eq!(c.kept(), 0);
        assert!(c.decisions[0].why.contains("you disabled it"));
    }

    #[test]
    fn local_only_removes_the_cloud_plane_and_keeps_the_programs() {
        let defs = vec![
            def("cloudy", Origin::User, json!({"url": "https://example.com/mcp"})),
            def("proggy", Origin::User, json!({"command": "node"})),
        ];
        let c = curate(
            &defs,
            &Approval::default(),
            ConnectorPolicy::LocalOnly,
            &[],
            Some(&apex()),
        );
        assert_eq!(servers(&c), vec!["proggy".to_string()]);
        let cloudy = c.decisions.iter().find(|d| d.name == "cloudy").expect("cloudy");
        assert!(cloudy.why.contains("cloud plane is off"), "{}", cloudy.why);
    }

    #[test]
    fn curated_names_the_connectors_one_at_a_time() {
        // P1-028's second criterion, and the thing APEX did not have: this
        // removes ONE cloud connector and keeps another. Before this, the only
        // reduction was `--sandbox strict` taking the whole plane.
        let defs = vec![
            def("keepme", Origin::User, json!({"url": "https://a.example/mcp"})),
            def("dropme", Origin::User, json!({"url": "https://b.example/mcp"})),
            def("proggy", Origin::User, json!({"command": "node"})),
        ];
        let c = curate(
            &defs,
            &Approval::default(),
            ConnectorPolicy::Curated,
            &["keepme".to_string()],
            Some(&apex()),
        );
        assert_eq!(servers(&c), vec!["keepme".to_string()]);
        assert_eq!(c.dropped(), 2);
        let dropped = c.decisions.iter().find(|d| d.name == "dropme").expect("dropme");
        assert!(dropped.why.contains("connector list"), "{}", dropped.why);
    }

    #[test]
    fn no_connectors_leaves_an_empty_document_rather_than_no_document() {
        // An empty `mcpServers` with `--strict-mcp-config` is what "no
        // connectors" has to be. Omitting the file instead would give the
        // session every connector on the machine, which is the opposite.
        let defs = vec![def("proggy", Origin::User, json!({"command": "node"}))];
        let c = curate(
            &defs,
            &Approval::default(),
            ConnectorPolicy::NoConnectors,
            &[],
            Some(&apex()),
        );
        assert_eq!(c.kept(), 0);
        assert_eq!(c.document, json!({"mcpServers": {}}));
    }

    #[test]
    fn a_definition_with_no_transport_is_kept_untouched_but_never_called_local() {
        // Under the default nothing was asked to be reduced, so removing a
        // definition this build cannot classify would be APEX breaking a
        // server it merely does not understand.
        let defs = vec![def("mystery", Origin::User, json!({"type": "quantum"}))];
        let c = curate(
            &defs,
            &Approval::default(),
            ConnectorPolicy::AsConfigured,
            &[],
            Some(&apex()),
        );
        assert_eq!(c.kept(), 1);
        assert_eq!(c.decisions[0].confined, None);
        assert!(c.decisions[0].why.contains("cannot be said"));

        // Under a reducing policy it goes, because it cannot be SHOWN to be on
        // the plane that was kept.
        let c = curate(
            &defs,
            &Approval::default(),
            ConnectorPolicy::LocalOnly,
            &[],
            Some(&apex()),
        );
        assert_eq!(c.kept(), 0);
        assert!(c.decisions[0].why.contains("cannot be shown"));
    }

    #[test]
    fn a_name_that_is_not_one_path_component_is_never_handed_to_the_wrapper() {
        // `apex mcp run <name>` joins the name onto two directories. A name
        // with a separator in it would put a policy file, and a private home,
        // somewhere else entirely.
        for bad in ["", ".", "..", "a/b", "..\\b", "a\0b"] {
            assert!(!usable_as_server_name(bad), "{bad:?} was accepted");
        }
        for good in ["memory", "plugin:github:github", "a.b-c_d"] {
            assert!(usable_as_server_name(good), "{good:?} was refused");
        }

        let defs = vec![def(
            "plugin:p:a/b",
            plugin("p"),
            json!({"command": "node"}),
        )];
        let c = curate(
            &defs,
            &Approval::default(),
            ConnectorPolicy::AsConfigured,
            &[],
            Some(&apex()),
        );
        assert_eq!(c.document["mcpServers"]["plugin:p:a/b"], defs[0].def);
        assert_eq!(c.confined(), 0);
        assert!(c.decisions[0].why.contains("directory component"));
    }

    #[test]
    fn the_wrapper_and_its_inverse_are_each_others() {
        // One argv shape, asserted from both ends. `mcp/servers.rs` recognises
        // what `mcp/confine.rs` writes only because both go through here.
        let (command, args) = wrap(
            Path::new("/usr/bin/apex"),
            "memory",
            "npx",
            &["-y".to_string(), "server-memory".to_string()],
        );
        assert_eq!(
            unwrap_wrapped(&command, &args),
            Some((
                "memory".to_string(),
                vec![
                    "npx".to_string(),
                    "-y".to_string(),
                    "server-memory".to_string()
                ]
            ))
        );
        // Not the wrapper: a program whose name merely contains apex, and a
        // wrapper with nothing after the separator.
        assert_eq!(unwrap_wrapped("apex-shim", &args), None);
        assert_eq!(
            unwrap_wrapped(
                "apex",
                &["mcp".to_string(), "run".to_string(), "x".to_string(), "--".to_string()]
            ),
            None
        );
    }

    #[test]
    fn every_surface_is_read_and_a_plugins_name_keeps_its_spelling() {
        let dir = std::env::temp_dir().join(format!(
            "apex-mcpconf-{}-{:?}",
            std::process::id(),
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)
        ));
        let home = dir.join("home");
        let proj = dir.join("proj");
        let plug = home.join(".claude/plugins/cache/mp/p/1.0.0");
        std::fs::create_dir_all(&proj).expect("proj");
        std::fs::create_dir_all(&plug).expect("plug");
        std::fs::create_dir_all(home.join(".claude")).expect("claude");

        std::fs::write(
            home.join(".claude.json"),
            serde_json::to_vec(&json!({
                "mcpServers": {"u": {"command": "u"}},
                "projects": {proj.to_string_lossy(): {
                    "mcpServers": {"d": {"command": "d"}},
                    "enabledMcpjsonServers": ["r"],
                    "disabledMcpjsonServers": ["nope"],
                }},
            }))
            .expect("json"),
        )
        .expect("write");
        std::fs::write(
            proj.join(".mcp.json"),
            br#"{"mcpServers": {"r": {"command": "r"}}}"#,
        )
        .expect("write");
        std::fs::write(
            home.join(".claude/settings.json"),
            br#"{"enabledPlugins": {"p@mp": true}}"#,
        )
        .expect("write");
        std::fs::write(
            home.join(".claude/plugins/installed_plugins.json"),
            serde_json::to_vec(&json!({
                "version": 2,
                "plugins": {"p@mp": [{"installPath": plug.to_string_lossy()}]},
            }))
            .expect("json"),
        )
        .expect("write");
        std::fs::write(plug.join(".mcp.json"), br#"{"mcpServers": {"s": {"command": "s"}}}"#)
            .expect("write");

        let found = read(&home, Some(&proj));
        let names: Vec<&str> = found.iter().map(|d| d.name.as_str()).collect();
        assert_eq!(names, vec!["d", "plugin:p:s", "r", "u"]);
        // Measurement 3: the plugin keeps the spelling the agent already uses,
        // so `mcp__plugin:p:s__…` does not move underneath a permission rule.
        let s = found.iter().find(|d| d.name == "plugin:p:s").expect("s");
        assert_eq!(s.key, "s");
        assert!(s.origin.is_third_party());

        let approval = approvals(&home, Some(&proj));
        assert!(approval.approved("r"));
        assert!(!approval.approved("nope"));
        assert!(approval.disabled.contains("nope"));

        std::fs::remove_dir_all(&dir).ok();
    }
}
