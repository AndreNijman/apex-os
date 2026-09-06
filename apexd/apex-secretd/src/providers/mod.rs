//! The providers this build ships.
//!
//! One module per provider, one `register` call in [`default_registry`]. That
//! is the whole of what adding a provider costs — §14's `CloudflareProvider`,
//! `AWSProvider`, `KubernetesProvider` and the rest land here without a line
//! changing in `apex-agent-core`, `apex-agentd`, the `apex` CLI or the wire.

use crate::provider::Registry;

/// A second provider, compiled only for tests.
///
/// An abstraction with one implementation is a description of that
/// implementation. This one is an HTTP API — bearer header, path resources, a
/// short-lived credential — and it shares nothing with git except the framework
/// between them. It is not registered below: it exists so the framework is
/// tested by something real that is not git, and shipping a provider with no
/// service behind it would be worse than not having one.
#[cfg(test)]
pub mod bearer;
pub mod git;
pub mod mcp;

/// Every provider, registered.
///
/// Fatal on a malformed declaration rather than skipping it: a daemon that
/// quietly dropped a provider would answer "that is not an operation this
/// build offers" for a capability the owner had granted, which is the most
/// confusing possible refusal.
/// `run_dir` is the store's own scratch directory, which only root may write.
/// Passed in rather than derived here so a provider cannot pick a directory of
/// its own: where a root process writes a file an unprivileged child then reads
/// is the framework's business, not a provider's.
pub fn default_registry(run_dir: std::path::PathBuf) -> Result<Registry, String> {
    let mut registry = Registry::new();
    registry.register(Box::new(git::GitProvider))?;
    registry.register(Box::new(mcp::McpProvider::new(run_dir)))?;
    Ok(registry)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_shipped_registry_builds_and_offers_every_provider_s_vocabulary() {
        let registry = default_registry(std::env::temp_dir()).expect("every shipped provider must declare validly");
        assert_eq!(
            registry.operation_ids(),
            vec!["git.fetch", "git.ls-remote", "git.push", "mcp.request"]
        );
    }

    #[test]
    fn p0_002_and_p0_003_spellings_still_resolve() {
        // Grants written before P1-001 say `git-push` and `mcp-request`. The
        // registry canonicalises them, so an owner does not have to re-grant
        // anything to keep a machine working across the upgrade.
        let registry = default_registry(std::env::temp_dir()).expect("registry");
        for (old, new) in [
            ("git-push", "git.push"),
            ("git-fetch", "git.fetch"),
            ("git-ls-remote", "git.ls-remote"),
            ("mcp-request", "mcp.request"),
        ] {
            let (_, op) = registry.lookup(old).unwrap_or_else(|e| panic!("{old}: {e}"));
            assert_eq!(op.id, new);
        }
    }
}
