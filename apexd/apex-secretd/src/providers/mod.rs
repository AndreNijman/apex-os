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
pub mod cloudflare;
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
    registry.register(Box::new(cloudflare::CloudflareProvider::new()))?;
    Ok(registry)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_shipped_registry_builds_and_offers_every_provider_s_vocabulary() {
        let registry = default_registry().expect("every shipped provider must declare validly");
        assert_eq!(
            registry.operation_ids(),
            vec![
                "cloudflare.account.read",
                "cloudflare.worker.deploy",
                "cloudflare.worker.read",
                "cloudflare.worker.rollback",
                "cloudflare.worker.route.read",
                "cloudflare.worker.tail",
                "cloudflare.worker.upload-version",
                "git.fetch",
                "git.ls-remote",
                "git.push",
            ]
        );
    }
}
