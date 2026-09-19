//! The credential a browser capsule is authenticated with and never holds.
//!
//! P2-012's last acceptance criterion is "capability auth": a capsule that can
//! present a credential to a site. `docs/browser-capsule-auth.md` records four
//! routes, rejects the one that hands the capsule a session cookie, and asks
//! the owner one question — may the runtime read the plaintext of a capsule's
//! connection to the one destination it was pinned to, in order to add a
//! credential the capsule is never given. The answer was yes; this is the half
//! of the answer that lives where credentials live.
//!
//! ## Why there is a provider here at all
//!
//! Nothing in this module talks to a network. What it exists for is the
//! **grant**: `Service::decide` matches a stored grant against an operation the
//! registry knows, and `Service::grant` refuses to write a grant for an
//! operation no provider offers. Without a declaration here,
//! `apex secret grant intranet browser.present` could not be typed, and an
//! interception with no grant behind it would be a way for anything running as
//! the user to spend a credential on arbitrary requests to the pinned host —
//! which is exactly what `apex secret use` requires a grant for.
//!
//! So the vocabulary entry is the boundary, and the two trait methods below are
//! deliberately dead ends: `apex secret use intranet browser.present` is
//! refused, because there is no request for the framework's one-shot
//! request/response path to carry. The operation is reachable only through
//! [`crate::present`], which the runtime opens for a capsule whose session was
//! started with `--present` and whose allowlist is the credential's own pin.
//!
//! ## Why it may be granted everywhere
//!
//! Because a capsule has no project to key a grant on. `apex browser run`
//! starts its session with `--cwd <capsule directory>`, a throwaway tree
//! created for that run and deleted when it ends, so a grant recorded against
//! it would match exactly one capsule and then name a directory that no longer
//! exists. `same_everywhere` is therefore `true` and the owner grants it with
//! `--everywhere`; what that widens is *where it may be asked for*, and the
//! endpoint it reaches is the credential's pin no matter who asks.

use apex_secret_core::operation::{
    Effect, OperationSpec, ProviderSpec, ResourceKind,
};
use apex_secret_core::SecretValue;

use crate::provider::{Bind, Bound, Performed, Provider, ProviderError};

/// The one operation, named once so the verb and the tests cannot drift from
/// the declaration.
pub const PRESENT: &str = "browser.present";

pub const SPEC: ProviderSpec = ProviderSpec {
    id: "browser",
    summary: "authenticate a browser capsule to the endpoint its credential is pinned to",
    operations: &[OperationSpec {
        id: PRESENT,
        summary: "let a browser capsule's requests to this credential's endpoint carry it",
        // A capsule can issue any request the site accepts, `POST` included.
        // `Read` would be a claim about what a browser does that nothing here
        // can enforce.
        effect: Effect::Write,
        resource: ResourceKind::None,
        params: &[],
        aliases: &[],
        // See the module note: a capsule's working directory is a throwaway
        // capsule tree, so a project-keyed grant could never match twice.
        same_everywhere: true,
        supersedes_credentials: false,
    }],
};

/// A provider that declares an operation and performs none.
pub struct BrowserProvider;

/// What both trait methods say, in one place so the refusal cannot be two
/// different sentences depending on which one the caller reached.
const ONLY_THROUGH_A_CAPSULE: &str =
    "`browser.present` is not an operation that can be performed on its own: it exists so a \
     browser capsule's own requests can carry this credential, and the runtime opens it for a \
     session started with `apex browser run --present`. Grant it if you want capsules to be \
     able to authenticate to this endpoint; there is nothing for `apex secret use` to do with \
     it.";

impl Provider for BrowserProvider {
    fn spec(&self) -> &'static ProviderSpec {
        &SPEC
    }

    fn bind(&self, _req: &Bind<'_>) -> Result<Bound, ProviderError> {
        Err(ProviderError::Refused(ONLY_THROUGH_A_CAPSULE.to_string()))
    }

    /// Unreachable through the framework, because [`Provider::bind`] refuses
    /// first and `use_capability` never gets past it. Written as a refusal
    /// rather than an `unreachable!()` on purpose: a future edit that made
    /// `bind` succeed would otherwise turn a wrong answer into a panic in a
    /// root daemon, and this one is the only method that is ever handed a
    /// value.
    fn perform(
        &self,
        _req: &Bind<'_>,
        _bound: &Bound,
        _value: &SecretValue,
    ) -> Result<Performed, ProviderError> {
        Err(ProviderError::Refused(ONLY_THROUGH_A_CAPSULE.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use apex_secret_core::operation::Params;
    use apex_secret_core::store::ServiceInfo;
    use crate::broker::Owner;

    fn info() -> ServiceInfo {
        ServiceInfo {
            service: "intranet".into(),
            host: "intranet.example".into(),
            scheme: "https".into(),
            username: "x".into(),
            path: String::new(),
            auth: "bearer".into(),
            port: None,
            added: 0,
        }
    }

    /// The whole point of the declaration is that it cannot be spent through
    /// the ordinary path. If `bind` ever succeeds, `use_capability` would read
    /// the value and hand it to `perform`, and `perform` returns the output to
    /// the caller — which is how a credential would reach a user process.
    #[test]
    fn the_operation_cannot_be_performed_through_the_ordinary_path() {
        let info = info();
        let params = Params::default();
        let owner = Owner {
            uid: 1000,
            gid: 1000,
            name: "someone".into(),
            home: "/home/someone".into(),
            groups: Vec::new(),
        };
        let req = Bind {
            operation: &SPEC.operations[0],
            resource: "",
            params: &params,
            body: b"",
            project: "/tmp/p",
            service: &info,
            owner: &owner,
            audit_id: "a1",
        };
        let e = BrowserProvider.bind(&req).expect_err("bind must refuse");
        assert!(
            matches!(e, ProviderError::Refused(_)),
            "a refusal, not a failure: {e:?}"
        );
        let text = e.to_string();
        assert!(text.contains("apex browser run --present"), "{text}");
    }

    /// `same_everywhere` is load-bearing here rather than cosmetic: with it
    /// false, `Service::grant` refuses `--everywhere`, and a capsule's own
    /// project — a directory deleted when the capsule ends — is the only key
    /// left. Asserted because a later editor reading the field's doc could
    /// reasonably decide a `Write` operation should not carry it.
    #[test]
    fn the_operation_may_be_granted_everywhere_because_a_capsule_has_no_project() {
        assert!(
            SPEC.operations[0].same_everywhere,
            "a capsule's cwd is its own throwaway directory, so a project-keyed \
             grant would match one capsule and then name a path that is gone"
        );
        assert!(crate::service::may_be_granted_everywhere(&SPEC.operations[0]));
    }
}
