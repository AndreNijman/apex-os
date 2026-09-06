//! The providers this build ships.
//!
//! One module per provider, one `register` call in [`default_registry`]. That
//! is the whole of what adding a provider costs — §14's `CloudflareProvider`,
//! `AWSProvider`, `KubernetesProvider` and the rest land here without a line
//! changing in `apex-agent-core`, `apex-agentd`, the `apex` CLI or the wire.

use crate::provider::Registry;

pub mod git;

/// Every provider, registered.
///
/// Fatal on a malformed declaration rather than skipping it: a daemon that
/// quietly dropped a provider would answer "that is not an operation this
/// build offers" for a capability the owner had granted, which is the most
/// confusing possible refusal.
pub fn default_registry() -> Result<Registry, String> {
    let mut registry = Registry::new();
    registry.register(Box::new(git::GitProvider))?;
    Ok(registry)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_shipped_registry_builds_and_offers_the_git_vocabulary() {
        let registry = default_registry().expect("every shipped provider must declare validly");
        assert_eq!(
            registry.operation_ids(),
            vec!["git.fetch", "git.ls-remote", "git.push"]
        );
    }
}
