//! The git provider: the framework's reference implementation.
//!
//! Everything P0-002 proved — a credential presented through a git credential
//! helper, a remote resolved by NAME against the repository's own config, a
//! host pinned against the credential — restated as [`Provider`] rather than as
//! a `match` in the runner. The behaviour is unchanged; what changed is that
//! none of it is in the pipeline any more, so a second provider gets the same
//! guarantees without copying them.
//!
//! It is the provider P1-001 ships end to end because it is the one that can be
//! exercised hermetically: a git smart-HTTP server on loopback, no account, no
//! network, no certificate authority. Cloudflare is P1-002 and needs an account.
//!
//! ## What this module supplies, and what it does not
//!
//! Supplies: three operation names, what a git remote name means
//! ([`GitProvider::bind`] resolves it with `git remote get-url` in the
//! repository, as the owner), and how a credential is presented
//! ([`GitProvider::perform`], via the helper in [`crate::broker`]).
//!
//! Does not supply, and cannot get wrong: whether the caller may do this, which
//! project it counts against, whether the host matches the credential, whether
//! the value is scrubbed, or what goes in the trail. Those are the framework's,
//! and the ordering argument for them is in [`crate::provider`].
//!
//! ## Why there is no short-lived form
//!
//! [`Provider::mint`] is left at its default. A git host's tokens are minted by
//! the host — a GitHub App installation token, say — which is an authenticated
//! API call and therefore an operation in its own right, not a step inside
//! another one. Cloudflare's §13.4 scoped tokens are the case `mint` exists
//! for.

use apex_secret_core::operation::{
    self, Effect, OperationSpec, ParamSpec, ProviderSpec, ResourceKind, Syntax,
};
use apex_secret_core::SecretValue;

use crate::broker;
use crate::provider::{Bind, Bound, Endpoint, Performed, Provider, ProviderError};

/// A git operation, typed, with its arguments.
///
/// P0-002 had this in `apex-secret-core` under the name `Capability`, where it
/// was the whole vocabulary and every future provider would have had to edit
/// it. It is now what it always was: **one provider's argv builder**, private
/// to the git provider, and the thing that keeps a remote name from reaching
/// `git` as anything other than a remote name.
///
/// [`crate::broker`] matches on it to build the command line, which is why it
/// is `pub` within the crate rather than private to this module.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GitOp {
    /// `git push <remote> <branch>` — remote by NAME, resolved by the daemon.
    Push {
        remote: String,
        /// `None` means the current branch, resolved by git.
        branch: Option<String>,
    },
    /// `git fetch <remote>`.
    Fetch { remote: String },
    /// `git ls-remote <remote>` — the refs the remote advertises.
    LsRemote { remote: String },
}

/// Why the git provider refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GitError {
    /// Not one of the three operations this provider implements.
    NotMine(String),
    BadRemoteName(String),
    BadBranchName(String),
    /// The named remote is not configured in this repository.
    NoSuchRemote(String),
}

impl std::fmt::Display for GitError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            GitError::NotMine(op) => write!(
                f,
                "the git provider does not implement '{}'",
                op.escape_debug()
            ),
            GitError::BadRemoteName(r) => write!(
                f,
                "'{}' is not a git remote name. The broker takes a NAME, never \
                 a URL — a URL would let a session choose where your token gets \
                 sent",
                r.escape_debug()
            ),
            GitError::BadBranchName(b) => {
                write!(f, "'{}' is not a valid branch name", b.escape_debug())
            }
            GitError::NoSuchRemote(r) => {
                write!(f, "this repository has no remote called '{}'", r.escape_debug())
            }
        }
    }
}

impl std::error::Error for GitError {}

impl From<GitError> for ProviderError {
    fn from(e: GitError) -> ProviderError {
        match e {
            GitError::NoSuchRemote(_) => ProviderError::NoSuchResource(e.to_string()),
            GitError::NotMine(_) => ProviderError::Failed(e.to_string()),
            other => ProviderError::Refused(other.to_string()),
        }
    }
}

impl GitOp {
    pub fn remote(&self) -> &str {
        match self {
            GitOp::Push { remote, .. } | GitOp::Fetch { remote } | GitOp::LsRemote { remote } => {
                remote
            }
        }
    }

    /// Whether the operation can change the remote.
    ///
    /// Decides which URL is resolved: git honours `remote.<name>.pushurl` and
    /// `url.<base>.pushInsteadOf`, so a repository can send pushes somewhere
    /// the fetch URL never mentions. Pinning the host against the fetch URL and
    /// then pushing to the push URL would check the wrong thing.
    pub fn is_write(&self) -> bool {
        matches!(self, GitOp::Push { .. })
    }

    /// One line for the audit trail and the reply.
    pub fn summary(&self) -> String {
        match self {
            GitOp::Push { remote, branch } => match branch {
                Some(b) => format!("git push {remote} {b}"),
                None => format!("git push {remote} (current branch)"),
            },
            GitOp::Fetch { remote } => format!("git fetch {remote}"),
            GitOp::LsRemote { remote } => format!("git ls-remote {remote}"),
        }
    }
}

/// What a caller names: a git remote.
const REMOTE: ResourceKind = ResourceKind::Name;

/// `branch`, shared by the operations that take one.
const BRANCH: ParamSpec = ParamSpec {
    name: "branch",
    syntax: Syntax::Ref,
    required: false,
    summary: "branch to push; defaults to the current one",
};

/// The git vocabulary, in §13.2's shape.
///
/// The aliases are P0-002's spellings. A grant on disk says `github:git-push`,
/// and it keeps working — the registry canonicalises, the grant table matches
/// either, and what gets written from now on is `git.push`.
pub const SPEC: ProviderSpec = ProviderSpec {
    id: "git",
    summary: "push, fetch and inspect this project's own remotes",
    operations: &[
        OperationSpec {
            id: "git.push",
            summary: "push a branch of this project to one of its own remotes",
            effect: Effect::Write,
            resource: REMOTE,
            params: &[BRANCH],
            aliases: &["git-push"],
        },
        OperationSpec {
            id: "git.fetch",
            summary: "fetch from one of this project's own remotes",
            effect: Effect::Read,
            resource: REMOTE,
            params: &[],
            aliases: &["git-fetch"],
        },
        OperationSpec {
            id: "git.ls-remote",
            summary: "list the refs one of this project's own remotes advertises",
            effect: Effect::Read,
            resource: REMOTE,
            params: &[],
            aliases: &["git-ls-remote"],
        },
    ],
};

pub struct GitProvider;

impl GitProvider {
    /// The typed git form of a checked request.
    ///
    /// The framework has already established that the operation is one of the
    /// three, that the resource is a name and not a URL, and that `branch` — if
    /// given — is a valid ref. This checks all of it again rather than
    /// constructing the variant directly: a provider that trusted the framework
    /// to have checked would be a provider that stops being correct the day the
    /// framework's checks move, and this is the last place before a remote name
    /// becomes a word on a command line.
    fn op(req: &Bind<'_>) -> Result<GitOp, GitError> {
        let remote = req.resource.to_string();
        if !operation::valid_name(&remote) {
            return Err(GitError::BadRemoteName(remote));
        }
        let branch = req.params.get("branch").cloned();
        if let Some(b) = &branch {
            if !operation::valid_ref(b) {
                return Err(GitError::BadBranchName(b.clone()));
            }
        }
        match req.operation.id {
            "git.push" => Ok(GitOp::Push { remote, branch }),
            "git.fetch" => Ok(GitOp::Fetch { remote }),
            "git.ls-remote" => Ok(GitOp::LsRemote { remote }),
            other => Err(GitError::NotMine(other.to_string())),
        }
    }
}

impl Provider for GitProvider {
    fn spec(&self) -> &'static ProviderSpec {
        &SPEC
    }

    /// What a remote NAME means: whatever this repository says it means.
    ///
    /// `--push` for a write, because `remote.<name>.pushurl` and
    /// `url.<base>.pushInsteadOf` can send a push somewhere the fetch URL never
    /// mentions — and the framework pins whatever comes back, so resolving the
    /// wrong one would pin the wrong host.
    fn bind(&self, req: &Bind<'_>) -> Result<Bound, ProviderError> {
        let op = GitProvider::op(req)?;
        let url = broker::resolve_url(req.project, &op, req.owner)?;
        let endpoint = Endpoint::from_url(&url).map_err(|e| {
            // The framework says a destination is not http; git is the layer
            // that knows why somebody hit this, which is an ssh remote.
            ProviderError::Refused(format!(
                "{e} — that remote is not an http remote, so a stored token is \
                 not how it authenticates. An ssh remote uses your ssh agent, \
                 which a confined session cannot reach, by design"
            ))
        })?;
        Ok(Bound {
            endpoint,
            detail: op.summary(),
        })
    }

    /// How the credential is presented: a git credential helper that names two
    /// environment variables and never interpolates their values.
    fn perform(
        &self,
        req: &Bind<'_>,
        _bound: &Bound,
        value: &SecretValue,
    ) -> Result<Performed, ProviderError> {
        let op = GitProvider::op(req)?;
        let out = broker::perform(req.project, &op, req.service, value, req.owner)
            .map_err(ProviderError::Failed)?;
        Ok(Performed {
            code: out.code,
            output: out.text,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use apex_secret_core::operation::Params;
    use apex_secret_core::store::ServiceInfo;

    fn service() -> ServiceInfo {
        ServiceInfo {
            service: "demo".into(),
            host: "127.0.0.1".into(),
            scheme: "http".into(),
            username: "x-access-token".into(),
            path: String::new(),
            auth: "bearer".into(),
            port: None,
            added: 0,
        }
    }

    fn params(pairs: &[(&str, &str)]) -> Params {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    #[test]
    fn the_declaration_is_well_formed() {
        SPEC.validate().expect("the shipped git vocabulary must validate");
    }

    #[test]
    fn every_operation_keeps_the_name_p0_002_shipped_as_an_alias() {
        // A grant already on disk says `github:git-push`. Losing the alias
        // would tell an owner that a capability they granted is not granted.
        for (id, old) in [
            ("git.push", "git-push"),
            ("git.fetch", "git-fetch"),
            ("git.ls-remote", "git-ls-remote"),
        ] {
            let op = SPEC.operation(old).unwrap_or_else(|| panic!("{old} no longer resolves"));
            assert_eq!(op.id, id);
            assert_eq!(SPEC.operation(id).unwrap().id, id);
        }
    }

    #[test]
    fn only_push_is_a_write() {
        // Decides whether the pushurl or the fetch url is resolved and pinned.
        // Getting it backwards checks the wrong host.
        assert!(SPEC.operation("git.push").unwrap().effect.is_write());
        assert!(!SPEC.operation("git.fetch").unwrap().effect.is_write());
        assert!(!SPEC.operation("git.ls-remote").unwrap().effect.is_write());
    }

    #[test]
    fn only_push_takes_a_branch() {
        assert_eq!(SPEC.operation("git.push").unwrap().params.len(), 1);
        assert!(SPEC.operation("git.fetch").unwrap().params.is_empty());
        // And a branch on a fetch is refused by the declaration, before the
        // provider is asked anything.
        assert!(SPEC
            .operation("git.fetch")
            .unwrap()
            .check("origin", &params(&[("branch", "main")]))
            .is_err());
    }

    #[test]
    fn a_request_the_framework_checked_is_checked_again_here() {
        let owner = broker::owner(unsafe { libc::getuid() }).expect("own uid");
        let info = service();
        let p = params(&[("branch", "feat/x-1.2")]);
        let req = Bind {
            operation: SPEC.operation("git.push").unwrap(),
            resource: "origin",
            params: &p,
            body: &[],
            project: "/home/x/p",
            service: &info,
            owner: &owner,
        };
        let op = GitProvider::op(&req).expect("a checked request must parse");
        assert_eq!(op.remote(), "origin");
        assert!(op.is_write());
        assert_eq!(op.summary(), "git push origin feat/x-1.2");
    }

    #[test]
    fn a_remote_that_is_a_url_is_refused_even_if_the_framework_let_it_past() {
        // Defence in depth, and the reason `capability()` re-parses: the
        // provider must not become the place a URL gets through.
        let owner = broker::owner(unsafe { libc::getuid() }).expect("own uid");
        let info = service();
        let p = Params::new();
        let req = Bind {
            operation: SPEC.operation("git.fetch").unwrap(),
            resource: "https://attacker.example/r",
            params: &p,
            body: &[],
            project: "/home/x/p",
            service: &info,
            owner: &owner,
        };
        assert!(matches!(
            GitProvider::op(&req),
            Err(GitError::BadRemoteName(_))
        ));
        // And a branch that is an option, however it got here.
        let p = params(&[("branch", "-f")]);
        let req = Bind {
            operation: SPEC.operation("git.push").unwrap(),
            resource: "origin",
            params: &p,
            body: &[],
            project: "/home/x/p",
            service: &info,
            owner: &owner,
        };
        assert!(matches!(
            GitProvider::op(&req),
            Err(GitError::BadBranchName(_))
        ));
    }

    #[test]
    fn binding_a_remote_this_repository_does_not_have_says_so() {
        // The `bind` failure a caller can act on, distinguished from one it
        // cannot: `NoSuchResource`, not `Failed`.
        let owner = broker::owner(unsafe { libc::getuid() }).expect("own uid");
        let info = service();
        let p = Params::new();
        let req = Bind {
            operation: SPEC.operation("git.fetch").unwrap(),
            resource: "nonexistent-remote",
            params: &p,
            body: &[],
            project: "/nonexistent-apex-secretd-provider-test",
            service: &info,
            owner: &owner,
        };
        assert!(matches!(
            GitProvider.bind(&req),
            Err(ProviderError::NoSuchResource(_))
        ));
    }
}
