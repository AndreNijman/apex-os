//! The MCP provider: one JSON-RPC message to the server a credential belongs to.
//!
//! §10 wants an MCP server's bearer token out of `~/.claude.json`, where
//! anything running as the user — a managed session included — can read it. The
//! broker holds the token and makes the request instead, and `apex mcp bridge`
//! is what a session's MCP client talks to.
//!
//! ## Why this operation names nothing
//!
//! [`SPEC`]'s one operation declares [`ResourceKind::None`] and no parameters,
//! and that is the whole security argument rather than an omission. There is no
//! field for a session to put a URL, a host or a path in, so the endpoint can
//! only be the one pinned when the credential was stored — host, scheme, port
//! and path, none of them caller-chosen. The framework then pins what
//! [`McpProvider::bind`] returns against the stored credential's own host, which
//! for this provider is true by construction and checked anyway, because the
//! pin is the framework's invariant and not a provider's promise.
//!
//! P0-003 made the same argument with a `Capability::McpRequest` variant that
//! had no fields. P1-001 deleted the enum, and `OperationSpec::check` refuses a
//! resource or a parameter on an operation that declares none — so the property
//! is now enforced by the vocabulary rather than by the shape of a variant.
//!
//! ## Why the message is not a parameter
//!
//! A parameter is declared by the operation and checked against a syntax. A
//! JSON-RPC body is opaque bytes — a `write_note` body is larger than a request
//! line may be — so it travels as [`crate::provider::Bind::body`], framed after
//! the request line, and nothing between the caller and `curl` parses it.
//!
//! ## Why the session id lives here
//!
//! A streamable-HTTP MCP server issues an `Mcp-Session-Id` on `initialize` and
//! expects it on every message after, so somebody has to remember it — and the
//! caller is a confined session, so the answer is not "the caller". It is not a
//! credential and the header is useless without the one the daemon attaches,
//! but it is conversation state belonging to a connection the broker owns.
//!
//! It sits in the provider and not in [`crate::service::Service`] because
//! knowing what an MCP conversation is belongs to the provider; a map of MCP
//! sessions in the framework would be exactly the coupling P1-001 removed.
//!
//! One entry per (account, service) pair, so two sessions talking to the same
//! server share a server-side conversation. That is a real limitation, and it is
//! named here rather than hidden behind a map that pretends otherwise.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Mutex;

use apex_secret_core::operation::{Effect, OperationSpec, ProviderSpec, ResourceKind};
use apex_secret_core::SecretValue;

use crate::broker;
use crate::provider::{Bind, Bound, Endpoint, Performed, Provider, ProviderError};

/// The MCP vocabulary, in §13.2's shape.
///
/// `mcp-request` is P0-003's spelling, kept as an alias for the same reason
/// git's are: a grant on disk says `memory:mcp-request` and it keeps working.
pub const SPEC: ProviderSpec = ProviderSpec {
    id: "mcp",
    summary: "carry a message to the MCP server a credential belongs to",
    operations: &[OperationSpec {
        id: "mcp.request",
        summary: "carry one message to the MCP server this credential belongs to",
        // A message can create, edit or delete whatever the server holds, and
        // this layer cannot tell which — `tools/call` and `tools/list` are the
        // same request shape. Declared as the stronger of the two, because a
        // write named as a read is the mistake that matters.
        effect: Effect::Write,
        resource: ResourceKind::None,
        params: &[],
        aliases: &["mcp-request"],
        // The endpoint comes ENTIRELY from the stored record's own host,
        // port and path — `bind` reads `req.service` and never `req.project`
        // — so the directory the caller stands in contributes nothing to
        // where this goes. That is what makes `*` safe here, and it is a
        // fact about this module rather than about the declaration above:
        // an operation that declared exactly the same thing and read the
        // project in `bind` would not qualify. See
        // `providers::tests::an_operation_that_claims_to_reach_the_same_
        // thing_everywhere_must_bind_the_same_in_two_projects`.
        same_everywhere: true,
    }],
};

/// The provider, carrying the conversations it is in the middle of and the
/// directory a message is staged in.
///
/// `run_dir` is the store's own, which only root may write. The child runs as
/// the owner and has to read the message by name, and a root write to a name an
/// ordinary account could have created first as a symlink is a root write to
/// wherever that symlink points — so the directory is part of the provider's
/// configuration rather than something it picks at call time.
pub struct McpProvider {
    run_dir: PathBuf,
    sessions: Mutex<BTreeMap<(u32, String), String>>,
}

impl McpProvider {
    pub fn new(run_dir: PathBuf) -> McpProvider {
        McpProvider {
            run_dir,
            sessions: Mutex::new(BTreeMap::new()),
        }
    }

    fn session(&self, uid: u32, service: &str) -> Option<String> {
        self.sessions
            .lock()
            .ok()?
            .get(&(uid, service.to_string()))
            .cloned()
    }

    fn remember(&self, uid: u32, service: &str, id: String) {
        if let Ok(mut map) = self.sessions.lock() {
            map.insert((uid, service.to_string()), id);
        }
    }
}

impl Provider for McpProvider {
    fn spec(&self) -> &'static ProviderSpec {
        &SPEC
    }

    /// The endpoint is the stored record, and nothing else contributes to it.
    ///
    /// No resolution step, unlike git: there is no repository to ask, and
    /// asking one would be asking something the caller controls.
    fn bind(&self, req: &Bind<'_>) -> Result<Bound, ProviderError> {
        if req.service.path.is_empty() {
            return Err(ProviderError::Refused(format!(
                "'{}' was stored without an endpoint path, so there is nowhere \
                 to carry a message to; re-add it with --path",
                req.service.service
            )));
        }
        if req.body.is_empty() {
            return Err(ProviderError::Refused(
                "an mcp request carries a message, and this one is empty".to_string(),
            ));
        }
        Ok(Bound {
            endpoint: Endpoint::from_url(&req.service.url())?,
            // The service, never the URL: a path is endpoint detail, and an
            // audit line that carried one would invite somebody to grep the
            // trail for what a session was asking a server about.
            detail: format!("mcp request to {}", req.service.service),
            // An MCP request answers; it does not issue a credential.
            creates: None,
        })
    }

    /// How the credential is presented: an `Authorization` header curl builds
    /// from a configuration on its stdin, never from a command line.
    fn perform(
        &self,
        req: &Bind<'_>,
        _bound: &Bound,
        value: &SecretValue,
    ) -> Result<Performed, ProviderError> {
        let carried = self.session(req.owner.uid, &req.service.service);
        let http = broker::perform_http(
            req.service,
            value,
            req.body,
            carried.as_deref(),
            req.owner,
            &self.run_dir,
        )
        .map_err(ProviderError::Failed)?;
        if let Some(id) = http.session {
            self.remember(req.owner.uid, &req.service.service, id);
        }
        Ok(Performed {
            code: http.out.code,
            output: http.out.text,
            created: None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use apex_secret_core::operation::Params;

    #[test]
    fn an_mcp_request_takes_nothing_from_the_caller() {
        // P0-003's security argument, restated in P1-001's vocabulary. It used
        // to be "the variant has no fields"; it is now "the operation declares
        // no resource and no parameters", and `OperationSpec::check` is what
        // enforces it — an undeclared option is refused, not ignored.
        let op = &SPEC.operations[0];
        assert!(matches!(op.resource, ResourceKind::None));
        assert!(op.params.is_empty());

        let none = Params::new();
        assert!(op.check("", &none).is_ok());
        // A session that tried to name a destination is refused rather than
        // having it quietly dropped.
        assert!(op.check("https://attacker.example", &none).is_err());
        assert!(op.check("anything-at-all", &none).is_err());
        let mut smuggled = Params::new();
        smuggled.insert("url".into(), "https://attacker.example".into());
        assert!(op.check("", &smuggled).is_err());

        // The two facts above are what make the claim below *coherent* —
        // `ProviderSpec::validate` refuses `same_everywhere` on an operation
        // that names something — but they are not what makes it TRUE. That is
        // this module's `bind`, which reads `req.service` and never
        // `req.project`, and it is checked by
        // `providers::tests::an_operation_that_claims_to_reach_the_same_thing_
        // everywhere_binds_the_same_in_two_projects`.
        assert!(op.names_nothing());
        assert!(op.same_everywhere);
    }

    #[test]
    fn the_declaration_is_one_a_registry_will_take() {
        SPEC.validate().expect("the mcp provider must declare validly");
        assert!(SPEC.operations[0].aliases.contains(&"mcp-request"));
    }

    #[test]
    fn a_credential_with_no_endpoint_path_is_refused_with_the_flag_that_fixes_it() {
        // The git credentials already on an upgraded machine have no path, and
        // a provider that guessed one would be a provider picking a
        // destination.
        let owner = broker::owner(unsafe { libc::getuid() }).expect("own uid");
        let service = apex_secret_core::store::ServiceInfo {
            service: "gh".into(),
            host: "127.0.0.1".into(),
            scheme: "http".into(),
            username: "x-access-token".into(),
            path: String::new(),
            auth: "bearer".into(),
            port: None,
            added: 0,
        };
        let params = Params::new();
        let req = Bind {
            operation: &SPEC.operations[0],
            resource: "",
            params: &params,
            body: b"{\"id\":1}",
            project: "/tmp",
            service: &service,
            owner: &owner,
            audit_id: "test",
        };
        let e = McpProvider::new(std::env::temp_dir()).bind(&req).expect_err("no path, no endpoint");
        assert!(e.to_string().contains("--path"), "{e}");
    }

    #[test]
    fn an_empty_message_is_refused_before_the_server_is_contacted() {
        // A POST with no body is a request the far end answers with an error
        // about JSON, which reads as the server being broken rather than as the
        // caller having sent nothing.
        let owner = broker::owner(unsafe { libc::getuid() }).expect("own uid");
        let service = apex_secret_core::store::ServiceInfo {
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
        let req = Bind {
            operation: &SPEC.operations[0],
            resource: "",
            params: &params,
            body: b"",
            project: "/tmp",
            service: &service,
            owner: &owner,
            audit_id: "test",
        };
        let provider = McpProvider::new(std::env::temp_dir());
        assert!(provider.bind(&req).is_err());

        // ...and with a message, the endpoint is the stored record's own, port
        // and all, with the port left out of what gets pinned and audited.
        let req = Bind {
            body: b"{\"jsonrpc\":\"2.0\",\"id\":1}",
            ..req
        };
        let bound = provider.bind(&req).expect("a message and a path");
        assert_eq!(bound.endpoint.to_string(), "http://127.0.0.1");
        assert_eq!(bound.detail, "mcp request to memory");
    }

    #[test]
    fn a_session_id_is_remembered_per_account_and_service() {
        // Two accounts talking to the same server are two conversations. A
        // single slot would replay one account's session id into the other's
        // request.
        let p = McpProvider::new(std::env::temp_dir());
        assert_eq!(p.session(1000, "memory"), None);
        p.remember(1000, "memory", "s-1".into());
        p.remember(1001, "memory", "s-2".into());
        p.remember(1000, "other", "s-3".into());
        assert_eq!(p.session(1000, "memory").as_deref(), Some("s-1"));
        assert_eq!(p.session(1001, "memory").as_deref(), Some("s-2"));
        assert_eq!(p.session(1000, "other").as_deref(), Some("s-3"));
    }
}
