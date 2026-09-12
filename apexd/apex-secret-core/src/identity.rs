//! §36's `[identity.*]`: which account a project is allowed to act as.
//!
//! Roadmap P2-013. §36 gives the file and the reason in one sentence —
//! *"This prevents deploying or pushing from the wrong account"* — and four
//! sections:
//!
//! ```toml
//! [identity.github]
//! account = "example"
//!
//! [identity.cloudflare]
//! account = "example"
//!
//! [identity.ssh]
//! host_group = "robotics"
//!
//! [identity.agent]
//! default = "claude"
//! ```
//!
//! # Declared is not the same as enforced, and this module says which is which
//!
//! A binding only means something where there is a place to check it. This
//! build has such a place for two of the four:
//!
//! * **cloudflare** — enforced since P1-002. `[identity.cloudflare] account_id`
//!   is what every Cloudflare operation is addressed by, and
//!   `binding.rs`'s resolvers refuse a bucket, zone or resource the project did
//!   not write down.
//! * **github** — enforced here, and it is this item's new work. The git
//!   provider resolves a remote NAME to a URL out of the repository's own
//!   config; [`GitHubIdentity::check`] then refuses a URL whose owner is not
//!   the account the project bound. That is §36's sentence, made true: an agent
//!   that can `git remote add` cannot push this project's code to an account
//!   the owner never named.
//! * **ssh** — **declared and reported, not enforced.** Nothing in this
//!   workspace authenticates over ssh: the git provider refuses an ssh remote
//!   outright ("an ssh remote uses your ssh agent, which a confined session
//!   cannot reach, by design"), and there is no ssh provider to hold to a
//!   `host_group`. The refusal now names what the project bound, which is the
//!   most an unenforceable binding can honestly do.
//! * **agent** — enforced, and it is this round's new work. `apex-agentd`
//!   resolves which assistant a session runs, and it now asks the project
//!   before it asks the user's own configuration: a session that names no
//!   agent gets the bound one rather than `~/.config/apex/agent.json`'s
//!   `default_agent`, and a session that names a *different* one is refused.
//!   [`AgentIdentity::check`] is that refusal; `apex-agentd/src/session.rs`
//!   is where it is applied, before the PTY, the worktree or the grant.
//!
//! [`Identities::report`] is what makes the difference visible rather than
//! buried: `apex project identity` prints every section, what it binds, and
//! whether anything checks it.
//!
//! # Reading it is the same problem the Cloudflare binding solved
//!
//! Same file, same reader, same guarantees: [`crate::project::ProjectConfig`]
//! opens every component below the project root with `O_NOFOLLOW`, requires a
//! regular file owned by the account the operation runs as, caps the size, and
//! reports a parse failure as a position and never as the text at it.
//!
//! **And the same caveat.** `apex.toml` is writable by the account that owns
//! the project, so a binding is the project describing itself and never an
//! authority over that account. What it buys is that a *mistake* — or an agent
//! adding a remote — is caught, in the same way `[cloudflare] buckets` catches
//! one. What bounds a hostile owner is the grant and the credential's own
//! scope.

use std::collections::BTreeMap;
use std::path::Path;

use crate::project::{ProjectConfig, ProjectError};

/// The four §36 names.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Kind {
    GitHub,
    Cloudflare,
    Ssh,
    Agent,
}

impl Kind {
    pub fn as_str(self) -> &'static str {
        match self {
            Kind::GitHub => "github",
            Kind::Cloudflare => "cloudflare",
            Kind::Ssh => "ssh",
            Kind::Agent => "agent",
        }
    }

    /// In §36's own order.
    pub const ALL: [Kind; 4] = [Kind::GitHub, Kind::Cloudflare, Kind::Ssh, Kind::Agent];

    /// Whether anything in this build checks a binding of this kind.
    ///
    /// A field rather than a comment because [`Identities::report`] prints it:
    /// telling somebody their project binds an ssh host group, without telling
    /// them nothing looks at it, would be worse than not printing it at all.
    pub fn is_enforced(self) -> bool {
        matches!(self, Kind::GitHub | Kind::Cloudflare | Kind::Agent)
    }

    /// Where the check lives, or what would have to exist for one to.
    pub fn enforced_by(self) -> &'static str {
        match self {
            Kind::GitHub => {
                "the git provider, when it resolves a remote name to a URL \
                 (apex-secretd/src/providers/git.rs)"
            }
            Kind::Cloudflare => {
                "every Cloudflare operation, which is addressed by the account \
                 id this binds (apex-secretd/src/providers/cloudflare/binding.rs)"
            }
            Kind::Ssh => {
                "nothing yet. No provider in this workspace authenticates over \
                 ssh — the git provider refuses an ssh remote outright — so \
                 there is nothing to hold to a host group. An ssh provider \
                 beside apex-secretd/src/providers/git.rs is what would check it"
            }
            Kind::Agent => {
                "the agent runtime, when a session starts in this project \
                 (apex-agentd/src/session.rs). A session that names no agent \
                 gets the bound one; a session that names a different one is \
                 refused before anything is created"
            }
        }
    }
}

/// `[identity.github]`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitHubIdentity {
    /// §36's `account`: the user or organisation this project pushes to.
    pub account: String,
    /// `host`, which §36 does not name and this build adds.
    ///
    /// Two reasons, neither cosmetic: GitHub Enterprise is not `github.com`,
    /// and a check that could only ever be exercised against the real
    /// `github.com` could not be tested at all — this repository's git fixture
    /// is a smart-HTTP server on loopback. Defaults to `github.com`.
    pub host: String,
}

/// The default for [`GitHubIdentity::host`].
pub const GITHUB_HOST: &str = "github.com";

/// `[identity.agent]`.
///
/// §36's `default = "claude"`, and the word is exact: it is the agent a session
/// that named none gets. What §36 does *not* say, and what this build decides,
/// is what happens when a session names a different one — and the sentence §36
/// gives for the whole file settles it. *"This prevents deploying or pushing
/// from the wrong account."* A binding that only applied when nobody said
/// otherwise would prevent nothing: `--agent codex` would step around it, and
/// stepping around it is exactly what an agent that can edit its own command
/// line would do.
///
/// So the binding does two things, and the second is the enforcement:
///
/// 1. It **displaces the user's own default**. `apex agent run` in this project
///    starts the bound agent, not `default_agent` from the user's config.
/// 2. It **refuses a session that names a different one**, with a message that
///    says where the binding is, because the way to change it is to change
///    `apex.toml` — the same escape hatch [`IdentityError::WrongAccount`]
///    names, and the same one §36's github binding has.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentIdentity {
    /// §36's `default`: the assistant this project's sessions run.
    pub default: String,
}

impl AgentIdentity {
    /// Whether a session that explicitly asked for `requested` may run here.
    ///
    /// Case-insensitive, because an adapter id is a lowercase word and
    /// `--agent Claude` is a typo rather than a different request. Everything
    /// else is refused.
    pub fn check(&self, requested: &str) -> Result<(), IdentityError> {
        if requested.eq_ignore_ascii_case(&self.default) {
            return Ok(());
        }
        Err(IdentityError::WrongAgent {
            requested: requested.to_string(),
            bound: self.default.clone(),
        })
    }
}

/// Why a remote is not one this project may push to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IdentityError {
    /// The URL is not one an owner can be read out of.
    Unreadable { url: String },
    /// The URL's owner is not the account the project bound.
    WrongAccount {
        got: String,
        bound: String,
        host: String,
    },
    /// The session asked to run an agent this project did not bind.
    WrongAgent { requested: String, bound: String },
}

impl std::fmt::Display for IdentityError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            IdentityError::Unreadable { url } => write!(
                f,
                "this project binds an identity for that host, and '{}' is not \
                 a URL an account can be read out of — so this build cannot \
                 tell whether it is the bound one, and will not guess",
                url.escape_debug()
            ),
            IdentityError::WrongAccount { got, bound, host } => write!(
                f,
                "that remote belongs to '{}' on {host}, and this project binds \
                 [identity.github] account = \"{}\". §36's whole purpose is to \
                 stop a push going to the wrong account, so it is refused \
                 rather than performed. If the move is deliberate, change the \
                 line in apex.toml",
                got.escape_debug(),
                bound.escape_debug()
            ),
            IdentityError::WrongAgent { requested, bound } => write!(
                f,
                "this project binds [identity.agent] default = \"{}\", and \
                 this session asked for '{}'. §36 binds a project to one \
                 assistant so that work in it is not started under another by \
                 accident, so the session is refused rather than started. If \
                 the change is deliberate, change the line in apex.toml",
                bound.escape_debug(),
                requested.escape_debug()
            ),
        }
    }
}

impl std::error::Error for IdentityError {}

impl GitHubIdentity {
    /// Whether a resolved remote URL is one this project may act on.
    ///
    /// **A URL on another host is not this binding's business.** Where a push
    /// goes is the credential's host pin, which the framework applies to every
    /// operation; what this checks is which *account on the bound host* the
    /// project may act as. Refusing a different host here would be a second,
    /// weaker copy of the pin.
    pub fn check(&self, url: &str) -> Result<(), IdentityError> {
        let Some((host, owner)) = host_and_owner(url) else {
            // The host could not be read either, so this is not a URL for the
            // bound host and not one for anywhere else. Left to the pin.
            return Ok(());
        };
        if !host.eq_ignore_ascii_case(&self.host) {
            return Ok(());
        }
        let Some(owner) = owner else {
            return Err(IdentityError::Unreadable {
                url: url.to_string(),
            });
        };
        if owner.eq_ignore_ascii_case(&self.account) {
            return Ok(());
        }
        Err(IdentityError::WrongAccount {
            got: owner,
            bound: self.account.clone(),
            host: self.host.clone(),
        })
    }
}

/// The host and the first path segment of an http(s) URL.
///
/// Deliberately small, and deliberately not a URL parser: it is fed a URL that
/// `git remote get-url` produced and that the framework has already turned into
/// an [`crate::capability`] endpoint, so the only question left is which
/// account the path names. A shape this does not recognise returns `None` for
/// the owner rather than a guess, and the caller refuses.
fn host_and_owner(url: &str) -> Option<(String, Option<String>)> {
    let rest = url
        .strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"))?;
    // Credentials in a URL are not this function's business, but they do sit
    // before the host.
    let rest = match rest.split_once('@') {
        Some((_, after)) => after,
        None => rest,
    };
    let (authority, path) = match rest.split_once('/') {
        Some((authority, path)) => (authority, path),
        None => (rest, ""),
    };
    let host = authority
        .split_once(':')
        .map(|(h, _)| h)
        .unwrap_or(authority)
        .to_string();
    if host.is_empty() {
        return None;
    }
    let owner = path
        .split('/')
        .find(|segment| !segment.is_empty())
        .map(|segment| segment.trim_end_matches(".git").to_string())
        .filter(|segment| !segment.is_empty());
    Some((host, owner))
}

/// Every `[identity.*]` a project declares.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Identities {
    pub github: Option<GitHubIdentity>,
    /// `[identity.cloudflare] account`, the human label. The id the API is
    /// addressed by is `account_id`, read by the Cloudflare provider's own
    /// binding — this is the name a person recognises.
    pub cloudflare: Option<String>,
    /// `[identity.ssh] host_group`.
    pub ssh_host_group: Option<String>,
    /// `[identity.agent] default`.
    pub agent: Option<AgentIdentity>,
}

impl Identities {
    /// Read `<project>/apex.toml` as the account the operation runs as.
    ///
    /// A project with no `apex.toml` binds nothing, which is
    /// [`ProjectError::Absent`] and is the caller's to treat as "no
    /// constraint". **Every other error is an error**, and the distinction is
    /// the one this repository keeps having to make: a file that could not be
    /// read is not a file that says nothing. See the note on
    /// [`Identities::read_or_unbound`].
    pub fn read(
        root: &Path,
        owner_uid: u32,
        owner_name: &str,
    ) -> Result<Identities, ProjectError> {
        let config = ProjectConfig::read(root, owner_uid, owner_name)?;
        Identities::from_project(&config)
    }

    /// The same, with "there is no such file" folded into "binds nothing".
    ///
    /// **Only `Absent` is folded.** A project whose `apex.toml` is unreadable,
    /// malformed, owned by somebody else or reached through a symlink comes
    /// back as an error, because proceeding as though it bound nothing is how
    /// an agent that can `chmod 000 apex.toml` would unbind the project it is
    /// running in. That is the same defect as a permission denial reported as
    /// an absence, one layer up, and it is the reason this function exists
    /// instead of a call site writing `.unwrap_or_default()`.
    pub fn read_or_unbound(
        root: &Path,
        owner_uid: u32,
        owner_name: &str,
    ) -> Result<Identities, ProjectError> {
        match Identities::read(root, owner_uid, owner_name) {
            Ok(identities) => Ok(identities),
            Err(ProjectError::Absent { .. }) => Ok(Identities::default()),
            Err(other) => Err(other),
        }
    }

    pub fn from_project(config: &ProjectConfig) -> Result<Identities, ProjectError> {
        let string = |keys: &[&str]| -> Result<Option<String>, ProjectError> {
            config.string(keys).map(|v| v.map(str::to_string))
        };
        let github = match string(&["identity", "github", "account"])? {
            Some(account) => Some(GitHubIdentity {
                account,
                host: string(&["identity", "github", "host"])?
                    .unwrap_or_else(|| GITHUB_HOST.to_string()),
            }),
            None => None,
        };
        Ok(Identities {
            github,
            cloudflare: string(&["identity", "cloudflare", "account"])?,
            ssh_host_group: string(&["identity", "ssh", "host_group"])?,
            agent: string(&["identity", "agent", "default"])?
                .map(|default| AgentIdentity { default }),
        })
    }

    /// What each of §36's four binds, whether anything checks it, and where.
    ///
    /// The middle field is the one that matters. A report that listed four
    /// bindings without saying that two of them are checked and two are not
    /// would tell an operator their ssh host group protects something.
    pub fn report(&self) -> BTreeMap<Kind, Bound> {
        let mut out = BTreeMap::new();
        for kind in Kind::ALL {
            let bound = match kind {
                Kind::GitHub => self
                    .github
                    .as_ref()
                    .map(|g| format!("account = {} on {}", g.account, g.host)),
                Kind::Cloudflare => self.cloudflare.as_ref().map(|a| format!("account = {a}")),
                Kind::Ssh => self
                    .ssh_host_group
                    .as_ref()
                    .map(|g| format!("host_group = {g}")),
                Kind::Agent => self
                    .agent
                    .as_ref()
                    .map(|a| format!("default = {}", a.default)),
            };
            out.insert(
                kind,
                Bound {
                    kind,
                    binds: bound,
                    enforced: kind.is_enforced(),
                    enforced_by: kind.enforced_by(),
                },
            );
        }
        out
    }
}

/// One row of [`Identities::report`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Bound {
    pub kind: Kind,
    /// What the project binds, or `None` for a section it does not have.
    pub binds: Option<String>,
    /// Whether anything in this build checks it.
    pub enforced: bool,
    /// Where the check is, or what would have to exist for one.
    pub enforced_by: &'static str,
}

#[cfg(test)]
mod tests;
