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
pub mod mcp;
pub mod s3;
pub mod webdav;

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
    registry.register(Box::new(cloudflare::CloudflareProvider::new()))?;
    registry.register(Box::new(mcp::McpProvider::new(run_dir.clone())))?;
    registry.register(Box::new(s3::S3Provider))?;
    registry.register(Box::new(webdav::WebdavProvider::new(run_dir)))?;
    Ok(registry)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_shipped_registry_builds_and_offers_every_provider_s_vocabulary() {
        let registry = default_registry(std::env::temp_dir())
            .expect("every shipped provider must declare validly");
        assert_eq!(
            registry.operation_ids(),
            vec![
                "cloudflare.access.edit",
                "cloudflare.access.read",
                "cloudflare.access.service-token.create",
                "cloudflare.account.read",
                "cloudflare.ai-gateway.edit",
                "cloudflare.ai-gateway.run",
                "cloudflare.d1.migrate",
                "cloudflare.d1.query",
                "cloudflare.d1.read",
                "cloudflare.dns.create",
                "cloudflare.dns.delete",
                "cloudflare.dns.read",
                "cloudflare.dns.update",
                "cloudflare.hyperdrive.edit",
                "cloudflare.hyperdrive.read",
                "cloudflare.kv.read",
                "cloudflare.kv.write",
                "cloudflare.queue.manage",
                "cloudflare.queue.publish",
                "cloudflare.r2.bucket.create",
                "cloudflare.r2.object.read",
                "cloudflare.r2.object.write",
                "cloudflare.secret.bind",
                "cloudflare.secret.create",
                "cloudflare.secret.rotate",
                "cloudflare.terraform.apply",
            "cloudflare.terraform.plan",
            "cloudflare.tunnel.edit",
                "cloudflare.tunnel.read",
                "cloudflare.worker.deploy",
                "cloudflare.worker.read",
                "cloudflare.worker.rollback",
                "cloudflare.worker.route.read",
                "cloudflare.worker.tail",
                "cloudflare.worker.upload-version",
                "cloudflare.workers-ai.run",
            "cloudflare.wrangler.deploy",
            "cloudflare.wrangler.versions-upload",
                "git.fetch",
                "git.ls-remote",
                "git.push",
                "mcp.request",
                "s3.object.read",
                "s3.object.write",
                "webdav.file.list",
                "webdav.file.read",
                "webdav.file.write",
            ]
        );
    }

    /// P2-017: **an account scope may not name an operation no provider
    /// offers.**
    ///
    /// `apex-secret-core::account::PROVIDERS` maps a scope a person types
    /// (`files.read`) onto a §13.2 operation id (`webdav.file.read`), and
    /// `apex account grant` records a grant for that id. Nothing checked that
    /// the id exists, because nothing could: `apex-secret-core` is a library
    /// and `default_registry` is in this binary crate, which depends on it and
    /// not the other way round. **This crate is the only place both are
    /// visible.**
    ///
    /// It was not hypothetical. Before this test, `apex account grant google
    /// files.read` succeeded and wrote a grant for `gdrive.file.read`; no
    /// provider has ever offered `gdrive.*` or `msgraph.*`, and `S3_SCOPES`
    /// named `s3.object.list` which the S3 provider landed in `5054be77`
    /// does not have either. Every one of those is a permission a user was
    /// told they had granted and which could never be exercised — the failure
    /// arrives later, somewhere else, as "that is not an operation this build
    /// offers".
    ///
    /// Two halves, and the second is the one that stops this rotting: the
    /// count is asserted, so emptying `PROVIDERS` or every scope table would
    /// fail here rather than passing on an empty loop.
    #[test]
    fn every_account_scope_names_an_operation_some_provider_actually_offers() {
        use apex_secret_core::account;

        let registry = default_registry(std::env::temp_dir()).expect("registry");
        let mut checked = 0;
        for provider in account::PROVIDERS {
            for scope in provider.scopes {
                let (_, op) = registry.lookup(scope.operation).unwrap_or_else(|e| {
                    panic!(
                        "`apex account grant {}.<name> {}` would record a grant for \
                         operation '{}', and no provider in this build offers it ({e}). \
                         Either add the provider that serves it or take the scope out \
                         of the table — a scope that cannot be performed is a \
                         permission the user was told they had.",
                        provider.id, scope.name, scope.operation
                    )
                });
                // Not just "it resolves": an ALIAS resolves too, and a grant is
                // stored under the canonical id. A scope pointing at an old
                // spelling would be recorded as something else and read back
                // as a scope nobody granted.
                assert_eq!(
                    op.id, scope.operation,
                    "scope '{}' on {} names '{}', which resolves to the canonical \
                     operation '{}'. Grants are stored canonically, so point the \
                     scope at the canonical id.",
                    scope.name, provider.id, scope.operation, op.id
                );
                // P1-001's routing rule, restated where it can be checked
                // against the live registry rather than against the table's own
                // idea of itself.
                assert!(
                    scope.operation.starts_with(&format!("{}.", provider.transport)),
                    "scope '{}' on {} routes into '{}' but its operation is '{}'; the \
                     first segment is what routes, so this grant would go to a \
                     different provider",
                    scope.name,
                    provider.id,
                    provider.transport,
                    scope.operation
                );
                checked += 1;
            }
        }
        assert!(
            checked >= 5,
            "only {checked} account scopes were checked; this test ran on almost \
             nothing, which is how it would pass while the tables were empty"
        );
        // The providers that deliberately offer NO scope yet, named one at a
        // time. Adding a sixth provider with an empty table has to be a line
        // somebody writes here on purpose, because "it has no scopes" is
        // exactly what the loop above cannot notice.
        let empty: Vec<&str> = account::PROVIDERS
            .iter()
            .filter(|p| p.scopes.is_empty())
            .map(|p| p.id)
            .collect();
        assert_eq!(
            empty,
            vec!["google", "microsoft"],
            "the set of account providers with nothing grantable changed. Each one is \
             a provider APEX can hold a credential for and spend on nothing; if a \
             transport landed, give it its scopes, and if one was added, say why here."
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

    #[test]
    fn the_everywhere_gate_reads_the_operations_own_declaration() {
        // The gate on `apex secret grant --everywhere`, checked against the
        // whole shipped vocabulary rather than against the one operation it was
        // written for.
        //
        // Two assertions, and the second is the one that stopped this being
        // silent. The gate agrees with the declaration — it computes nothing of
        // its own — and the set of operations that carry the claim is spelled
        // out, so adding one is a line in this test somebody has to write on
        // purpose. It is a review gate, not a safety property: the safety comes
        // from the field being mandatory (there is no `Default` for
        // `OperationSpec`) and from
        // `an_operation_that_claims_to_reach_the_same_thing_everywhere_binds_
        // the_same_in_two_projects` below, which asks the provider's `bind`
        // rather than its declaration.
        let registry = default_registry(std::env::temp_dir()).expect("registry");
        let mut everywhere = Vec::new();
        for id in registry.operation_ids() {
            let (_, op) = registry.lookup(&id).expect("declared");
            assert_eq!(
                crate::service::may_be_granted_everywhere(op),
                op.same_everywhere,
                "the gate on '{id}' disagrees with its own declaration"
            );
            if op.same_everywhere {
                everywhere.push(id);
            }
        }
        assert_eq!(
            everywhere,
            vec!["mcp.request"],
            "the set of operations grantable in every project changed; each one \
             has to be true of the provider's `bind`, not just of its declaration"
        );
    }

    /// The claim, asked of the provider instead of the declaration.
    ///
    /// `same_everywhere` says the operation reaches the same thing whichever
    /// directory it is asked in. `bind` is where that is decided — it is handed
    /// `req.project`, and what it does with it is the entire question — so this
    /// binds every operation that carries the claim in **two different
    /// projects** and requires the two `Bound`s to be identical. The endpoint
    /// is not enough on its own: `cloudflare.account.read` resolves to the same
    /// host either way and to a different *path*, which is why `detail` — the
    /// sentence the framework pins and audits, and which the provider is
    /// required to make name every value that decides where the request goes —
    /// is compared too.
    ///
    /// **The two projects have to differ in a way a provider would read.** One
    /// gets §13.1's `apex.toml` binding an account; the other is bare. Two
    /// empty directories would make this pass for `cloudflare.account.read`,
    /// which is the mutation it exists to fail on.
    ///
    /// What it does not prove: that a provider reads nothing else. A `bind`
    /// keying on some third thing in the project — a lockfile, an env file —
    /// would go unnoticed until that thing is planted here too. It catches the
    /// mechanism every provider in this build actually uses.
    #[test]
    fn an_operation_that_claims_to_reach_the_same_thing_everywhere_binds_the_same_in_two_projects()
    {
        use crate::provider::Bind;
        use apex_secret_core::operation::Params;
        use apex_secret_core::store::ServiceInfo;

        // §13.1's file, the same shape `providers::cloudflare::tests` uses.
        const BOUND_PROJECT: &str = r#"
[identity.cloudflare]
account = "example-account"
account_id = "0123456789abcdef0123456789abcdef"

[cloudflare]
zone = "example.com"
zone_id = "fedcba9876543210fedcba9876543210"

[cloudflare.production]
worker = "project"
"#;

        let tag = format!("apex-everywhere-{}", std::process::id());
        let root = std::env::temp_dir().join(&tag);
        let bound_dir = root.join("bound");
        let bare_dir = root.join("bare");
        std::fs::remove_dir_all(&root).ok();
        std::fs::create_dir_all(&bound_dir).expect("bound project");
        std::fs::create_dir_all(&bare_dir).expect("bare project");
        std::fs::write(bound_dir.join("apex.toml"), BOUND_PROJECT).expect("apex.toml");

        let run_dir = root.join("run");
        std::fs::create_dir_all(&run_dir).expect("run dir");
        let registry = default_registry(run_dir).expect("registry");
        // Safe: getuid cannot fail.
        let owner = crate::broker::owner(unsafe { libc::getuid() }).expect("own uid");
        // MCP-shaped, because `mcp.request` is the only operation that carries
        // the claim today. A provider whose `bind` needs a different record
        // will hit the "binds in NEITHER project" panic below, which is the
        // signal to add its own fixture here rather than to loosen the check.
        let service = ServiceInfo {
            service: "memory".into(),
            host: "127.0.0.1".into(),
            scheme: "http".into(),
            username: "x-access-token".into(),
            path: "/mcp".into(),
            auth: "bearer".into(),
            port: Some(9000),
            added: 0,
        };
        let params = Params::new();

        let mut checked = 0;
        for id in registry.operation_ids() {
            let (provider, op) = registry.lookup(&id).expect("declared");
            if !op.same_everywhere {
                continue;
            }
            // Guaranteed by `ProviderSpec::validate`, restated here because the
            // `Bind` below gives no resource and no parameters and would
            // otherwise be testing a request the operation never takes.
            assert!(op.names_nothing(), "'{id}' claims `*` and names something");

            let bind = |project: &std::path::Path| {
                provider.bind(&Bind {
                    operation: op,
                    resource: "",
                    params: &params,
                    body: br#"{"jsonrpc":"2.0","id":1,"method":"tools/list"}"#,
                    project: project.to_str().expect("utf8"),
                    service: &service,
                    owner: &owner,
                    audit_id: "test",
                })
            };
            let in_bound = bind(&bound_dir);
            let in_bare = bind(&bare_dir);
            match (&in_bound, &in_bare) {
                (Ok(a), Ok(b)) => {
                    assert_eq!(
                        (a.endpoint.to_string(), a.detail.as_str()),
                        (b.endpoint.to_string(), b.detail.as_str()),
                        "'{id}' claims to reach the same thing in every project, but \
                         binding it in a project that binds an account and in one that \
                         does not gives two different requests. Either its `bind` reads \
                         the project — in which case `same_everywhere: false` — or the \
                         difference is harmless and this test needs to say why."
                    );
                }
                // Not "the same refusal in both, so it is consistent". A
                // provider whose `bind` fails in BOTH directories for a reason
                // that belongs to this fixture — a `ServiceInfo` of the wrong
                // shape, a missing path — would make this loop bind nothing and
                // call the claim proven. That is the same silent pass, in a new
                // coat. Refuse it and make somebody extend the fixture.
                (Err(a), Err(b)) => panic!(
                    "'{id}' binds in NEITHER project with this fixture ({a} / {b}), so \
                     nothing about its claim was checked. Extend the fixture below \
                     until it binds; a test that binds nothing proves nothing."
                ),
                _ => panic!(
                    "'{id}' binds in one project and not the other: {in_bound:?} vs {in_bare:?}"
                ),
            }
            checked += 1;
        }
        // A loop over an empty set proves nothing, and the whole point of the
        // gate is that its set is small.
        assert!(checked > 0, "no operation carries the claim; this test ran on nothing");

        std::fs::remove_dir_all(&root).ok();
    }
}
