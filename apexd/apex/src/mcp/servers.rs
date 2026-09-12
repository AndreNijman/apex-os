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

use apex_agent_core::mcpconf;
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
    /// The runtime's name for the same four surfaces.
    ///
    /// One direction only. [`apex_agent_core::mcpconf::Origin`] is what the
    /// session launcher decides against and this is what a report prints, and
    /// keeping the conversion here means a fifth surface added upstream fails
    /// to compile rather than arriving as a silent fifth case.
    pub fn from_origin(origin: mcpconf::Origin) -> Surface {
        match origin {
            mcpconf::Origin::User => Surface::User,
            mcpconf::Origin::Directory(dir) => Surface::Directory(dir),
            mcpconf::Origin::Repository(file) => Surface::Repository(file),
            mcpconf::Origin::Plugin { plugin, file } => Surface::Plugin { plugin, file },
        }
    }

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
    /// A remote server with no credential in its definition, which the agent
    /// therefore authenticates **itself**.
    ///
    /// §13.12, in one variant: *"do not hand provider credentials directly to
    /// the model just because the MCP server supports OAuth."* A remote MCP
    /// server that carries no `Authorization` header is not a server with no
    /// credential. Either it needs none, or the agent performs an OAuth flow
    /// against it and holds the resulting token — and a token the agent's own
    /// runtime obtained is agent-readable *by construction*, because the agent
    /// is the thing that sends it. Which of the two it is cannot be read off
    /// the definition, and the difference does not change the answer: in one
    /// case there is no credential to protect, in the other the credential is
    /// already in the model's reach.
    ///
    /// Reported rather than assumed absent, which is what `None` used to mean
    /// here. Cloudflare's own remote servers are the instance §13.12 names —
    /// `mcp.cloudflare.com`, `bindings.mcp.cloudflare.com` and the rest — and
    /// they are OAuth by default. Nothing in this file knows their names: the
    /// rule is about remote servers, and a list of hostnames would be a list
    /// that goes stale.
    AgentAuthenticates { url: String },
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
    ///
    /// [`Credential::AgentAuthenticates`] answers **yes**, and that is the
    /// P1-017 change rather than a detail. It used to be `None` — a remote
    /// server with no header reported as carrying no credential — which is an
    /// absence inferred from a file that would not mention it either way. If
    /// the server authenticates at all, the agent is what authenticates to it,
    /// and what the agent holds the agent can read.
    pub fn agent_readable(&self) -> bool {
        matches!(
            self,
            Credential::InConfig { .. }
                | Credential::FromEnvironment { .. }
                | Credential::AgentAuthenticates { .. }
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
///
/// The four surfaces are walked by [`apex_agent_core::mcpconf::read`] and not
/// here. This adds what a *report* needs and a launcher does not — where the
/// credential is, and what shape it has — over definitions the session
/// launcher reads from the same function, so `apex mcp list` and the file a
/// session is actually started with cannot disagree about which servers exist.
pub fn discover(home: &Path, cwd: Option<&Path>) -> Vec<Server> {
    let env = Environment::read(home);
    mcpconf::read(home, cwd)
        .into_iter()
        .map(|d| {
            let transport = transport_of_value(&d.def);
            Server {
                name: d.name,
                key: d.key,
                credential: credential_of_value(&d.def, &transport, &env),
                surface: Surface::from_origin(d.origin),
                transport,
            }
        })
        .collect()
}

/// Each enabled plugin's `.mcp.json`, when it has one.
///
/// One line, for the reason [`confined`] is one line: the launcher and the
/// listing must not read different files.
pub fn enabled_plugin_configs(home: &Path) -> Vec<(String, PathBuf)> {
    mcpconf::enabled_plugin_configs(home)
}

fn transport_of_value(def: &Value) -> Transport {
    match def.as_object() {
        Some(map) => transport_of(map),
        None => Transport::Other("not an object".to_string()),
    }
}

fn credential_of_value(def: &Value, transport: &Transport, env: &Environment) -> Credential {
    match def.as_object() {
        Some(map) => credential_of(map, transport, env),
        None => Credential::None,
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
    // Nothing in the definition. For a program the agent starts that is the
    // whole story — a stdio server's secrets, if it has any, are its own
    // business and its `env` block is already covered above. For an ENDPOINT
    // it is not: something authenticates to a remote server, and if the
    // definition does not, the agent does.
    match transport {
        Transport::Endpoint { url, .. } => Credential::AgentAuthenticates { url: url.clone() },
        _ => Credential::None,
    }
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
pub fn as_json(found: &[Server], launch: &mcpconf::Curated) -> Value {
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
                Credential::AgentAuthenticates { url } => {
                    serde_json::json!({"where": "agent", "url": url})
                }
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
            let wrap = launch
                .decisions
                .iter()
                .find(|d| d.name == s.name)
                .and_then(|d| d.confined);
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
                // P1-028: `transport` says how the agent reaches it, `plane`
                // says which side of this machine it is on. They are not the
                // same question — a reader who had only `transport` would have
                // to know that "stdio" means confinable and that every other
                // value means the credential is the only boundary.
                "plane": crate::connector::Plane::of(&s.transport).tag(),
                "transport": transport,
                "address": address,
                "credential": credential,
                "agentReadable": s.credential.agent_readable(),
                "confined": confined,
                // `confined` above is what the DEFINITION says; this is what
                // the session launcher decided, which is not the same fact for
                // a third-party definition it wraps at launch. Both are here
                // because a consumer that only had the first would report a
                // plugin's server as unconfined in a session that confines it,
                // and one that only had the second would report a sandbox that
                // a hand-run session does not get.
                "sandbox": {
                    "state": wrap.map_or("unknown", |w| w.tag()),
                    "sandboxed": wrap.and_then(|w| w.confined()),
                    "survivesAHandRun": wrap.is_some_and(|w| w.survives_a_hand_run()),
                },
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
///
/// One line, because the argv shape belongs to
/// [`apex_agent_core::mcpconf`] now: `mcp/confine.rs` writes that shape into a
/// definition, the session launcher writes it into a curated configuration,
/// and this recognises it. Three writers and one reader agreeing by
/// coincidence is how a wrapper stops being seen.
pub fn confined(command: &str, args: &[String]) -> Option<(String, Vec<String>)> {
    apex_agent_core::mcpconf::unwrap_wrapped(command, args)
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
        let doc = as_json(
            &discover(&home, None),
            &crate::connector::launch_verdicts(&home, None),
        );
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

        // `confined` is what the DEFINITION says; `sandbox` is what the
        // launcher decided. For the user's own definitions those agree, and
        // the point of carrying both is the case below where they do not.
        assert_eq!(wrapped["sandbox"]["state"], "definition");
        assert_eq!(wrapped["sandbox"]["survivesAHandRun"], Value::Bool(true));
        assert_eq!(by_name("plain")["sandbox"]["state"], "none");
        assert_eq!(by_name("remote")["sandbox"]["state"], "no_process");
        std::fs::remove_dir_all(&home).ok();
    }

    #[test]
    fn the_listing_does_not_call_a_plugins_wrapped_server_unsandboxed() {
        // A plugin's `.mcp.json` naming a bare program. Nothing on disk wraps
        // it, so `confined` is null and the listing used to stop there and say
        // "none — everything the session has" — false of every session
        // `apex agent` starts, which wraps every third-party definition.
        let home = fixture("plugin-atlaunch");
        let install = home.join("plug");
        std::fs::create_dir_all(&install).expect("mkdir");
        write(
            &install.join(".mcp.json"),
            &serde_json::json!({"mcpServers": {"srv": {"command": "node", "args": ["s.js"]}}})
                .to_string(),
        );
        std::fs::create_dir_all(home.join(".claude/plugins")).expect("mkdir");
        write(
            &home.join(".claude/settings.json"),
            &serde_json::json!({"enabledPlugins": {"p@mk": true}}).to_string(),
        );
        write(
            &home.join(".claude/plugins/installed_plugins.json"),
            &serde_json::json!({"version": 2, "plugins": {
                "p@mk": [{"installPath": install.display().to_string(), "version": "1"}]
            }})
            .to_string(),
        );

        let found = discover(&home, None);
        let launch = crate::connector::launch_verdicts(&home, None);
        let doc = as_json(&found, &launch);
        let srv = doc["servers"]
            .as_array()
            .expect("servers")
            .iter()
            .find(|s| s["name"] == "plugin:p:srv")
            .expect("the plugin's server")
            .clone();

        // The definition on disk really is bare — so this is not the wrapped
        // case wearing a different name.
        assert!(srv["confined"].is_null(), "{srv}");
        // And the launcher wraps it, only where the launcher runs.
        assert_eq!(srv["sandbox"]["state"], "launch");
        assert_eq!(srv["sandbox"]["sandboxed"], Value::Bool(true));
        assert_eq!(srv["sandbox"]["survivesAHandRun"], Value::Bool(false));
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
        // …and the fifth answer, which is a yes: see the variant's own note.
        assert!(Credential::AgentAuthenticates {
            url: "https://mcp.cloudflare.com/mcp".into()
        }
        .agent_readable());
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
    fn a_remote_server_with_no_header_authenticates_through_the_agent_itself() {
        // P1-017 / §13.12. `{"type":"http","url":"https://bindings.mcp.
        // cloudflare.com/mcp"}` is what `claude mcp add` writes for a server
        // that uses OAuth — the whole point of OAuth being that no token goes
        // in the file. Reading that as "carries no credential" is an absence
        // inferred from a file that would not mention it either way, and the
        // consequence is the criterion reading as met: the agent holds the
        // token it obtained, and what the agent holds the agent can read.
        //
        // Which of "needs no auth" and "the agent authenticates" it is cannot
        // be read off the definition, and it does not change the answer: in
        // one case there is nothing to protect, in the other the credential is
        // already in the model's reach.
        let env = Environment {
            from_settings: BTreeMap::new(),
        };
        let remote = serde_json::json!({
            "type": "http",
            "url": "https://bindings.mcp.cloudflare.com/mcp"
        });
        let transport = transport_of_value(&remote);
        match credential_of_value(&remote, &transport, &env) {
            Credential::AgentAuthenticates { url } => {
                assert_eq!(url, "https://bindings.mcp.cloudflare.com/mcp")
            }
            other => panic!("a remote server's OAuth token was reported as absent: {other:?}"),
        }
        assert!(
            credential_of_value(&remote, &transport, &env).agent_readable(),
            "the criterion read as met for a server the agent authenticates to"
        );

        // A program the agent starts is the other case, and it stays `None`: a
        // stdio server's own secrets are its own business, and its `env` block
        // is already classified above this line.
        let stdio = serde_json::json!({"type": "stdio", "command": "some-server"});
        let transport = transport_of_value(&stdio);
        assert_eq!(
            credential_of_value(&stdio, &transport, &env),
            Credential::None
        );

        // And a remote server whose definition DOES carry a header is still
        // reported by its header, not swallowed by the new arm.
        let with_header = serde_json::json!({
            "type": "http",
            "url": "https://example.invalid/mcp",
            "headers": {"Authorization": "Bearer abc"}
        });
        let transport = transport_of_value(&with_header);
        assert!(matches!(
            credential_of_value(&with_header, &transport, &env),
            Credential::InConfig { .. }
        ));

        // Nor is a brokered one, whose definition is stdio by construction.
        let bridged = serde_json::json!({
            "type": "stdio", "command": "apex",
            "args": ["mcp", "bridge", "cf-bindings"]
        });
        let transport = transport_of_value(&bridged);
        assert!(!credential_of_value(&bridged, &transport, &env).agent_readable());

        // The document `apex mcp list --json` emits has to say it too, and
        // with its own word: a consumer branching on `where` must be able to
        // tell "nothing to protect" from "the model holds it".
        let doc = as_json(
            &[Server {
                name: "cf".into(),
                key: "cf".into(),
                surface: Surface::User,
                transport: Transport::Endpoint {
                    kind: "http".into(),
                    url: "https://bindings.mcp.cloudflare.com/mcp".into(),
                },
                credential: Credential::AgentAuthenticates {
                    url: "https://bindings.mcp.cloudflare.com/mcp".into(),
                },
            }],
            &mcpconf::Curated {
                document: serde_json::json!({}),
                decisions: Vec::new(),
            },
        );
        assert_eq!(doc["servers"][0]["credential"]["where"], "agent");
        assert_eq!(doc["servers"][0]["agentReadable"], true);
        assert_eq!(doc["agentReadable"], 1);
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
