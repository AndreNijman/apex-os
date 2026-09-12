//! The WebDAV provider: read, list and write files on an online account
//! (roadmap P2-017).
//!
//! This is the transport behind the `webdav` and `nextcloud` entries in
//! `apex_secret_core::account::PROVIDERS`. Both route here, because Nextcloud
//! *is* WebDAV — they differ in how the user obtains the credential and in
//! nothing that happens after. P1-001 routes on an operation id's first
//! segment, so two spellings would mean two registry entries that drift.
//!
//! ## Why this operation may name a resource, when `mcp.request` may not
//!
//! `mcp.request` declares [`ResourceKind::None`] precisely so there is no field
//! a session could put a URL in. A file operation cannot do that: "read a file"
//! without saying which file is not an operation. So the resource is a
//! [`Syntax::Path`] — `/`-joined names, no leading `/`, no `//`, no `..`, no
//! `:` — checked by `OperationSpec::check` before this module is reached, and
//! checked again in [`crate::broker::webdav_url`] because the function that
//! builds the URL cannot see that somebody else checked.
//!
//! What that buys: the scheme, host, port and base path all come from the
//! credential as it was stored, and the caller contributes one path *within*
//! it. The framework then pins what [`WebdavProvider::bind`] returns against
//! the stored host, which here is true by construction and checked anyway.
//!
//! ## Why `same_everywhere` is false on all three
//!
//! `documents/notes.md` is a different file on every account and the grant is
//! per account, but the *resource* is resolved from what the caller sends, so
//! a grant held in every project would be a different permission in every
//! directory — which is the exact test `git.push` fails and `mcp.request`
//! passes. `apex account grant` is per project for this reason.
//!
//! ## What a write replaces
//!
//! `PUT` on WebDAV replaces the whole resource. There is no append and no
//! partial write here, and the operation summary says "replacing what is
//! there" rather than "write", because an agent that thought it was appending
//! would find that out by losing the file.

use std::path::PathBuf;

use apex_secret_core::account::Presentation;
use apex_secret_core::operation::{Effect, OperationSpec, ProviderSpec, ResourceKind};
use apex_secret_core::SecretValue;

use crate::broker::{self, HttpAuth, WebdavRequest};
use crate::provider::{Approval, Bind, Bound, Endpoint, Performed, Provider, ProviderError};

/// What a `PROPFIND` asks for: the four properties a listing needs and nothing
/// else.
///
/// `allprop` would be shorter and would also ask a server for every property it
/// holds, including ones a hardened deployment declines to answer — a listing
/// that fails because it asked for too much is worse than a listing that asks
/// for what it shows.
const PROPFIND_BODY: &str = concat!(
    "<?xml version=\"1.0\" encoding=\"utf-8\"?>\n",
    "<d:propfind xmlns:d=\"DAV:\"><d:prop>",
    "<d:displayname/><d:getcontentlength/><d:getlastmodified/><d:resourcetype/>",
    "</d:prop></d:propfind>\n",
);

/// The WebDAV vocabulary, in §13.2's shape.
pub const SPEC: ProviderSpec = ProviderSpec {
    id: "webdav",
    summary: "read, list and write files on a WebDAV or Nextcloud account",
    operations: &[
        OperationSpec {
            id: "webdav.file.list",
            summary: "list the files in a folder on this account",
            effect: Effect::Read,
            resource: ResourceKind::Path,
            params: &[],
            aliases: &[],
            same_everywhere: false,
        },
        OperationSpec {
            id: "webdav.file.read",
            summary: "read a file from this account",
            effect: Effect::Read,
            resource: ResourceKind::Path,
            params: &[],
            aliases: &[],
            same_everywhere: false,
        },
        OperationSpec {
            id: "webdav.file.write",
            summary: "write a file to this account, replacing what is there",
            effect: Effect::Write,
            resource: ResourceKind::Path,
            params: &[],
            aliases: &[],
            same_everywhere: false,
        },
    ],
};

/// The provider, carrying the directory a request body is staged in.
///
/// The store's own `run/`, which only root may write — the same argument
/// `McpProvider` makes: the child runs as the owner and reads the body by name,
/// and a root write to a name an ordinary account could have created first as a
/// symlink is a root write to wherever that symlink points.
pub struct WebdavProvider {
    run_dir: PathBuf,
}

impl WebdavProvider {
    pub fn new(run_dir: PathBuf) -> WebdavProvider {
        WebdavProvider { run_dir }
    }
}

impl Provider for WebdavProvider {
    fn spec(&self) -> &'static ProviderSpec {
        &SPEC
    }

    fn bind(&self, req: &Bind<'_>) -> Result<Bound, ProviderError> {
        // The URL is built here as well as in `perform`, so a path this
        // provider could not turn into a request is refused BEFORE the grant
        // check spends anything and before a credential is read.
        broker::webdav_url(req.service, req.resource).map_err(ProviderError::NoSuchResource)?;
        if req.operation.id == "webdav.file.write" && req.body.is_empty() {
            return Err(ProviderError::Refused(
                "a write carries the bytes to write, and this one carries none. \
                 Pipe them in: printf %s hello | apex secret use <account> \
                 webdav.file.write <path>"
                    .to_string(),
            ));
        }
        Ok(Bound {
            endpoint: Endpoint::from_url(&req.service.url())?,
            // The account and the path, not the URL: an audit line that carried
            // the endpoint would invite somebody to grep the trail for where a
            // person keeps their files. The path is what makes the line useful
            // and it is already the caller's own word.
            detail: format!(
                "{} {} on {}",
                req.operation.id, req.resource, req.service.service
            ),
            // A file operation issues no credential.
            creates: None,
            // §13.8 is about environments — a production deploy against a
            // preview one. A WebDAV account has no such halves, and a per-file
            // approval prompt would be a prompt per file, which is how people
            // are taught to approve without reading.
            approval: Approval::Standing,
        })
    }

    fn perform(
        &self,
        req: &Bind<'_>,
        _bound: &Bound,
        value: &SecretValue,
    ) -> Result<Performed, ProviderError> {
        let (method, body, depth, content_type) = match req.operation.id {
            "webdav.file.list" => ("PROPFIND", PROPFIND_BODY.as_bytes(), Some("1"), Some("application/xml")),
            "webdav.file.read" => ("GET", &b""[..], None, None),
            "webdav.file.write" => ("PUT", req.body, None, Some("application/octet-stream")),
            // The registry routed here on the id's first segment, so an id this
            // match does not have is a registry entry with no implementation —
            // a refusal, never a default method.
            other => {
                return Err(ProviderError::Refused(format!(
                    "{other} is declared by this provider and not implemented by it"
                )))
            }
        };
        let auth = presentation(req.service.auth.as_str(), &req.service.username);
        let out = broker::perform_webdav(
            req.service,
            value,
            &auth,
            &WebdavRequest {
                method,
                rel_path: req.resource,
                body,
                depth,
                content_type,
            },
            req.owner,
            &self.run_dir,
        )
        .map_err(ProviderError::Failed)?;
        Ok(Performed {
            code: out.code,
            output: out.text,
            created: None,
        })
    }
}

/// How to present the stored value, from what the store recorded.
///
/// `bearer` is the one `ServiceInfo::auth` names outright. Anything else is
/// `raw`, which for this transport means an app password — Basic, with the
/// username stored beside it. A `raw` credential stored without a username is
/// the one case where Basic cannot be built, and it falls back to the header
/// form rather than sending `:password`, so the far end's 401 says "no
/// credentials" instead of "wrong ones".
fn presentation<'a>(auth: &str, username: &'a str) -> HttpAuth<'a> {
    if auth == Presentation::Bearer.service_auth() || username.is_empty() {
        HttpAuth::Header
    } else {
        HttpAuth::Basic { username }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use apex_secret_core::account;
    use apex_secret_core::operation::{Params, Syntax};
    use apex_secret_core::store::ServiceInfo;

    fn service() -> ServiceInfo {
        ServiceInfo {
            service: "account.nextcloud.home".into(),
            host: "cloud.example".into(),
            scheme: "https".into(),
            username: "me".into(),
            path: "/remote.php/dav/files".into(),
            auth: "raw".into(),
            port: None,
            added: 0,
        }
    }

    #[test]
    fn the_declaration_is_one_a_registry_will_take() {
        SPEC.validate().expect("the webdav provider must declare validly");
        // Every scope the account model offers for this transport must exist
        // here. The two halves are in different crates — the vocabulary a user
        // is shown, and the operations a daemon will run — and a scope naming
        // an operation nothing implements is a grant that can never be used.
        for p in account::PROVIDERS.iter().filter(|p| p.transport == SPEC.id) {
            for scope in p.scopes {
                assert!(
                    SPEC.operations.iter().any(|o| o.id == scope.operation),
                    "{} offers scope {} -> {}, which this provider does not implement",
                    p.id,
                    scope.name,
                    scope.operation
                );
            }
        }
    }

    #[test]
    fn a_resource_may_be_a_path_within_the_account_and_nothing_else() {
        // The whole of this provider's argument for taking a resource at all.
        // Each of these is a way to leave the endpoint the credential was
        // pinned to, and each is refused by the declaration rather than by this
        // module remembering to check.
        let op = &SPEC.operations[1];
        assert!(matches!(op.resource, ResourceKind::Path));
        let none = Params::new();
        assert!(op.check("documents/notes.md", &none).is_ok());
        for escape in [
            "https://attacker.example/x",
            "/etc/passwd",
            "../../etc/passwd",
            "a//b",
            "a/../b",
            "host:8080/x",
            "",
        ] {
            assert!(
                op.check(escape, &none).is_err(),
                "{escape:?} was accepted as a resource"
            );
        }
        // And a parameter nobody declared is refused, not ignored.
        let mut smuggled = Params::new();
        smuggled.insert("url".into(), "https://attacker.example".into());
        assert!(op.check("documents/notes.md", &smuggled).is_err());
        // Syntax::Path is the shape being relied on; named so a reader can
        // find it.
        assert!(Syntax::Path.accepts("documents/notes.md"));
    }

    #[test]
    fn the_url_is_the_pinned_endpoint_with_the_callers_path_under_it() {
        let info = service();
        assert_eq!(
            broker::webdav_url(&info, "documents/notes.md").unwrap(),
            "https://cloud.example/remote.php/dav/files/documents/notes.md"
        );
        // A second, independent refusal of the same escapes. `OperationSpec::
        // check` runs first in production; this one runs in the function that
        // actually builds the string, because "the caller checked" is not a
        // property that function can see.
        for escape in ["../../../etc/passwd", "/etc/passwd", "a//b", "x:1/y"] {
            assert!(broker::webdav_url(&info, escape).is_err(), "{escape}");
        }
    }

    #[test]
    fn no_operation_here_may_be_granted_everywhere() {
        // A file path is resolved from what the caller sends, so the same grant
        // in another directory is a different permission. `mcp.request` earns
        // `*` because its endpoint is entirely the stored record's; none of
        // these do.
        for op in SPEC.operations {
            assert!(!op.same_everywhere, "{}", op.id);
        }
    }

    #[test]
    fn a_write_with_nothing_to_write_is_refused_before_the_credential_is_read() {
        let owner = broker::owner(unsafe { libc::getuid() }).expect("own uid");
        let info = service();
        let params = Params::new();
        let provider = WebdavProvider::new(std::env::temp_dir());
        let write = SPEC.operations.iter().find(|o| o.id == "webdav.file.write").unwrap();
        let req = Bind {
            operation: write,
            resource: "documents/notes.md",
            params: &params,
            body: b"",
            project: "/tmp",
            service: &info,
            owner: &owner,
            audit_id: "test",
        };
        assert!(provider.bind(&req).is_err());
        // With bytes, it binds, and the endpoint it binds is the stored one —
        // which is what the framework then pins the request against.
        let req = Bind { body: b"hello", ..req };
        let bound = provider.bind(&req).expect("a write with bytes");
        assert_eq!(bound.endpoint.to_string(), "https://cloud.example");
        assert!(bound.detail.contains("documents/notes.md"), "{}", bound.detail);
        assert!(bound.creates.is_none());
    }

    #[test]
    fn a_read_of_a_path_that_could_not_become_a_url_is_refused_at_bind() {
        // Bind runs before the grant check spends a budget and before any
        // credential is read, so this is where a bad path should die.
        let owner = broker::owner(unsafe { libc::getuid() }).expect("own uid");
        let info = service();
        let params = Params::new();
        let provider = WebdavProvider::new(std::env::temp_dir());
        let read = SPEC.operations.iter().find(|o| o.id == "webdav.file.read").unwrap();
        let req = Bind {
            operation: read,
            resource: "../../etc/passwd",
            params: &params,
            body: b"",
            project: "/tmp",
            service: &info,
            owner: &owner,
            audit_id: "test",
        };
        let e = provider.bind(&req).expect_err("a climbing path");
        assert!(matches!(e, ProviderError::NoSuchResource(_)), "{e:?}");
    }

    #[test]
    fn an_app_password_is_basic_and_a_bearer_token_is_a_header() {
        // The account model says Nextcloud and WebDAV are `Basic` and that
        // `Basic` stores as `raw`. This is the other end of that mapping, and
        // getting it backwards would send an app password as
        // `Authorization: <password>` — which a server answers with 401, so it
        // would read as a wrong password rather than a wrong scheme.
        assert!(matches!(presentation("raw", "me"), HttpAuth::Basic { username: "me" }));
        assert!(matches!(presentation("bearer", "me"), HttpAuth::Header));
        // Stored without a username there is no Basic pair to build, and
        // sending `:password` would tell the far end the account is empty.
        assert!(matches!(presentation("raw", ""), HttpAuth::Header));
        assert_eq!(Presentation::Basic.service_auth(), "raw");
    }
}
