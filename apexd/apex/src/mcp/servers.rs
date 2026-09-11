//! Every MCP server this machine has, and where each one's credential is.
//!
//! P0-003 moved one server's bearer token into the broker and rewrote its entry
//! in `~/.claude.json`. That works for the servers `apex secret migrate` can
//! see, and `~/.claude.json` is not the only place a server is defined. On this
//! machine there are four surfaces:
//!
//! | surface | who writes it | reaches |
//! |---|---|---|
//! | `~/.claude.json` → `mcpServers` | `claude mcp add`, and this | every directory |
//! | `~/.claude.json` → `projects.<dir>.mcpServers` | `claude mcp add --scope local` | one directory |
//! | a repository's `.mcp.json` | whoever wrote the repository | everyone who clones it |
//! | a plugin's `.mcp.json` | the plugin author, on every update | every directory |
//!
//! The migration reads the first of those. The other three are why
//! `plugin:github` on this machine still carries `"Authorization": "Bearer
//! ${GITHUB_PERSONAL_ACCESS_TOKEN}"` and nothing noticed: the definition is in
//! `~/.claude/plugins/cache/…/github/unknown/.mcp.json`, and the value it
//! interpolates is in `~/.claude/settings.json`'s `env` block, which is a
//! *third* file.
//!
//! ## Why this reads rather than edits
//!
//! A plugin's `.mcp.json` is replaced whenever the plugin updates, so an edit
//! there is undone without warning and the credential is back. This module
//! therefore reports; [`super::connect`] writes only to `~/.claude.json`, which
//! belongs to the user.
//!
//! ## What a credential's *shape* is for
//!
//! Never to decide whether a credential is correct — only the far end knows
//! that. It answers a narrower question: is this a value somebody meant to
//! replace? `GITHUB_PERSONAL_ACCESS_TOKEN` on this machine is 23 bytes of
//! nothing but capitals and underscores, which no bearer token is and every
//! `REPLACE_WITH_…` is. Reported as a placeholder, so "the server does not
//! connect" stops being a mystery.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde_json::Value;

/// Where a server definition was read from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Surface {
    /// `~/.claude.json` → `mcpServers`. The user's own, every directory.
    User,
    /// `~/.claude.json` → `projects.<dir>.mcpServers`. One directory.
    Directory(String),
    /// A repository's own `.mcp.json`, shared with everyone who clones it.
    Repository(PathBuf),
    /// A plugin's `.mcp.json`. Replaced whenever the plugin updates, which is
    /// why nothing here writes to one.
    Plugin { plugin: String, file: PathBuf },
}

impl Surface {
    /// Whether `apex mcp connect` may rewrite this definition in place.
    ///
    /// Only the user's own file. A repository's `.mcp.json` is under version
    /// control and belongs to whoever wrote it; a plugin's is overwritten on
    /// update.
    pub fn is_writable_here(&self) -> bool {
        matches!(self, Surface::User | Surface::Directory(_))
    }

    pub fn describe(&self) -> String {
        match self {
            Surface::User => "~/.claude.json (every directory)".to_string(),
            Surface::Directory(dir) => format!("~/.claude.json, only in {dir}"),
            Surface::Repository(file) => format!("{} (in the repository)", file.display()),
            Surface::Plugin { plugin, .. } => {
                format!("the '{plugin}' plugin, replaced on every update")
            }
        }
    }
}

/// How the agent reaches the server.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Transport {
    /// A program the agent starts. P1-019's subject.
    Stdio { command: String, args: Vec<String> },
    /// An endpoint the agent posts to. P1-018's subject.
    Endpoint { kind: String, url: String },
    /// Something neither this nor the roadmap has seen.
    Other(String),
}

/// What a value looks like, which is not the same as whether it works.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Shape {
    /// Nothing about it says it is not a credential.
    Opaque,
    /// A value somebody was meant to replace and did not.
    Placeholder,
    /// Present and empty, which is a configured server that cannot authenticate.
    Empty,
}

impl Shape {
    pub fn describe(&self) -> &'static str {
        match self {
            Shape::Opaque => "a value",
            Shape::Placeholder => "a placeholder nobody replaced",
            Shape::Empty => "empty",
        }
    }
}

/// Where a server's credential is, which is the whole question P1-018 asks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Credential {
    /// The definition carries none.
    None,
    /// `apex-secretd` holds it and the definition names only the bridge. What
    /// P1-018 is for.
    Brokered { service: String },
    /// A literal value in a file the agent reads. What P1-018 removes.
    InConfig { header: String, shape: Shape },
    /// `${VAR}`, which the agent expands from an environment the agent also
    /// reads — so the value is agent-readable wherever it is defined.
    FromEnvironment {
        header: String,
        var: String,
        /// The shape of what the variable holds, when it holds anything.
        defined: Option<Shape>,
        /// Where the value came from, for the report.
        source: Option<String>,
    },
}

impl Credential {
    /// Whether the agent can read this credential.
    ///
    /// The acceptance criterion in one method: `Brokered` is the only answer
    /// that is no, and `None` is not a credential at all.
    pub fn agent_readable(&self) -> bool {
        matches!(
            self,
            Credential::InConfig { .. } | Credential::FromEnvironment { .. }
        )
    }
}

/// One MCP server as some file on this machine defines it.
#[derive(Debug, Clone)]
pub struct Server {
    /// The name the agent addresses it by. A plugin's is `plugin:<p>:<name>`,
    /// which is the spelling the agent's own error messages use.
    pub name: String,
    /// The key inside its own file, which is what an edit has to use.
    pub key: String,
    pub surface: Surface,
    pub transport: Transport,
    pub credential: Credential,
}

/// Header names whose value is a credential.
///
/// `Authorization` is the one every MCP server uses. The others are here
/// because a server that wants its key in `X-Api-Key` is not thereby exempt
/// from the criterion, and a list that only knew one name would report such a
/// server as carrying no credential at all.
const CREDENTIAL_HEADERS: &[&str] = &[
    "authorization",
    "x-api-key",
    "api-key",
    "x-auth-token",
    "x-goog-api-key",
    "proxy-authorization",
];

/// Whether a header name carries a credential.
pub fn credential_header(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    CREDENTIAL_HEADERS.contains(&lower.as_str())
        || lower.contains("token")
        || lower.contains("secret")
        || lower.contains("apikey")
}

/// What a value looks like.
///
/// Deliberately conservative in one direction: calling a working credential a
/// placeholder would tell somebody to replace a token that is fine. Every rule
/// here is one no bearer token satisfies.
pub fn shape_of(value: &str) -> Shape {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Shape::Empty;
    }
    let lower = trimmed.to_ascii_lowercase();
    // Substring markers. Every one is long enough, or punctuated enough, that
    // no credential contains it by accident.
    for marker in [
        "replace", "your-", "your_", "yourtoken", "changeme", "change-me", "placeholder",
        "xxxxx", "<", "put-your", "example.com", "sk-...", "…",
    ] {
        if lower.contains(marker) {
            return Shape::Placeholder;
        }
    }
    // Matched as whole words rather than as substrings, because these three are
    // short enough to turn up *inside* a token by chance: `abc123` is six
    // hexadecimal characters, so a 64-character hex credential carries it about
    // once in three hundred thousand, and `todo` is four base64 ones. The cost
    // of being wrong in that direction is telling somebody to replace a
    // credential that works. Delimited they are unambiguous, so `TODO`,
    // `abc123` and `INSERT YOUR TOKEN HERE` are all still named — which
    // deleting the markers outright would have given up.
    if lower
        .split(|c: char| !c.is_ascii_alphanumeric())
        .any(|word| matches!(word, "todo" | "insert" | "abc123"))
    {
        return Shape::Placeholder;
    }
    // Nothing but capitals, digits and underscores, with at least one
    // underscore and short enough to be a name: that is a SHOUTING_NAME, which
    // is what an unexpanded placeholder looks like and what no token does.
    // `ghp_…`, a JWT and every base64 or hex token all carry lowercase.
    if trimmed.len() <= 64
        && trimmed.contains('_')
        && trimmed
            .chars()
            .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_')
    {
        return Shape::Placeholder;
    }
    Shape::Opaque
}

/// `${NAME}` or `$NAME`, when the whole value after a scheme word is one.
///
/// Claude expands these itself before it sends the header, so a definition
/// carrying one is a definition whose credential lives in an environment block
/// — which is a different file to read, not an absence of a credential.
fn env_reference(value: &str) -> Option<String> {
    let after_scheme = value
        .split_once(' ')
        .map(|(_, rest)| rest)
        .unwrap_or(value)
        .trim();
    if let Some(inner) = after_scheme.strip_prefix("${").and_then(|v| v.strip_suffix('}')) {
        // `${VAR:-default}` carries a literal after the marker, and treating it
        // as a bare name would report a credential that is in the file as one
        // that is not.
        if !inner.is_empty() && inner.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
            return Some(inner.to_string());
        }
        return None;
    }
    let bare = after_scheme.strip_prefix('$')?;
    if !bare.is_empty() && bare.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
        Some(bare.to_string())
    } else {
        None
    }
}

/// Everywhere a variable's value could come from, in the order Claude uses.
///
/// `settings.json`'s `env` block wins, because Claude applies it to every tool
/// it runs — including its own MCP client — and it does so whether or not the
/// variable is already in the environment.
pub struct Environment {
    pub from_settings: BTreeMap<String, String>,
}

impl Environment {
    pub fn read(home: &Path) -> Environment {
        let mut from_settings = BTreeMap::new();
        if let Some(doc) = read_json(&home.join(".claude/settings.json")) {
            if let Some(env) = doc.get("env").and_then(Value::as_object) {
                for (k, v) in env {
                    if let Some(v) = v.as_str() {
                        from_settings.insert(k.clone(), v.to_string());
                    }
                }
            }
        }
        Environment { from_settings }
    }

    /// The shape of what `name` holds, and where that came from.
    fn resolve(&self, name: &str) -> (Option<Shape>, Option<String>) {
        if let Some(v) = self.from_settings.get(name) {
            return (
                Some(shape_of(v)),
                Some("~/.claude/settings.json → env".to_string()),
            );
        }
        match std::env::var(name) {
            Ok(v) => (Some(shape_of(&v)), Some("the environment".to_string())),
            Err(_) => (None, None),
        }
    }
}

/// Read a JSON document, or nothing.
///
/// A document that does not parse is not an error here: a half-written
/// `~/.claude.json` must not stop the listing from reporting the plugin servers
/// it can still see.
pub fn read_json(path: &Path) -> Option<Value> {
    serde_json::from_slice(&std::fs::read(path).ok()?).ok()
}

/// Every MCP server defined on this machine, from every surface.
///
/// `cwd` decides which directory-scoped and repository-scoped definitions
/// apply; pass the directory the person is asking about.
pub fn discover(home: &Path, cwd: Option<&Path>) -> Vec<Server> {
    let env = Environment::read(home);
    let mut out = Vec::new();

    let sidecar = read_json(&home.join(".claude.json"));
    if let Some(doc) = &sidecar {
        collect(
            doc.get("mcpServers"),
            Surface::User,
            |k| k.to_string(),
            &env,
            &mut out,
        );
        if let Some(cwd) = cwd {
            let dir = cwd.to_string_lossy().into_owned();
            let scoped = doc
                .get("projects")
                .and_then(|p| p.get(&dir))
                .and_then(|p| p.get("mcpServers"));
            collect(
                scoped,
                Surface::Directory(dir),
                |k| k.to_string(),
                &env,
                &mut out,
            );
        }
    }

    if let Some(cwd) = cwd {
        let file = cwd.join(".mcp.json");
        if let Some(doc) = read_json(&file) {
            collect(
                doc.get("mcpServers").or(Some(&doc)),
                Surface::Repository(file),
                |k| k.to_string(),
                &env,
                &mut out,
            );
        }
    }

    for (plugin, file) in enabled_plugin_configs(home) {
        let Some(doc) = read_json(&file) else { continue };
        let named = plugin.clone();
        collect(
            doc.get("mcpServers").or(Some(&doc)),
            Surface::Plugin {
                plugin: plugin.clone(),
                file,
            },
            move |k| format!("plugin:{named}:{k}"),
            &env,
            &mut out,
        );
    }

    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

/// Each enabled plugin's `.mcp.json`, when it has one.
///
/// Enabled is read from `settings.json`, because a plugin that is installed and
/// switched off defines no server the agent will start — and reporting one
/// would send somebody to fix a file nothing reads.
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
        // The name the agent uses is the part before the marketplace, which is
        // the spelling in `plugin:github:github`.
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

/// Turn one `mcpServers` object into [`Server`]s.
fn collect(
    node: Option<&Value>,
    surface: Surface,
    name_of: impl Fn(&str) -> String,
    env: &Environment,
    out: &mut Vec<Server>,
) {
    let Some(servers) = node.and_then(Value::as_object) else {
        return;
    };
    for (key, def) in servers {
        let Some(def) = def.as_object() else { continue };
        let transport = transport_of(def);
        out.push(Server {
            name: name_of(key),
            key: key.clone(),
            credential: credential_of(def, &transport, env),
            surface: surface.clone(),
            transport,
        });
    }
}

fn transport_of(def: &serde_json::Map<String, Value>) -> Transport {
    let kind = def.get("type").and_then(Value::as_str).unwrap_or("");
    if let Some(url) = def.get("url").and_then(Value::as_str) {
        let kind = if kind.is_empty() { "http" } else { kind };
        return Transport::Endpoint {
            kind: kind.to_string(),
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
        return Transport::Stdio {
            command: command.to_string(),
            args,
        };
    }
    Transport::Other(if kind.is_empty() {
        "no transport".to_string()
    } else {
        kind.to_string()
    })
}

/// Where this definition's credential is.
fn credential_of(
    def: &serde_json::Map<String, Value>,
    transport: &Transport,
    env: &Environment,
) -> Credential {
    if let Transport::Stdio { command, args } = transport {
        if let Some(service) = bridged_service(command, args) {
            return Credential::Brokered { service };
        }
    }
    for block in ["headers", "env"] {
        let Some(map) = def.get(block).and_then(Value::as_object) else {
            continue;
        };
        for (name, value) in map {
            let is_credential = if block == "headers" {
                credential_header(name)
            } else {
                apex_agent_core::profile::credential_name(name)
            };
            if !is_credential {
                continue;
            }
            let Some(value) = value.as_str() else { continue };
            if let Some(var) = env_reference(value) {
                let (defined, source) = env.resolve(&var);
                return Credential::FromEnvironment {
                    header: name.clone(),
                    var,
                    defined,
                    source,
                };
            }
            return Credential::InConfig {
                header: name.clone(),
                shape: shape_of(value),
            };
        }
    }
    Credential::None
}

/// The service an `apex mcp bridge <service>` definition brokers.
///
/// Matched on the argument vector rather than on the command string, because
/// the command may be `apex` or an absolute path to it, and a definition that
/// merely mentioned the word would be matched by a server that runs a script
/// with `apex` in its name.
pub fn bridged_service(command: &str, args: &[String]) -> Option<String> {
    let program = Path::new(command).file_name()?.to_str()?;
    if program != "apex" {
        return None;
    }
    let mut rest = args.iter();
    if rest.next().map(String::as_str) != Some("mcp") {
        return None;
    }
    if rest.next().map(String::as_str) != Some("bridge") {
        return None;
    }
    rest.next().cloned()
}

/// The listing as a document, for `apex mcp list --json` and the shell.
///
/// A credential's *value* is in none of it, and its length is not either: a
/// length is a fact about a secret, and a report that carried one would be a
/// report worth grepping.
pub fn as_json(found: &[Server]) -> Value {
    let servers: Vec<Value> = found
        .iter()
        .map(|s| {
            let (transport, address) = match &s.transport {
                Transport::Stdio { command, args } => (
                    "stdio",
                    serde_json::json!({"command": command, "args": args}),
                ),
                Transport::Endpoint { kind, url } => {
                    (kind.as_str(), serde_json::json!({"url": url}))
                }
                Transport::Other(what) => ("other", serde_json::json!({"what": what})),
            };
            let credential = match &s.credential {
                Credential::None => serde_json::json!({"where": "none"}),
                Credential::Brokered { service } => {
                    serde_json::json!({"where": "broker", "service": service})
                }
                Credential::InConfig { header, shape } => serde_json::json!({
                    "where": "config", "header": header, "shape": shape.describe(),
                }),
                Credential::FromEnvironment {
                    header,
                    var,
                    defined,
                    source,
                } => serde_json::json!({
                    "where": "environment", "header": header, "variable": var,
                    "shape": defined.map(|s| s.describe()), "source": source,
                }),
            };
            // The definition's literal argv is what `address` reports, because
            // that is the fact on disk. `confined` is the thing derived from
            // it, so a reader of `--json` learns what the text listing shows
            // without having to re-implement the wrapper match. The *policy* is
            // deliberately not here: it is a file, `apex mcp policy` names the
            // file, and a listing that paraphrased it would be a second place
            // to read the same settings out of date.
            let confined = match &s.transport {
                Transport::Stdio { command, args } => confined(command, args)
                    .map(|(name, inner)| {
                        serde_json::json!({
                            "as": name,
                            "command": inner[0],
                            "args": inner[1..],
                        })
                    })
                    .unwrap_or(Value::Null),
                _ => Value::Null,
            };
            serde_json::json!({
                "name": s.name,
                "transport": transport,
                "address": address,
                "credential": credential,
                "agentReadable": s.credential.agent_readable(),
                "confined": confined,
                "definedIn": s.surface.describe(),
                "rewritable": s.surface.is_writable_here(),
            })
        })
        .collect();
    serde_json::json!({
        "servers": servers,
        "agentReadable": found.iter().filter(|s| s.credential.agent_readable()).count(),
    })
}

/// The server an `apex mcp run <name> -- …` definition confines, and the
/// command it confines.
///
/// The counterpart to [`bridged_service`], and matched the same way: on the
/// argument vector, so a program that merely has `apex` in its name is not
/// mistaken for the wrapper.
pub fn confined(command: &str, args: &[String]) -> Option<(String, Vec<String>)> {
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

/// Just the name, for a caller that only wants to know whether it is wrapped.
pub fn confined_server(command: &str, args: &[String]) -> Option<String> {
    confined(command, args).map(|(name, _)| name)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "apex-mcp-servers-{}-{tag}-{:?}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        std::fs::create_dir_all(dir.join(".claude/plugins")).expect("fixture home");
        dir
    }

    fn write(path: &Path, text: &str) {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("parent");
        }
        std::fs::write(path, text).expect("write");
    }

    #[test]
    fn a_placeholder_is_told_apart_from_a_credential() {
        // The live defect this exists to name: `plugin:github` on this machine
        // holds 23 bytes of capitals and underscores, and the agent's only
        // report is "Authorization header is badly formatted" from the far end.
        assert_eq!(shape_of("REPLACE_WITH_ACTUAL_PAT"), Shape::Placeholder);
        assert_eq!(shape_of("REPLACE_WITH_YOUR_TOKEN"), Shape::Placeholder);
        assert_eq!(shape_of("<your token here>"), Shape::Placeholder);
        assert_eq!(shape_of("changeme"), Shape::Placeholder);
        assert_eq!(shape_of(""), Shape::Empty);
        assert_eq!(shape_of("   "), Shape::Empty);

        // And the direction that must never be wrong: a real credential
        // reported as a placeholder tells somebody to replace a token that
        // works. Every shape a token actually takes.
        for real in [
            "ghp_16C7e42F292c6912E7710c838347Ae178B4a",
            "github_pat_11ABCDE0Y0aBcDeFgHiJkL_mNoPqRsTuVwXyZ0123456789",
            "sk-ant-api03-AbCdEf_GhIjKl-MnOpQrStUvWxYz",
            "eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.eyJzdWIiOiIxIn0.dBjftJeZ4CVP",
            "9f86d081884c7d659a2feaa0c55ad015a3bf4f1b2b0b822cd15d6c15b0f00a08",
            "AbCd1234EfGh5678",
        ] {
            assert_eq!(shape_of(real), Shape::Opaque, "{real}");
        }
    }

    #[test]
    fn a_short_marker_inside_a_token_is_not_a_placeholder_but_the_same_word_is() {
        // The two halves of one rule, which is why they are one test: `todo`,
        // `insert` and `abc123` are short enough to fall inside a real token —
        // `abc123` is six hexadecimal characters — so they are matched as
        // delimited words. Deleting them instead would have been the other
        // way to stop the false positive, and it would have given up all three
        // of the placeholders below.
        for placeholder in [
            "TODO",
            "todo",
            "abc123",
            "INSERT YOUR TOKEN HERE",
            "Bearer TODO",
            "insert token",
        ] {
            assert_eq!(shape_of(placeholder), Shape::Placeholder, "{placeholder}");
        }

        // The same letters inside an undelimited run are part of a token.
        for real in [
            "ghp_todoNOTaMarker16C7e42F292c6912E7710c8",
            "9f86abc123884c7d659a2feaa0c55ad015a3bf4f1b2b0b822cd15d6c15b0f00a0",
            "sk-ant-api03-InsertedKeyMaterial9xQ",
            "eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiJhYmMxMjMifQ.dBjftJeZ4CVPtodoX",
        ] {
            assert_eq!(shape_of(real), Shape::Opaque, "{real}");
        }
    }

    #[test]
    fn the_json_listing_names_the_wrapper_the_text_listing_unwraps() {
        // Parity: `apex mcp list` prints a `sandbox` line, so `--json` has to
        // carry the same fact or a script would have to re-implement the
        // wrapper match to learn it. The definition's own argv stays in
        // `address`, because that is what is on disk.
        let home = fixture("json-confined");
        write(
            &home.join(".claude.json"),
            &serde_json::json!({"mcpServers": {
                "memory": {"command": "apex", "args": [
                    "mcp", "run", "memory", "--", "npx", "-y", "@modelcontextprotocol/server-memory"]},
                "plain": {"command": "node", "args": ["server.js"]},
                "remote": {"type": "http", "url": "https://m.example.org/mcp"}
            }})
            .to_string(),
        );
        let doc = as_json(&discover(&home, None));
        let by_name = |name: &str| {
            doc["servers"]
                .as_array()
                .expect("servers")
                .iter()
                .find(|s| s["name"] == name)
                .expect(name)
                .clone()
        };

        let wrapped = by_name("memory");
        assert_eq!(wrapped["confined"]["as"], "memory");
        assert_eq!(wrapped["confined"]["command"], "npx");
        assert_eq!(
            wrapped["confined"]["args"],
            serde_json::json!(["-y", "@modelcontextprotocol/server-memory"])
        );
        // The definition itself is still reported verbatim.
        assert_eq!(wrapped["address"]["command"], "apex");

        // And the two shapes that are not confined report so, rather than
        // omitting the key and leaving a reader to guess.
        assert!(by_name("plain")["confined"].is_null());
        assert!(by_name("remote")["confined"].is_null());
        std::fs::remove_dir_all(&home).ok();
    }

    #[test]
    fn an_env_reference_is_not_mistaken_for_a_value() {
        assert_eq!(
            env_reference("Bearer ${GITHUB_PERSONAL_ACCESS_TOKEN}").as_deref(),
            Some("GITHUB_PERSONAL_ACCESS_TOKEN")
        );
        assert_eq!(env_reference("${TOKEN}").as_deref(), Some("TOKEN"));
        assert_eq!(env_reference("Bearer $TOKEN").as_deref(), Some("TOKEN"));
        // A literal is a literal, however much it looks like a shell fragment.
        assert_eq!(env_reference("Bearer ghp_realtoken"), None);
        assert_eq!(env_reference("Bearer ${TOKEN:-fallback}"), None);
        assert_eq!(env_reference("Bearer ${A}${B}"), None);
    }

    #[test]
    fn a_bridged_definition_is_recognised_however_apex_is_spelled() {
        let args = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(
            bridged_service("apex", &args(&["mcp", "bridge", "memory"])).as_deref(),
            Some("memory")
        );
        assert_eq!(
            bridged_service("/usr/bin/apex", &args(&["mcp", "bridge", "memory"])).as_deref(),
            Some("memory")
        );
        // The failure the argv match prevents: a server that merely runs
        // something with `apex` in its name reported as brokered.
        assert_eq!(bridged_service("apex-helper", &args(&["mcp", "bridge", "x"])), None);
        assert_eq!(bridged_service("apex", &args(&["agent", "run"])), None);
        assert_eq!(bridged_service("apex", &args(&["mcp", "bridge"])), None);
        assert_eq!(bridged_service("npx", &args(&["mcp", "bridge", "memory"])), None);
    }

    #[test]
    fn every_surface_a_server_can_be_defined_on_is_found() {
        // The whole point of this module. `apex secret migrate` reads one of
        // these four, which is why `plugin:github` was invisible to it.
        let home = fixture("surfaces");
        let cwd = home.join("work/repo");
        std::fs::create_dir_all(&cwd).expect("cwd");
        write(
            &home.join(".claude.json"),
            &serde_json::json!({
                "mcpServers": {
                    "memory": {"type": "http", "url": "https://m.example.com/mcp",
                               "headers": {"Authorization": "Bearer sekrit-value-9"}},
                    "bridged": {"type": "stdio", "command": "apex",
                                "args": ["mcp", "bridge", "already-done"]}
                },
                "projects": {
                    cwd.to_string_lossy(): {"mcpServers": {
                        "local-only": {"command": "npx", "args": ["-y", "thing"]}
                    }}
                }
            })
            .to_string(),
        );
        write(
            &cwd.join(".mcp.json"),
            r#"{"mcpServers":{"from-the-repo":{"type":"http","url":"https://r.example.com/mcp"}}}"#,
        );
        write(
            &home.join(".claude/settings.json"),
            &serde_json::json!({
                "env": {"SOME_API_TOKEN": "REPLACE_WITH_ACTUAL_PAT"},
                "enabledPlugins": {"gh@market": true, "off@market": true}
            })
            .to_string(),
        );
        let installed = home.join(".claude/plugins/gh");
        write(
            &home.join(".claude/plugins/installed_plugins.json"),
            &serde_json::json!({
                "plugins": {"gh@market": [{"installPath": installed.to_string_lossy()}]}
            })
            .to_string(),
        );
        write(
            &installed.join(".mcp.json"),
            r#"{"github":{"type":"http","url":"https://api.example.com/mcp",
                          "headers":{"Authorization":"Bearer ${SOME_API_TOKEN}"}}}"#,
        );

        let found = discover(&home, Some(&cwd));
        let names: Vec<&str> = found.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(
            names,
            vec!["bridged", "from-the-repo", "local-only", "memory", "plugin:gh:github"]
        );

        let by = |n: &str| found.iter().find(|s| s.name == n).expect(n).clone();
        assert!(matches!(by("bridged").credential, Credential::Brokered { .. }));
        assert!(matches!(
            by("memory").credential,
            Credential::InConfig { shape: Shape::Opaque, .. }
        ));
        assert_eq!(by("local-only").surface, Surface::Directory(cwd.to_string_lossy().into()));
        assert!(matches!(by("from-the-repo").surface, Surface::Repository(_)));
        // The live case, reproduced: a plugin definition whose credential is a
        // placeholder in a third file.
        match by("plugin:gh:github").credential {
            Credential::FromEnvironment { var, defined, source, .. } => {
                assert_eq!(var, "SOME_API_TOKEN");
                assert_eq!(defined, Some(Shape::Placeholder));
                assert_eq!(source.as_deref(), Some("~/.claude/settings.json → env"));
            }
            other => panic!("{other:?}"),
        }
        // A plugin nobody enabled defines nothing, even when it is installed.
        assert!(!names.iter().any(|n| n.starts_with("plugin:off:")));
        std::fs::remove_dir_all(&home).ok();
    }

    #[test]
    fn only_the_users_own_file_is_one_this_may_rewrite() {
        // A plugin's `.mcp.json` is replaced on every update and a
        // repository's is under somebody else's version control, so an edit to
        // either is undone without warning — and the credential comes back.
        assert!(Surface::User.is_writable_here());
        assert!(Surface::Directory("/p".into()).is_writable_here());
        assert!(!Surface::Repository("/p/.mcp.json".into()).is_writable_here());
        assert!(!Surface::Plugin {
            plugin: "gh".into(),
            file: "/x/.mcp.json".into()
        }
        .is_writable_here());
    }

    #[test]
    fn a_credential_is_agent_readable_unless_the_broker_holds_it() {
        // P1-018's second acceptance criterion as a predicate, so the listing
        // and the tests answer it the same way.
        assert!(!Credential::None.agent_readable());
        assert!(!Credential::Brokered { service: "m".into() }.agent_readable());
        assert!(Credential::InConfig {
            header: "Authorization".into(),
            shape: Shape::Opaque
        }
        .agent_readable());
        assert!(Credential::FromEnvironment {
            header: "Authorization".into(),
            var: "T".into(),
            defined: None,
            source: None
        }
        .agent_readable());
    }

    #[test]
    fn a_header_that_carries_a_credential_is_recognised_by_more_than_one_name() {
        for yes in ["Authorization", "authorization", "X-Api-Key", "X-Auth-Token", "My-Token"] {
            assert!(credential_header(yes), "{yes}");
        }
        for no in ["Content-Type", "Accept", "User-Agent", "Mcp-Session-Id"] {
            assert!(!credential_header(no), "{no}");
        }
    }

    #[test]
    fn a_config_that_does_not_parse_does_not_hide_the_ones_that_do() {
        // The listing is a diagnostic. A half-written `~/.claude.json` is
        // exactly when somebody runs it, and reporting nothing would say the
        // machine has no MCP servers.
        let home = fixture("broken");
        write(&home.join(".claude.json"), "{not json");
        write(
            &home.join(".claude/settings.json"),
            &serde_json::json!({"enabledPlugins": {"gh@m": true}}).to_string(),
        );
        let installed = home.join(".claude/plugins/gh");
        write(
            &home.join(".claude/plugins/installed_plugins.json"),
            &serde_json::json!({"plugins": {"gh@m": [{"installPath": installed.to_string_lossy()}]}})
                .to_string(),
        );
        write(&installed.join(".mcp.json"), r#"{"srv":{"type":"http","url":"https://x/mcp"}}"#);
        let found = discover(&home, None);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].name, "plugin:gh:srv");
        std::fs::remove_dir_all(&home).ok();
    }
}
