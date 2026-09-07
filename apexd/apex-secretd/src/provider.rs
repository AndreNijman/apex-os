//! The provider seam: what the framework fixes, and what a provider supplies.
//!
//! §14: *"do not hard-code the capability architecture around Cloudflare"*, with
//! `GitHubProvider`, `CloudflareProvider`, `AWSProvider`, `GCPProvider`,
//! `KubernetesProvider`, `SSHProvider`, `DatabaseProvider` and custom providers
//! all named as targets. P1-001 is the shape those get, and eleven Cloudflare
//! tasks are downstream of it.
//!
//! ## The split
//!
//! A trait only Cloudflare could satisfy would be a failure; so would one so
//! abstract that each provider reimplements policy. §11's record already fixes
//! identity, scoping, expiry and audit, so those belong to the framework and a
//! provider cannot get them wrong by omission. What genuinely varies is
//! narrower than it looks.
//!
//! **The framework fixes** — [`crate::service::Service::use_capability`], in
//! this order, and every step can refuse before the next one runs:
//!
//! 1. **whose account this is** — `SO_PEERCRED`, never the request;
//! 2. **that the operation exists** — [`Registry::lookup`], so the vocabulary is
//!    closed by what is registered;
//! 3. **that the arguments are the declared ones** — `OperationSpec::check`,
//!    which refuses an undeclared option rather than ignoring it;
//! 4. **expiry, project, and §7's origin labels**;
//! 5. **the grant** — per project, per credential, per operation;
//! 6. **the host pin** — the provider says where the credential would go and
//!    the framework compares it with where the credential was stored for. A
//!    provider cannot skip this, because it never sees the value until after;
//! 7. **reading the value**, once, only after every check above;
//! 8. **scrubbing** the value — and a minted one — out of anything returned;
//! 9. **the audit line**, from the record the decision was made on.
//!
//! **A provider supplies** four things and no policy:
//!
//! * **which operations exist** — a `ProviderSpec`, the §13.2 vocabulary;
//! * **what a resource name means** — [`Provider::bind`], which resolves a name
//!   the caller gave into a concrete target *and declares the endpoint*;
//! * **how a credential is presented** — [`Provider::perform`]: a git credential
//!   helper, an `Authorization: Bearer`, a signed request, a `wrangler` child;
//! * **how to mint a short-lived credential**, if it can — [`Provider::mint`],
//!   §13.4. The default is that it cannot.
//!
//! ## Why `bind` and `perform` are separate
//!
//! Because the host pin has to sit between them. If a provider both resolved
//! and acted, the framework's only options would be to trust the provider to
//! check where it was sending the credential, or to re-derive the destination
//! itself — which it cannot, because knowing what `my-bucket` means is the
//! provider's whole job. Splitting the call puts the invariant in the one place
//! it can be enforced for every provider that will ever exist, including ones
//! written after this file.
//!
//! `bind` runs *after* the grant check, deliberately. For git it forks a child
//! and reads a caller-controlled repository, and neither should happen for a
//! request that was never allowed.

use apex_secret_core::capability::{self, EndpointError};
use apex_secret_core::operation::{
    OperationInfo, OperationSpec, Params, ProviderSpec, VocabularyError,
};
use apex_secret_core::store::ServiceInfo;
use apex_secret_core::SecretValue;

use crate::broker::Owner;

/// Everything the framework established before it asked the provider anything.
///
/// Read-only. A provider cannot reach the store, the grant table or the socket
/// through it, and it never holds a [`SecretValue`] — the value is passed
/// separately, to the one method that needs it, after the pin.
pub struct Bind<'a> {
    /// The operation, as its provider declared it.
    pub operation: &'a OperationSpec,
    /// What the caller named. Already checked against
    /// [`apex_secret_core::operation::ResourceKind`] — a name, never a URL.
    pub resource: &'a str,
    /// Already checked against the operation's declared parameters.
    pub params: &'a Params,
    /// The message the operation carries, when it carries one.
    ///
    /// Opaque to the framework, and the one thing here it does not check: what
    /// a message means belongs to the provider, and a framework that parsed it
    /// would be a framework that knew what JSON-RPC was. Empty for every
    /// operation that carries none, which is all of them but `mcp.request`.
    ///
    /// Not a parameter, deliberately. A parameter is declared by the operation
    /// and checked against a syntax; a body is bytes, and declaring one as a
    /// parameter would mean declaring a value nothing can validate.
    pub body: &'a [u8],
    /// Absolute project root the grant was matched on.
    pub project: &'a str,
    /// The stored credential's metadata. **Never the value.**
    pub service: &'a ServiceInfo,
    /// The account the operation runs as.
    pub owner: &'a Owner,
}

/// Scheme and host a credential would be sent to.
///
/// The unit the framework pins on, and the unit the audit line records. No
/// port: a credential is stored for a host, so a loopback fixture on a random
/// port must still match and a remote that spells out `:443` must not stop
/// working. No path: a private repository or bucket name has no business in a
/// trail an administrator greps.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Endpoint {
    pub scheme: String,
    pub host: String,
}

impl Endpoint {
    /// From a resolved http(s) URL, which is how most providers will build one.
    pub fn from_url(url: &str) -> Result<Endpoint, EndpointError> {
        let (scheme, host) = capability::http_endpoint(url)
            .ok_or_else(|| EndpointError::NotHttp(url.to_string()))?;
        Ok(Endpoint {
            scheme: scheme.to_string(),
            host,
        })
    }
}

impl std::fmt::Display for Endpoint {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}://{}", self.scheme, self.host)
    }
}

/// What a provider resolved a request into.
#[derive(Debug, Clone)]
pub struct Bound {
    /// Where the credential would go. The framework pins this against the
    /// stored credential's host and scheme before reading the value.
    pub endpoint: Endpoint,
    /// The operation in words, for the audit line and the reply. Read by a
    /// person, so it says what happened, not what type it was.
    pub detail: String,
}

/// What an operation produced.
///
/// The credential is *not* scrubbed here — the framework does that, with both
/// the stored value and any minted one, so scrubbing is an invariant rather
/// than a thing each provider has to remember.
#[derive(Debug, Clone)]
pub struct Performed {
    pub code: i32,
    pub output: String,
}

/// Why a provider refused or failed.
///
/// Three cases, because the framework turns them into three different answers
/// and a provider that could only say "error" would make every failure look
/// like a bug in APEX.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProviderError {
    /// The named resource does not exist, or does not resolve here.
    NoSuchResource(String),
    /// The provider's own rules say no.
    Refused(String),
    /// It went wrong. The caller cannot fix it by asking differently.
    Failed(String),
}

impl std::fmt::Display for ProviderError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ProviderError::NoSuchResource(m)
            | ProviderError::Refused(m)
            | ProviderError::Failed(m) => f.write_str(m),
        }
    }
}

impl From<EndpointError> for ProviderError {
    fn from(e: EndpointError) -> ProviderError {
        ProviderError::Refused(e.to_string())
    }
}

/// A capability provider.
///
/// Everything here runs inside `apex-secretd`, as root, after the framework has
/// decided the request is allowed. Nothing here decides *whether* it is.
pub trait Provider: Send + Sync {
    /// What this provider offers. Validated at registration.
    fn spec(&self) -> &'static ProviderSpec;

    /// Resolve what the caller named, and say where the credential would go.
    ///
    /// This is the "what a resource name means" half. For git it is
    /// `git remote get-url` in the repository, which is why the caller gives a
    /// NAME: the repository's own configuration decides the URL, not the
    /// session. For Cloudflare it will be the project's account/zone binding
    /// from §13.1.
    ///
    /// **Must not use a credential**, and cannot: no [`SecretValue`] is in
    /// scope. If a provider needs an authenticated call to resolve a name, that
    /// call is itself an operation and belongs in the vocabulary.
    fn bind(&self, req: &Bind<'_>) -> Result<Bound, ProviderError>;

    /// Perform the bound operation with the credential attached.
    ///
    /// This is the "how a credential is presented" half, and the only place a
    /// provider ever sees a value. The value must not be returned, logged, or
    /// put anywhere a caller can read: the framework scrubs the output as a
    /// backstop, and [`apex_secret_core::protocol::Response`] cannot carry one,
    /// but neither of those is a licence to hand it out.
    fn perform(
        &self,
        req: &Bind<'_>,
        bound: &Bound,
        value: &SecretValue,
    ) -> Result<Performed, ProviderError>;

    /// §13.4: exchange the stored credential for a short-lived one scoped to
    /// this operation, if the provider can.
    ///
    /// Returning `Ok(None)` — the default — means it cannot, and the stored
    /// credential is used directly. When it returns a value, that value is what
    /// [`Provider::perform`] is given, and the framework scrubs **both** out of
    /// the output: a minted token is still a credential, and §13.4 is explicit
    /// that even a temporary one is not handed to the agent.
    fn mint(
        &self,
        _req: &Bind<'_>,
        _bound: &Bound,
        _value: &SecretValue,
    ) -> Result<Option<SecretValue>, ProviderError> {
        Ok(None)
    }
}

/// The providers this daemon serves.
///
/// The set is fixed at startup and never changes, so a request cannot cause a
/// provider to appear. Adding one is a `register` call in `main` plus a module
/// — no change to `apex-agent-core`, `apex-agentd`, the `apex` CLI or the wire,
/// which is P1-001's second acceptance criterion.
#[derive(Default)]
pub struct Registry {
    providers: Vec<Box<dyn Provider>>,
}

impl Registry {
    pub fn new() -> Registry {
        Registry::default()
    }

    /// Add a provider, refusing a malformed declaration.
    ///
    /// A `Result` and not a panic, so a test can assert the refusal — but
    /// `main` treats it as fatal, because a daemon serving a provider whose
    /// operations do not parse would advertise names nothing can reach.
    pub fn register(&mut self, provider: Box<dyn Provider>) -> Result<(), String> {
        let spec = provider.spec();
        spec.validate()?;
        if self.providers.iter().any(|p| p.spec().id == spec.id) {
            return Err(format!("'{}' is registered twice", spec.id));
        }
        self.providers.push(provider);
        Ok(())
    }

    /// Resolve an operation name to the provider that offers it.
    ///
    /// The **one** place an alias is canonicalised. Everything downstream — the
    /// grant key, the audit line, the reply — uses `spec.id`, so an old
    /// spelling is an input and never a stored fact.
    pub fn lookup(
        &self,
        name: &str,
    ) -> Result<(&dyn Provider, &'static OperationSpec), VocabularyError> {
        // Two passes so a canonical id always wins over another operation's
        // alias, whatever order providers were registered in.
        for exact in [true, false] {
            for provider in &self.providers {
                let found = provider.spec().operations.iter().find(|op| {
                    if exact {
                        op.id == name
                    } else {
                        op.aliases.contains(&name)
                    }
                });
                if let Some(op) = found {
                    return Ok((provider.as_ref(), op));
                }
            }
        }
        Err(VocabularyError::UnknownOperation(name.to_string()))
    }

    /// Every operation, canonical ids only, sorted.
    ///
    /// What `Response::Hello` advertises and `apex secret capabilities` prints,
    /// so the CLI does not hardcode a vocabulary it would have to keep in step.
    pub fn operation_ids(&self) -> Vec<String> {
        let mut names: Vec<String> = self
            .specs()
            .flat_map(|spec| spec.operations.iter().map(|op| op.id.to_string()))
            .collect();
        names.sort_unstable();
        names
    }

    /// Every operation with what a person needs to use it.
    ///
    /// Sent in `Response::Hello`, so `apex secret capabilities` prints the
    /// registry rather than a list the CLI keeps in step by hand.
    pub fn vocabulary(&self) -> Vec<OperationInfo> {
        let mut out: Vec<OperationInfo> = self
            .specs()
            .flat_map(|spec| spec.operations.iter().map(OperationInfo::of))
            .collect();
        out.sort_by(|a, b| a.id.cmp(&b.id));
        out
    }

    pub fn specs(&self) -> impl Iterator<Item = &'static ProviderSpec> + '_ {
        self.providers.iter().map(|p| p.spec())
    }

}

/// The names one grant may be recorded under: the canonical id, then any older
/// spelling.
///
/// Passed to `Grants::allows_any`, which is why the slice is built from one
/// operation's declaration and never from several — a grant must not widen
/// across operations because two of them share an alias.
pub fn grant_names(op: &'static OperationSpec) -> Vec<&'static str> {
    let mut names = vec![op.id];
    names.extend_from_slice(op.aliases);
    names
}

#[cfg(test)]
mod tests {
    use super::*;
    use apex_secret_core::operation::{Effect, OperationSpec, ResourceKind};

    const ONE: ProviderSpec = ProviderSpec {
        id: "one",
        summary: "the first",
        operations: &[OperationSpec {
            id: "one.thing.read",
            summary: "read a thing",
            effect: Effect::Read,
            resource: ResourceKind::Name,
            params: &[],
            aliases: &["one-read"],
            same_everywhere: false,
        }],
    };

    const TWO: ProviderSpec = ProviderSpec {
        id: "two",
        summary: "the second",
        operations: &[OperationSpec {
            id: "two.thing.read",
            summary: "read a thing",
            effect: Effect::Read,
            resource: ResourceKind::Name,
            params: &[],
            aliases: &[],
            same_everywhere: false,
        }],
    };

    const IMPOSTOR: ProviderSpec = ProviderSpec {
        id: "three",
        summary: "declares somebody else's operation",
        operations: &[OperationSpec {
            id: "one.thing.read",
            summary: "read a thing",
            effect: Effect::Read,
            resource: ResourceKind::Name,
            params: &[],
            aliases: &[],
            same_everywhere: false,
        }],
    };

    struct Stub(&'static ProviderSpec);

    impl Provider for Stub {
        fn spec(&self) -> &'static ProviderSpec {
            self.0
        }
        fn bind(&self, _req: &Bind<'_>) -> Result<Bound, ProviderError> {
            Err(ProviderError::Failed("stub".into()))
        }
        fn perform(
            &self,
            _req: &Bind<'_>,
            _bound: &Bound,
            _value: &SecretValue,
        ) -> Result<Performed, ProviderError> {
            Err(ProviderError::Failed("stub".into()))
        }
    }

    fn registry(specs: &[&'static ProviderSpec]) -> Registry {
        let mut r = Registry::new();
        for spec in specs {
            r.register(Box::new(Stub(spec))).expect("register");
        }
        r
    }

    #[test]
    fn an_operation_routes_to_its_own_provider_by_name() {
        // The property that lets a provider be added without a routing table:
        // the operation id carries the provider in its first segment.
        let r = registry(&[&ONE, &TWO]);
        let (p, op) = r.lookup("two.thing.read").expect("registered");
        assert_eq!(p.spec().id, "two");
        assert_eq!(op.id, "two.thing.read");
    }

    #[test]
    fn a_name_no_provider_declares_is_refused() {
        // The closed-vocabulary property, now closed by the registry. An
        // operation nothing offers must not reach a credential.
        let r = registry(&[&ONE]);
        for evil in ["exec", "one.thing.delete", "two.thing.read", "", "sh"] {
            assert!(
                matches!(
                    r.lookup(evil),
                    Err(VocabularyError::UnknownOperation(_))
                ),
                "'{evil}' resolved"
            );
        }
    }

    #[test]
    fn an_alias_resolves_but_the_canonical_id_is_what_comes_back() {
        let r = registry(&[&ONE]);
        let (_, op) = r.lookup("one-read").expect("alias");
        assert_eq!(op.id, "one.thing.read", "an alias must not become a stored fact");
        assert_eq!(grant_names(op), vec!["one.thing.read", "one-read"]);
    }

    #[test]
    fn a_provider_that_answers_for_another_is_refused_at_registration() {
        // Cheaper to find at startup than when a credential is in flight, and
        // it is the shape a mistyped operation id takes: `cloudflre.dns.read`
        // in a Cloudflare provider would register a whole second provider
        // nobody can route to.
        let mut r = registry(&[&ONE]);
        let err = r.register(Box::new(Stub(&IMPOSTOR))).unwrap_err();
        assert!(err.contains("belongs to 'one'"), "{err}");
    }

    #[test]
    fn the_same_provider_cannot_be_registered_twice() {
        let mut r = registry(&[&ONE]);
        assert!(r.register(Box::new(Stub(&ONE))).is_err());
    }

    #[test]
    fn the_advertised_vocabulary_is_canonical_ids_only() {
        // `apex secret capabilities` prints this. An alias in the list would
        // teach people to use the spelling that is on its way out.
        let r = registry(&[&TWO, &ONE]);
        assert_eq!(r.operation_ids(), vec!["one.thing.read", "two.thing.read"]);
    }

    #[test]
    fn an_endpoint_is_scheme_and_host_with_no_port_and_no_path() {
        // What the pin compares and what the trail records. A path here would
        // put a private repository name in a log an administrator greps.
        let e = Endpoint::from_url("https://x:tok@github.com:443/acme/secret.git").unwrap();
        assert_eq!(e.scheme, "https");
        assert_eq!(e.host, "github.com");
        assert_eq!(e.to_string(), "https://github.com");
        assert!(Endpoint::from_url("git@github.com:a/b").is_err());
    }

    #[test]
    fn a_provider_that_cannot_mint_says_so_rather_than_failing() {
        // §13.4's default. A provider with no short-lived form must not make
        // every request fail, and must not silently look like it minted one.
        let owner = crate::broker::owner(unsafe { libc::getuid() }).expect("own uid");
        let service = ServiceInfo {
            service: "demo".into(),
            host: "example.com".into(),
            scheme: "https".into(),
            username: "x-access-token".into(),
            path: String::new(),
            auth: "bearer".into(),
            port: None,
            added: 0,
        };
        let params = Params::new();
        let req = Bind {
            operation: &ONE.operations[0],
            resource: "thing",
            params: &params,
            body: &[],
            project: "/tmp",
            service: &service,
            owner: &owner,
        };
        let bound = Bound {
            endpoint: Endpoint {
                scheme: "https".into(),
                host: "example.com".into(),
            },
            detail: "read a thing".into(),
        };
        let minted = Stub(&ONE)
            .mint(&req, &bound, &SecretValue::new(b"not-a-real-token".to_vec()))
            .expect("the default must not be an error");
        assert!(minted.is_none());
    }
}
