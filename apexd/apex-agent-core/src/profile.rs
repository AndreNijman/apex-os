//! An agent installation as a profile, not merely an executable (§5).
//!
//! `claude` is not a binary the runtime launches; it is a binary plus a
//! directory of instructions, skills, commands, plugins and MCP definitions
//! that decide what that binary does. Two machines running the same version
//! behave differently because that directory differs. §5 calls the directory a
//! profile and asks for one thing above all: know which of it is reusable and
//! which belongs to this machine alone.
//!
//! ## Four classes, and why two of them had to be added
//!
//! * [`Class::Reusable`] — the same on every machine you would want it on: an
//!   instruction file, a skill, a slash command.
//! * [`Class::MachineLocal`] — true only here: conversation transcripts, shell
//!   snapshots, caches, install paths, the account this machine logged in as.
//! * [`Class::Mixed`] — both, in one file. Carried in part; its [`Edit`]s say
//!   which part.
//! * [`Class::Secret`] — a credential. Neither reusable nor a thing to copy
//!   about, and the export must never carry one.
//!
//! Reusable and machine-local are the axis §5 names. Secret is separate because
//! the two questions differ: an OAuth token is not machine-local state that
//! happens to be private, it is a capability, and §3.2 says a capability is
//! brokered rather than copied. Mixed is separate because two of the files §5
//! names are genuinely both, and a class that could not say so would have to
//! round each of them to the wrong answer — losing the MCP definitions, or
//! exporting the machine id beside them.
//!
//! ## The exclusion is a property, not a list
//!
//! [`classify`](Profile::classify) returns [`Class::MachineLocal`] for anything
//! the table does not name, and the export carries the reusable state and
//! nothing else — a [`Class::Reusable`] file whole, a [`Class::Mixed`] one down
//! to the part its [`Edit`]s leave standing. So a directory Claude invents in
//! its next release is excluded
//! the day it appears, with nobody editing anything — the same default-deny
//! reasoning [`crate::sandbox`] uses for mounts.
//!
//! That is what makes the secret rules below safe to be incomplete. They exist
//! so [`doctor`] can name the credentials it found and so a reader can see them
//! called what they are; they are not what keeps them out of the bundle. A
//! secret rule can only ever move a path from reusable to secret, never the
//! reverse, so being wrong about one costs a file in the export and never a
//! leak — [`Plan::verify`] asserts that on the artifact itself before a byte is
//! written.
//!
//! ## Mixed files
//!
//! Two files are reusable and machine-local at once, and file-level exclusion
//! cannot express that:
//!
//! * `settings.json` carries the model, the hooks and the enabled plugins —
//!   and an `env` block whose values are environment values, which is a normal
//!   place to find a token. The export keeps the *names* and drops the values:
//!   a machine importing it then knows what to ask for, which deleting the
//!   block would lose.
//! * `~/.claude.json` is mostly this machine — the account, the machine id, the
//!   per-directory history — around the one thing §5 wants, the MCP server
//!   definitions. The export takes that key and leaves the rest.
//!
//! Both are [`Edit`]s in the table rather than special cases in the exporter,
//! so what a file gives up is visible beside the file. An import merges these
//! two key by key rather than overwriting them, which is the other half of the
//! redaction: a whole-file copy would replace the target's `env` with the
//! bundle's blanks and so delete the token on the machine that had one.
//!
//! ## The one reusable thing a session can still change
//!
//! `~/.claude.json` has to be writable — Claude records onboarding state, the
//! directories it has been trusted in and a dozen caches there on every run,
//! and a read-only one is an agent that will not start. It is also where the
//! MCP definitions live. So the MCP definitions are the single piece of
//! exportable profile state a confined session can still edit, and everything
//! else reusable is mounted read-only. [`doctor`] lists the servers by name for
//! exactly this reason: a definition that appeared on its own should be
//! visible before it is exported to another machine.

use std::collections::BTreeMap;
use std::io;
use std::path::{Path, PathBuf};

use serde_json::{Map, Value};

/// Whether a piece of profile state can be carried to another machine.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Class {
    /// The same on any machine: instructions, skills, commands, definitions.
    /// Carried whole.
    Reusable,
    /// Reusable and machine-local in one file. Carried in part, and the
    /// entry's [`Edit`]s are what say which part.
    Mixed,
    /// True of this machine only: transcripts, caches, install paths, ids.
    MachineLocal,
    /// A credential. Never exported, whatever else is true of it.
    Secret,
}

impl Class {
    /// The word used in output and in the bundle manifest.
    pub fn as_str(self) -> &'static str {
        match self {
            Class::Reusable => "reusable",
            Class::Mixed => "mixed",
            Class::MachineLocal => "machine-local",
            Class::Secret => "secret",
        }
    }

    /// Whether an export carries this, whole or in part. The single place the
    /// question is answered, so no caller can decide it differently.
    pub fn exportable(self) -> bool {
        matches!(self, Class::Reusable | Class::Mixed)
    }
}

impl std::fmt::Display for Class {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// How a confined session reaches one path (P0-010).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mount {
    /// Bound read-only. The default, and everything reusable is this.
    ReadOnly,
    /// Bound read-write, because the agent writes it while it runs.
    Writable,
}

/// Which part of the profile a path is, for [`doctor`]'s report.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    /// `CLAUDE.md` and anything else the agent reads as instruction.
    Instructions,
    /// Settings the agent reads at startup.
    Config,
    /// The status line program.
    Statusline,
    /// Slash commands.
    Commands,
    /// Skills.
    Skills,
    /// Subagent definitions.
    Agents,
    /// Plugin installations and their runtime state.
    Plugins,
    /// Where plugins come from.
    Marketplaces,
    /// MCP server definitions.
    Mcp,
    /// Conversations, tasks, history — what this machine has done.
    Session,
    /// Recomputable state.
    Cache,
    /// Credentials.
    Credential,
}

impl Role {
    /// The heading this role appears under.
    pub fn as_str(self) -> &'static str {
        match self {
            Role::Instructions => "instructions",
            Role::Config => "config",
            Role::Statusline => "statusline",
            Role::Commands => "commands",
            Role::Skills => "skills",
            Role::Agents => "agents",
            Role::Plugins => "plugins",
            Role::Marketplaces => "marketplaces",
            Role::Mcp => "mcp",
            Role::Session => "session",
            Role::Cache => "cache",
            Role::Credential => "credential",
        }
    }
}

impl std::fmt::Display for Role {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// What a path is relative to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Base {
    /// Inside the profile directory, e.g. `~/.claude`.
    Root,
    /// Beside it in `$HOME`. `~/.claude.json` is the only one so far, and it
    /// holds the MCP definitions §5 asks for, so it cannot be ignored.
    Home,
}

/// One edit applied to a mixed JSON file on its way into a bundle.
///
/// Paths are key sequences and `*` matches any key, which is enough for every
/// shape these files take and stops short of being a query language nobody
/// asked for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Edit {
    /// At every object this path selects, discard each key that is not named.
    /// An empty path is the document itself.
    KeepOnly(&'static [&'static str], &'static [&'static str]),
    /// Replace each value under this path with null, keeping the names.
    RedactValues(&'static [&'static str]),
    /// Remove the key this path ends at.
    Drop(&'static [&'static str]),
}

/// The keys of an MCP server definition that are the definition.
///
/// Default-deny at the level a credential is most likely to be invented at: a
/// server is a transport, an address and the names of what it needs, and a key
/// nobody here has heard of is dropped rather than carried. `env` and `headers`
/// stay so their *names* can be kept and their values emptied — an importing
/// machine that is not told the server wants an `Authorization` header has a
/// definition it cannot use.
const MCP_KEYS: &[&str] = &[
    "type", "command", "args", "url", "cwd", "timeout", "disabled", "env", "headers",
];

/// One named piece of a profile.
#[derive(Debug, Clone, Copy)]
pub struct Entry {
    /// Path relative to [`Entry::base`].
    pub path: &'static str,
    /// What it is relative to.
    pub base: Base,
    /// Reusable, machine-local or secret.
    pub class: Class,
    /// How a confined session gets at it.
    pub mount: Mount,
    /// Which part of the profile it is.
    pub role: Role,
    /// One line, shown by `apex agent profile inspect`.
    pub what: &'static str,
    /// Edits applied before it is exported. Empty for anything copied whole.
    pub edits: &'static [Edit],
}

/// One agent's profile: where it lives and what is in it.
#[derive(Debug, Clone, Copy)]
pub struct Profile {
    /// The adapter id this belongs to.
    pub agent: &'static str,
    /// Human-facing name.
    pub display: &'static str,
    /// The profile directory, relative to `$HOME`.
    pub root: &'static str,
    /// Everything the runtime knows about. Anything absent from this list is
    /// machine-local by default and is neither exported nor mounted.
    pub entries: &'static [Entry],
}

/// Claude Code's profile, read from a real installation rather than from
/// upstream documentation: the names below are what `claude` 2.1 actually
/// keeps in `~/.claude`.
///
/// Read-only is the default and writable is the exception, so the list of
/// exceptions is the list of things Claude writes while it runs. Getting that
/// wrong does not fail visibly — it fails as an agent that starts and then
/// cannot save a shell snapshot, which reads as a bug in Claude.
pub const CLAUDE: Profile = Profile {
    agent: "claude",
    display: "Claude Code",
    root: ".claude",
    entries: &[
        // ── reusable ────────────────────────────────────────────────────────
        Entry {
            path: "CLAUDE.md",
            base: Base::Root,
            class: Class::Reusable,
            mount: Mount::ReadOnly,
            role: Role::Instructions,
            what: "user instructions applied to every project",
            edits: &[],
        },
        Entry {
            path: "settings.json",
            base: Base::Root,
            class: Class::Mixed,
            mount: Mount::ReadOnly,
            role: Role::Config,
            what: "model, hooks, permissions, enabled plugins",
            // The values under `env` become the agent's environment, which is
            // where an API token goes when someone has one.
            edits: &[Edit::RedactValues(&["env"])],
        },
        Entry {
            path: "remote-settings.json",
            base: Base::Root,
            class: Class::Reusable,
            mount: Mount::ReadOnly,
            role: Role::Config,
            what: "settings for remote-driven sessions",
            edits: &[],
        },
        Entry {
            path: "statusline.sh",
            base: Base::Root,
            class: Class::Reusable,
            mount: Mount::ReadOnly,
            role: Role::Statusline,
            what: "the status line program settings.json points at",
            edits: &[],
        },
        Entry {
            path: "commands",
            base: Base::Root,
            class: Class::Reusable,
            mount: Mount::ReadOnly,
            role: Role::Commands,
            what: "slash commands",
            edits: &[],
        },
        Entry {
            path: "skills",
            base: Base::Root,
            class: Class::Reusable,
            mount: Mount::ReadOnly,
            role: Role::Skills,
            what: "skills",
            edits: &[],
        },
        Entry {
            path: "agents",
            base: Base::Root,
            class: Class::Reusable,
            mount: Mount::ReadOnly,
            role: Role::Agents,
            what: "subagent definitions",
            edits: &[],
        },
        Entry {
            path: "plugins/known_marketplaces.json",
            base: Base::Root,
            class: Class::Mixed,
            mount: Mount::ReadOnly,
            role: Role::Marketplaces,
            what: "where each marketplace comes from",
            // The repository is the definition; the checkout path under this
            // machine's home is not, and a bundle carrying it would import a
            // path that exists on one machine.
            edits: &[
                Edit::Drop(&["*", "installLocation"]),
                Edit::Drop(&["*", "lastUpdated"]),
            ],
        },
        // ── machine-local, still mounted ────────────────────────────────────
        Entry {
            path: "plugins/marketplaces",
            base: Base::Root,
            class: Class::MachineLocal,
            mount: Mount::ReadOnly,
            role: Role::Marketplaces,
            what: "marketplace checkouts, re-fetchable from their source",
            edits: &[],
        },
        Entry {
            path: "plugins/cache",
            base: Base::Root,
            class: Class::MachineLocal,
            mount: Mount::Writable,
            role: Role::Plugins,
            what: "installed plugin trees",
            edits: &[],
        },
        Entry {
            path: "plugins/data",
            base: Base::Root,
            class: Class::MachineLocal,
            mount: Mount::Writable,
            role: Role::Plugins,
            what: "per-plugin runtime state",
            edits: &[],
        },
        Entry {
            path: "plugins/installed_plugins.json",
            base: Base::Root,
            class: Class::MachineLocal,
            mount: Mount::Writable,
            role: Role::Plugins,
            what: "which plugin version is installed where",
            edits: &[],
        },
        Entry {
            path: "plugins/.last_inuse_sweep",
            base: Base::Root,
            class: Class::MachineLocal,
            mount: Mount::Writable,
            role: Role::Plugins,
            what: "when unused plugin trees were last swept",
            edits: &[],
        },
        Entry {
            path: "projects",
            base: Base::Root,
            class: Class::MachineLocal,
            mount: Mount::Writable,
            role: Role::Session,
            what: "conversation transcripts, per project",
            edits: &[],
        },
        Entry {
            path: "sessions",
            base: Base::Root,
            class: Class::MachineLocal,
            mount: Mount::Writable,
            role: Role::Session,
            what: "session index",
            edits: &[],
        },
        Entry {
            path: "session-env",
            base: Base::Root,
            class: Class::MachineLocal,
            mount: Mount::Writable,
            role: Role::Session,
            what: "the environment each session was started with",
            edits: &[],
        },
        Entry {
            path: "shell-snapshots",
            base: Base::Root,
            class: Class::MachineLocal,
            mount: Mount::Writable,
            role: Role::Session,
            what: "the shell state the Bash tool restores per session",
            edits: &[],
        },
        Entry {
            path: "todos",
            base: Base::Root,
            class: Class::MachineLocal,
            mount: Mount::Writable,
            role: Role::Session,
            what: "task lists (Claude 1.x and early 2.x)",
            edits: &[],
        },
        Entry {
            path: "tasks",
            base: Base::Root,
            class: Class::MachineLocal,
            mount: Mount::Writable,
            role: Role::Session,
            what: "task lists",
            edits: &[],
        },
        Entry {
            path: "jobs",
            base: Base::Root,
            class: Class::MachineLocal,
            mount: Mount::Writable,
            role: Role::Session,
            what: "background job output",
            edits: &[],
        },
        Entry {
            path: "file-history",
            base: Base::Root,
            class: Class::MachineLocal,
            mount: Mount::Writable,
            role: Role::Session,
            what: "copies of files edited, for undo",
            edits: &[],
        },
        Entry {
            path: "history.jsonl",
            base: Base::Root,
            class: Class::MachineLocal,
            mount: Mount::Writable,
            role: Role::Session,
            what: "prompt history",
            edits: &[],
        },
        Entry {
            path: "paste-cache",
            base: Base::Root,
            class: Class::MachineLocal,
            mount: Mount::Writable,
            role: Role::Cache,
            what: "pasted content held for the current turn",
            edits: &[],
        },
        Entry {
            path: "backups",
            base: Base::Root,
            class: Class::MachineLocal,
            mount: Mount::Writable,
            role: Role::Cache,
            what: "settings written before an automatic change",
            edits: &[],
        },
        Entry {
            path: "cache",
            base: Base::Root,
            class: Class::MachineLocal,
            mount: Mount::Writable,
            role: Role::Cache,
            what: "release notes, changelog, fetched metadata",
            edits: &[],
        },
        Entry {
            path: "statsig",
            base: Base::Root,
            class: Class::MachineLocal,
            mount: Mount::Writable,
            role: Role::Cache,
            what: "feature flag cache (Claude 1.x and early 2.x)",
            edits: &[],
        },
        Entry {
            path: "stats-cache.json",
            base: Base::Root,
            class: Class::MachineLocal,
            mount: Mount::Writable,
            role: Role::Cache,
            what: "usage counters",
            edits: &[],
        },
        Entry {
            path: "policy-limits.json",
            base: Base::Root,
            class: Class::MachineLocal,
            mount: Mount::Writable,
            role: Role::Cache,
            what: "the account's current rate limits",
            edits: &[],
        },
        Entry {
            path: ".last-cleanup",
            base: Base::Root,
            class: Class::MachineLocal,
            mount: Mount::Writable,
            role: Role::Cache,
            what: "when old session state was last removed",
            edits: &[],
        },
        Entry {
            path: ".last-update-result.json",
            base: Base::Root,
            class: Class::MachineLocal,
            mount: Mount::Writable,
            role: Role::Cache,
            what: "the outcome of the last self-update",
            edits: &[],
        },
        Entry {
            path: ".claude.json",
            base: Base::Root,
            class: Class::MachineLocal,
            mount: Mount::Writable,
            role: Role::Cache,
            what: "per-directory state written beside the profile",
            edits: &[],
        },
        Entry {
            path: "daemon.lock",
            base: Base::Root,
            class: Class::MachineLocal,
            mount: Mount::Writable,
            role: Role::Session,
            what: "the local daemon's lock file",
            edits: &[],
        },
        Entry {
            path: "daemon.status.json",
            base: Base::Root,
            class: Class::MachineLocal,
            mount: Mount::Writable,
            role: Role::Session,
            what: "the local daemon's last reported status",
            edits: &[],
        },
        // ── the sidecar in $HOME ────────────────────────────────────────────
        Entry {
            path: ".claude.json",
            base: Base::Home,
            class: Class::Mixed,
            // Writable, and the one exportable thing that is: Claude records
            // onboarding state, trusted directories and caches here on every
            // run, so a read-only one is a session that will not start.
            mount: Mount::Writable,
            role: Role::Mcp,
            what: "account, machine id, per-project history — and MCP servers",
            edits: &[
                Edit::KeepOnly(&[], &["mcpServers"]),
                Edit::KeepOnly(&["mcpServers", "*"], MCP_KEYS),
                Edit::RedactValues(&["mcpServers", "*", "env"]),
                // An HTTP server's bearer token lives here, not in `env`. Found
                // by exporting a real profile and reading the bundle.
                Edit::RedactValues(&["mcpServers", "*", "headers"]),
            ],
        },
        // ── credentials ─────────────────────────────────────────────────────
        Entry {
            path: ".credentials.json",
            base: Base::Root,
            class: Class::Secret,
            mount: Mount::Writable,
            role: Role::Credential,
            what: "the account's OAuth tokens",
            edits: &[],
        },
        Entry {
            path: "daemon",
            base: Base::Root,
            class: Class::Secret,
            mount: Mount::Writable,
            role: Role::Credential,
            what: "the local daemon's control key and dispatch queue",
            edits: &[],
        },
    ],
};

/// Every profile the runtime knows.
pub const PROFILES: &[Profile] = &[CLAUDE];

/// The profile for an adapter id, when it has one.
pub fn by_agent(id: &str) -> Option<&'static Profile> {
    PROFILES.iter().find(|p| p.agent == id)
}

/// Names that are a credential wherever they turn up.
///
/// A safety net over the table, not the mechanism: the export carries
/// [`Class::Reusable`] only, so anything this misses is already excluded for
/// being unlisted. Matching here can therefore only ever exclude more, which is
/// the direction a mistake is allowed to go in.
fn secret_component(name: &str) -> bool {
    // `.credentials.json`, and the `.bak-oauthproxy` copy an upgrade left
    // beside it — the second is why this is a prefix and not equality.
    name.starts_with(".credentials.json")
        // Session keys: `sessions/2.<hash>.key`, `daemon/control.key`.
        || name.ends_with(".key")
        || name == "daemon"
}

/// Whether any component of `rel` is a credential by name.
pub fn looks_secret(rel: &Path) -> bool {
    rel.components()
        .any(|c| secret_component(&c.as_os_str().to_string_lossy()))
}

impl Profile {
    /// The profile directory on this machine.
    pub fn root_dir(&self, home: &Path) -> PathBuf {
        home.join(self.root)
    }

    /// Where one entry lives on this machine.
    pub fn entry_path(&self, home: &Path, entry: &Entry) -> PathBuf {
        match entry.base {
            Base::Root => self.root_dir(home).join(entry.path),
            Base::Home => home.join(entry.path),
        }
    }

    /// The table entry covering `rel`, which is relative to `base`.
    ///
    /// The longest match wins, so `plugins/known_marketplaces.json` is found
    /// before the `plugins/cache` sibling and a file deep inside `skills` is
    /// found through the `skills` entry.
    pub fn entry_for(&self, base: Base, rel: &Path) -> Option<&'static Entry> {
        let want: Vec<_> = rel.components().collect();
        self.entries
            .iter()
            .filter(|e| e.base == base)
            .filter(|e| {
                let have: Vec<_> = Path::new(e.path).components().collect();
                have.len() <= want.len() && want[..have.len()] == have[..]
            })
            .max_by_key(|e| Path::new(e.path).components().count())
    }

    /// Classify a path relative to `base`.
    ///
    /// Unlisted is machine-local: that is the whole of the reusable/local
    /// split's safety. A credential by name is secret whatever the table says,
    /// because that rule may only ever take something out of the export.
    pub fn classify(&self, base: Base, rel: &Path) -> Class {
        if looks_secret(rel) {
            return Class::Secret;
        }
        self.entry_for(base, rel)
            .map(|e| e.class)
            .unwrap_or(Class::MachineLocal)
    }

    /// Home-relative paths a confined session gets, split by how.
    ///
    /// The sandbox reads this rather than a second list of its own, so a path
    /// classified reusable here cannot be mounted writable there.
    pub fn mounts(&self) -> (Vec<String>, Vec<String>) {
        let mut ro = Vec::new();
        let mut rw = Vec::new();
        for e in self.entries {
            let rel = match e.base {
                Base::Root => format!("{}/{}", self.root, e.path),
                Base::Home => e.path.to_string(),
            };
            match e.mount {
                Mount::ReadOnly => ro.push(rel),
                Mount::Writable => rw.push(rel),
            }
        }
        (ro, rw)
    }
}

// ── export ──────────────────────────────────────────────────────────────────

/// One file an export would carry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlanItem {
    /// Where it is read from.
    pub source: PathBuf,
    /// Where it goes inside the bundle, relative to the bundle root.
    pub bundle: PathBuf,
    /// Its class. Always an exportable one — [`Class::Reusable`] carried whole
    /// or [`Class::Mixed`] carried in part; [`Plan::verify`] is what makes that
    /// a checked fact rather than a comment.
    pub class: Class,
    /// Edits applied on the way in.
    pub edits: &'static [Edit],
}

/// Everything an export would carry, before anything is written.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Plan {
    /// The agent this is a profile of.
    pub agent: String,
    /// The files, in a stable order.
    pub items: Vec<PlanItem>,
    /// Paths the walk found and refused, with the class that refused them.
    /// Reported by `export`, so nothing is silently dropped.
    pub excluded: Vec<(PathBuf, Class)>,
}

/// A plan that would have carried something it must not.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NotExportable {
    /// The offending bundle path.
    pub path: PathBuf,
    /// What it actually is.
    pub class: Class,
}

impl std::fmt::Display for NotExportable {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{} is {} and a portable profile carries reusable state only",
            self.path.display(),
            self.class
        )
    }
}

impl std::error::Error for NotExportable {}

impl Plan {
    /// Re-derive the class of every item and refuse the plan if any of them is
    /// not reusable.
    ///
    /// The export is already an allowlist walk, so this can only fire on a
    /// mistake — which is the point. It is the post-condition a test asserts
    /// and the last check before the bundle is written, so "secrets are
    /// excluded" is a property of the artifact and not a property of whoever
    /// last edited the table.
    pub fn verify(&self, profile: &Profile) -> Result<(), NotExportable> {
        for item in &self.items {
            let (base, rel) = split_bundle_path(&item.bundle);
            let class = profile.classify(base, rel);
            if !class.exportable() || !item.class.exportable() {
                return Err(NotExportable {
                    path: item.bundle.clone(),
                    class: if class.exportable() { item.class } else { class },
                });
            }
        }
        Ok(())
    }
}

/// Bundle layout: `profile/…` for the profile directory, `home/…` for the
/// files beside it. Two roots rather than one because an import has to put them
/// in different places and a bundle that cannot say which is which would guess.
const BUNDLE_ROOT: &str = "profile";
const BUNDLE_HOME: &str = "home";

fn bundle_path(base: Base, rel: &Path) -> PathBuf {
    match base {
        Base::Root => Path::new(BUNDLE_ROOT).join(rel),
        Base::Home => Path::new(BUNDLE_HOME).join(rel),
    }
}

fn split_bundle_path(p: &Path) -> (Base, &Path) {
    match p.strip_prefix(BUNDLE_HOME) {
        Ok(rest) => (Base::Home, rest),
        Err(_) => (Base::Root, p.strip_prefix(BUNDLE_ROOT).unwrap_or(p)),
    }
}

/// Work out what an export of `profile` from `home` would carry.
///
/// Walks the reusable entries only. A directory entry contributes its files;
/// anything under it that classifies as a credential by name is left behind and
/// recorded in [`Plan::excluded`] rather than dropped in silence.
///
/// Symbolic links are not followed and not carried: a link is a statement about
/// this machine's filesystem, and following one out of the profile is how an
/// export grows a copy of something nobody meant to send.
pub fn plan_export(profile: &Profile, home: &Path) -> io::Result<Plan> {
    let mut items = Vec::new();
    let mut excluded = Vec::new();

    for entry in profile.entries {
        let source = profile.entry_path(home, entry);
        if !entry.class.exportable() {
            if source.symlink_metadata().is_ok() {
                excluded.push((source, entry.class));
            }
            continue;
        }
        let meta = match source.symlink_metadata() {
            Ok(m) => m,
            Err(_) => continue, // not installed here; nothing to carry
        };
        if meta.file_type().is_symlink() {
            excluded.push((source, Class::MachineLocal));
            continue;
        }
        if meta.is_dir() {
            let mut found = Vec::new();
            walk(&source, &source, &mut found)?;
            found.sort();
            for rel_in_entry in found {
                let rel = Path::new(entry.path).join(&rel_in_entry);
                let class = profile.classify(entry.base, &rel);
                let full = source.join(&rel_in_entry);
                if !class.exportable() {
                    excluded.push((full, class));
                    continue;
                }
                items.push(PlanItem {
                    source: full,
                    bundle: bundle_path(entry.base, &rel),
                    class,
                    edits: entry.edits,
                });
            }
        } else {
            let rel = Path::new(entry.path);
            let class = profile.classify(entry.base, rel);
            if !class.exportable() {
                excluded.push((source, class));
                continue;
            }
            items.push(PlanItem {
                source,
                bundle: bundle_path(entry.base, rel),
                class,
                edits: entry.edits,
            });
        }
    }

    items.sort_by(|a, b| a.bundle.cmp(&b.bundle));
    excluded.sort();
    excluded.dedup();
    Ok(Plan {
        agent: profile.agent.to_string(),
        items,
        excluded,
    })
}

/// Collect the files under `dir`, as paths relative to `base`.
fn walk(base: &Path, dir: &Path, out: &mut Vec<PathBuf>) -> io::Result<()> {
    for e in std::fs::read_dir(dir)? {
        let e = e?;
        let path = e.path();
        let ft = e.file_type()?;
        if ft.is_symlink() {
            continue;
        }
        if ft.is_dir() {
            walk(base, &path, out)?;
        } else if let Ok(rel) = path.strip_prefix(base) {
            out.push(rel.to_path_buf());
        }
    }
    Ok(())
}

/// What an export did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExportReport {
    /// Where the bundle was written.
    pub dest: PathBuf,
    /// How many files it carries.
    pub files: usize,
    /// Total bytes written.
    pub bytes: u64,
    /// Files that were edited on the way in, and what each gave up.
    pub edited: Vec<(PathBuf, String)>,
    /// What the walk found and left behind.
    pub excluded: Vec<(PathBuf, Class)>,
}

/// Write a portable bundle of `profile` into `dest`.
///
/// The plan is verified before the directory is created, so a profile that
/// cannot be exported cleanly leaves nothing half-written behind.
pub fn export(profile: &Profile, home: &Path, dest: &Path) -> io::Result<ExportReport> {
    let plan = plan_export(profile, home)?;
    plan.verify(profile)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e.to_string()))?;

    let mut files = 0usize;
    let mut bytes = 0u64;
    let mut edited = Vec::new();
    let mut manifest = Vec::new();

    for item in &plan.items {
        let target = dest.join(&item.bundle);
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let raw = std::fs::read(&item.source)?;
        let (out, gave_up) = if item.edits.is_empty() {
            (raw, Vec::new())
        } else {
            apply_edits(&raw, item.edits)?
        };
        if !gave_up.is_empty() {
            edited.push((item.bundle.clone(), gave_up.join(", ")));
        }
        std::fs::write(&target, &out)?;
        copy_exec_bit(&item.source, &target)?;
        files += 1;
        bytes += out.len() as u64;
        manifest.push(serde_json::json!({
            "path": item.bundle.to_string_lossy(),
            "class": item.class.as_str(),
            "bytes": out.len(),
            "edited": !gave_up.is_empty(),
        }));
    }

    let doc = serde_json::json!({
        "version": BUNDLE_VERSION,
        "agent": profile.agent,
        "root": profile.root,
        "files": manifest,
    });
    std::fs::create_dir_all(dest)?;
    std::fs::write(
        dest.join(MANIFEST),
        format!("{}\n", serde_json::to_string_pretty(&doc)?),
    )?;

    Ok(ExportReport {
        dest: dest.to_path_buf(),
        files,
        bytes,
        edited,
        excluded: plan.excluded,
    })
}

/// The bundle format. Bumped when an import would misread an older bundle.
pub const BUNDLE_VERSION: u32 = 1;

/// The file naming a bundle as one.
pub const MANIFEST: &str = "manifest.json";

/// A statusline is a program: an export that dropped its executable bit would
/// import a profile whose status line silently does nothing.
fn copy_exec_bit(from: &Path, to: &Path) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let mode = std::fs::metadata(from)?.permissions().mode();
    if mode & 0o111 != 0 {
        std::fs::set_permissions(to, std::fs::Permissions::from_mode(0o755))?;
    }
    Ok(())
}

/// Key names whose value is a credential, wherever a mixed file puts one.
///
/// The JSON counterpart of [`secret_component`], and there for the same reason.
/// A path the table does not name is machine-local by default, so the *file*
/// list is an allowlist; but a mixed file is carried by naming what it gives
/// up, and that is a blocklist, which is wrong the moment upstream adds a key.
///
/// This is not theoretical. Exporting a real profile put an MCP server's bearer
/// token in the bundle: it sits in `headers.Authorization`, and the entry's
/// edits had been written against `env`. The entry now names `headers` too, and
/// this net is what stops the next such key from waiting to be noticed.
///
/// Names are kept and only values are emptied, so a false positive costs an
/// importing machine one value it has to supply and never costs a leak.
fn secret_key(name: &str) -> bool {
    let name = name.to_ascii_lowercase().replace(['-', '_', ' '], "");
    [
        "token",
        "secret",
        "password",
        "passwd",
        "apikey",
        "authorization",
        "credential",
        "bearer",
        "privatekey",
        "accesskey",
        "sessionkey",
    ]
    .iter()
    .any(|needle| name.contains(needle))
}

/// Null every scalar at or below `v`, and answer with the names emptied.
///
/// Structure and names survive: a key called `credentials` holding a map of
/// account names to keys becomes the same map with the keys gone, which is what
/// tells the importing machine which accounts it has to fill in.
fn blank_scalars(v: &mut Value, name: &str, out: &mut Vec<String>) {
    match v {
        Value::Object(map) => {
            for (k, inner) in map.iter_mut() {
                let k = k.clone();
                blank_scalars(inner, &k, out);
            }
        }
        Value::Array(items) => {
            for inner in items.iter_mut() {
                blank_scalars(inner, name, out);
            }
        }
        Value::Null => {}
        scalar => {
            *scalar = Value::Null;
            out.push(name.to_string());
        }
    }
}

/// Walk the whole document and empty the value of every key that is a
/// credential by name. Answers with the names emptied.
fn blank_secret_keys(v: &mut Value, out: &mut Vec<String>) {
    match v {
        Value::Object(map) => {
            for (k, inner) in map.iter_mut() {
                if secret_key(k) {
                    let k = k.clone();
                    blank_scalars(inner, &k, out);
                } else {
                    blank_secret_keys(inner, out);
                }
            }
        }
        Value::Array(items) => {
            for inner in items.iter_mut() {
                blank_secret_keys(inner, out);
            }
        }
        _ => {}
    }
}

/// Apply a mixed file's edits, returning the new bytes and a description of
/// what was given up, for the export report.
fn apply_edits(raw: &[u8], edits: &[Edit]) -> io::Result<(Vec<u8>, Vec<String>)> {
    let mut doc: Value = serde_json::from_slice(raw)?;
    let mut gave_up = Vec::new();
    for edit in edits {
        match edit {
            Edit::KeepOnly(path, keys) => {
                let mut dropped = 0usize;
                visit(&mut doc, path, &mut |v| {
                    if let Some(map) = v.as_object_mut() {
                        dropped += map.keys().filter(|k| !keys.contains(&k.as_str())).count();
                        map.retain(|k, _| keys.contains(&k.as_str()));
                    }
                });
                if dropped > 0 {
                    gave_up.push(format!("{dropped} machine-local keys"));
                }
            }
            Edit::RedactValues(path) => {
                let mut names = Vec::new();
                visit(&mut doc, path, &mut |v| {
                    if let Some(map) = v.as_object_mut() {
                        for (k, val) in map.iter_mut() {
                            if !val.is_null() {
                                names.push(k.clone());
                                *val = Value::Null;
                            }
                        }
                    }
                });
                if !names.is_empty() {
                    names.sort();
                    names.dedup();
                    gave_up.push(format!("values of {}", names.join(", ")));
                }
            }
            Edit::Drop(path) => {
                let (parent, last) = match path.split_last() {
                    Some((last, parent)) => (parent, *last),
                    None => continue,
                };
                let mut hit = false;
                visit(&mut doc, parent, &mut |v| {
                    if let Some(map) = v.as_object_mut() {
                        hit |= map.remove(last).is_some();
                    }
                });
                if hit {
                    gave_up.push(last.to_string());
                }
            }
        }
    }
    // Last, over whatever the edits left: the net that does not depend on
    // anyone having remembered a key.
    let mut caught = Vec::new();
    blank_secret_keys(&mut doc, &mut caught);
    if !caught.is_empty() {
        caught.sort();
        caught.dedup();
        gave_up.push(format!("values of {}", caught.join(", ")));
    }
    let mut out = serde_json::to_vec_pretty(&doc)?;
    out.push(b'\n');
    Ok((out, gave_up))
}

/// Call `f` on every value `path` selects, where `*` matches any key.
fn visit(value: &mut Value, path: &[&str], f: &mut impl FnMut(&mut Value)) {
    let Some((head, rest)) = path.split_first() else {
        f(value);
        return;
    };
    let Some(map) = value.as_object_mut() else {
        return;
    };
    if *head == "*" {
        for (_, v) in map.iter_mut() {
            visit(v, rest, f);
        }
    } else if let Some(v) = map.get_mut(*head) {
        visit(v, rest, f);
    }
}

// ── import ──────────────────────────────────────────────────────────────────

/// One thing an import would do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Change {
    /// A file that is not on this machine yet.
    Create(PathBuf),
    /// A file whose bytes differ.
    Update(PathBuf),
    /// A JSON file whose named keys are merged in, leaving the rest alone.
    Merge(PathBuf, Vec<String>),
    /// An environment variable the bundle named but could not carry, because
    /// its value was a secret on the machine that exported it.
    Unfilled(PathBuf, String),
}

impl std::fmt::Display for Change {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Change::Create(p) => write!(f, "create  {}", p.display()),
            Change::Update(p) => write!(f, "update  {}", p.display()),
            Change::Merge(p, keys) => write!(f, "merge   {} ({})", p.display(), keys.join(", ")),
            Change::Unfilled(p, name) => {
                write!(f, "unset   {name} — {} names it, the bundle carried no value", p.display())
            }
        }
    }
}

/// Files an import merges key by key instead of overwriting. Exactly the mixed
/// files: the ones an export redacted are the ones a whole-file copy would
/// damage. See [`merged`].
fn merges(bundle_rel: &Path) -> bool {
    let name = bundle_rel.file_name().unwrap_or_default().to_string_lossy();
    matches!(
        name.as_ref(),
        "settings.json" | ".claude.json" | "known_marketplaces.json"
    )
}

/// Work out what importing the bundle at `src` into `home` would do.
pub fn plan_import(profile: &Profile, src: &Path, home: &Path) -> io::Result<Vec<Change>> {
    let manifest = std::fs::read(src.join(MANIFEST))?;
    let doc: Value = serde_json::from_slice(&manifest)?;
    let agent = doc.get("agent").and_then(Value::as_str).unwrap_or_default();
    if agent != profile.agent {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!(
                "this bundle is a {agent} profile and {} was asked for",
                profile.agent
            ),
        ));
    }
    let version = doc.get("version").and_then(Value::as_u64).unwrap_or(0);
    if version > BUNDLE_VERSION as u64 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!(
                "this bundle is format {version} and this APEX reads {BUNDLE_VERSION}; \
                 update APEX to import it"
            ),
        ));
    }

    let mut changes = Vec::new();
    let files = doc.get("files").and_then(Value::as_array).cloned().unwrap_or_default();
    for f in files {
        let Some(rel) = f.get("path").and_then(Value::as_str) else {
            continue;
        };
        let rel = PathBuf::from(rel);
        let (base, inner) = split_bundle_path(&rel);
        // The import is an allowlist too: a bundle is a file someone sent, and
        // a manifest naming `../.ssh/authorized_keys` must reach nothing.
        if !profile.classify(base, inner).exportable() {
            continue;
        }
        let target = match base {
            Base::Root => profile.root_dir(home).join(inner),
            Base::Home => home.join(inner),
        };
        let from = src.join(&rel);
        let incoming = std::fs::read(&from)?;

        if merges(&rel) && target.exists() {
            let keys = merge_keys(&incoming, &std::fs::read(&target)?)?;
            if !keys.is_empty() {
                changes.push(Change::Merge(target.clone(), keys));
            }
        } else if !target.exists() {
            changes.push(Change::Create(target.clone()));
        } else if std::fs::read(&target)? != incoming {
            changes.push(Change::Update(target.clone()));
        }

        for name in redacted_names(&incoming) {
            changes.push(Change::Unfilled(target.clone(), name));
        }
    }
    Ok(changes)
}

/// Merge `incoming` into `existing`, and answer with the result.
///
/// A null is a redaction, never a value: the export writes one where it refused
/// to carry something, so a null must leave what is already there alone and
/// must not be written where there is nothing. Without that rule an import
/// deletes exactly the secrets the export protected — the null under
/// `settings.json` → `env` would land on top of a real token.
///
/// Everything else merges by key, one level at a time, so importing a profile
/// adds its MCP servers and its model without taking this machine's own
/// settings with it.
fn merged(incoming: &Value, existing: &Value) -> Value {
    let (Some(a), Some(b)) = (incoming.as_object(), existing.as_object()) else {
        return strip_nulls(incoming);
    };
    let mut out = b.clone();
    for (k, v) in a {
        if v.is_null() {
            continue;
        }
        match (out.get(k), v.is_object()) {
            (Some(have), true) if have.is_object() => {
                out.insert(k.clone(), merged(v, have));
            }
            _ => {
                out.insert(k.clone(), strip_nulls(v));
            }
        }
    }
    Value::Object(out)
}

/// A copy of `v` with every redacted key removed, for the case where a merge
/// has nothing to merge into and would otherwise write the blanks out.
fn strip_nulls(v: &Value) -> Value {
    match v.as_object() {
        Some(map) => Value::Object(
            map.iter()
                .filter(|(_, v)| !v.is_null())
                .map(|(k, v)| (k.clone(), strip_nulls(v)))
                .collect(),
        ),
        None => v.clone(),
    }
}

/// The top-level keys a merge would actually change.
fn merge_keys(incoming: &[u8], existing: &[u8]) -> io::Result<Vec<String>> {
    let a: Value = serde_json::from_slice(incoming)?;
    let b: Value = serde_json::from_slice(existing).unwrap_or(Value::Object(Map::new()));
    let after = merged(&a, &b);
    let (Some(after), Some(before)) = (after.as_object(), b.as_object()) else {
        return Ok(Vec::new());
    };
    Ok(after
        .iter()
        .filter(|(k, v)| before.get(*k) != Some(*v))
        .map(|(k, _)| k.clone())
        .collect())
}

/// Names whose values a redaction emptied, so an import can say what is still
/// needed instead of writing a null into somebody's settings.
fn redacted_names(bytes: &[u8]) -> Vec<String> {
    let Ok(doc) = serde_json::from_slice::<Value>(bytes) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    collect_nulls(&doc, &mut out);
    out.sort();
    out.dedup();
    out
}

fn collect_nulls(v: &Value, out: &mut Vec<String>) {
    if let Some(map) = v.as_object() {
        for (k, val) in map {
            if val.is_null() {
                out.push(k.clone());
            } else {
                collect_nulls(val, out);
            }
        }
    }
}

/// Apply a bundle. Returns what it did, in the same shape [`plan_import`]
/// returns what it would do.
pub fn import(profile: &Profile, src: &Path, home: &Path) -> io::Result<Vec<Change>> {
    let changes = plan_import(profile, src, home)?;
    for change in &changes {
        match change {
            Change::Create(target) | Change::Update(target) => {
                let rel = bundle_rel_for(profile, home, target);
                let from = src.join(&rel);
                if let Some(parent) = target.parent() {
                    std::fs::create_dir_all(parent)?;
                }
                if merges(&rel) {
                    // No local file to merge into, so the blanks have to come
                    // out before it is written rather than after: a fresh
                    // settings.json carrying `"env": {"TOKEN": null}` would
                    // hand Claude a variable set to nothing.
                    let doc: Value = serde_json::from_slice(&std::fs::read(&from)?)?;
                    std::fs::write(
                        target,
                        format!("{}\n", serde_json::to_string_pretty(&strip_nulls(&doc))?),
                    )?;
                } else {
                    std::fs::copy(&from, target)?;
                }
                copy_exec_bit(&from, target)?;
            }
            Change::Merge(target, _) => {
                let rel = bundle_rel_for(profile, home, target);
                let incoming: Value = serde_json::from_slice(&std::fs::read(src.join(&rel))?)?;
                let existing: Value = serde_json::from_slice(&std::fs::read(target)?)
                    .unwrap_or(Value::Object(Map::new()));
                let out = merged(&incoming, &existing);
                std::fs::write(target, format!("{}\n", serde_json::to_string_pretty(&out)?))?;
            }
            // Nothing to write: this one is a sentence for the operator.
            Change::Unfilled(_, _) => {}
        }
    }
    Ok(changes)
}

/// The bundle-relative path a target on this machine came from.
fn bundle_rel_for(profile: &Profile, home: &Path, target: &Path) -> PathBuf {
    if let Ok(rest) = target.strip_prefix(profile.root_dir(home)) {
        Path::new(BUNDLE_ROOT).join(rest)
    } else if let Ok(rest) = target.strip_prefix(home) {
        Path::new(BUNDLE_HOME).join(rest)
    } else {
        target.to_path_buf()
    }
}

// ── doctor ──────────────────────────────────────────────────────────────────

/// One heading of the doctor's report.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Section {
    /// What this section is about.
    pub title: &'static str,
    /// Its lines, already worded for a terminal.
    pub lines: Vec<String>,
}

/// What `apex agent profile doctor` found.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Report {
    /// Whether the profile directory is there at all.
    pub installed: bool,
    /// The report body.
    pub sections: Vec<Section>,
    /// Things that are wrong. A non-empty list is a non-zero exit.
    pub problems: Vec<String>,
}

fn read_json(path: &Path) -> Option<Value> {
    serde_json::from_slice(&std::fs::read(path).ok()?).ok()
}

fn count_dir(path: &Path, want_dirs: bool) -> usize {
    std::fs::read_dir(path)
        .map(|it| {
            it.flatten()
                .filter(|e| e.file_type().map(|t| t.is_dir()).unwrap_or(false) == want_dirs)
                .count()
        })
        .unwrap_or(0)
}

/// The names in a settings block that names things, whichever of the two
/// shapes Claude wrote it in.
///
/// `enabledPlugins` is an object in Claude 2.1 — `{"name@market": true}` — and
/// was a list of strings before it. A reader that assumed the list finds no
/// plugins at all on a current install and reports a clean profile for a
/// machine running seven of them, which is worse than reporting nothing.
/// `known_marketplaces.json` and `extraKnownMarketplaces` are objects keyed by
/// name, so the object arm serves all three.
///
/// A `false` value is a name that is present and switched off; it is not one of
/// the names.
fn names_in(value: Option<&Value>) -> Vec<String> {
    match value {
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

/// Inspect a profile and report on config, hooks, plugins, MCP and skills.
///
/// Reads; never repairs. A doctor that fixed things would be a doctor nobody
/// could run to find out what state they were in.
pub fn doctor(profile: &Profile, home: &Path) -> Report {
    let root = profile.root_dir(home);
    let mut report = Report {
        installed: root.is_dir(),
        ..Report::default()
    };
    if !report.installed {
        report.problems.push(format!(
            "{} does not exist, so {} has no profile on this machine",
            root.display(),
            profile.display
        ));
        return report;
    }

    let settings = read_json(&root.join("settings.json"));
    let sidecar = read_json(&home.join(".claude.json"));

    // ── config ──────────────────────────────────────────────────────────────
    let mut lines = Vec::new();
    match &settings {
        Some(s) => {
            lines.push(format!(
                "model          {}",
                s.get("model").and_then(Value::as_str).unwrap_or("(default)")
            ));
            lines.push(format!(
                "permissions    {}",
                s.get("permissions")
                    .and_then(|p| p.get("defaultMode"))
                    .and_then(Value::as_str)
                    .unwrap_or("(default)")
            ));
            let env: Vec<String> = s
                .get("env")
                .and_then(Value::as_object)
                .map(|m| m.keys().cloned().collect())
                .unwrap_or_default();
            lines.push(if env.is_empty() {
                "env            none".to_string()
            } else {
                format!("env            {} (values not exported)", env.join(", "))
            });
        }
        None if root.join("settings.json").exists() => {
            report
                .problems
                .push("settings.json is not valid JSON, so Claude starts with its defaults".into());
        }
        None => lines.push("settings.json  absent".to_string()),
    }
    let instructions = root.join("CLAUDE.md");
    lines.push(match std::fs::metadata(&instructions) {
        Ok(m) => format!("CLAUDE.md      {} bytes", m.len()),
        Err(_) => "CLAUDE.md      absent".to_string(),
    });
    report.sections.push(Section {
        title: "config",
        lines,
    });

    // ── statusline ──────────────────────────────────────────────────────────
    if let Some(cmd) = settings
        .as_ref()
        .and_then(|s| s.get("statusLine"))
        .and_then(|s| s.get("command"))
        .and_then(Value::as_str)
    {
        let resolved = PathBuf::from(cmd.replacen('~', &home.to_string_lossy(), 1));
        let state = if !resolved.exists() {
            report
                .problems
                .push(format!("the status line command {cmd} does not exist"));
            "missing"
        } else if !is_executable(&resolved) {
            report
                .problems
                .push(format!("the status line command {cmd} is not executable"));
            "not executable"
        } else {
            "ok"
        };
        report.sections.push(Section {
            title: "statusline",
            lines: vec![format!("{cmd}  {state}")],
        });
    }

    // ── hooks ───────────────────────────────────────────────────────────────
    let hooks = settings
        .as_ref()
        .and_then(|s| s.get("hooks"))
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    let mut lines = Vec::new();
    if hooks.is_empty() {
        lines.push("none".to_string());
    }
    for (event, matchers) in &hooks {
        let count = matchers
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|m| m.get("hooks").and_then(Value::as_array))
                    .map(|h| h.len())
                    .sum::<usize>()
            })
            .unwrap_or(0);
        lines.push(format!("{event:<20} {count}"));
    }
    report.sections.push(Section {
        title: "hooks",
        lines,
    });

    // ── commands, skills, agents ────────────────────────────────────────────
    let commands = std::fs::read_dir(root.join("commands"))
        .map(|it| {
            it.flatten()
                .filter(|e| e.path().extension().is_some_and(|x| x == "md"))
                .count()
        })
        .unwrap_or(0);
    report.sections.push(Section {
        title: "commands",
        lines: vec![format!("{commands} slash commands")],
    });

    let skills_dir = root.join("skills");
    let mut skills = 0usize;
    let mut without_manifest = Vec::new();
    if let Ok(it) = std::fs::read_dir(&skills_dir) {
        for e in it.flatten() {
            if !e.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                continue;
            }
            skills += 1;
            if !e.path().join("SKILL.md").exists() {
                without_manifest.push(e.file_name().to_string_lossy().into_owned());
            }
        }
    }
    let mut lines = vec![format!("{skills} skills")];
    if !without_manifest.is_empty() {
        without_manifest.sort();
        lines.push(format!("no SKILL.md: {}", without_manifest.join(", ")));
        report.problems.push(format!(
            "{} skill director{} have no SKILL.md and will not load: {}",
            without_manifest.len(),
            if without_manifest.len() == 1 { "y" } else { "ies" },
            without_manifest.join(", ")
        ));
    }
    report.sections.push(Section {
        title: "skills",
        lines,
    });

    report.sections.push(Section {
        title: "agents",
        lines: vec![format!(
            "{} subagent definitions",
            std::fs::read_dir(root.join("agents"))
                .map(|it| it.flatten().count())
                .unwrap_or(0)
        )],
    });

    // ── plugins and marketplaces ────────────────────────────────────────────
    let mut known_names = names_in(read_json(&root.join("plugins/known_marketplaces.json")).as_ref());
    // A marketplace declared in settings but not yet fetched is still known:
    // `extraKnownMarketplaces` is how one is added, and treating it as unknown
    // would report a problem for every marketplace on a machine that has just
    // imported a profile and not run Claude yet.
    known_names.extend(names_in(
        settings.as_ref().and_then(|s| s.get("extraKnownMarketplaces")),
    ));
    known_names.sort();
    known_names.dedup();
    let enabled = names_in(settings.as_ref().and_then(|s| s.get("enabledPlugins")));

    let mut lines = vec![
        format!("{} enabled", enabled.len()),
        format!(
            "{} installed, {} marketplaces",
            read_json(&root.join("plugins/installed_plugins.json"))
                .as_ref()
                .and_then(|d| d.get("plugins"))
                .and_then(Value::as_object)
                .map(|m| m.len())
                .unwrap_or(0),
            known_names.len()
        ),
    ];
    for plugin in &enabled {
        // `name@marketplace`: an enabled plugin whose marketplace this machine
        // has never heard of does not load, and nothing says so at startup.
        let Some((_, market)) = plugin.rsplit_once('@') else {
            continue;
        };
        if !known_names.iter().any(|k| k == market) {
            report.problems.push(format!(
                "{plugin} is enabled but the {market} marketplace is not known here"
            ));
        } else if !root.join("plugins/marketplaces").join(market).is_dir() {
            report.problems.push(format!(
                "{plugin} is enabled and {market} is known, but its checkout is missing; \
                 run `claude plugin marketplace update {market}`"
            ));
        }
    }
    lines.push(format!(
        "{} marketplace checkouts on disk",
        count_dir(&root.join("plugins/marketplaces"), true)
    ));
    report.sections.push(Section {
        title: "plugins",
        lines,
    });

    // ── MCP ─────────────────────────────────────────────────────────────────
    let servers = sidecar
        .as_ref()
        .and_then(|d| d.get("mcpServers"))
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    let mut lines = Vec::new();
    if servers.is_empty() {
        lines.push("none".to_string());
    }
    for (name, def) in &servers {
        let kind = def
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or(if def.get("url").is_some() { "http" } else { "stdio" });
        // Both places a definition keeps a credential. An HTTP server's bearer
        // token is a header, not an environment variable, and a report that
        // named only the environment would say a server needs nothing while it
        // carries the one value the export refuses to send.
        let mut needs: Vec<String> = Vec::new();
        for key in ["env", "headers"] {
            if let Some(map) = def.get(key).and_then(Value::as_object) {
                needs.extend(map.keys().cloned());
            }
        }
        lines.push(if needs.is_empty() {
            format!("{name:<20} {kind}")
        } else {
            format!(
                "{name:<20} {kind}  needs {} (values not exported)",
                needs.join(", ")
            )
        });
    }
    report.sections.push(Section {
        title: "mcp",
        lines,
    });

    // ── credentials ─────────────────────────────────────────────────────────
    let mut found = Vec::new();
    for entry in profile.entries {
        if entry.class != Class::Secret {
            continue;
        }
        let p = profile.entry_path(home, entry);
        if p.symlink_metadata().is_ok() {
            found.push(format!("{}  {}", display_home(&p, home), entry.what));
        }
    }
    if found.is_empty() {
        found.push("none on this machine".to_string());
    }
    found.push("excluded from `apex agent profile export`".to_string());
    report.sections.push(Section {
        title: "credentials",
        lines: found,
    });

    report
}

/// `~/…` rather than the full path, so a report is the same on every machine.
pub fn display_home(path: &Path, home: &Path) -> String {
    match path.strip_prefix(home) {
        Ok(rest) => format!("~/{}", rest.display()),
        Err(_) => path.display().to_string(),
    }
}

fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path)
        .map(|m| m.permissions().mode() & 0o111 != 0)
        .unwrap_or(false)
}

/// What one entry looks like on this machine, for `inspect`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Found {
    /// The entry's home-relative path, as `~/…`.
    pub path: String,
    /// Its class.
    pub class: Class,
    /// How a session mounts it.
    pub mount: Mount,
    /// Which part of the profile it is.
    pub role: Role,
    /// One line describing it.
    pub what: &'static str,
    /// Whether it is on this machine.
    pub present: bool,
    /// Files under it, for a directory; 1 for a file that is present.
    pub files: usize,
}

/// Describe every entry of `profile` as it stands on this machine.
pub fn inspect(profile: &Profile, home: &Path) -> Vec<Found> {
    profile
        .entries
        .iter()
        .map(|e| {
            let path = profile.entry_path(home, e);
            let meta = path.symlink_metadata().ok();
            let files = match &meta {
                Some(m) if m.is_dir() => {
                    let mut v = Vec::new();
                    walk(&path, &path, &mut v).ok();
                    v.len()
                }
                Some(_) => 1,
                None => 0,
            };
            Found {
                path: display_home(&path, home),
                class: e.class,
                mount: e.mount,
                role: e.role,
                what: e.what,
                present: meta.is_some(),
                files,
            }
        })
        .collect()
}

/// A machine-readable summary of one profile, for `apex agent profile list
/// --json` and for anything that wants to reason about profiles without
/// parsing a table.
pub fn summary(profile: &Profile, home: &Path) -> BTreeMap<String, Value> {
    let found = inspect(profile, home);
    let count = |c: Class| found.iter().filter(|f| f.class == c && f.present).count();
    let mut out = BTreeMap::new();
    out.insert("agent".into(), Value::from(profile.agent));
    out.insert(
        "root".into(),
        Value::from(display_home(&profile.root_dir(home), home)),
    );
    out.insert(
        "installed".into(),
        Value::from(profile.root_dir(home).is_dir()),
    );
    out.insert("reusable".into(), Value::from(count(Class::Reusable)));
    out.insert("mixed".into(), Value::from(count(Class::Mixed)));
    out.insert(
        "machine_local".into(),
        Value::from(count(Class::MachineLocal)),
    );
    out.insert("secret".into(), Value::from(count(Class::Secret)));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A profile tree with the shape of a real one, built in a temp directory.
    /// No test in this module reads the user's own `~/.claude`.
    struct Fixture {
        home: PathBuf,
    }

    impl Fixture {
        fn new(tag: &str) -> Fixture {
            let home = std::env::temp_dir().join(format!(
                "apex-profile-{}-{}-{tag}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.subsec_nanos())
                    .unwrap_or(0)
            ));
            let root = home.join(".claude");
            std::fs::create_dir_all(root.join("skills/demo")).unwrap();
            std::fs::create_dir_all(root.join("commands")).unwrap();
            std::fs::create_dir_all(root.join("projects/some-project")).unwrap();
            std::fs::create_dir_all(root.join("plugins/marketplaces/mkt")).unwrap();
            std::fs::create_dir_all(root.join("shell-snapshots")).unwrap();
            std::fs::create_dir_all(root.join("daemon")).unwrap();

            std::fs::write(root.join("CLAUDE.md"), "be brief\n").unwrap();
            std::fs::write(
                root.join("settings.json"),
                r#"{"model":"opus","env":{"GITHUB_TOKEN":"ghp_realvalue"},
                    "enabledPlugins":["p@mkt"],
                    "statusLine":{"type":"command","command":"~/.claude/statusline.sh"}}"#,
            )
            .unwrap();
            std::fs::write(root.join("statusline.sh"), "#!/bin/sh\necho hi\n").unwrap();
            std::fs::write(root.join("skills/demo/SKILL.md"), "# demo\n").unwrap();
            std::fs::write(root.join("commands/go.md"), "go\n").unwrap();
            std::fs::write(
                root.join("plugins/known_marketplaces.json"),
                r#"{"mkt":{"source":{"source":"github","repo":"acme/mkt"},
                    "installLocation":"/home/someone/.claude/plugins/marketplaces/mkt",
                    "lastUpdated":"2026-01-01T00:00:00Z"}}"#,
            )
            .unwrap();
            std::fs::write(
                root.join("projects/some-project/chat.jsonl"),
                "{\"secret work\":1}\n",
            )
            .unwrap();
            std::fs::write(root.join("shell-snapshots/snap.sh"), "export A=1\n").unwrap();
            std::fs::write(root.join(".credentials.json"), "{\"token\":\"oauth\"}").unwrap();
            std::fs::write(root.join(".credentials.json.bak-oauthproxy"), "{}").unwrap();
            std::fs::write(root.join("daemon/control.key"), "key").unwrap();
            std::fs::write(
                home.join(".claude.json"),
                r#"{"userID":"u1","machineID":"m1",
                    "mcpServers":{
                      "memory":{"command":"npx","env":{"API":"sk-real"}},
                      "vault":{"type":"http","url":"https://vault.example/mcp",
                               "headers":{"Authorization":"Bearer tok-real"},
                               "oauthAccount":{"emailAddress":"someone@example"}}}}"#,
            )
            .unwrap();
            Fixture { home }
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            std::fs::remove_dir_all(&self.home).ok();
        }
    }

    // ── the split itself ────────────────────────────────────────────────────

    #[test]
    fn anything_the_table_does_not_name_is_machine_local() {
        // P0-009 criterion 1, and the reason criterion 4 holds without a
        // blocklist. A directory a future Claude release invents is excluded
        // the day it ships, because unlisted means machine-local and only
        // reusable is carried.
        for invented in [
            "telemetry",
            "conversations-v3",
            "oauth-cache",
            "skills-experimental",
            "plugins/secrets",
        ] {
            assert_eq!(
                CLAUDE.classify(Base::Root, Path::new(invented)),
                Class::MachineLocal,
                "{invented} was not machine-local by default"
            );
        }
    }

    #[test]
    fn a_credential_by_name_is_secret_wherever_it_appears() {
        for p in [
            ".credentials.json",
            ".credentials.json.bak-oauthproxy",
            "sessions/2.abcdef.key",
            "daemon/control.key",
            "daemon/dispatch/queue",
            "skills/demo/private.key",
        ] {
            assert_eq!(
                CLAUDE.classify(Base::Root, Path::new(p)),
                Class::Secret,
                "{p}"
            );
        }
    }

    #[test]
    fn the_secret_rule_can_only_ever_take_something_out_of_the_export() {
        // The rule is a safety net over default-deny, and its direction is
        // what makes an incomplete net harmless: it may turn a reusable path
        // secret, never a secret path reusable.
        for e in CLAUDE.entries {
            if looks_secret(Path::new(e.path)) {
                assert_eq!(e.class, Class::Secret, "{} is named like a credential but classified {}", e.path, e.class);
            }
        }
        for p in ["CLAUDE.md", "skills/demo/SKILL.md", "commands/go.md"] {
            assert!(!looks_secret(Path::new(p)), "{p}");
        }
    }

    #[test]
    fn the_longest_matching_entry_wins() {
        // `plugins/known_marketplaces.json` is reusable while its `plugins`
        // siblings are not, so a shortest-match lookup would export the plugin
        // cache or refuse the marketplace definitions.
        assert_eq!(
            CLAUDE
                .entry_for(Base::Root, Path::new("plugins/known_marketplaces.json"))
                .unwrap()
                .path,
            "plugins/known_marketplaces.json"
        );
        assert_eq!(
            CLAUDE
                .entry_for(Base::Root, Path::new("plugins/cache/x/y.js"))
                .unwrap()
                .path,
            "plugins/cache"
        );
        assert_eq!(
            CLAUDE
                .entry_for(Base::Root, Path::new("skills/demo/SKILL.md"))
                .unwrap()
                .path,
            "skills"
        );
    }

    #[test]
    fn a_name_that_merely_starts_the_same_is_not_a_match() {
        // Component-wise, not string-wise: `skills-experimental` must not be
        // found through the `skills` entry and exported.
        assert!(CLAUDE
            .entry_for(Base::Root, Path::new("skills-experimental/x"))
            .is_none());
        assert!(CLAUDE
            .entry_for(Base::Root, Path::new("commands-old"))
            .is_none());
    }

    // ── the disjointness the two tasks share ────────────────────────────────

    #[test]
    fn nothing_reusable_is_ever_writable_to_a_session() {
        // P0-010 criterion 2 and P0-009 criterion 4 are one property: the
        // portable half of the profile is what an export carries AND what a
        // confined session may not rewrite. A single table means the two
        // cannot drift apart.
        //
        // The exception is named rather than implied. `~/.claude.json` is
        // mixed and writable because Claude cannot start without writing it;
        // it is the only entry allowed to be both, and if a second one ever
        // appears this fails until somebody has justified it.
        const WRITABLE_AND_EXPORTED: &[(&str, Base)] = &[(".claude.json", Base::Home)];
        for p in PROFILES {
            for e in p.entries {
                if e.class == Class::Reusable {
                    assert_eq!(
                        e.mount,
                        Mount::ReadOnly,
                        "{}/{} is reusable and mounted writable",
                        p.agent,
                        e.path
                    );
                }
                if e.mount == Mount::Writable && e.class.exportable() {
                    assert!(
                        WRITABLE_AND_EXPORTED.contains(&(e.path, e.base)),
                        "{}/{} is writable and would be exported",
                        p.agent,
                        e.path
                    );
                }
            }
        }
    }

    #[test]
    fn a_class_and_its_edits_agree() {
        // Mixed means "carried in part", and the edits are what say which
        // part. A mixed entry with no edits would carry the whole file; a
        // reusable one with edits would be mixed and not say so.
        for p in PROFILES {
            for e in p.entries {
                match e.class {
                    Class::Mixed => assert!(
                        !e.edits.is_empty(),
                        "{}/{} is mixed and gives up nothing",
                        p.agent,
                        e.path
                    ),
                    _ => assert!(
                        e.edits.is_empty(),
                        "{}/{} is {} and has edits, so it is mixed",
                        p.agent,
                        e.path,
                        e.class
                    ),
                }
            }
        }
    }

    #[test]
    fn every_entry_is_absolute_free_and_inside_its_base() {
        for p in PROFILES {
            for e in p.entries {
                let path = Path::new(e.path);
                assert!(path.is_relative(), "{} is absolute", e.path);
                assert!(
                    !e.path.contains(".."),
                    "{} escapes its base",
                    e.path
                );
            }
            // Two entries may share a path only across different bases —
            // `.claude.json` exists both inside `~/.claude` and beside it.
            let mut seen: Vec<(Base, &str)> =
                p.entries.iter().map(|e| (e.base, e.path)).collect();
            let count = seen.len();
            seen.sort_by_key(|(b, path)| (*b == Base::Home, *path));
            seen.dedup_by_key(|(b, path)| (*b == Base::Home, path.to_string()));
            assert_eq!(seen.len(), count, "{} has a duplicate entry", p.agent);
        }
    }

    // ── export ──────────────────────────────────────────────────────────────

    #[test]
    fn the_export_carries_reusable_state_and_refuses_everything_else() {
        let f = Fixture::new("export");
        let plan = plan_export(&CLAUDE, &f.home).unwrap();
        plan.verify(&CLAUDE).expect("a plan of reusable files only");

        let carried: Vec<String> = plan
            .items
            .iter()
            .map(|i| i.bundle.to_string_lossy().into_owned())
            .collect();
        for want in [
            "profile/CLAUDE.md",
            "profile/settings.json",
            "profile/statusline.sh",
            "profile/skills/demo/SKILL.md",
            "profile/commands/go.md",
            "profile/plugins/known_marketplaces.json",
            "home/.claude.json",
        ] {
            assert!(carried.iter().any(|c| c == want), "{want} not carried: {carried:?}");
        }
        for never in [
            "projects",
            "shell-snapshots",
            "credentials",
            "daemon",
            "plugins/marketplaces",
        ] {
            assert!(
                !carried.iter().any(|c| c.contains(never)),
                "{never} reached the bundle: {carried:?}"
            );
        }
    }

    #[test]
    fn a_transcript_never_reaches_a_bundle() {
        // The single worst thing an export could carry: `projects/` holds the
        // full text of every conversation on this machine.
        let f = Fixture::new("transcript");
        let dest = f.home.join("bundle");
        export(&CLAUDE, &f.home, &dest).unwrap();

        let mut files = Vec::new();
        walk(&dest, &dest, &mut files).unwrap();
        for file in &files {
            let text = std::fs::read_to_string(dest.join(file)).unwrap_or_default();
            assert!(!text.contains("secret work"), "{file:?} carries transcript text");
        }
        assert!(!dest.join("profile/projects").exists());
    }

    #[test]
    fn a_secret_inside_a_reusable_file_is_redacted_and_its_name_kept() {
        // File-level exclusion cannot express settings.json: the model and the
        // hooks are reusable and the `env` values are not. Keeping the names
        // is what lets the importing machine know what to supply.
        let f = Fixture::new("redact");
        let dest = f.home.join("bundle");
        let report = export(&CLAUDE, &f.home, &dest).unwrap();

        let out = std::fs::read_to_string(dest.join("profile/settings.json")).unwrap();
        assert!(!out.contains("ghp_realvalue"), "{out}");
        assert!(out.contains("GITHUB_TOKEN"), "the name was dropped too: {out}");
        assert!(out.contains("\"model\""), "{out}");
        assert!(report.edited.iter().any(|(p, _)| p.ends_with("settings.json")));

        // The same shape one level down, for an MCP server's environment.
        let mcp = std::fs::read_to_string(dest.join("home/.claude.json")).unwrap();
        assert!(!mcp.contains("sk-real"), "{mcp}");
        assert!(mcp.contains("\"API\""), "{mcp}");
        // And nothing else from the sidecar came with it.
        assert!(!mcp.contains("machineID"), "{mcp}");
        assert!(!mcp.contains("userID"), "{mcp}");
    }

    #[test]
    fn an_http_mcp_server_does_not_carry_its_bearer_token() {
        // A real leak, found by exporting a real profile and reading the
        // bundle: an HTTP server keeps its credential in `headers`, and the
        // entry's edits had been written against `env`.
        let f = Fixture::new("mcp-header");
        let dest = f.home.join("bundle");
        export(&CLAUDE, &f.home, &dest).unwrap();
        let out = std::fs::read_to_string(dest.join("home/.claude.json")).unwrap();
        assert!(!out.contains("tok-real"), "{out}");
        // The name stays: a machine not told the server wants an Authorization
        // header has a definition it cannot use.
        assert!(out.contains("Authorization"), "{out}");
        assert!(out.contains("vault.example"), "the definition was lost: {out}");
    }

    #[test]
    fn a_key_of_an_mcp_server_that_nobody_named_is_dropped() {
        // Default-deny at the level where a credential is most likely to be
        // invented. `oauthAccount` is a real key Claude writes beside an HTTP
        // server, and it is this machine's account.
        let f = Fixture::new("mcp-unknown");
        let dest = f.home.join("bundle");
        export(&CLAUDE, &f.home, &dest).unwrap();
        let out = std::fs::read_to_string(dest.join("home/.claude.json")).unwrap();
        assert!(!out.contains("oauthAccount"), "{out}");
        assert!(!out.contains("someone@example"), "{out}");
    }

    #[test]
    fn a_credential_by_key_name_is_emptied_wherever_a_mixed_file_puts_it() {
        // The net under the per-file edits, and the reason the edits being a
        // blocklist is survivable. Nothing in the table names any of these.
        let f = Fixture::new("keynet");
        std::fs::write(
            f.home.join(".claude/settings.json"),
            r#"{"model":"opus",
                "apiKeyHelper":"sk-inline",
                "awsAuth":{"accessKeyId":"AKIAREAL","region":"ap-southeast-2"},
                "nested":{"clientSecret":"cs-real"}}"#,
        )
        .unwrap();
        let dest = f.home.join("bundle");
        let report = export(&CLAUDE, &f.home, &dest).unwrap();
        let out = std::fs::read_to_string(dest.join("profile/settings.json")).unwrap();
        for value in ["sk-inline", "AKIAREAL", "cs-real"] {
            assert!(!out.contains(value), "{value} reached the bundle: {out}");
        }
        // Names and everything that is not a credential survive.
        for name in ["apiKeyHelper", "accessKeyId", "clientSecret", "ap-southeast-2"] {
            assert!(out.contains(name), "{name} was dropped: {out}");
        }
        assert!(
            report
                .edited
                .iter()
                .any(|(p, said)| p.ends_with("settings.json") && said.contains("accessKeyId")),
            "the report did not say what was emptied: {:?}",
            report.edited
        );
    }

    #[test]
    fn the_key_net_reads_a_name_the_way_a_person_would() {
        for yes in [
            "token",
            "GITHUB_TOKEN",
            "api-key",
            "apiKeyHelper",
            "Authorization",
            "clientSecret",
            "AWS_SECRET_ACCESS_KEY",
            "private_key",
            "password",
        ] {
            assert!(secret_key(yes), "{yes}");
        }
        // The words a real settings.json is made of.
        for no in [
            "model",
            "permissions",
            "enabledPlugins",
            "extraKnownMarketplaces",
            "statusLine",
            "author",
            "source",
            "installLocation",
            "hooks",
        ] {
            assert!(!secret_key(no), "{no}");
        }
    }

    #[test]
    fn a_machine_path_is_dropped_from_a_marketplace_definition() {
        let f = Fixture::new("mkt");
        let dest = f.home.join("bundle");
        export(&CLAUDE, &f.home, &dest).unwrap();
        let out =
            std::fs::read_to_string(dest.join("profile/plugins/known_marketplaces.json")).unwrap();
        assert!(out.contains("acme/mkt"), "the definition was lost: {out}");
        assert!(!out.contains("installLocation"), "{out}");
        assert!(!out.contains("/home/someone"), "{out}");
    }

    #[test]
    fn a_plan_carrying_something_unexportable_is_refused_before_anything_is_written() {
        // The post-condition, exercised by handing verify a plan the walk
        // would never build. This is what makes "secrets are excluded" a
        // property of the artifact rather than of the table's author.
        let plan = Plan {
            agent: "claude".into(),
            items: vec![PlanItem {
                source: PathBuf::from("/home/t/.claude/.credentials.json"),
                bundle: PathBuf::from("profile/.credentials.json"),
                class: Class::Reusable, // lied about, as a bug would
                edits: &[],
            }],
            excluded: Vec::new(),
        };
        let err = plan.verify(&CLAUDE).unwrap_err();
        assert_eq!(err.class, Class::Secret);
        assert!(err.to_string().contains("reusable state only"), "{err}");
    }

    #[test]
    fn what_the_export_left_behind_is_reported_rather_than_dropped_in_silence() {
        let f = Fixture::new("excluded");
        let report = export(&CLAUDE, &f.home, &f.home.join("bundle")).unwrap();
        let names: Vec<String> = report
            .excluded
            .iter()
            .map(|(p, c)| format!("{} {c}", p.display()))
            .collect();
        assert!(
            names.iter().any(|n| n.contains(".credentials.json") && n.ends_with("secret")),
            "{names:?}"
        );
        assert!(
            names.iter().any(|n| n.contains("projects") && n.ends_with("machine-local")),
            "{names:?}"
        );
    }

    #[test]
    fn the_status_line_keeps_its_executable_bit() {
        let f = Fixture::new("exec");
        let dest = f.home.join("bundle");
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(
            f.home.join(".claude/statusline.sh"),
            std::fs::Permissions::from_mode(0o755),
        )
        .unwrap();
        export(&CLAUDE, &f.home, &dest).unwrap();
        let mode = std::fs::metadata(dest.join("profile/statusline.sh"))
            .unwrap()
            .permissions()
            .mode();
        assert_ne!(mode & 0o111, 0, "a status line that cannot run is not a status line");
    }

    #[test]
    fn the_manifest_says_what_the_bundle_is_and_what_class_each_file_has() {
        let f = Fixture::new("manifest");
        let dest = f.home.join("bundle");
        export(&CLAUDE, &f.home, &dest).unwrap();
        let doc: Value =
            serde_json::from_slice(&std::fs::read(dest.join(MANIFEST)).unwrap()).unwrap();
        assert_eq!(doc["agent"], "claude");
        assert_eq!(doc["version"], BUNDLE_VERSION);
        for file in doc["files"].as_array().unwrap() {
            let class = file["class"].as_str().unwrap();
            assert!(
                class == "reusable" || class == "mixed",
                "the manifest carries a {class} file: {file}"
            );
        }
    }

    // ── import ──────────────────────────────────────────────────────────────

    #[test]
    fn an_import_lands_on_a_machine_with_no_profile_at_all() {
        let src = Fixture::new("roundtrip-src");
        let bundle = src.home.join("bundle");
        export(&CLAUDE, &src.home, &bundle).unwrap();

        let dst = std::env::temp_dir().join(format!("apex-profile-empty-{}", std::process::id()));
        std::fs::create_dir_all(&dst).unwrap();
        let changes = import(&CLAUDE, &bundle, &dst).unwrap();
        assert!(changes.iter().any(|c| matches!(c, Change::Create(p) if p.ends_with("CLAUDE.md"))));
        assert_eq!(
            std::fs::read_to_string(dst.join(".claude/CLAUDE.md")).unwrap(),
            "be brief\n"
        );
        assert!(dst.join(".claude/skills/demo/SKILL.md").exists());
        // Nothing machine-local followed it.
        assert!(!dst.join(".claude/projects").exists());
        assert!(!dst.join(".claude/.credentials.json").exists());
        std::fs::remove_dir_all(&dst).ok();
    }

    #[test]
    fn an_import_merges_settings_instead_of_overwriting_the_local_secret() {
        // The other half of the redaction. A whole-file copy would replace the
        // target's `env` with the bundle's nulls, so importing a profile would
        // delete the token on the machine that had one.
        let src = Fixture::new("merge-src");
        let bundle = src.home.join("bundle");
        export(&CLAUDE, &src.home, &bundle).unwrap();

        let dst = Fixture::new("merge-dst");
        std::fs::write(
            dst.home.join(".claude/settings.json"),
            r#"{"model":"sonnet","env":{"GITHUB_TOKEN":"ghp_localvalue"}}"#,
        )
        .unwrap();

        let changes = import(&CLAUDE, &bundle, &dst.home).unwrap();
        let after: Value =
            serde_json::from_slice(&std::fs::read(dst.home.join(".claude/settings.json")).unwrap())
                .unwrap();
        assert_eq!(after["model"], "opus", "the reusable half did not arrive");
        assert_eq!(
            after["env"]["GITHUB_TOKEN"], "ghp_localvalue",
            "the import overwrote a local secret with the bundle's null"
        );
        assert!(
            changes
                .iter()
                .any(|c| matches!(c, Change::Unfilled(_, n) if n == "GITHUB_TOKEN")),
            "the operator was not told what the bundle could not carry: {changes:?}"
        );
    }

    #[test]
    fn a_bundle_for_another_agent_is_refused() {
        let f = Fixture::new("wrong-agent");
        let bundle = f.home.join("bundle");
        std::fs::create_dir_all(&bundle).unwrap();
        std::fs::write(
            bundle.join(MANIFEST),
            r#"{"version":1,"agent":"codex","files":[]}"#,
        )
        .unwrap();
        let err = plan_import(&CLAUDE, &bundle, &f.home).unwrap_err();
        assert!(err.to_string().contains("codex"), "{err}");
    }

    #[test]
    fn a_manifest_naming_a_path_outside_the_profile_reaches_nothing() {
        // A bundle is a file someone sent. The import classifies every path it
        // is asked to write exactly as the export classified what it read, so
        // a hand-edited manifest cannot walk out of the profile.
        let f = Fixture::new("hostile");
        let bundle = f.home.join("bundle");
        std::fs::create_dir_all(bundle.join("home/.ssh")).unwrap();
        std::fs::write(bundle.join("home/.ssh/authorized_keys"), "ssh-rsa AAAA\n").unwrap();
        std::fs::write(
            bundle.join(MANIFEST),
            r#"{"version":1,"agent":"claude","files":[
                {"path":"home/.ssh/authorized_keys","class":"reusable"}]}"#,
        )
        .unwrap();
        let changes = import(&CLAUDE, &bundle, &f.home).unwrap();
        assert!(changes.is_empty(), "{changes:?}");
        assert!(!f.home.join(".ssh/authorized_keys").exists());
    }

    #[test]
    fn a_bundle_from_a_newer_apex_is_refused_rather_than_half_read() {
        let f = Fixture::new("newer");
        let bundle = f.home.join("bundle");
        std::fs::create_dir_all(&bundle).unwrap();
        std::fs::write(
            bundle.join(MANIFEST),
            r#"{"version":99,"agent":"claude","files":[]}"#,
        )
        .unwrap();
        let err = plan_import(&CLAUDE, &bundle, &f.home).unwrap_err();
        assert!(err.to_string().contains("update APEX"), "{err}");
    }

    #[test]
    fn a_second_import_of_the_same_bundle_changes_nothing() {
        let src = Fixture::new("idem-src");
        let bundle = src.home.join("bundle");
        export(&CLAUDE, &src.home, &bundle).unwrap();
        let dst = Fixture::new("idem-dst");
        import(&CLAUDE, &bundle, &dst.home).unwrap();
        let again = import(&CLAUDE, &bundle, &dst.home).unwrap();
        assert!(
            again
                .iter()
                .all(|c| matches!(c, Change::Unfilled(_, _))),
            "a repeat import was not a no-op: {again:?}"
        );
    }

    // ── doctor ──────────────────────────────────────────────────────────────

    #[test]
    fn the_doctor_reports_config_hooks_plugins_mcp_and_skills() {
        let f = Fixture::new("doctor");
        let report = doctor(&CLAUDE, &f.home);
        assert!(report.installed);
        let titles: Vec<&str> = report.sections.iter().map(|s| s.title).collect();
        for want in ["config", "hooks", "plugins", "mcp", "skills", "commands"] {
            assert!(titles.contains(&want), "{want} missing from {titles:?}");
        }
        let text = report
            .sections
            .iter()
            .flat_map(|s| s.lines.iter())
            .cloned()
            .collect::<Vec<_>>()
            .join("\n");
        assert!(text.contains("opus"), "{text}");
        assert!(text.contains("1 skills"), "{text}");
        assert!(text.contains("memory"), "the MCP server was not reported: {text}");
    }

    #[test]
    fn the_doctor_names_the_credentials_and_says_they_are_not_exported() {
        let f = Fixture::new("doctor-secret");
        let report = doctor(&CLAUDE, &f.home);
        let creds = report
            .sections
            .iter()
            .find(|s| s.title == "credentials")
            .expect("a credentials section");
        assert!(creds.lines.iter().any(|l| l.contains(".credentials.json")));
        assert!(creds.lines.iter().any(|l| l.contains("excluded")));
        // The values themselves are never read, only the paths.
        assert!(!creds.lines.iter().any(|l| l.contains("oauth")));
    }

    #[test]
    fn a_skill_without_a_manifest_is_a_problem_and_not_a_silence() {
        let f = Fixture::new("badskill");
        std::fs::create_dir_all(f.home.join(".claude/skills/broken")).unwrap();
        let report = doctor(&CLAUDE, &f.home);
        assert!(
            report.problems.iter().any(|p| p.contains("broken")),
            "{:?}",
            report.problems
        );
    }

    #[test]
    fn an_enabled_plugin_with_no_marketplace_is_a_problem() {
        let f = Fixture::new("badplugin");
        std::fs::write(
            f.home.join(".claude/settings.json"),
            r#"{"enabledPlugins":["ghost@nowhere"]}"#,
        )
        .unwrap();
        let report = doctor(&CLAUDE, &f.home);
        assert!(
            report.problems.iter().any(|p| p.contains("nowhere")),
            "{:?}",
            report.problems
        );
    }

    #[test]
    fn plugins_are_counted_whichever_shape_the_settings_are_in() {
        // Claude 2.1 writes `enabledPlugins` as an object keyed by
        // `name@marketplace`; it was a list of strings before. A reader that
        // knows only the list reports zero plugins on a current install — a
        // clean bill of health for a machine running seven of them, which is
        // worse than reporting nothing at all. Measured against a real 2.1
        // settings.json.
        let f = Fixture::new("pluginshape");
        std::fs::write(
            f.home.join(".claude/settings.json"),
            r#"{"enabledPlugins":{"p@mkt":true,"ghost@nowhere":true,"old@mkt":false}}"#,
        )
        .unwrap();
        let report = doctor(&CLAUDE, &f.home);
        let plugins = report
            .sections
            .iter()
            .find(|s| s.title == "plugins")
            .expect("a plugins section");
        // p@mkt and ghost@nowhere. old@mkt is a name that is present and
        // switched off, which is not one of the enabled ones.
        assert!(
            plugins.lines.iter().any(|l| l == "2 enabled"),
            "{:?}",
            plugins.lines
        );
        assert!(
            report.problems.iter().any(|p| p.contains("ghost@nowhere")),
            "the object form was not read: {:?}",
            report.problems
        );
        assert!(
            !report.problems.iter().any(|p| p.contains("old@mkt")),
            "a disabled plugin was reported as a problem: {:?}",
            report.problems
        );
    }

    #[test]
    fn a_marketplace_declared_in_settings_counts_as_known() {
        // `extraKnownMarketplaces` is the second marketplace source, and it is
        // the one that carries across a `profile sync`: a machine that has
        // imported a profile and not yet run Claude has the declaration and no
        // checkout. Reading only known_marketplaces.json reports every plugin
        // on that machine as enabled from nowhere.
        let f = Fixture::new("extramkt");
        std::fs::write(
            f.home.join(".claude/settings.json"),
            r#"{"enabledPlugins":{"p@extra":true},
                "extraKnownMarketplaces":{"extra":{"source":{"source":"github","repo":"a/b"}}}}"#,
        )
        .unwrap();
        let report = doctor(&CLAUDE, &f.home);
        assert!(
            !report.problems.iter().any(|p| p.contains("not known here")),
            "{:?}",
            report.problems
        );
        // Known but not fetched is still worth saying, and says what to run.
        assert!(
            report
                .problems
                .iter()
                .any(|p| p.contains("checkout is missing")),
            "{:?}",
            report.problems
        );
    }

    #[test]
    fn a_missing_profile_is_reported_and_not_treated_as_an_empty_one() {
        let empty = std::env::temp_dir().join(format!("apex-profile-none-{}", std::process::id()));
        std::fs::create_dir_all(&empty).unwrap();
        let report = doctor(&CLAUDE, &empty);
        assert!(!report.installed);
        assert!(report.problems.iter().any(|p| p.contains("no profile")));
        std::fs::remove_dir_all(&empty).ok();
    }

    // ── inspect and list ────────────────────────────────────────────────────

    #[test]
    fn inspect_says_what_is_present_and_how_it_is_mounted() {
        let f = Fixture::new("inspect");
        let found = inspect(&CLAUDE, &f.home);
        let skills = found.iter().find(|x| x.path.ends_with("skills")).unwrap();
        assert!(skills.present);
        assert_eq!(skills.class, Class::Reusable);
        assert_eq!(skills.mount, Mount::ReadOnly);
        assert_eq!(skills.files, 1);

        let projects = found.iter().find(|x| x.path.ends_with("projects")).unwrap();
        assert_eq!(projects.class, Class::MachineLocal);
        assert_eq!(projects.mount, Mount::Writable);

        // Paths are shown home-relative, so a report reads the same anywhere.
        assert!(found.iter().all(|x| x.path.starts_with("~/")), "{found:?}");
    }

    #[test]
    fn the_summary_counts_each_class_separately() {
        let f = Fixture::new("summary");
        let s = summary(&CLAUDE, &f.home);
        assert_eq!(s["agent"], "claude");
        assert_eq!(s["installed"], true);
        // CLAUDE.md, statusline.sh, commands/, skills/ — the fixture has no
        // agents/ and no remote-settings.json.
        assert_eq!(s["reusable"], 4);
        // settings.json, known_marketplaces.json, ~/.claude.json.
        assert_eq!(s["mixed"], 3);
        assert!(s["secret"].as_u64().unwrap() >= 1);
        assert!(s["machine_local"].as_u64().unwrap() >= 1);
    }

    #[test]
    fn the_mount_lists_cover_every_entry_and_overlap_in_nothing() {
        let (ro, rw) = CLAUDE.mounts();
        assert_eq!(ro.len() + rw.len(), CLAUDE.entries.len());
        for p in &ro {
            assert!(!rw.contains(p), "{p} is both read-only and writable");
        }
        // Every path is under the home, and the profile root is never itself
        // one of them: binding the whole directory is what P0-010 replaced.
        for p in ro.iter().chain(rw.iter()) {
            assert!(!p.starts_with('/'), "{p}");
            assert_ne!(p.as_str(), CLAUDE.root, "the whole profile was bound");
        }
    }
}
