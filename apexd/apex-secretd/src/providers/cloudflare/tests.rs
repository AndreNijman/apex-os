//! The only place any of this has been run.
//!
//! There is no Cloudflare account and no Cloudflare token on this machine, so
//! the alternative to a loopback double was a provider nobody had ever seen
//! execute. The double speaks Cloudflare's envelope — `{"success":…,"result":…,
//! "errors":[…]}` — answers the documented paths, and **refuses anything with
//! no `Authorization` header**, which is what makes a passing test mean
//! something: a provider that quietly did nothing would come back 401, not 200.
//!
//! What that proves and what it does not:
//!
//! * proved — the request the provider builds is the shape it means to build,
//!   the credential reaches the far side, none of it reaches the caller, the
//!   binding decides what a name means, and the framework's checks all fire;
//! * not proved — that `api.cloudflare.com` accepts any of these bodies. Every
//!   path is the documented one and not one of them has been called for real.
//!   That is what P1-004's `partial` is for.

use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use apex_secret_core::audit::{self, AuditEvent};
use apex_secret_core::capability::CapabilityRecord;
use apex_secret_core::protocol::{ErrorKind, Response};
use apex_secret_core::store::Store;
use apex_secret_core::SecretValue;

use crate::peer::Peer;
use crate::provider::Registry;
use crate::service::{NewService, Service};

use super::*;

/// The stored credential. Distinctive enough that a grep for it cannot match
/// by accident, and long enough to look like a token.
const TOKEN: &str = "apex-cf-sentinel-4f21c9ae7b3d-do-not-leak";

/// The account id the fixture's project binds.
const ACCOUNT: &str = "0123456789abcdef0123456789abcdef";

/// The zone id it binds.
const ZONE: &str = "fedcba9876543210fedcba9876543210";

/// The ids §13.1's file binds for P1-006's four resource tables. A D1 database
/// is a UUID and the other three are 32 hex, which is Cloudflare's own split
/// and the reason the binding has two id shapes rather than one.
const DB_ID: &str = "1a2b3c4d-5e6f-4a8b-9c0d-1e2f3a4b5c6d";
const KV_ID: &str = "00112233445566778899aabbccddeeff";
const QUEUE_ID: &str = "ffeeddccbbaa99887766554433221100";
const HD_ID: &str = "0f0e0d0c0b0a09080706050403020100";

/// What the double has stored under the one KV key a test reads. Not JSON —
/// KV answers a read with the value's bytes — and it carries the credential the
/// request arrived with.
const KV_VALUE: &str = "stored by apex, read back with {{authorization}}";

/// The two record ids the double hands out. 32 lowercase hex, which is the
/// shape the provider checks before one becomes part of a URL.
const RECORD_A: &str = "aa11bb22cc33dd44ee55ff6677889900";
const RECORD_B: &str = "00998877ff66ee55dd44cc33bb22aa11";

/// The one bucket §13.1's file binds.
const BUCKET: &str = "example-assets";

/// §13.10's two ids the fixture's file binds. An Access application id is
/// `oneOf [32 hex, uuid]` in Cloudflare's own schema, so the fixture uses the
/// UUID half — the shape a validator written for hex alone would refuse.
const APP_ID: &str = "f174e90a-fafe-4643-bbbc-4a0ed4fc8415";
const TUNNEL_ID: &str = "f70ff985-a4ef-4643-bbbc-4a0ed4fc8415";

/// What the double answers with when a service token is issued. The secret is
/// the thing this whole unit exists to keep out of the caller's hands, so it is
/// distinctive enough that finding it anywhere means it travelled.
const CLIENT_ID: &str = "8a1b2c3d4e5f60718293a4b5c6d7e8f9.access";
const CLIENT_SECRET: &str = "apex-cf-service-token-secret-0d4e1a-do-not-leak";

/// The SaaS client secret the double puts in an Access application, which a
/// self-hosted one would not carry and an OIDC SaaS one does.
const APP_SECRET: &str = "apex-cf-saas-client-secret-77b2-do-not-leak";

/// §13.6's store, and the two secrets in it. The second name CONTAINS the
/// first, which is the whole hazard: the list endpoint's filter is `search`,
/// and `search` is a substring match.
const STORE_ID: &str = "8c8b1387108e49be85669169793e7bd2";
const SECRET_ID: &str = "3fd85f74b32742f1bff64a85009dda07";
const OLD_SECRET_ID: &str = "11112222333344445555666677778888";

/// What a test sends as a secret's value. Distinctive, so finding it in the
/// body the double received means the caller's own bytes arrived — and finding
/// it anywhere else means they leaked.
const SECRET_VALUE: &str = "apex-secret-store-value-5c2f-do-not-leak";

/// §13.11's two, as §13.1's file binds them. A gateway id is a slug the ACCOUNT
/// chose and a model name starts with `@` — neither is a shape any other id in
/// this provider has.
const GATEWAY_ID: &str = "apex-gateway";
const MODEL: &str = "@cf/meta/llama-3.1-8b-instruct";

/// The exporter credential the double puts on a gateway, nested inside an
/// array — which is where a real one is, and where a scrub that walked only
/// objects would never look.
const OTEL_SECRET: &str = "apex-otel-authorization-9f31-do-not-leak";

/// The connector token the double would hand back for a tunnel. Nothing in this
/// build asks for it; the constant is here so a test can prove that.
const TUNNEL_TOKEN: &str = "apex-cf-tunnel-token-3e9c-do-not-leak";

/// What the double stores for the one object a test reads back. Not JSON, and
/// it carries the credential the request arrived with, so a read that came back
/// unscrubbed would show it.
const OBJECT: &str = "-- apex backup\n-- fetched with {{authorization}}\nCREATE TABLE t (id INTEGER);\n";

/// The `wss://` URL the double hands back for a tail session. It carries its
/// own authorisation, which is why the provider must not pass it on.
const TAIL_URL: &str = "wss://tail.example.invalid/session/apex-tail-secret-91be";

/// §13.1's file, with the ids the REST API needs.
const PROJECT_FILE: &str = r#"
[identity.cloudflare]
account = "example-account"
account_id = "0123456789abcdef0123456789abcdef"

[cloudflare]
zone = "example.com"
zone_id = "fedcba9876543210fedcba9876543210"
buckets = ["example-assets"]

[cloudflare.d1]
project-db = "1a2b3c4d-5e6f-4a8b-9c0d-1e2f3a4b5c6d"

[cloudflare.kv]
cache = "00112233445566778899aabbccddeeff"

[cloudflare.queues]
jobs = "ffeeddccbbaa99887766554433221100"

[cloudflare.hyperdrive]
pg = "0f0e0d0c0b0a09080706050403020100"

[cloudflare.access]
dashboard = "f174e90a-fafe-4643-bbbc-4a0ed4fc8415"

[cloudflare.secrets]
app = "8c8b1387108e49be85669169793e7bd2"

[cloudflare.gateways]
main = "apex-gateway"

[cloudflare.models]
fast = "@cf/meta/llama-3.1-8b-instruct"

[cloudflare.tunnels]
office = "f70ff985-a4ef-4643-bbbc-4a0ed4fc8415"

[cloudflare.preview]
worker = "project-preview"

[cloudflare.production]
worker = "project"
"#;

/// One request the double saw.
#[derive(Debug, Clone)]
struct Seen {
    method: String,
    path: String,
    authorization: Option<String>,
    /// What the request said its body was. R2 puts an object's media type here
    /// and the provider chooses it from a table, so it is worth recording.
    content_type: Option<String>,
    /// Every header the request carried, lower-cased. §13.11 puts the gateway
    /// and the task's own id in headers, and a test cannot measure what the
    /// double does not record.
    headers: BTreeMap<String, String>,
    body: String,
}

/// A stand-in for `api.cloudflare.com`.
struct Fake {
    port: u16,
    seen: Arc<Mutex<Vec<Seen>>>,
}

/// Whether the double behaves, or answers 401 with the header echoed back the
/// way a badly written API reports an auth failure.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    Normal,
    EchoUnauthorized,
    /// An account whose stored credential may create account-owned tokens, so
    /// §13.4's exchange succeeds and the operation spends a token that did not
    /// exist a moment ago.
    ///
    /// A mode rather than the default, because it is a property of the
    /// *account* and not of this build: Cloudflare requires Super
    /// Administrator to create an account-owned token, so most stored
    /// credentials cannot, and [`Mode::Normal`] is the ordinary case where the
    /// exchange is attempted and comes back with nothing.
    Minting,
    /// An account that answers the permission-group list but refuses to create
    /// a token. The shape that separates "would not issue one" from "there is
    /// not one".
    MintingDenied,
    /// An account that issues a token and then will not take the revoke back.
    /// The credential stands until it expires, and the trail has to say so.
    RevokeFails,
}

impl Mode {
    /// Whether the double knows about account-owned tokens at all.
    fn mints(self) -> bool {
        matches!(self, Mode::Minting | Mode::MintingDenied | Mode::RevokeFails)
    }
}

/// The token the double issues, and the id it issues it under. Distinct from
/// the stored credential so a test can say which of the two reached an
/// operation.
const MINTED: &str = "apex-minted-cf-6b31d0a4-do-not-leak";
const MINTED_ID: &str = "abcdef0123456789abcdef0123456789";

/// Whether a path is the credential exchange rather than an operation.
///
/// [`Fake::seen`] hides these and [`Fake::minting`] shows them, because they
/// answer different questions: *what did this operation do* and *what did it
/// cost to narrow the credential first*. A test about `dns.create` should not
/// have to know that the request before it asked which permission groups the
/// account has.
fn is_credential_exchange(path: &str) -> bool {
    let path = path.strip_prefix("/client/v4").unwrap_or(path);
    let path = path.split('?').next().unwrap_or(path);
    path.starts_with(&format!("/accounts/{ACCOUNT}/tokens"))
}

impl Fake {
    fn start(mode: Mode) -> Fake {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind loopback");
        let port = listener.local_addr().expect("addr").port();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let recorder = Arc::clone(&seen);
        std::thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                let recorder = Arc::clone(&recorder);
                std::thread::spawn(move || serve(stream, &recorder, mode));
            }
        });
        Fake { port, seen }
    }

    /// What the OPERATION did — every request except §13.4's credential
    /// exchange. See [`is_credential_exchange`] for why the two are apart.
    fn seen(&self) -> Vec<Seen> {
        self.everything()
            .into_iter()
            .filter(|s| !is_credential_exchange(&s.path))
            .collect()
    }

    /// What narrowing the credential cost: the permission-group list, the
    /// token creation, and the revoke.
    fn minting(&self) -> Vec<Seen> {
        self.everything()
            .into_iter()
            .filter(|s| is_credential_exchange(&s.path))
            .collect()
    }

    /// Both, in the order they happened. Only the tests that care about the
    /// ordering of one against the other use this.
    fn everything(&self) -> Vec<Seen> {
        self.seen.lock().expect("lock").clone()
    }

    fn authorizations(&self) -> Vec<String> {
        self.seen()
            .into_iter()
            .filter_map(|s| s.authorization)
            .collect()
    }
}

fn serve(mut stream: TcpStream, recorder: &Arc<Mutex<Vec<Seen>>>, mode: Mode) {
    let mut reader = BufReader::new(stream.try_clone().expect("clone"));
    let mut first = String::new();
    if reader.read_line(&mut first).is_err() {
        return;
    }
    let mut words = first.split_whitespace();
    let method = words.next().unwrap_or("").to_string();
    let path = words.next().unwrap_or("").to_string();

    let mut authorization = None;
    let mut content_type = None;
    let mut headers: BTreeMap<String, String> = BTreeMap::new();
    let mut length = 0usize;
    loop {
        let mut line = String::new();
        match reader.read_line(&mut line) {
            Ok(0) => break,
            Ok(_) => {}
            Err(_) => return,
        }
        if line.trim().is_empty() {
            break;
        }
        if let Some(value) = line.trim().strip_prefix("Authorization: ") {
            authorization = Some(value.to_string());
        }
        if let Some(value) = line.trim().strip_prefix("Content-Length: ") {
            length = value.trim().parse().unwrap_or(0);
        }
        if let Some(value) = line.trim().strip_prefix("Content-Type: ") {
            content_type = Some(value.to_string());
        }
        if let Some((name, value)) = line.trim().split_once(": ") {
            headers.insert(name.to_ascii_lowercase(), value.to_string());
        }
    }
    let mut body = vec![0u8; length];
    if length > 0 && reader.read_exact(&mut body).is_err() {
        return;
    }
    let body = String::from_utf8_lossy(&body).into_owned();

    recorder.lock().expect("lock").push(Seen {
        method: method.clone(),
        path: path.clone(),
        authorization: authorization.clone(),
        content_type: content_type.clone(),
        headers,
        body,
    });

    // The property that makes every 200 below mean something: with no
    // credential there is no answer.
    let Some(authorization) = authorization else {
        return reply(&mut stream, 401, r#"{"success":false,"errors":[{"code":10000,"message":"Authentication error"}],"result":null}"#);
    };
    if mode == Mode::EchoUnauthorized {
        // What a badly written API does, and the case the framework's scrub
        // exists for. The provider does not scrub this; the framework does.
        let body = format!(
            r#"{{"success":false,"errors":[{{"code":10000,"message":"Authentication error: you sent {authorization}"}}],"result":null}}"#
        );
        return reply(&mut stream, 401, &body);
    }

    let (status, body) = if is_credential_exchange(&path) {
        tokens(&method, &path, mode)
    } else {
        answer(&method, &path)
    };
    // Every success carries the credential back too, in a `messages` entry.
    // Cloudflare does not do this; the point is that it would not matter if it
    // did, and a test where the token never comes back cannot tell a working
    // scrub from a missing one.
    let body = body.replace(
        r#""messages":[]"#,
        &format!(r#""messages":[{{"code":1,"message":"authenticated with {authorization}"}}]"#),
    );
    // ...and a reply that is not the envelope carries it too. R2 and KV answer
    // a read with the stored bytes and `application/octet-stream`, so there is
    // no `messages` array to put it in, and a scrub that only ever ran on JSON
    // would go unnoticed on exactly the two operations whose replies are not
    // JSON.
    let body = body.replace("{{authorization}}", &authorization);
    reply(&mut stream, status, &body);
}

/// §13.4's three endpoints, as the pinned schema documents them.
///
/// Kept apart from [`answer`] so that the ordinary surface and the credential
/// exchange cannot be confused for one another, and so the permission-group
/// list can be wrong in one specific way — see
/// `a_permission_group_this_build_does_not_recognise_is_not_a_refusal`.
fn tokens(method: &str, target: &str, mode: Mode) -> (u16, String) {
    let target = target.strip_prefix("/client/v4").unwrap_or(target);
    let path = target.split('?').next().unwrap_or(target);
    let base = format!("/accounts/{ACCOUNT}/tokens");
    let ok = |result: &str| {
        (
            200u16,
            format!(r#"{{"success":true,"errors":[],"messages":[],"result":{result}}}"#),
        )
    };
    // An account that cannot create tokens does not have a permission-group
    // list to show either: both are the same permission at Cloudflare.
    if !mode.mints() {
        return (
            404,
            r#"{"success":false,"errors":[{"code":7003,"message":"No route for that URI"}],"messages":[],"result":null}"#
                .to_string(),
        );
    }
    match (method, path) {
        ("GET", p) if p == format!("{base}/permission_groups") => ok(&permission_groups()),
        ("POST", p) if p == base => {
            if mode == Mode::MintingDenied {
                return (
                    403,
                    r#"{"success":false,"errors":[{"code":10000,"message":"Authentication error"}],"messages":[],"result":null}"#
                        .to_string(),
                );
            }
            ok(&format!(
                r#"{{"id":"{MINTED_ID}","name":"apex","status":"active","value":"{MINTED}"}}"#
            ))
        }
        ("DELETE", p) if p == format!("{base}/{MINTED_ID}") => {
            if mode == Mode::RevokeFails {
                return (
                    500,
                    r#"{"success":false,"errors":[{"code":1000,"message":"internal"}],"messages":[],"result":null}"#
                        .to_string(),
                );
            }
            ok(&format!(r#"{{"id":"{MINTED_ID}"}}"#))
        }
        _ => (
            404,
            r#"{"success":false,"errors":[{"code":7003,"message":"No route for that URI"}],"messages":[],"result":null}"#
                .to_string(),
        ),
    }
}

/// The account's permission groups, as `GET .../permission_groups` answers.
///
/// Every name [`super::temporary::POLICY`] can ask for, so that the tests
/// measure the exchange rather than a gap in the fixture — except one, which
/// is deliberately spelled the way this build does *not* expect, so there is a
/// row whose absence is a real absence. See
/// `a_permission_group_this_build_does_not_recognise_is_not_a_refusal`.
fn permission_groups() -> String {
    let mut groups: Vec<String> = Vec::new();
    let mut id = 0u32;
    let mut push = |name: &str, groups: &mut Vec<String>, id: &mut u32| {
        *id += 1;
        groups.push(format!(
            r#"{{"id":"{:032x}","name":"{name}","scopes":["com.cloudflare.api.account"]}}"#,
            0xcf00_0000u32 + *id
        ));
    };
    for name in super::temporary::POLICY
        .iter()
        .flat_map(|(_, policy)| policy.groups.iter())
        .map(|slot| slot[0])
        .collect::<std::collections::BTreeSet<_>>()
    {
        // The one gap: an account that does not offer Hyperdrive at all.
        if name == "Hyperdrive Read" {
            continue;
        }
        push(name, &mut groups, &mut id);
    }
    format!("[{}]", groups.join(","))
}

/// The documented paths, with the documented shapes.
///
/// The query string is split off before matching: DNS is the only surface here
/// that carries one, and it carries it on the *lookup* that decides which
/// record a name means, so a double that matched on the whole request-target
/// would 404 exactly the request whose answer the mutation depends on.
fn answer(method: &str, target: &str) -> (u16, String) {
    let target = target.strip_prefix("/client/v4").unwrap_or(target);
    let (path, query) = match target.split_once('?') {
        Some((path, query)) => (path, query),
        None => (target, ""),
    };
    let param = |name: &str| -> String {
        query
            .split('&')
            .find_map(|pair| pair.strip_prefix(&format!("{name}=")))
            .unwrap_or("")
            .to_string()
    };
    let ok = |result: &str| {
        (
            200u16,
            format!(r#"{{"success":true,"errors":[],"messages":[],"result":{result}}}"#),
        )
    };
    match (method, path) {
        ("GET", "/accounts") => ok(&format!(
            r#"[{{"id":"{ACCOUNT}","name":"example-account"}}]"#
        )),
        ("GET", p) if p == format!("/accounts/{ACCOUNT}") => {
            ok(&format!(r#"{{"id":"{ACCOUNT}","name":"example-account"}}"#))
        }
        ("GET", p) if p == format!("/zones/{ZONE}/workers/routes") => ok(
            r#"[{"id":"route1","pattern":"example.com/*","script":"project"},
                {"id":"route2","pattern":"admin.example.com/*","script":"somebody-else"}]"#,
        ),
        ("GET", p) if p.ends_with("/settings") => {
            // A binding that is already there. `secret.bind` sends the whole
            // list back, so a build that forgot to merge would take this one
            // off the worker — silently, and only noticed in production.
            //
            // The `secret_text` one is the hazard: its `text` is `writeOnly`
            // and REQUIRED, so the API returns it without a value and a build
            // that echoed the object back would write an empty secret or be
            // refused for a missing field.
            ok(
                r#"{"bindings":[{"type":"plain_text","name":"GREETING","text":"hi"},{"type":"secret_text","name":"OLD_SECRET"}],"compatibility_date":"2026-09-01","usage_model":"standard"}"#,
            )
        }
        ("POST", p) if p.ends_with("/versions") => {
            ok(r#"{"id":"1c4dd6be-0000-4000-8000-abcdefabcdef","number":7}"#)
        }
        ("POST", p) if p.starts_with("/accounts/") && p.contains("/deployments") => {
            ok(r#"{"id":"dep-9","strategy":"percentage","versions":[{"version_id":"1c4dd6be-0000-4000-8000-abcdefabcdef","percentage":100}]}"#)
        }
        ("POST", p) if p.ends_with("/tails") => ok(&format!(
            r#"{{"id":"tail-1","url":"{TAIL_URL}","expires_at":"2026-09-07T00:00:00Z"}}"#
        )),

        // ── §13.5, R2 ───────────────────────────────────────────────────────
        ("POST", p) if p == format!("/accounts/{ACCOUNT}/r2/buckets") => ok(
            r#"{"name":"example-assets","location":"apac","storage_class":"Standard","creation_date":"2026-09-12T00:00:00.000Z"}"#,
        ),
        ("GET", p) if p == objects_path("") => ok(
            r#"[{"key":"db/today.sql","size":19,"etag":"aa","last_modified":"2026-09-12T00:00:00.000Z"}]"#,
        ),
        // A read answers with the object's own bytes and no envelope at all —
        // `application/octet-stream`, which is what the documented endpoint
        // returns on success even though its failures are JSON.
        ("GET", p) if p.starts_with(&objects_path("/")) => (200, OBJECT.to_string()),
        ("PUT", p) if p.starts_with(&objects_path("/")) => {
            ok(r#"{"key":"db/today.sql","size":19,"etag":"bb","version":"v2"}"#)
        }

        // ── §13.3's storage surfaces ────────────────────────────────────────
        ("GET", p) if p == format!("/accounts/{ACCOUNT}/d1/database/{DB_ID}") => ok(&format!(
            r#"{{"uuid":"{DB_ID}","name":"project-db","num_tables":3,"file_size":16384}}"#
        )),
        ("POST", p) if p == format!("/accounts/{ACCOUNT}/d1/database/{DB_ID}/query") => ok(
            r#"[{"success":true,"results":[{"n":1}],"meta":{"changes":0,"duration":0.4}}]"#,
        ),
        // A KV read answers with the value's bytes and no envelope, the same
        // way an R2 object read does.
        ("GET", p) if p.starts_with(&kv_path("")) => (200, KV_VALUE.to_string()),
        ("PUT", p) if p.starts_with(&kv_path("")) => ok("null"),
        ("POST", p) if p == format!("/accounts/{ACCOUNT}/queues/{QUEUE_ID}/messages") => {
            ok(r#"{"errors":[],"messages":[]}"#)
        }
        ("PATCH", p) if p == format!("/accounts/{ACCOUNT}/queues/{QUEUE_ID}") => ok(&format!(
            r#"{{"queue_id":"{QUEUE_ID}","queue_name":"jobs","settings":{{"delivery_paused":true}}}}"#
        )),
        // The documentation says an origin password is write-only and never
        // comes back. This double sends one anyway: what a build does when the
        // far side hands it a secret it did not ask for is not something to
        // find out in production.
        ("GET", p) if p == format!("/accounts/{ACCOUNT}/hyperdrive/configs/{HD_ID}") => ok(&format!(
            r#"{{"id":"{HD_ID}","name":"pg","origin":{{"host":"db.example.invalid","port":5432,"database":"app","user":"app","password":"{ORIGIN_PASSWORD}","scheme":"postgres"}},"caching":{{"disabled":false}}}}"#
        )),
        ("PATCH", p) if p == format!("/accounts/{ACCOUNT}/hyperdrive/configs/{HD_ID}") => {
            ok(&format!(r#"{{"id":"{HD_ID}","name":"pg","caching":{{"disabled":false}}}}"#))
        }

        // ── §13.11, Workers AI and the AI Gateway ───────────────────────────
        ("POST", p) if p == format!("/accounts/{ACCOUNT}/ai/run/{MODEL}") => {
            ok(r#"{"response":"Cloudflare is a network.","usage":{"prompt_tokens":9,"completion_tokens":6}}"#)
        }
        // **Every value here is one no fallback could have produced.** The
        // read-modify-write fills in five defaults for fields the schema marks
        // required, so a gateway answering with those same defaults would make
        // "it was preserved" and "it was defaulted" the same observation — and
        // a mutation that dropped the read entirely stayed green against an
        // earlier version of this reply. `rate_limiting_limit` is 42 against a
        // fallback of 0, `cache_invalidate_on_update` is true against false,
        // and `logpush` and `retry_max_attempts` are not defaulted at all.
        ("GET", p) if p == gateway_path() => ok(&format!(
            r#"{{"id":"{GATEWAY_ID}","cache_ttl":60,"collect_logs":true,"rate_limiting_limit":42,"rate_limiting_interval":60,"cache_invalidate_on_update":true,"logpush":true,"retry_max_attempts":3,"created_at":"2026-09-01T00:00:00Z","modified_at":"2026-09-01T00:00:00Z","is_default":false}}"#
        )),
        ("GET", p) if p == gateway_path_of("with-otel") => ok(&format!(
            r#"{{"id":"with-otel","cache_ttl":60,"collect_logs":true,"rate_limiting_limit":0,"rate_limiting_interval":0,"cache_invalidate_on_update":false,"otel":[{{"url":"https://otel.example.invalid","authorization":"{OTEL_SECRET}"}}]}}"#
        )),
        ("PUT", p) if p == gateway_path() => ok(&format!(
            r#"{{"id":"{GATEWAY_ID}","cache_ttl":300,"collect_logs":true}}"#
        )),

        // ── §13.6, the Secrets Store ────────────────────────────────────────
        //
        // Every reply is METADATA. `secrets-store_value` is `writeOnly` in
        // Cloudflare's own schema — "the API never returns this value" — so a
        // double that echoed a value back would be testing against an API that
        // does not exist.
        ("POST", p) if p == secrets_path("") => ok(&format!(
            r#"[{{"id":"{SECRET_ID}","name":"API_KEY","store_id":"{STORE_ID}","status":"active","scopes":["workers"],"created":"2026-09-12T00:00:00Z"}}]"#
        )),
        ("GET", p) if p == secrets_path("") => {
            // `search` is a SUBSTRING match, and this double behaves like one.
            // A build that trusted the filter would rotate whichever of these
            // came back first.
            let wanted = param("search");
            let one = |id: &str, name: &str| {
                format!(
                    r#"{{"id":"{id}","name":"{name}","store_id":"{STORE_ID}","status":"active","scopes":["workers"]}}"#
                )
            };
            let mut found: Vec<String> = Vec::new();
            for (id, name) in [(SECRET_ID, "API_KEY"), (OLD_SECRET_ID, "API_KEY_OLD")] {
                if wanted.is_empty() || name.contains(&wanted) {
                    found.push(one(id, name));
                }
            }
            let n = found.len();
            (
                200,
                format!(
                    r#"{{"success":true,"errors":[],"messages":[],"result":[{}],"result_info":{{"count":{n},"page":1,"per_page":100,"total_count":{n}}}}}"#,
                    found.join(",")
                ),
            )
        }
        ("PATCH", p) if p == secrets_path(&format!("/{SECRET_ID}")) => ok(&format!(
            r#"{{"id":"{SECRET_ID}","name":"API_KEY","store_id":"{STORE_ID}","status":"active","scopes":["workers"]}}"#
        )),
        ("PATCH", p) if p.ends_with("/settings") => {
            ok(r#"{"bindings":[],"compatibility_date":"2026-09-01"}"#)
        }

        // ── §13.10, Cloudflare One ──────────────────────────────────────────
        //
        // The Access application is an OIDC SaaS one, so it carries a
        // `client_secret` the documented schema really does return. A double
        // that answered with a self-hosted application would let a build with
        // no scrub at all pass.
        ("GET", p) if p == format!("/accounts/{ACCOUNT}/access/apps/{APP_ID}") => ok(&format!(
            r#"{{"id":"{APP_ID}","name":"dashboard","domain":"admin.example.com","type":"saas","saas_app":{{"client_id":"{CLIENT_ID}","client_secret":"{APP_SECRET}","auth_type":"oidc"}}}}"#
        )),
        ("POST", p) if p == format!("/accounts/{ACCOUNT}/access/apps/{APP_ID}/revoke_tokens") => {
            ok("true")
        }
        ("POST", p) if p == format!("/accounts/{ACCOUNT}/access/service_tokens") => (
            201,
            format!(
                r#"{{"success":true,"errors":[],"messages":[],"result":{{"id":"1e0b9a4c-0000-4000-8000-abcdefabcdef","name":"ci","client_id":"{CLIENT_ID}","client_secret":"{CLIENT_SECRET}","duration":"8760h"}}}}"#
            ),
        ),
        ("GET", p) if p == format!("/accounts/{ACCOUNT}/cfd_tunnel/{TUNNEL_ID}") => ok(&format!(
            r#"{{"id":"{TUNNEL_ID}","name":"office","status":"healthy","connections":[],"created_at":"2026-09-01T00:00:00Z"}}"#
        )),
        ("PATCH", p) if p == format!("/accounts/{ACCOUNT}/cfd_tunnel/{TUNNEL_ID}") => ok(&format!(
            r#"{{"id":"{TUNNEL_ID}","name":"branch-office","status":"healthy"}}"#
        )),
        // The endpoint this build does not declare. It answers, so that a test
        // asserting the token never comes back is measuring a refusal to ask
        // rather than a double that had nothing to give.
        ("GET", p) if p == format!("/accounts/{ACCOUNT}/cfd_tunnel/{TUNNEL_ID}/token") => {
            ok(&format!(r#""{TUNNEL_TOKEN}""#))
        }

        // ── §13.9, DNS ──────────────────────────────────────────────────────
        //
        // The zone answers about the name it was asked about, and each name
        // below is one of the five things a lookup can run into. A double that
        // only ever answered "here is your record" would let a build that
        // could not tell a 403 from an empty zone pass every test in this file.
        ("GET", p) if p == records_path() => {
            let name = param("name.exact");
            let kind = param("type");
            let one = |id: &str, name: &str, content: &str| {
                format!(r#"{{"id":"{id}","name":"{name}","type":"{kind}","content":"{content}","ttl":1,"proxied":false}}"#)
            };
            match name.as_str() {
                // Denied. A body that looks like every other failure, with the
                // one documented denial signal in it.
                "denied.example.com" => (
                    403,
                    r#"{"success":false,"errors":[{"code":10000,"message":"Forbidden","documentation_url":"https://developers.cloudflare.com/api/resources/dns/subresources/records/methods/list"}],"messages":[],"result":null}"#.to_string(),
                ),
                // Absent: the zone answered, successfully, with nothing.
                "gone.example.com" => ok("[]"),
                // Ambiguous: two A records at one name is ordinary
                // round-robin, and picking either would be a guess.
                "many.example.com" => ok(&format!(
                    "[{},{}]",
                    one(RECORD_A, "many.example.com", "203.0.113.1"),
                    one(RECORD_B, "many.example.com", "203.0.113.2")
                )),
                // A zone that ignored the filter and answered with the whole
                // thing. The build has to notice that none of it is the record
                // it asked for.
                "unfiltered.example.com" => ok(&format!(
                    "[{},{}]",
                    one(RECORD_A, "www.example.com", "203.0.113.1"),
                    one(RECORD_B, "other.example.com", "203.0.113.2")
                )),
                other => ok(&format!("[{}]", one(RECORD_A, other, "203.0.113.1"))),
            }
        }
        ("POST", p) if p == records_path() => ok(&format!(
            r#"{{"id":"{RECORD_A}","name":"new.example.com","type":"A","content":"203.0.113.7"}}"#
        )),
        ("PATCH", p) if p == format!("{}/{RECORD_A}", records_path()) => ok(&format!(
            r#"{{"id":"{RECORD_A}","name":"www.example.com","type":"A","content":"203.0.113.8"}}"#
        )),
        ("DELETE", p) if p == format!("{}/{RECORD_A}", records_path()) => {
            ok(&format!(r#"{{"id":"{RECORD_A}"}}"#))
        }

        _ => (
            404,
            format!(
                r#"{{"success":false,"errors":[{{"code":7003,"message":"no route for {method} {path}"}}],"result":null}}"#
            ),
        ),
    }
}

/// The password the double puts in a Hyperdrive reply, which the documented
/// API never would. Distinctive, so finding it anywhere downstream means it
/// travelled rather than merely resembling something.
const ORIGIN_PASSWORD: &str = "apex-hyperdrive-origin-4c7f-do-not-leak";

/// Where the double keeps one namespace's values.
fn kv_path(suffix: &str) -> String {
    format!("/accounts/{ACCOUNT}/storage/kv/namespaces/{KV_ID}/values{suffix}")
}

/// Where this project's one gateway lives.
fn gateway_path() -> String {
    gateway_path_of(GATEWAY_ID)
}

fn gateway_path_of(id: &str) -> String {
    format!("/accounts/{ACCOUNT}/ai-gateway/gateways/{id}")
}

/// Where the double keeps this store's secrets.
fn secrets_path(suffix: &str) -> String {
    format!("/accounts/{ACCOUNT}/secrets_store/stores/{STORE_ID}/secrets{suffix}")
}

/// Where the double keeps this zone's records.
fn records_path() -> String {
    format!("/zones/{ZONE}/dns_records")
}

fn reply(stream: &mut TcpStream, status: u16, body: &str) {
    let reason = if status == 200 { "OK" } else { "Error" };
    // An object's bytes are not the envelope, and saying they are would be the
    // one lie in this double that a provider could come to depend on.
    let kind = if body.starts_with('{') || body.starts_with('[') {
        "application/json"
    } else {
        "application/octet-stream"
    };
    let _ = write!(
        stream,
        "HTTP/1.1 {status} {reason}\r\nContent-Type: {kind}\r\n\
         Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
}

/// The prefix every R2 object path in the double shares, with `suffix` after
/// `objects` — `""` for the bucket's listing, `"/"` for one object.
fn objects_path(suffix: &str) -> String {
    format!("/accounts/{ACCOUNT}/r2/buckets/{BUCKET}/objects{suffix}")
}

struct Fixture {
    service: Service,
    store: PathBuf,
    project: PathBuf,
    fake: Fake,
}

impl Drop for Fixture {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.store).ok();
        std::fs::remove_dir_all(&self.project).ok();
    }
}

fn me() -> Peer {
    // Safe: getuid/getgid cannot fail.
    Peer {
        pid: std::process::id() as libc::pid_t,
        uid: unsafe { libc::getuid() },
        gid: unsafe { libc::getgid() },
    }
}

impl Fixture {
    /// A service serving ONLY the Cloudflare provider, with a credential
    /// stored for the double and a project bound the way §13.1 says.
    fn new(name: &str, mode: Mode, granted: &[&str]) -> Fixture {
        let fake = Fake::start(mode);
        let tag = format!("{name}-{}-{}", std::process::id(), fake.port);
        let store = std::env::temp_dir().join(format!("apex-cf-store-{tag}"));
        let project = std::env::temp_dir().join(format!("apex-cf-project-{tag}"));
        std::fs::remove_dir_all(&store).ok();
        std::fs::remove_dir_all(&project).ok();
        std::fs::create_dir_all(&project).expect("project");
        std::fs::write(project.join("apex.toml"), PROJECT_FILE).expect("apex.toml");

        let mut registry = Registry::new();
        registry
            .register(Box::new(CloudflareProvider::at(fake.port)))
            .expect("register");
        let service = Service::new(Store::new(store.clone()), false, registry);

        let peer = me();
        // `http` is allowed for a loopback host: nothing crosses a network.
        assert_eq!(
            service.add(
                peer,
                NewService {
                    service: "cloudflare",
                    host: "127.0.0.1",
                    scheme: "http",
                    username: None,
                    path: "",
                    auth: None,
                    port: None,
                },
                SecretValue::new(TOKEN.as_bytes().to_vec()),
            ),
            Response::Ok
        );
        for operation in granted {
            assert!(service
                .grant(
                    peer,
                    project.to_str().expect("utf8"),
                    "cloudflare",
                    operation,
                    false
                )
                .as_error()
                .is_none());
        }
        Fixture {
            service,
            store,
            project,
            fake,
        }
    }

    fn record(&self, operation: &str, resource: &str) -> CapabilityRecord {
        let mut rec = CapabilityRecord::new("cloudflare", operation, resource);
        rec.project = Some(self.project.to_string_lossy().into_owned());
        rec
    }

    fn use_it(&self, record: CapabilityRecord) -> Response {
        self.service.use_capability(me(), record, Vec::new())
    }

    /// The same, with bytes after the request line — which is how a secret's
    /// value travels, and the reason it is not an option.
    fn use_with_input(&self, record: CapabilityRecord, input: &str) -> Response {
        self.service
            .use_capability(me(), record, input.as_bytes().to_vec())
    }

    fn trail(&self) -> String {
        std::fs::read_to_string(Store::new(self.store.clone()).audit_path()).unwrap_or_default()
    }
}

/// One row of [`every_operation`]: the operation id, the resource to ask it
/// for, the options its own declaration accepts, and **how many authenticated
/// requests performing it should take**.
///
/// The count is a field rather than an assumption because it stopped being one
/// everywhere. `dns.update` and `dns.delete` ask the zone which record a name
/// means before changing it, so they are two requests each — and a test that
/// asserted "one call per operation" would either have to be loosened into
/// meaninglessness or be wrong. Written down, it stays a measurement: an
/// operation that quietly started making an extra call would fail here.
type OperationCase = (
    &'static str,
    &'static str,
    Vec<(&'static str, &'static str)>,
    usize,
);

/// Every operation the provider declares, with a resource and options that its
/// own declaration accepts.
fn every_operation() -> Vec<OperationCase> {
    vec![
        ("cloudflare.account.read", "", vec![], 1),
        ("cloudflare.worker.read", "project", vec![], 1),
        (
            "cloudflare.worker.upload-version",
            "project",
            vec![
                ("script", "dist/worker.js"),
                ("compatibility-date", "2026-09-01"),
                ("message", "released by apex"),
            ],
            1,
        ),
        (
            "cloudflare.worker.deploy",
            "project",
            vec![("version", "1c4dd6be-0000-4000-8000-abcdefabcdef")],
            1,
        ),
        (
            "cloudflare.worker.rollback",
            "project",
            vec![("version", "0a0b0c0d-0000-4000-8000-000000000000")],
            1,
        ),
        ("cloudflare.worker.tail", "project", vec![], 1),
        ("cloudflare.worker.route.read", "project", vec![], 1),
        ("cloudflare.r2.object.read", "example-assets/db/today.sql", vec![], 1),
        (
            "cloudflare.r2.object.write",
            "example-assets/db/today.sql",
            vec![("file", "dist/today.sql")],
            1,
        ),
        (
            "cloudflare.r2.bucket.create",
            "example-assets",
            vec![("location", "apac"), ("storage-class", "Standard")],
            1,
        ),
        ("cloudflare.d1.read", "project-db", vec![], 1),
        ("cloudflare.d1.query", "project-db", vec![("sql", "SELECT 1")], 1),
        (
            "cloudflare.d1.migrate",
            "project-db",
            vec![("file", "migrations/001.sql")],
            1,
        ),
        ("cloudflare.kv.read", "cache/greeting", vec![], 1),
        ("cloudflare.kv.write", "cache/greeting", vec![("value", "hello")], 1),
        ("cloudflare.queue.publish", "jobs", vec![("message", "work to do")], 1),
        ("cloudflare.queue.manage", "jobs", vec![("paused", "true")], 1),
        ("cloudflare.hyperdrive.read", "pg", vec![], 1),
        ("cloudflare.hyperdrive.edit", "pg", vec![("caching", "on")], 1),
        (
            "cloudflare.secret.create",
            "app/API_KEY",
            vec![("scopes", "workers"), ("file", "secrets/value.txt")],
            1,
        ),
        (
            "cloudflare.secret.rotate",
            "app/API_KEY",
            vec![("file", "secrets/value.txt")],
            2,
        ),
        (
            "cloudflare.secret.bind",
            "app/API_KEY",
            vec![("worker", "project"), ("binding", "API_KEY")],
            2,
        ),
        ("cloudflare.workers-ai.run", "fast", vec![("prompt", "What is Cloudflare?")], 1),
        (
            "cloudflare.ai-gateway.run",
            "main/fast",
            vec![("prompt", "What is Cloudflare?")],
            1,
        ),
        ("cloudflare.ai-gateway.edit", "main", vec![("cache-ttl", "300")], 2),
        ("cloudflare.access.read", "dashboard", vec![], 1),
        ("cloudflare.access.edit", "dashboard", vec![], 1),
        (
            "cloudflare.access.service-token.create",
            "admin.example.com",
            vec![("name", "ci"), ("duration", "8760h")],
            1,
        ),
        ("cloudflare.tunnel.read", "office", vec![], 1),
        ("cloudflare.tunnel.edit", "office", vec![("name", "branch-office")], 1),
        ("cloudflare.dns.read", "www.example.com", vec![("type", "A")], 1),
        (
            "cloudflare.dns.create",
            "new.example.com",
            vec![
                ("type", "A"),
                ("content", "203.0.113.7"),
                ("ttl", "300"),
                ("proxied", "false"),
                ("comment", "added by apex"),
            ],
            1,
        ),
        (
            "cloudflare.dns.update",
            "www.example.com",
            vec![("type", "A"), ("content", "203.0.113.8")],
            2,
        ),
        ("cloudflare.dns.delete", "old.example.com", vec![("type", "A")], 2),
    ]
}

/// How many authenticated requests the whole surface should take.
fn expected_calls() -> usize {
    every_operation().iter().map(|(_, _, _, calls)| calls).sum()
}

/// The files the project holds for the operations that upload one: a Worker
/// module, and something to put in a bucket.
fn with_files(fixture: &Fixture) {
    std::fs::create_dir_all(fixture.project.join("dist")).expect("dist");
    std::fs::write(
        fixture.project.join("dist/worker.js"),
        "export default { fetch: () => new Response('hi') };\n",
    )
    .expect("worker.js");
    std::fs::write(fixture.project.join("dist/today.sql"), UPLOADED).expect("today.sql");
    std::fs::create_dir_all(fixture.project.join("migrations")).expect("migrations");
    std::fs::write(fixture.project.join("migrations/001.sql"), MIGRATION).expect("001.sql");
    std::fs::create_dir_all(fixture.project.join("secrets")).expect("secrets");
    std::fs::write(fixture.project.join("secrets/value.txt"), SECRET_VALUE).expect("value.txt");
}

/// A migration: several statements over several lines, which is exactly what a
/// `Syntax::Text` parameter could not have carried.
const MIGRATION: &str = "CREATE TABLE t (id INTEGER PRIMARY KEY);\nINSERT INTO t VALUES (1);\n";

/// What a test puts into a bucket. Distinctive, so finding it in the body the
/// double received means the project's own file arrived and not something that
/// merely has the right length.
const UPLOADED: &str = "INSERT INTO t VALUES (1);\n";

fn granted_everything() -> Vec<&'static str> {
    SPEC.operations.iter().map(|op| op.id).collect()
}

#[test]
fn the_double_answers_nothing_without_a_credential() {
    // The assumption every other test in this file rests on. If the double
    // answered 200 to an unauthenticated request, a provider that never sent
    // the token would still pass, which is the failure mode this whole
    // program keeps finding.
    let fake = Fake::start(Mode::Normal);
    let mut stream = TcpStream::connect(("127.0.0.1", fake.port)).expect("connect");
    write!(
        stream,
        "GET /client/v4/accounts HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n"
    )
    .expect("write");
    let mut answer = String::new();
    stream.read_to_string(&mut answer).expect("read");
    assert!(answer.starts_with("HTTP/1.1 401"), "{answer}");
    assert!(answer.contains("Authentication error"), "{answer}");
}

#[test]
fn every_declared_operation_reaches_cloudflare_with_the_credential_and_returns_without_it() {
    // P1-002's third criterion, as a measurement rather than an assertion.
    // The agent side is handed the whole operation surface; for each one the
    // credential is shown to have reached the far side, and shown not to be in
    // the reply, the output, the serialised response or the audit trail.
    let f = Fixture::new("surface", Mode::Normal, &granted_everything());
    with_files(&f);

    for (operation, resource, options, _) in every_operation() {
        let mut rec = f.record(operation, resource);
        for (name, value) in &options {
            rec = rec.param(name, value);
        }
        let reply = f.use_it(rec);
        let Response::Performed {
            endpoint,
            exit_code,
            output,
            record,
        } = &reply
        else {
            panic!("{operation} was refused: {reply:?}");
        };
        assert_eq!(*exit_code, 0, "{operation}: {output}");
        assert_eq!(endpoint, "http://127.0.0.1", "{operation}");
        assert_eq!(record.operation, operation);
        assert_eq!(record.approval_policy, "grant");
        assert!(!output.contains(TOKEN), "{operation} handed back the token");
        // The far side echoed it in every reply, so an unscrubbed answer would
        // contain it and a scrubbed one says where it was.
        assert!(
            output.contains("«redacted»"),
            "{operation}: the reply was not scrubbed, it just had nothing in it"
        );
        assert!(
            !serde_json::to_string(&reply).unwrap().contains(TOKEN),
            "{operation}: the serialised reply carries the token"
        );
    }

    // One authenticated request per operation, every one of them carrying the
    // stored credential. A provider that exited zero without calling anything
    // would leave this empty.
    let authorizations = f.fake.authorizations();
    assert_eq!(
        authorizations.len(),
        expected_calls(),
        "every request the surface makes, and no others: {authorizations:?}"
    );
    for authorization in &authorizations {
        assert_eq!(authorization, &format!("Bearer {TOKEN}"));
    }

    let trail = f.trail();
    assert!(!trail.contains(TOKEN), "the audit trail holds the credential");
    assert_eq!(
        trail.matches("\"event\":\"used\"").count(),
        SPEC.operations.len(),
        "every operation should have left a used line"
    );
    // One trail line per operation even where an operation took two requests:
    // the trail records what was authorised, not what the wire carried.
    assert!(expected_calls() > SPEC.operations.len());
}

#[test]
fn the_agent_side_holds_no_credential_even_when_it_holds_every_operation() {
    // The same criterion from the other end: with every capability granted,
    // there is no request in the protocol whose reply carries a value, and the
    // store the value is in is not the caller's to read. `Response` cannot
    // even be constructed with one — `SecretValue` implements no `Serialize` —
    // so this checks the reachable surface rather than restating the type.
    let f = Fixture::new("noread", Mode::Normal, &granted_everything());
    with_files(&f);
    let peer = me();

    let mut replies = vec![
        f.service.hello(),
        f.service.list(peer),
        f.service.grants(peer),
        f.service.audit(peer, 100),
    ];
    for (operation, resource, options, _) in every_operation() {
        let mut rec = f.record(operation, resource);
        for (name, value) in &options {
            rec = rec.param(name, value);
        }
        replies.push(f.use_it(rec));
    }

    for reply in &replies {
        let text = serde_json::to_string(reply).expect("every reply serialises");
        assert!(
            !text.contains(TOKEN),
            "{} carried the credential",
            reply.variant()
        );
    }
    // ...and it did reach Cloudflare, so this is a brokered operation and not
    // a set of calls that never happened.
    assert_eq!(f.fake.authorizations().len(), expected_calls());
}

#[test]
fn an_api_that_echoes_the_credential_back_does_not_get_to_hand_it_over() {
    // The scrub, against a Cloudflare credential, on the path a real API takes
    // when it reports an auth failure badly. The provider does nothing here;
    // the framework does it, which is what makes it hold for the next provider
    // as well.
    let f = Fixture::new("echo", Mode::EchoUnauthorized, &["cloudflare.account.read"]);
    let reply = f.use_it(f.record("cloudflare.account.read", ""));
    let Response::Performed { exit_code, output, .. } = &reply else {
        panic!("expected a result carrying the failure: {reply:?}");
    };
    assert_eq!(*exit_code, 1, "a 401 is not a success");
    assert!(output.contains("HTTP 401"), "{output}");
    // The far side put the credential in the body; it is not in the answer.
    assert!(!output.contains(TOKEN), "{output}");
    assert!(output.contains("«redacted»"), "{output}");
    assert!(!serde_json::to_string(&reply).unwrap().contains(TOKEN));
    assert!(!f.trail().contains(TOKEN), "the trail holds the credential");
    // It really was sent, so the echo really was of the real thing.
    assert_eq!(f.fake.authorizations(), vec![format!("Bearer {TOKEN}")]);
}

#[test]
fn a_tail_session_url_is_a_credential_and_does_not_come_back() {
    // `POST …/tails` answers with a wss:// URL that authorises whoever holds
    // it to read the worker's live logs. Returning it would hand the agent a
    // credential by another name.
    let f = Fixture::new("tail", Mode::Normal, &["cloudflare.worker.tail"]);
    let reply = f.use_it(f.record("cloudflare.worker.tail", "project"));
    let Response::Performed { output, .. } = &reply else {
        panic!("refused: {reply:?}");
    };
    // The session was created — the id and the expiry are there.
    assert!(output.contains("tail-1"), "{output}");
    assert!(output.contains("expires_at"), "{output}");
    // ...and the URL is not.
    assert!(!output.contains(TAIL_URL), "the tail URL came back: {output}");
    assert!(!output.contains("wss://"), "{output}");
    assert!(output.contains("not returned"), "{output}");
    assert!(!f.trail().contains(TAIL_URL));
    // The double really did send one, so this is a removal and not an absence.
    let sent = f.fake.seen();
    assert!(sent.iter().any(|s| s.path.ends_with("/tails")), "{sent:?}");
}

#[test]
fn a_name_this_project_did_not_bind_never_reaches_cloudflare() {
    // The semantic check. `project` is bound to production; `somebody-elses`
    // is not bound to anything, and the difference is decided before a
    // credential is read.
    let f = Fixture::new("unbound", Mode::Normal, &granted_everything());
    let reply = f.use_it(f.record("cloudflare.worker.read", "somebody-elses-worker"));
    let (kind, message) = reply.as_error().expect("an unbound name must be refused");
    assert_eq!(kind, ErrorKind::BadRequest);
    assert!(message.contains("somebody-elses-worker"), "{message}");
    assert!(message.contains("apex.toml"), "{message}");
    assert!(message.contains("project and project-preview"), "{message}");
    assert!(
        f.fake.seen().is_empty(),
        "a refused request still reached the api"
    );
}

#[test]
fn a_project_that_binds_no_cloudflare_account_is_told_what_to_write() {
    // A project with no binding at all. The account read still works, because
    // it is how you find out what to put in the file; everything else refuses
    // with the file to edit named.
    let f = Fixture::new("nobinding", Mode::Normal, &granted_everything());
    std::fs::write(f.project.join("apex.toml"), "[something.else]\nkey = \"v\"\n").expect("write");

    let reply = f.use_it(f.record("cloudflare.worker.deploy", "project").param("version", "v1"));
    let (_, message) = reply.as_error().expect("refused");
    assert!(message.contains("[identity.cloudflare]"), "{message}");

    // ...and the discovery path still answers, against /accounts.
    let reply = f.use_it(f.record("cloudflare.account.read", ""));
    assert!(matches!(reply, Response::Performed { exit_code: 0, .. }), "{reply:?}");
    assert_eq!(f.fake.seen()[0].path, "/client/v4/accounts");
}

#[test]
fn the_binding_decides_which_account_and_which_worker_the_call_names() {
    // "Account/zone/project binding works", as a path rather than a promise:
    // the account id and the worker name in the URL came out of apex.toml, and
    // the zone id in the routes call came out of the same file.
    let f = Fixture::new("paths", Mode::Normal, &granted_everything());
    f.use_it(f.record("cloudflare.worker.read", "project-preview"));
    f.use_it(f.record("cloudflare.worker.route.read", "project"));

    let paths: Vec<String> = f.fake.seen().into_iter().map(|s| s.path).collect();
    assert_eq!(
        paths,
        vec![
            format!("/client/v4/accounts/{ACCOUNT}/workers/scripts/project-preview/settings"),
            format!("/client/v4/zones/{ZONE}/workers/routes"),
        ]
    );
}

#[test]
fn an_upload_carries_the_project_s_own_module_and_the_metadata_that_names_it() {
    // The multipart body is built here rather than by curl's `-F`, so this is
    // where it gets checked: the module arrives, the metadata names it, and
    // the annotation goes in the body rather than on a command line.
    let f = Fixture::new("upload", Mode::Normal, &["cloudflare.worker.upload-version"]);
    with_files(&f);
    let reply = f.use_it(
        f.record("cloudflare.worker.upload-version", "project")
            .param("script", "dist/worker.js")
            .param("compatibility-date", "2026-09-01")
            .param("message", "released by apex"),
    );
    assert!(matches!(reply, Response::Performed { exit_code: 0, .. }), "{reply:?}");

    let sent = f.fake.seen();
    let upload = sent.first().expect("one request");
    assert_eq!(upload.method, "POST");
    assert!(upload.path.ends_with("/versions"), "{}", upload.path);
    assert!(upload.body.contains("name=\"metadata\""), "{}", upload.body);
    assert!(upload.body.contains(r#""main_module":"worker.js""#), "{}", upload.body);
    assert!(upload.body.contains(r#""compatibility_date":"2026-09-01""#));
    assert!(upload.body.contains(r#""workers/message":"released by apex""#));
    assert!(upload.body.contains("filename=\"worker.js\""));
    assert!(upload.body.contains("export default { fetch:"), "the module itself");
}

#[test]
fn an_upload_will_not_follow_a_link_out_of_the_project() {
    // The provider reads a caller-named path as root. A module that is a
    // symlink to somebody else's file is refused, and nothing is sent.
    let f = Fixture::new("uploadlink", Mode::Normal, &["cloudflare.worker.upload-version"]);
    std::fs::create_dir_all(f.project.join("dist")).expect("dist");
    std::os::unix::fs::symlink("/etc/hostname", f.project.join("dist/worker.js")).expect("link");
    let reply = f.use_it(
        f.record("cloudflare.worker.upload-version", "project")
            .param("script", "dist/worker.js"),
    );
    let (_, message) = reply.as_error().expect("a symlinked module must be refused");
    assert!(message.contains("symbolic link"), "{message}");
    assert!(f.fake.seen().is_empty(), "it was sent anyway");
}

#[test]
fn a_module_that_is_not_a_module_is_refused_before_it_is_read() {
    let f = Fixture::new("notmodule", Mode::Normal, &["cloudflare.worker.upload-version"]);
    std::fs::write(f.project.join("secrets.env"), "TOKEN=x\n").expect("write");
    let reply = f.use_it(
        f.record("cloudflare.worker.upload-version", "project")
            .param("script", "secrets.env"),
    );
    let (_, message) = reply.as_error().expect("refused");
    assert!(message.contains(".js or .mjs"), "{message}");
    assert!(f.fake.seen().is_empty());
}

#[test]
fn a_grant_for_one_worker_verb_does_not_grant_its_siblings() {
    // §13.2's whole argument: semantic operations rather than one broad
    // "Cloudflare write". Deploying is not rolling back and neither is
    // uploading, so a grant for one has to stop at one.
    let f = Fixture::new("siblings", Mode::Normal, &["cloudflare.worker.deploy"]);
    with_files(&f);
    let allowed = f.use_it(
        f.record("cloudflare.worker.deploy", "project")
            .param("version", "1c4dd6be-0000-4000-8000-abcdefabcdef"),
    );
    assert!(matches!(allowed, Response::Performed { exit_code: 0, .. }), "{allowed:?}");

    for refused in [
        "cloudflare.worker.rollback",
        "cloudflare.worker.upload-version",
        "cloudflare.worker.read",
        "cloudflare.worker.tail",
        "cloudflare.account.read",
    ] {
        let reply = f.use_it(
            f.record(refused, "project")
                .param("version", "1c4dd6be-0000-4000-8000-abcdefabcdef")
                .param("script", "dist/worker.js"),
        );
        assert!(
            reply.as_error().is_some(),
            "'{refused}' went through on a grant for deploy"
        );
    }
    // One call: the allowed one.
    assert_eq!(f.fake.seen().len(), 1);
}

#[test]
fn a_rollback_is_told_apart_from_a_deploy_on_the_wire() {
    // Same endpoint, and the grant is what separates them — so the request
    // still has to say which it is, or Cloudflare refuses a return to an older
    // version as a stale deployment.
    let f = Fixture::new("rollback", Mode::Normal, &granted_everything());
    f.use_it(f.record("cloudflare.worker.deploy", "project").param("version", "v-new"));
    f.use_it(f.record("cloudflare.worker.rollback", "project").param("version", "v-old"));
    let sent = f.fake.seen();
    assert!(sent[0].path.ends_with("/deployments"), "{}", sent[0].path);
    assert!(sent[1].path.ends_with("/deployments?force=true"), "{}", sent[1].path);
    assert!(sent[0].body.contains("v-new"));
    assert!(sent[1].body.contains("v-old"));
}

#[test]
fn a_message_with_a_quote_in_it_survives_two_layers_of_escaping() {
    // `message` is Syntax::Text, which admits a quote and a backslash. It is
    // then escaped by serde_json, and the deployment body is escaped AGAIN by
    // the curl config writer, and curl unescapes once. Three transformations
    // and no test on the round trip is how every quoted deploy message ends up
    // as malformed JSON at Cloudflare — which is a functional bug a scrub test
    // and a binding test would both pass straight over.
    let f = Fixture::new("quoting", Mode::Normal, &granted_everything());
    with_files(&f);
    let awkward = r#"say "hi" \ and "then" stop"#;

    // The JSON body path: through `quoted()` into a curl config line.
    f.use_it(
        f.record("cloudflare.worker.deploy", "project")
            .param("version", "1c4dd6be-0000-4000-8000-abcdefabcdef")
            .param("message", awkward),
    );
    // The multipart path: written to a file as raw bytes, no curl escaping.
    f.use_it(
        f.record("cloudflare.worker.upload-version", "project")
            .param("script", "dist/worker.js")
            .param("message", awkward),
    );

    let sent = f.fake.seen();
    let deploy: serde_json::Value = serde_json::from_str(&sent[0].body)
        .unwrap_or_else(|e| panic!("the deployment body is not JSON: {e}\n{}", sent[0].body));
    assert_eq!(
        deploy["annotations"]["workers/message"].as_str(),
        Some(awkward),
        "the message did not survive the round trip"
    );

    // The metadata part is JSON inside a multipart body; pull it back out.
    let body = &sent[1].body;
    let start = body.find('{').expect("metadata json");
    let end = body[start..].find("\r\n--").map(|i| start + i).unwrap_or(body.len());
    let metadata: serde_json::Value = serde_json::from_str(body[start..end].trim())
        .unwrap_or_else(|e| panic!("the metadata part is not JSON: {e}\n{body}"));
    assert_eq!(
        metadata["annotations"]["workers/message"].as_str(),
        Some(awkward)
    );
}

#[test]
fn a_route_read_answers_about_the_worker_that_was_named_and_no_other() {
    // The zone endpoint returns every route in the zone. The caller named one
    // worker, the grant is for one worker, and the audit line claims the
    // answer is about that worker — so the answer has to be.
    let f = Fixture::new("routes", Mode::Normal, &["cloudflare.worker.route.read"]);
    let reply = f.use_it(f.record("cloudflare.worker.route.read", "project"));
    let Response::Performed { output, .. } = &reply else {
        panic!("refused: {reply:?}");
    };
    assert!(output.contains("route1"), "{output}");
    assert!(
        !output.contains("somebody-else") && !output.contains("route2"),
        "the rest of the zone's routing came back: {output}"
    );
}

#[test]
fn the_host_pin_refuses_a_credential_stored_for_anywhere_else() {
    // Cloudflare's endpoint is a constant, so the pin is a flat statement: a
    // token stored for another host cannot be spent on a Cloudflare operation.
    // The provider contains no such check and cannot skip one.
    let f = Fixture::new("pin", Mode::Normal, &["cloudflare.account.read"]);
    let peer = me();
    assert_eq!(
        f.service.add(
            peer,
            NewService {
                service: "elsewhere",
                host: "github.com",
                scheme: "https",
                username: None,
                path: "",
                auth: None,
                port: None,
            },
            SecretValue::new(TOKEN.as_bytes().to_vec()),
        ),
        Response::Ok
    );
    assert!(f
        .service
        .grant(
            peer,
            f.project.to_str().expect("utf8"),
            "elsewhere",
            "cloudflare.account.read",
            false
        )
        .as_error()
        .is_none());

    let mut rec = f.record("cloudflare.account.read", "");
    rec.provider = "elsewhere".into();
    let reply = f.use_it(rec);
    let (kind, message) = reply.as_error().expect("the pin must refuse this");
    assert_eq!(kind, ErrorKind::PermissionDenied);
    assert!(message.contains("github.com"), "{message}");
    assert!(f.fake.seen().is_empty());
}

#[test]
fn a_binding_that_moves_between_the_check_and_the_call_stops_the_call() {
    // `Bound` carries an endpoint and a sentence and nothing a provider
    // defines, so what `bind` resolved cannot be handed to `perform` and the
    // project file is read twice. The owner is the caller and can change it in
    // between; the sentence is rebuilt and compared, so what runs is what was
    // authorised or nothing runs.
    struct Moving {
        inner: CloudflareProvider,
        project: Mutex<Option<PathBuf>>,
    }
    impl Provider for Moving {
        fn spec(&self) -> &'static ProviderSpec {
            self.inner.spec()
        }
        fn bind(&self, req: &Bind<'_>) -> Result<Bound, ProviderError> {
            let bound = self.inner.bind(req);
            // Between the two, exactly as a caller's own process could.
            if let Some(project) = self.project.lock().expect("lock").take() {
                // The account id, not the worker name: a changed name would
                // stop resolving and be refused for that reason instead, and
                // the interesting case is the one where everything still
                // resolves and the request would go somewhere else.
                std::fs::write(
                    project.join("apex.toml"),
                    PROJECT_FILE.replace(ACCOUNT, "99999999999999999999999999999999"),
                )
                .expect("rewrite");
            }
            bound
        }
        fn perform(
            &self,
            req: &Bind<'_>,
            bound: &Bound,
            value: &SecretValue,
        ) -> Result<Performed, ProviderError> {
            self.inner.perform(req, bound, value)
        }
    }

    let fake = Fake::start(Mode::Normal);
    let tag = format!("moving-{}-{}", std::process::id(), fake.port);
    let store = std::env::temp_dir().join(format!("apex-cf-store-{tag}"));
    let project = std::env::temp_dir().join(format!("apex-cf-project-{tag}"));
    std::fs::remove_dir_all(&store).ok();
    std::fs::create_dir_all(&project).expect("project");
    std::fs::write(project.join("apex.toml"), PROJECT_FILE).expect("apex.toml");

    let mut registry = Registry::new();
    registry
        .register(Box::new(Moving {
            inner: CloudflareProvider::at(fake.port),
            project: Mutex::new(Some(project.clone())),
        }))
        .expect("register");
    let service = Service::new(Store::new(store.clone()), false, registry);
    let peer = me();
    service.add(
        peer,
        NewService {
            service: "cloudflare",
            host: "127.0.0.1",
            scheme: "http",
            username: None,
            path: "",
            auth: None,
            port: None,
        },
        SecretValue::new(TOKEN.as_bytes().to_vec()),
    );
    service.grant(
        peer,
        project.to_str().expect("utf8"),
        "cloudflare",
        "cloudflare.worker.read",
        false,
    );

    let mut rec = CapabilityRecord::new("cloudflare", "cloudflare.worker.read", "project");
    rec.project = Some(project.to_string_lossy().into_owned());
    let reply = service.use_capability(peer, rec, Vec::new());
    let (_, message) = reply
        .as_error()
        .expect("a binding that moved must stop the call");
    assert!(message.contains("changed while the request"), "{message}");
    assert!(fake.seen().is_empty(), "it was sent anyway");

    std::fs::remove_dir_all(&store).ok();
    std::fs::remove_dir_all(&project).ok();
}

#[test]
fn the_declared_shape_is_enforced_before_the_provider_is_asked_anything() {
    // All of it from the provider's own declaration, all of it checked by the
    // framework: an operation that takes no resource, a resource that is a
    // URL, an option nobody declared, a required option left out.
    let f = Fixture::new("shape", Mode::Normal, &granted_everything());
    with_files(&f);
    for (operation, resource, options) in [
        ("cloudflare.account.read", "something", vec![]),
        ("cloudflare.worker.read", "https://attacker.example/x", vec![]),
        ("cloudflare.worker.read", "../../etc", vec![]),
        ("cloudflare.worker.read", "-f", vec![]),
        ("cloudflare.worker.deploy", "project", vec![]),
        (
            "cloudflare.worker.read",
            "project",
            vec![("branch", "main")],
        ),
        (
            "cloudflare.worker.upload-version",
            "project",
            vec![("script", "/etc/passwd")],
        ),
        (
            "cloudflare.worker.upload-version",
            "project",
            vec![("script", "dist/worker.js"), ("message", "two\nlines")],
        ),
    ] {
        let mut rec = f.record(operation, resource);
        for (name, value) in &options {
            rec = rec.param(name, value);
        }
        let reply = f.use_it(rec);
        let (kind, message) = reply
            .as_error()
            .unwrap_or_else(|| panic!("{operation} {resource} {options:?} was accepted"));
        assert_eq!(kind, ErrorKind::BadRequest, "{operation} {resource}: {message}");
    }
    assert!(f.fake.seen().is_empty());
}

#[test]
fn an_operation_no_provider_declares_is_refused_even_under_the_cloudflare_prefix() {
    // The vocabulary is closed by the registry, and a name that looks like one
    // of §13.2's twenty-five not-yet-implemented operations is not one this
    // build offers.
    let f = Fixture::new("closed", Mode::Normal, &granted_everything());
    for evil in [
        "cloudflare.dns.delete",
        "cloudflare.r2.object.write",
        "cloudflare.worker.delete",
        "cloudflare.secret.rotate",
        "git.push",
        "exec",
    ] {
        let reply = f.use_it(f.record(evil, "project"));
        assert_eq!(
            reply.as_error().map(|(k, _)| k),
            Some(ErrorKind::BadRequest),
            "'{evil}' was not refused"
        );
    }
    assert!(f.fake.seen().is_empty());
}

#[test]
fn the_trail_records_the_operation_in_the_provider_s_own_words() {
    let f = Fixture::new("trail", Mode::Normal, &["cloudflare.worker.deploy"]);
    f.use_it(
        f.record("cloudflare.worker.deploy", "project")
            .param("version", "1c4dd6be-0000-4000-8000-abcdefabcdef"),
    );
    let lines = audit::tail(&Store::new(f.store.clone()).audit_path(), 10);
    let used = lines
        .iter()
        .find(|l| l.event == AuditEvent::Used)
        .expect("a use was recorded");
    assert_eq!(used.operation, "cloudflare.worker.deploy");
    assert_eq!(used.resource, "project");
    assert_eq!(
        used.detail,
        format!(
            "deploy version 1c4dd6be-0000-4000-8000-abcdefabcdef of project \
             (production) in account example-account [{ACCOUNT}]"
        )
    );
    assert_eq!(used.endpoint.as_deref(), Some("http://127.0.0.1"));
    assert_eq!(used.exit_code, Some(0));
}

#[test]
fn the_declaration_is_well_formed_and_every_name_is_one_section_thirteen_two_lists() {
    // §13.2 is the specification, and a name invented here would be a name no
    // other task in the Cloudflare block could depend on.
    SPEC.validate().expect("the shipped cloudflare vocabulary must validate");
    let listed: Vec<&str> = SECTION_13_2.to_vec();
    for op in SPEC.operations {
        // `worker.route.read` is the one addition, and it is §13.3's "Worker
        // routes" rather than an invention.
        if op.id == "cloudflare.worker.route.read"
            || op.id == "cloudflare.access.service-token.create"
        {
            continue;
        }
        assert!(listed.contains(&op.id), "'{}' is not in §13.2", op.id);
    }
    // Twenty-six of §13.2's thirty-two, and two additions. The arithmetic is
    // asserted because the module note states it and a later task will read
    // that note to work out what is left.
    let from_13_2 = SPEC
        .operations
        .iter()
        .filter(|op| listed.contains(&op.id))
        .count();
    assert_eq!(from_13_2, 32, "every name §13.2 lists is implemented");
    assert_eq!(SPEC.operations.len(), 34);
    assert_eq!(SECTION_13_2.len() - from_13_2, 0, "still unimplemented");

    // **Running a model is a write, and this is the only thing that says so.**
    //
    // Nothing stored, nothing changed — so `Effect::Read` looks defensible, and
    // it was reachable: flipping `cloudflare.workers-ai.run` to `Read` turned
    // no test red before this assertion existed. It has to be `Write` because
    // `Effect` is what a grant is scoped by, and a read-only grant that can
    // spend the account's balance is a read-only grant in name only. Inference
    // costs money and does not give the same answer twice; both are what
    // `Write` means here.
    for op in SPEC.operations {
        if op.id.ends_with(".run") {
            assert!(
                op.effect.is_write(),
                "'{}' costs money, so a Read grant must not reach it",
                op.id
            );
        }
    }

    // Nothing is declared twice, and every summary reads as a sentence about
    // what the owner is being asked to allow.
    let mut ids: Vec<&str> = SPEC.operations.iter().map(|op| op.id).collect();
    ids.sort_unstable();
    let mut unique = ids.clone();
    unique.dedup();
    assert_eq!(ids, unique);
    for op in SPEC.operations {
        assert!(op.summary.len() > 20, "'{}' has a summary nobody can act on", op.id);
    }
}

// ── §13.5, R2 ───────────────────────────────────────────────────────────────
//
// Six mutations were run against the arms below, one at a time, each restored
// by copying the pristine file back so that cargo rebuilt rather than reusing
// the mutant's binary. Every one turns a named test red:
//
// * the bucket dropped out of the object path — `an_r2_object_is_addressed_…`;
// * `binding.bucket()` replaced by the caller's own string, which is the
//   defect §13.5 exists to prevent — `a_bucket_this_project_does_not_bind_…`;
// * the upload sending the path instead of the file's bytes, the media type
//   table answering `text/plain` for `.sql`, and the `Content-Type` header
//   dropped from a raw body — all three, `an_upload_to_r2_carries_…`;
// * the location hint forwarded instead of checked against Cloudflare's own
//   set — `a_bucket_is_created_with_the_name_…`.

#[test]
fn an_r2_object_is_addressed_by_the_bucket_this_project_bound_and_the_key_under_it() {
    // The documented paths, exactly. `example-assets` on its own lists the
    // bucket; `example-assets/db/today.sql` is one object in it; and the
    // account id in front of both came out of apex.toml rather than out of the
    // request.
    let f = Fixture::new("r2paths", Mode::Normal, &granted_everything());
    with_files(&f);
    f.use_it(f.record("cloudflare.r2.object.read", "example-assets"));
    f.use_it(f.record("cloudflare.r2.object.read", "example-assets/db/today.sql"));
    f.use_it(
        f.record("cloudflare.r2.object.write", "example-assets/db/today.sql")
            .param("file", "dist/today.sql"),
    );
    f.use_it(f.record("cloudflare.r2.bucket.create", "example-assets"));

    let seen: Vec<(String, String)> = f
        .fake
        .seen()
        .into_iter()
        .map(|s| (s.method, s.path))
        .collect();
    assert_eq!(
        seen,
        vec![
            ("GET".into(), format!("/client/v4/accounts/{ACCOUNT}/r2/buckets/{BUCKET}/objects")),
            (
                "GET".into(),
                format!("/client/v4/accounts/{ACCOUNT}/r2/buckets/{BUCKET}/objects/db/today.sql")
            ),
            (
                "PUT".into(),
                format!("/client/v4/accounts/{ACCOUNT}/r2/buckets/{BUCKET}/objects/db/today.sql")
            ),
            ("POST".into(), format!("/client/v4/accounts/{ACCOUNT}/r2/buckets")),
        ]
    );
}

#[test]
fn a_bucket_this_project_does_not_bind_never_reaches_cloudflare() {
    // §13.5 asks for bucket-scoped capabilities, and this is what scoped means
    // here: the grant is per operation, and *which* bucket it may touch is the
    // project file's answer, not the caller's. All three R2 operations are
    // checked, because a guard on the two that read a key and not on the one
    // that creates a bucket would let an agent make buckets in the account at
    // will.
    let f = Fixture::new("r2unbound", Mode::Normal, &granted_everything());
    with_files(&f);
    for (operation, resource, options) in [
        ("cloudflare.r2.object.read", "somebody-elses-bucket/db/today.sql", vec![]),
        (
            "cloudflare.r2.object.write",
            "somebody-elses-bucket/db/today.sql",
            vec![("file", "dist/today.sql")],
        ),
        ("cloudflare.r2.bucket.create", "somebody-elses-bucket", vec![]),
    ] {
        let mut rec = f.record(operation, resource);
        for (name, value) in &options {
            rec = rec.param(name, value);
        }
        let reply = f.use_it(rec);
        let (kind, message) = reply
            .as_error()
            .unwrap_or_else(|| panic!("{operation} accepted an unbound bucket"));
        assert_eq!(kind, ErrorKind::BadRequest, "{operation}");
        assert!(message.contains("somebody-elses-bucket"), "{message}");
        assert!(message.contains("example-assets"), "{message}");
        assert!(message.contains("apex.toml"), "{message}");
    }
    assert!(
        f.fake.seen().is_empty(),
        "an unbound bucket still reached the api"
    );
}

#[test]
fn an_upload_to_r2_carries_the_project_s_own_file_and_a_media_type_from_a_table() {
    // The body IS the object, so there is nothing here to escape and nothing
    // to wrap — which makes the two things worth checking the two that could
    // go wrong: that the bytes are the project's file rather than a path, and
    // that the media type came from this build rather than from the caller.
    let f = Fixture::new("r2upload", Mode::Normal, &["cloudflare.r2.object.write"]);
    with_files(&f);
    let reply = f.use_it(
        f.record("cloudflare.r2.object.write", "example-assets/db/today.sql")
            .param("file", "dist/today.sql"),
    );
    assert!(matches!(reply, Response::Performed { exit_code: 0, .. }), "{reply:?}");

    let sent = f.fake.seen();
    let put = sent.first().expect("one request");
    assert_eq!(put.method, "PUT");
    assert_eq!(put.body, UPLOADED, "the object's bytes are the project's file");
    assert_eq!(put.content_type.as_deref(), Some("application/sql"));
    assert_eq!(sent.len(), 1);
}

#[test]
fn an_object_named_for_something_this_build_does_not_know_is_bytes_rather_than_a_guess() {
    let f = Fixture::new("r2type", Mode::Normal, &["cloudflare.r2.object.write"]);
    with_files(&f);
    std::fs::write(f.project.join("dist/blob.whatever"), "x").expect("blob");
    f.use_it(
        f.record("cloudflare.r2.object.write", "example-assets/db/blob.whatever")
            .param("file", "dist/blob.whatever"),
    );
    assert_eq!(
        f.fake.seen()[0].content_type.as_deref(),
        Some("application/octet-stream")
    );
}

#[test]
fn an_upload_to_r2_will_not_follow_a_link_out_of_the_project() {
    // The same root-reading-a-caller's-path problem as a Worker module, on the
    // operation that was written second. A guard that held for one upload and
    // not the other would be no guard at all.
    let f = Fixture::new("r2link", Mode::Normal, &["cloudflare.r2.object.write"]);
    std::fs::create_dir_all(f.project.join("dist")).expect("dist");
    std::os::unix::fs::symlink("/etc/hostname", f.project.join("dist/today.sql")).expect("link");
    let reply = f.use_it(
        f.record("cloudflare.r2.object.write", "example-assets/db/today.sql")
            .param("file", "dist/today.sql"),
    );
    let (_, message) = reply.as_error().expect("a symlinked payload must be refused");
    assert!(message.contains("symbolic link"), "{message}");
    assert!(f.fake.seen().is_empty(), "it was sent anyway");
}

#[test]
fn a_write_that_names_only_a_bucket_is_refused_rather_than_writing_the_bucket() {
    // `example-assets` is a legal resource for this operation's declaration —
    // a `Path` of one segment — and it names no object. Sending it would `PUT`
    // the bucket's own listing URL.
    let f = Fixture::new("r2nokey", Mode::Normal, &["cloudflare.r2.object.write"]);
    with_files(&f);
    let reply = f.use_it(
        f.record("cloudflare.r2.object.write", "example-assets")
            .param("file", "dist/today.sql"),
    );
    let (error, message) = reply.as_error().expect("refused");
    assert!(message.contains("names a bucket and not an object"), "{message}");
    // The KIND, not just that it was refused: a resource that does not resolve
    // is a `BadRequest`, and reporting it as `PermissionDenied` would send
    // somebody who made a typo to look at the grant table.
    assert_eq!(error, ErrorKind::BadRequest, "{message}");
    assert!(f.fake.seen().is_empty());
}

#[test]
fn a_bucket_is_created_with_the_name_the_project_declares_and_options_from_a_closed_set() {
    let f = Fixture::new("r2create", Mode::Normal, &["cloudflare.r2.bucket.create"]);
    let reply = f.use_it(
        f.record("cloudflare.r2.bucket.create", "example-assets")
            .param("location", "apac")
            .param("storage-class", "InfrequentAccess"),
    );
    assert!(matches!(reply, Response::Performed { exit_code: 0, .. }), "{reply:?}");
    let sent = f.fake.seen();
    let create = sent.first().expect("one request");
    assert_eq!(create.method, "POST");
    assert!(create.body.contains(r#""name":"example-assets""#), "{}", create.body);
    assert!(create.body.contains(r#""locationHint":"apac""#), "{}", create.body);
    assert!(create.body.contains(r#""storageClass":"InfrequentAccess""#), "{}", create.body);

    // ...and a value outside the documented set is refused here rather than
    // spent on a request that will fail at the other end.
    for (param, value) in [("location", "mars"), ("storage-class", "Cheap")] {
        let reply = f.use_it(
            f.record("cloudflare.r2.bucket.create", "example-assets").param(param, value),
        );
        let (_, message) = reply.as_error().unwrap_or_else(|| panic!("{param}={value} was sent"));
        assert!(message.contains(value), "{message}");
    }
    assert_eq!(f.fake.seen().len(), 1, "a refused option still reached the api");
}

#[test]
fn an_object_read_answers_with_bytes_and_still_has_the_credential_taken_out() {
    // R2 answers a read with the object itself and no envelope. Every other
    // operation's reply is JSON, so a scrub that happened to key on the
    // envelope would pass everywhere except here — and here is where a
    // project's own data comes back through the broker.
    let f = Fixture::new("r2read", Mode::Normal, &["cloudflare.r2.object.read"]);
    let reply = f.use_it(f.record("cloudflare.r2.object.read", "example-assets/db/today.sql"));
    let Response::Performed { output, .. } = &reply else {
        panic!("refused: {reply:?}");
    };
    // The object arrived...
    assert!(output.contains("CREATE TABLE t (id INTEGER);"), "{output}");
    // ...the double really did echo the credential into it...
    assert!(output.contains("fetched with Bearer"), "{output}");
    // ...and it is not in what came back.
    assert!(!output.contains(TOKEN), "the object read handed back the token");
    assert!(output.contains("«redacted»"), "{output}");
    assert!(!f.trail().contains(TOKEN));
}

// ── §13.3's storage surfaces: D1, KV, Queues, Hyperdrive ────────────────────
//
// Nine mutations were run against the arms below, one at a time, each restored
// by copying the pristine file back so that cargo rebuilt rather than reusing
// the mutant's binary. Every one turns a named test red:
//
// * one id shape for all four tables, so a KV id passes as a D1 one, and the
//   reserved-table guard removed so an environment may shadow `[cloudflare.kv]`
//   — `a_d1_id_and_a_kv_id_are_different_shapes_…`,
//   `an_environment_may_not_be_named_after_a_resource_table`;
// * `is_empty` forgetting the new tables — `a_project_that_binds_only_a_…`;
// * `NoResource` falling out of the `NoSuchResource` arm, which is the same
//   defect `NoRecord` had one unit earlier and which this test found again —
//   `a_database_namespace_queue_or_config_…`;
// * a migration accepting any file, and a queue setting skipping its
//   documented range — `a_migration_is_a_file_of_statements_…`,
//   `a_queue_change_is_checked_against_the_ranges_…`;
// * `caching = off` no longer inverted into Cloudflare's `disabled` —
//   `a_hyperdrive_edit_cannot_carry_a_database_credential`;
// * the origin scrub not descending into nested objects, which is where the
//   password actually is — `an_origin_credential_the_api_volunteers_…`;
// * a KV write given both a value and a file quietly picking one —
//   `a_kv_write_takes_a_value_or_a_file_and_never_both`.

#[test]
fn every_storage_resource_is_addressed_by_the_id_its_project_wrote_down() {
    // Nine operations, nine documented paths, and every id in them came out of
    // apex.toml. None of these four products has a name-based path, so this is
    // the whole of what "project resource binding enforced" can mean for them.
    let f = Fixture::new("p6paths", Mode::Normal, &granted_everything());
    with_files(&f);
    for (operation, resource, options, _) in every_operation() {
        if !["d1", "kv", "queue", "hyperdrive"].contains(&operation.split('.').nth(1).unwrap_or("")) {
            continue;
        }
        let mut rec = f.record(operation, resource);
        for (name, value) in &options {
            rec = rec.param(name, value);
        }
        let reply = f.use_it(rec);
        assert!(
            matches!(reply, Response::Performed { exit_code: 0, .. }),
            "{operation}: {reply:?}"
        );
    }
    let seen: Vec<(String, String)> = f.fake.seen().into_iter().map(|s| (s.method, s.path)).collect();
    assert_eq!(
        seen,
        vec![
            ("GET".into(), format!("/client/v4/accounts/{ACCOUNT}/d1/database/{DB_ID}")),
            ("POST".into(), format!("/client/v4/accounts/{ACCOUNT}/d1/database/{DB_ID}/query")),
            ("POST".into(), format!("/client/v4/accounts/{ACCOUNT}/d1/database/{DB_ID}/query")),
            (
                "GET".into(),
                format!("/client/v4/accounts/{ACCOUNT}/storage/kv/namespaces/{KV_ID}/values/greeting")
            ),
            (
                "PUT".into(),
                format!("/client/v4/accounts/{ACCOUNT}/storage/kv/namespaces/{KV_ID}/values/greeting")
            ),
            ("POST".into(), format!("/client/v4/accounts/{ACCOUNT}/queues/{QUEUE_ID}/messages")),
            ("PATCH".into(), format!("/client/v4/accounts/{ACCOUNT}/queues/{QUEUE_ID}")),
            ("GET".into(), format!("/client/v4/accounts/{ACCOUNT}/hyperdrive/configs/{HD_ID}")),
            ("PATCH".into(), format!("/client/v4/accounts/{ACCOUNT}/hyperdrive/configs/{HD_ID}")),
        ]
    );
}

#[test]
fn a_database_namespace_queue_or_config_this_project_did_not_bind_never_reaches_cloudflare() {
    let f = Fixture::new("p6unbound", Mode::Normal, &granted_everything());
    with_files(&f);
    for (operation, resource, kind, options) in [
        ("cloudflare.d1.read", "somebody-elses-db", "D1 database", vec![]),
        ("cloudflare.kv.read", "somebody-elses-ns/key", "KV namespace", vec![]),
        (
            "cloudflare.queue.publish",
            "somebody-elses-queue",
            "queue",
            vec![("message", "x")],
        ),
        (
            "cloudflare.hyperdrive.read",
            "somebody-elses-config",
            "Hyperdrive config",
            vec![],
        ),
    ] {
        let mut rec = f.record(operation, resource);
        for (name, value) in &options {
            rec = rec.param(name, value);
        }
        let reply = f.use_it(rec);
        let (error, message) = match reply.as_error() {
            Some(pair) => pair,
            None => panic!("{operation} accepted an unbound resource"),
        };
        assert_eq!(error, ErrorKind::BadRequest, "{operation}");
        assert!(message.contains(kind), "{message}");
        assert!(message.contains("apex.toml"), "{message}");
    }
    assert!(f.fake.seen().is_empty(), "an unbound resource reached the api");
}

#[test]
fn a_d1_id_and_a_kv_id_are_different_shapes_and_neither_is_accepted_for_the_other() {
    // Cloudflare's own split: a D1 database id is a UUID, everything else here
    // is 32 hex. One validator would either refuse every real D1 id or let 36
    // characters of anything into a URL path.
    use apex_secret_core::project::ProjectConfig;
    use std::path::Path;
    let binding = |text: &str| {
        let config = ProjectConfig::parse(Path::new("/p/apex.toml"), text).expect("parses");
        super::binding::Binding::of(&config)
    };
    let head = format!("[identity.cloudflare]\naccount_id = \"{ACCOUNT}\"\n");

    // Each in its own table: fine.
    assert!(binding(&format!("{head}[cloudflare.d1]\ndb = \"{DB_ID}\"\n")).is_ok());
    assert!(binding(&format!("{head}[cloudflare.kv]\nns = \"{KV_ID}\"\n")).is_ok());
    // Crossed over: refused, and the message names the shape that key wants.
    let err = binding(&format!("{head}[cloudflare.d1]\ndb = \"{KV_ID}\"\n")).unwrap_err();
    assert!(err.to_string().contains("UUID"), "{err}");
    let err = binding(&format!("{head}[cloudflare.kv]\nns = \"{DB_ID}\"\n")).unwrap_err();
    assert!(err.to_string().contains("32 hexadecimal"), "{err}");
    // ...and a name no request could ever carry is refused where it is written
    // rather than being silently unaddressable.
    let err = binding(&format!("{head}[cloudflare.kv]\n\"my cache\" = \"{KV_ID}\"\n")).unwrap_err();
    assert!(err.to_string().contains("my cache"), "{err}");
}

#[test]
fn an_environment_may_not_be_named_after_a_resource_table() {
    // `[cloudflare.kv]` cannot be both a list of namespaces and a worker
    // binding. Without this the key `worker` would be read as a namespace
    // called "worker" with an id of whatever the worker is called, and the
    // whole file would be refused for a bad id — a true refusal about the
    // wrong thing.
    use apex_secret_core::project::ProjectConfig;
    use std::path::Path;
    let text = format!(
        "[identity.cloudflare]\naccount_id = \"{ACCOUNT}\"\n\
         [cloudflare.kv]\nworker = \"project\"\n"
    );
    let config = ProjectConfig::parse(Path::new("/p/apex.toml"), &text).expect("parses");
    let err = super::binding::Binding::of(&config).unwrap_err();
    let message = err.to_string();
    assert!(message.contains("[cloudflare.kv]"), "{message}");
    assert!(message.contains("Rename the environment"), "{message}");
}

#[test]
fn a_project_that_binds_only_a_namespace_is_told_to_add_a_line_not_to_write_a_file() {
    // `is_empty` decides which of two refusals a person gets, and a new binding
    // that is not counted there turns "add an account id" into "this project
    // binds no Cloudflare account at all".
    use apex_secret_core::project::ProjectConfig;
    use std::path::Path;
    let text = format!("[cloudflare.kv]\ncache = \"{KV_ID}\"\n");
    let config = ProjectConfig::parse(Path::new("/p/apex.toml"), &text).expect("parses");
    let binding = super::binding::Binding::of(&config).expect("binds");
    assert!(!binding.is_empty(), "a project binding a namespace binds something");
    let err = binding.resource("kv", "cache").unwrap_err();
    assert!(err.to_string().contains("account_id"), "{err}");
}

#[test]
fn a_migration_is_a_file_of_statements_and_a_query_is_one_line() {
    // The two D1 write verbs, and why §13.2 has both. A `Syntax::Text` option
    // is one line, so a migration could never have travelled as one.
    let f = Fixture::new("p6d1", Mode::Normal, &granted_everything());
    with_files(&f);
    f.use_it(f.record("cloudflare.d1.query", "project-db").param("sql", "SELECT 1"));
    f.use_it(
        f.record("cloudflare.d1.migrate", "project-db")
            .param("file", "migrations/001.sql"),
    );
    let sent = f.fake.seen();
    assert!(sent[0].body.contains(r#""sql":"SELECT 1""#), "{}", sent[0].body);
    // The whole file, newlines and all, JSON-escaped into the body.
    assert!(sent[1].body.contains("CREATE TABLE t (id INTEGER PRIMARY KEY);"), "{}", sent[1].body);
    assert!(sent[1].body.contains("INSERT INTO t VALUES (1);"), "{}", sent[1].body);
    assert!(sent[1].body.contains(r"\n"), "the newlines were escaped, not dropped");

    // A migration that is not a migration is refused before it is read.
    std::fs::write(f.project.join("migrations/notes.txt"), "DROP TABLE t;\n").expect("notes");
    let reply = f.use_it(
        f.record("cloudflare.d1.migrate", "project-db")
            .param("file", "migrations/notes.txt"),
    );
    let (_, message) = reply.as_error().expect("refused");
    assert!(message.contains(".sql"), "{message}");
    assert_eq!(f.fake.seen().len(), 2, "the .txt was sent anyway");
}

#[test]
fn a_kv_write_takes_a_value_or_a_file_and_never_both() {
    let f = Fixture::new("p6kv", Mode::Normal, &granted_everything());
    with_files(&f);
    f.use_it(f.record("cloudflare.kv.write", "cache/greeting").param("value", "hello"));
    assert_eq!(f.fake.seen()[0].body, "hello");

    f.use_it(
        f.record("cloudflare.kv.write", "cache/greeting").param("file", "dist/today.sql"),
    );
    assert_eq!(f.fake.seen()[1].body, UPLOADED);

    for options in [
        vec![("value", "hello"), ("file", "dist/today.sql")],
        vec![],
    ] {
        let mut rec = f.record("cloudflare.kv.write", "cache/greeting");
        for (name, value) in &options {
            rec = rec.param(name, value);
        }
        assert!(f.use_it(rec).as_error().is_some(), "{options:?} was accepted");
    }
    assert_eq!(f.fake.seen().len(), 2);

    // ...and a namespace with no key names no value.
    let reply = f.use_it(f.record("cloudflare.kv.read", "cache"));
    let (error, message) = reply.as_error().expect("refused");
    assert!(message.contains("names a namespace and not a key"), "{message}");
    assert_eq!(error, ErrorKind::BadRequest, "{message}");
}

#[test]
fn a_kv_read_answers_with_bytes_and_still_has_the_credential_taken_out() {
    let f = Fixture::new("p6kvread", Mode::Normal, &["cloudflare.kv.read"]);
    let reply = f.use_it(f.record("cloudflare.kv.read", "cache/greeting"));
    let Response::Performed { output, .. } = &reply else {
        panic!("refused: {reply:?}");
    };
    assert!(output.contains("stored by apex"), "{output}");
    assert!(output.contains("read back with Bearer"), "the double really echoed it");
    assert!(!output.contains(TOKEN), "the value read handed back the token");
    assert!(output.contains("«redacted»"), "{output}");
}

#[test]
fn a_queue_change_is_checked_against_the_ranges_cloudflare_documents() {
    let f = Fixture::new("p6queue", Mode::Normal, &granted_everything());
    f.use_it(
        f.record("cloudflare.queue.manage", "jobs")
            .param("paused", "true")
            .param("delivery-delay", "60")
            .param("retention", "345600"),
    );
    let body = &f.fake.seen()[0].body;
    assert!(body.contains(r#""delivery_paused":true"#), "{body}");
    assert!(body.contains(r#""delivery_delay":60"#), "{body}");
    assert!(body.contains(r#""message_retention_period":345600"#), "{body}");

    for (param, value) in [
        ("delivery-delay", "86401"),
        ("retention", "59"),
        ("retention", "1209601"),
        ("paused", "yes"),
        ("delivery-delay", "soon"),
    ] {
        let reply = f.use_it(f.record("cloudflare.queue.manage", "jobs").param(param, value));
        assert!(reply.as_error().is_some(), "{param}={value} was sent");
    }
    // ...and a change with nothing in it does not spend a request.
    assert!(f.use_it(f.record("cloudflare.queue.manage", "jobs")).as_error().is_some());
    assert_eq!(f.fake.seen().len(), 1);
}

#[test]
fn a_published_message_does_not_get_to_choose_its_own_content_type() {
    let f = Fixture::new("p6publish", Mode::Normal, &["cloudflare.queue.publish"]);
    f.use_it(f.record("cloudflare.queue.publish", "jobs").param("message", "work to do"));
    let body = &f.fake.seen()[0].body;
    assert!(body.contains(r#""body":"work to do""#), "{body}");
    assert!(body.contains(r#""content_type":"text""#), "{body}");
    // A content type is a header-shaped string, and this operation declares no
    // option for one — so the framework refuses it before the provider is
    // asked anything.
    let reply = f.use_it(
        f.record("cloudflare.queue.publish", "jobs")
            .param("message", "x")
            .param("content_type", "application/json"),
    );
    assert!(reply.as_error().is_some(), "a caller chose a content type");
    assert_eq!(f.fake.seen().len(), 1);
}

#[test]
fn a_hyperdrive_edit_cannot_carry_a_database_credential() {
    // The inverse of everything else here: an agent handing a secret TO the
    // broker. `edit` declares a name and three caching settings, so the
    // framework refuses the rest — and this is the assertion that would go red
    // if somebody later added an `origin` option.
    let f = Fixture::new("p6hdedit", Mode::Normal, &["cloudflare.hyperdrive.edit"]);
    for (param, value) in [
        ("password", "hunter2"),
        ("user", "postgres"),
        ("host", "db.attacker.test"),
        ("origin", "postgres://user:pw@db.attacker.test/app"),
        ("access_client_secret", "x"),
    ] {
        let reply = f.use_it(f.record("cloudflare.hyperdrive.edit", "pg").param(param, value));
        let (_, message) = reply
            .as_error()
            .unwrap_or_else(|| panic!("hyperdrive.edit accepted '{param}'"));
        assert!(message.contains("no '"), "{message}");
    }
    assert!(f.fake.seen().is_empty(), "a credential reached the api");

    // What it can do.
    f.use_it(
        f.record("cloudflare.hyperdrive.edit", "pg")
            .param("caching", "off")
            .param("max-age", "60"),
    );
    let body = &f.fake.seen()[0].body;
    assert_eq!(f.fake.seen()[0].method, "PATCH");
    // `caching = off` is Cloudflare's `disabled = true`, inverted here so the
    // owner grants something spelled the way they think about it.
    assert!(body.contains(r#""disabled":true"#), "{body}");
    assert!(body.contains(r#""max_age":60"#), "{body}");
    assert!(!body.contains("origin"), "{body}");
}

#[test]
fn an_origin_credential_the_api_volunteers_does_not_reach_the_caller() {
    // Cloudflare documents `origin.password` as write-only and says the API
    // never returns it. The double returns one anyway, because a guarantee that
    // an agent gets capabilities and not credentials cannot rest on a remark in
    // somebody else's documentation staying true.
    let f = Fixture::new("p6hdread", Mode::Normal, &["cloudflare.hyperdrive.read"]);
    let reply = f.use_it(f.record("cloudflare.hyperdrive.read", "pg"));
    let Response::Performed { output, .. } = &reply else {
        panic!("refused: {reply:?}");
    };
    // The configuration came back...
    assert!(output.contains("db.example.invalid"), "{output}");
    assert!(output.contains(r#""user":"app""#), "{output}");
    // ...and the password did not.
    assert!(!output.contains(ORIGIN_PASSWORD), "the origin password came back: {output}");
    assert!(!output.contains("password"), "{output}");
    assert!(output.contains("origin credential"), "{output}");
    assert!(!f.trail().contains(ORIGIN_PASSWORD), "the trail holds it");
    // The double really did send one, so this is a removal and not an absence.
    assert_eq!(f.fake.seen().len(), 1);
}

#[test]
fn a_grant_for_one_storage_verb_does_not_grant_its_siblings() {
    // "Semantic read/write/migrate/manage operations represented" — and
    // separately grantable, which is the point of representing them.
    let f = Fixture::new("p6verbs", Mode::Normal, &["cloudflare.d1.read"]);
    with_files(&f);
    let allowed = f.use_it(f.record("cloudflare.d1.read", "project-db"));
    assert!(matches!(allowed, Response::Performed { exit_code: 0, .. }), "{allowed:?}");
    for (refused, options) in [
        ("cloudflare.d1.query", vec![("sql", "SELECT 1")]),
        ("cloudflare.d1.migrate", vec![("file", "migrations/001.sql")]),
    ] {
        let mut rec = f.record(refused, "project-db");
        for (name, value) in &options {
            rec = rec.param(name, value);
        }
        assert!(
            f.use_it(rec).as_error().is_some(),
            "'{refused}' went through on a grant for read"
        );
    }
    assert_eq!(f.fake.seen().len(), 1);
}

// ── §13.9, DNS ──────────────────────────────────────────────────────────────
//
// Eight mutations were run against the arms below, one at a time, each restored
// by copying the pristine file back so that cargo rebuilt rather than reusing
// the mutant's binary. Every one turns a named test red:
//
// * `inside()` suffix-matching without the label boundary, so `notexample.com`
//   is inside `example.com` — `a_name_outside_the_bound_zone_…`;
// * the §13.9 type guard skipped on writes — `changing_a_delegation_…`;
// * `Lookup::Denied` folded into `Lookup::Absent`, which is the defect this
//   whole module is shaped around — `a_lookup_that_was_refused_…`;
// * an ambiguous name answered with the first record instead of a refusal —
//   `a_name_that_more_than_one_record_answers_to_…`;
// * the name/type re-check dropped, so the far side's filter is trusted —
//   `a_zone_that_answered_about_other_records_…`;
// * `PUT` in place of `PATCH` on an update — `an_update_sends_only_the_fields_…`;
// * the project's own `records` narrowing ignored — `a_project_that_narrows_…`;
// * `is_record_id` accepting anything non-empty — `a_record_id_off_the_wire_…`.

#[test]
fn a_dns_record_is_addressed_by_name_in_the_zone_this_project_bound() {
    // The documented paths, and the shape of the two-request verbs. An update
    // asks the zone which record `www.example.com` means and only then changes
    // it, which is why there are six requests here for four operations.
    let f = Fixture::new("dnspaths", Mode::Normal, &granted_everything());
    f.use_it(f.record("cloudflare.dns.read", "www.example.com").param("type", "A"));
    f.use_it(
        f.record("cloudflare.dns.create", "new.example.com")
            .param("type", "A")
            .param("content", "203.0.113.7"),
    );
    f.use_it(
        f.record("cloudflare.dns.update", "www.example.com")
            .param("type", "A")
            .param("content", "203.0.113.8"),
    );
    f.use_it(f.record("cloudflare.dns.delete", "old.example.com").param("type", "A"));

    let seen: Vec<(String, String)> = f.fake.seen().into_iter().map(|s| (s.method, s.path)).collect();
    let records = format!("/client/v4/zones/{ZONE}/dns_records");
    assert_eq!(
        seen,
        vec![
            ("GET".into(), format!("{records}?name.exact=www.example.com&type=A")),
            ("POST".into(), records.clone()),
            ("GET".into(), format!("{records}?name.exact=www.example.com&type=A")),
            ("PATCH".into(), format!("{records}/{RECORD_A}")),
            ("GET".into(), format!("{records}?name.exact=old.example.com&type=A")),
            ("DELETE".into(), format!("{records}/{RECORD_A}")),
        ]
    );
}

#[test]
fn a_name_outside_the_bound_zone_never_reaches_cloudflare() {
    // §13.9's first sentence. The label boundary is the case worth writing
    // down: `notexample.com` ends in `example.com` and is a different
    // registration, and a suffix test without the dot would hand it over.
    let f = Fixture::new("dnszone", Mode::Normal, &granted_everything());
    for outside in [
        "notexample.com",
        "example.com.attacker.test",
        "attacker.test",
        "com",
    ] {
        let reply = f.use_it(
            f.record("cloudflare.dns.update", outside)
                .param("type", "A")
                .param("content", "203.0.113.9"),
        );
        let (kind, message) = reply
            .as_error()
            .unwrap_or_else(|| panic!("'{outside}' was accepted as a name in this zone"));
        assert_eq!(kind, ErrorKind::BadRequest, "{outside}");
        assert!(message.contains("example.com"), "{message}");
    }
    // ...and the apex itself IS in its own zone, which is the other half: a
    // build that refused everything would pass the loop above.
    let apex = f.use_it(f.record("cloudflare.dns.read", "example.com").param("type", "A"));
    assert!(matches!(apex, Response::Performed { exit_code: 0, .. }), "{apex:?}");
    assert_eq!(f.fake.seen().len(), 1, "only the apex read should have been sent");
}

#[test]
fn a_project_that_narrows_itself_to_some_records_cannot_touch_the_others() {
    // The second half of "bound zones/records": a project may cut itself down
    // to a list, and then the zone is not enough.
    let f = Fixture::new("dnsnarrow", Mode::Normal, &granted_everything());
    std::fs::write(
        f.project.join("apex.toml"),
        format!(
            "[identity.cloudflare]\naccount_id = \"{ACCOUNT}\"\n\
             [cloudflare]\nzone = \"example.com\"\nzone_id = \"{ZONE}\"\n\
             records = [\"www\", \"api.example.com\"]\n"
        ),
    )
    .expect("write");

    // Both spellings of a bound record resolve — `www` and the full name.
    for allowed in ["www.example.com", "api.example.com"] {
        let reply = f.use_it(f.record("cloudflare.dns.read", allowed).param("type", "A"));
        assert!(
            matches!(reply, Response::Performed { exit_code: 0, .. }),
            "{allowed} is bound and was refused: {reply:?}"
        );
    }
    // ...and a name in the same zone that is not on the list is not.
    let reply = f.use_it(
        f.record("cloudflare.dns.delete", "mail.example.com").param("type", "A"),
    );
    let (kind, message) = reply.as_error().expect("a narrowed project must not reach it");
    assert_eq!(kind, ErrorKind::BadRequest);
    assert!(message.contains("mail.example.com"), "{message}");
    assert!(message.contains("www and api.example.com"), "{message}");
    assert_eq!(f.fake.seen().len(), 2, "the refused name still reached the api");
}

#[test]
fn changing_a_delegation_or_a_dnssec_record_is_refused_as_an_elevated_shape() {
    // §13.9's second sentence, as far as a build with one capability class can
    // take it. The owner has granted all four DNS verbs; every reserved type
    // is still refused, and nothing reaches the api — which is the property
    // that matters whether or not an elevated class ever exists.
    let f = Fixture::new("dnselevated", Mode::Normal, &granted_everything());
    for kind in ["NS", "DS", "DNSKEY", "SOA", "ns", "ds"] {
        for (operation, options) in [
            ("cloudflare.dns.create", vec![("content", "ns1.attacker.test")]),
            ("cloudflare.dns.update", vec![("content", "ns1.attacker.test")]),
            ("cloudflare.dns.delete", vec![]),
        ] {
            let mut rec = f.record(operation, "example.com").param("type", kind);
            for (name, value) in &options {
                rec = rec.param(name, value);
            }
            let reply = f.use_it(rec);
            let (error, message) = reply
                .as_error()
                .unwrap_or_else(|| panic!("{operation} accepted a {kind} record"));
            assert_eq!(error, ErrorKind::PermissionDenied, "{operation} {kind}");
            assert!(message.contains("elevated"), "{message}");
            assert!(message.contains("§13.9"), "{message}");
        }
    }
    assert!(
        f.fake.seen().is_empty(),
        "a reserved record type reached the api: {:?}",
        f.fake.seen()
    );

    // Reading one is allowed: §13.9 reserves *changes*, and an agent that
    // cannot see where a zone is delegated cannot check its own work.
    let read = f.use_it(f.record("cloudflare.dns.read", "example.com").param("type", "NS"));
    assert!(matches!(read, Response::Performed { exit_code: 0, .. }), "{read:?}");
    assert_eq!(f.fake.seen().len(), 1);
}

#[test]
fn a_lookup_that_was_refused_is_not_reported_as_a_record_that_is_not_there() {
    // The defect shape this codebase has found about fifteen times, on the one
    // path here that could reintroduce it. The zone answers 403 — a body that
    // looks like every other failure — and the answer must not be "no such
    // record", because a caller told that would go and create a second one.
    let f = Fixture::new("dnsdenied", Mode::Normal, &granted_everything());
    let reply = f.use_it(
        f.record("cloudflare.dns.update", "denied.example.com")
            .param("type", "A")
            .param("content", "203.0.113.9"),
    );
    let (kind, message) = reply.as_error().expect("a refused lookup is not a success");
    assert_eq!(
        kind,
        ErrorKind::PermissionDenied,
        "a denial must not be reported as a bad request about a missing record"
    );
    assert!(message.contains("403"), "{message}");
    assert!(message.contains("not the same as the record not being there"), "{message}");
    assert!(!message.contains("holds no"), "{message}");
    // One request — the lookup. Nothing was changed.
    let seen = f.fake.seen();
    assert_eq!(seen.len(), 1, "{seen:?}");
    assert_eq!(seen[0].method, "GET");
}

#[test]
fn a_record_that_is_absent_says_the_zone_answered_and_holds_none() {
    // The other side of the same distinction, and the reason it is worth
    // having: this one really is an absence, and it says so in those words.
    let f = Fixture::new("dnsabsent", Mode::Normal, &granted_everything());
    let reply = f.use_it(f.record("cloudflare.dns.delete", "gone.example.com").param("type", "A"));
    let (kind, message) = reply.as_error().expect("deleting nothing is not a success");
    assert_eq!(kind, ErrorKind::BadRequest);
    assert!(message.contains("answered"), "{message}");
    assert!(message.contains("holds no A record"), "{message}");
    assert_eq!(f.fake.seen().len(), 1, "it deleted something anyway");
}

#[test]
fn a_name_that_more_than_one_record_answers_to_is_refused_rather_than_guessed() {
    // Two A records at one name is round-robin, not a fault. Changing the
    // first would change an arbitrary one of them.
    let f = Fixture::new("dnsmany", Mode::Normal, &granted_everything());
    let reply = f.use_it(
        f.record("cloudflare.dns.update", "many.example.com")
            .param("type", "A")
            .param("content", "203.0.113.9"),
    );
    let (kind, message) = reply.as_error().expect("an ambiguous name must be refused");
    assert_eq!(kind, ErrorKind::PermissionDenied);
    assert!(message.contains("2 A records"), "{message}");
    assert_eq!(f.fake.seen().len(), 1, "one of them was changed");
}

#[test]
fn a_zone_that_answered_about_other_records_is_not_taken_at_its_word() {
    // A filter that was sent is not a filter that was applied. If the query
    // parameter were ever spelled wrongly, or stopped being honoured, the
    // lookup would come back holding the whole zone — and the first record in
    // it would be modified. Checking the names here turns that into a refusal.
    let f = Fixture::new("dnsunfiltered", Mode::Normal, &granted_everything());
    let reply = f.use_it(
        f.record("cloudflare.dns.update", "unfiltered.example.com")
            .param("type", "A")
            .param("content", "203.0.113.9"),
    );
    let (kind, message) = reply.as_error().expect("records for other names are not this one");
    assert_eq!(kind, ErrorKind::BadRequest);
    assert!(message.contains("holds no A record"), "{message}");
    assert_eq!(f.fake.seen().len(), 1, "somebody else's record was changed");
}

#[test]
fn an_update_that_changes_nothing_is_refused_before_anything_is_looked_up() {
    let f = Fixture::new("dnsnoop", Mode::Normal, &granted_everything());
    let reply = f.use_it(f.record("cloudflare.dns.update", "www.example.com").param("type", "A"));
    let (_, message) = reply.as_error().expect("an update with no fields is not an update");
    assert!(message.contains("changes nothing"), "{message}");
    assert!(f.fake.seen().is_empty(), "it spent a request finding out");
}

#[test]
fn an_update_sends_only_the_fields_it_was_given() {
    // PATCH and not PUT. Cloudflare's PUT overwrites a record with what the
    // request carries, so an update that set only the content through PUT
    // would quietly reset the TTL and the proxy flag.
    let f = Fixture::new("dnspatch", Mode::Normal, &["cloudflare.dns.update"]);
    f.use_it(
        f.record("cloudflare.dns.update", "www.example.com")
            .param("type", "A")
            .param("content", "203.0.113.8"),
    );
    let sent = f.fake.seen();
    let patch = sent.last().expect("two requests");
    assert_eq!(patch.method, "PATCH");
    assert!(patch.body.contains(r#""content":"203.0.113.8""#), "{}", patch.body);
    assert!(!patch.body.contains("ttl"), "{}", patch.body);
    assert!(!patch.body.contains("proxied"), "{}", patch.body);
    assert!(!patch.body.contains("name"), "{}", patch.body);
}

#[test]
fn a_create_carries_the_whole_record_and_values_cloudflare_would_accept() {
    let f = Fixture::new("dnscreate", Mode::Normal, &["cloudflare.dns.create"]);
    f.use_it(
        f.record("cloudflare.dns.create", "new.example.com")
            .param("type", "a")
            .param("content", "203.0.113.7")
            .param("ttl", "300")
            .param("proxied", "true")
            .param("comment", "added by apex"),
    );
    let sent = f.fake.seen();
    let create = sent.first().expect("one request");
    // The type is upper-cased, because that is what the zone stores.
    assert!(create.body.contains(r#""type":"A""#), "{}", create.body);
    assert!(create.body.contains(r#""name":"new.example.com""#), "{}", create.body);
    assert!(create.body.contains(r#""content":"203.0.113.7""#), "{}", create.body);
    assert!(create.body.contains(r#""ttl":300"#), "{}", create.body);
    assert!(create.body.contains(r#""proxied":true"#), "{}", create.body);
    assert!(create.body.contains(r#""comment":"added by apex""#), "{}", create.body);

    // Values the far side would refuse are refused here, where refusing costs
    // nothing and the message can name the option.
    for (param, value) in [
        ("ttl", "30"),
        ("ttl", "999999"),
        ("ttl", "soon"),
        ("proxied", "yes"),
        ("type", "NOTATYPE"),
    ] {
        let reply = f.use_it(
            f.record("cloudflare.dns.create", "new.example.com")
                .param("type", "A")
                .param("content", "203.0.113.7")
                .param(param, value),
        );
        assert!(
            reply.as_error().is_some(),
            "{param}={value} was sent to cloudflare"
        );
    }
    assert_eq!(f.fake.seen().len(), 1, "a refused value still reached the api");
}

#[test]
fn the_four_dns_verbs_are_four_grants() {
    // §13.2's argument where it matters most: an agent that may point a
    // hostname at a new worker should not thereby be able to delete the zone's
    // records.
    let f = Fixture::new("dnsverbs", Mode::Normal, &["cloudflare.dns.update"]);
    let allowed = f.use_it(
        f.record("cloudflare.dns.update", "www.example.com")
            .param("type", "A")
            .param("content", "203.0.113.8"),
    );
    assert!(matches!(allowed, Response::Performed { exit_code: 0, .. }), "{allowed:?}");
    for refused in [
        "cloudflare.dns.read",
        "cloudflare.dns.create",
        "cloudflare.dns.delete",
    ] {
        let reply = f.use_it(
            f.record(refused, "www.example.com")
                .param("type", "A")
                .param("content", "203.0.113.8"),
        );
        assert!(reply.as_error().is_some(), "'{refused}' went through on a grant for update");
    }
    // The lookup and the patch, and nothing else.
    assert_eq!(f.fake.seen().len(), 2);
}

#[test]
fn the_trail_names_the_record_and_the_zone_that_was_changed() {
    let f = Fixture::new("dnstrail", Mode::Normal, &["cloudflare.dns.update"]);
    f.use_it(
        f.record("cloudflare.dns.update", "www.example.com")
            .param("type", "A")
            .param("content", "203.0.113.8"),
    );
    let lines = audit::tail(&Store::new(f.store.clone()).audit_path(), 10);
    let used = lines.iter().find(|l| l.event == AuditEvent::Used).expect("a use");
    assert_eq!(used.operation, "cloudflare.dns.update");
    assert_eq!(used.resource, "www.example.com");
    assert_eq!(
        used.detail,
        format!("change the A record at www.example.com in zone example.com [{ZONE}]")
    );
}

// ── §13.11, Workers AI and the AI Gateway ───────────────────────────────────
//
// Nine mutations were run against the arms below, one at a time, each restored
// by copying the pristine file back — and then touching it, which the earlier
// rounds of this program did not have to do and this one did. `cp -p` restores
// the ORIGINAL mtime, which is older than the artifact cargo just built from
// the mutant, so cargo skips the rebuild and the next run measures the MUTANT'S
// BINARY against pristine source. That was caught here by a restored tree
// failing a test it had just passed; every verdict below was re-taken after the
// restore started bumping the mtime.
//
// Every one turns a named test red:
//
// * the header guard in `api::call` stops consulting `printable` —
//   `a_header_carrying_a_second_line_is_refused_before_curl_is_configured`;
// * `cf-aig-gateway-id` is not sent — `a_gatewayed_run_is_the_same_host_…`;
// * `binding.resource("models", …)` replaced by the caller's own string, which
//   is the defect §13.11 exists to prevent — `a_model_or_a_gateway_this_…`;
// * the project's whole PATH sent as the metadata's project —
//   `a_gatewayed_run_is_tagged_with_this_task_…`;
// * `carries_authorization` never fires — `a_gateway_carrying_an_exporter_…`;
// * the `stream` check dropped — `a_streamed_response_is_refused_…`;
// * `GATEWAY_FIELDS` put back nothing — `a_gateway_edit_puts_back_everything_…`;
// * `workers-ai.run` declared `Effect::Read` — `the_declaration_is_well_formed_…`;
// * `valid_model` stops checking segments — `a_model_name_is_checked_as_…`.
//
// **Three of those nine did not bite the first time**, and the tests were the
// thing that changed, not the code:
//
// * the project-path leak passed, because the filter that makes a name
//   header-safe strips `/` as a side effect, so a leaked path arrives with no
//   slash in it and neither "contains no slash" nor "is not the path" notices;
// * the read-modify-write passed with the read discarded entirely, because the
//   assertion asked about `collect_logs` — which the required-field fallback
//   sets to `true` regardless, so preserved and defaulted looked the same;
// * `Effect::Read` on a paid operation passed, because nothing asserted it.
//
// All three are the shape this unit was warned about: a check that is real in
// the code and invisible to the test, which is indistinguishable from no check
// at all the day somebody edits the code.

#[test]
fn a_gatewayed_run_is_the_same_host_and_the_same_credential_as_an_ungatewayed_one() {
    // The finding this design rests on. The provider-specific endpoint at
    // `gateway.ai.cloudflare.com` would be a second host, and the framework
    // pins a credential to the host it was stored for — so it would need a
    // second stored credential. Cloudflare's REST API page documents the
    // gatewayed form of the ORDINARY call instead: same URL, plus a
    // `cf-aig-gateway-id` header. This asserts that shape, because if it ever
    // stopped being the shape, the pin would be the thing that noticed.
    let f = Fixture::new("aigateway", Mode::Normal, &granted_everything());
    let plain = f.use_it(f.record("cloudflare.workers-ai.run", "fast").param("prompt", "hi"));
    assert!(matches!(plain, Response::Performed { exit_code: 0, .. }), "{plain:?}");
    let gatewayed = f.use_it(
        f.record("cloudflare.ai-gateway.run", "main/fast").param("prompt", "hi"),
    );
    assert!(matches!(gatewayed, Response::Performed { exit_code: 0, .. }), "{gatewayed:?}");

    let sent = f.fake.seen();
    let runs: Vec<&Seen> = sent.iter().filter(|s| s.path.contains("/ai/run/")).collect();
    assert_eq!(runs.len(), 2, "two runs, two requests");
    assert_eq!(runs[0].path, runs[1].path, "the gateway changed the URL");
    // The ungatewayed one carries no gateway header, and the gatewayed one
    // carries the id from the PROJECT'S FILE.
    assert!(!runs[0].headers.contains_key("cf-aig-gateway-id"), "{:?}", runs[0].headers);
    assert_eq!(
        runs[1].headers.get("cf-aig-gateway-id").map(String::as_str),
        Some(GATEWAY_ID),
        "{:?}",
        runs[1].headers
    );
}

#[test]
fn a_gatewayed_run_is_tagged_with_this_task_and_not_with_this_machine_s_paths() {
    // P1-009's third criterion: usage attributable to a task and a project.
    // The correlation key is the audit id, which this machine's own trail maps
    // to the project, the session and the origin — so the far side's log needs
    // nothing else, and the owner's directory layout stays here.
    let f = Fixture::new("aimeta", Mode::Normal, &granted_everything());
    let reply = f.use_it(
        f.record("cloudflare.ai-gateway.run", "main/fast").param("prompt", "hi"),
    );
    assert!(matches!(reply, Response::Performed { exit_code: 0, .. }), "{reply:?}");

    let sent = f.fake.seen();
    let run = sent.iter().find(|s| s.path.contains("/ai/run/")).expect("nothing was sent");
    let raw = run.headers.get("cf-aig-metadata").expect("no metadata header");
    let meta: serde_json::Value = serde_json::from_str(raw).expect("metadata must be json");
    let object = meta.as_object().expect("an object");

    // AI Gateway keeps the first five entries and strips anything beginning
    // `cf.`, which is its own namespace.
    assert!(object.len() <= 5, "more than five entries: {raw}");
    assert!(object.keys().all(|k| !k.starts_with("cf.")), "{raw}");
    assert!(object.values().all(|v| v.is_string() || v.is_number() || v.is_boolean()), "{raw}");

    assert_eq!(
        object.get("apex_operation").and_then(|v| v.as_str()),
        Some("cloudflare.ai-gateway.run")
    );
    // The audit id is a real one from this request, and it is what joins the
    // far side's log to this machine's trail.
    let audit = object.get("apex_audit").and_then(|v| v.as_str()).expect("no audit id");
    assert!(!audit.is_empty() && audit != "test", "{raw}");
    assert!(f.trail().contains(audit), "the id sent is not one this trail holds: {audit}");

    // The project's NAME is there and its PATH is not.
    //
    // **The equality is the assertion, and the two weaker checks below it are
    // not enough on their own.** A mutation that sent `req.project` whole was
    // run against this test and it stayed green: the character filter that
    // makes a name header-safe also strips `/`, so the full path arrives as
    // `tmpapex-cf-project-…` — no slash in it, and not equal to the path
    // either, so both of the obvious checks pass while every directory on the
    // way to the project is in somebody else's logs. Only pinning the value to
    // the directory's OWN name catches that.
    let project = f.project.to_string_lossy().into_owned();
    let expected: String = f
        .project
        .file_name()
        .and_then(|n| n.to_str())
        .expect("the fixture's project has a name")
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
        .take(48)
        .collect();
    assert_eq!(
        object.get("apex_project").and_then(|v| v.as_str()),
        Some(expected.as_str()),
        "{raw}"
    );
    assert!(!raw.contains(&project), "the project's path was sent to cloudflare: {raw}");
    // The same path with its separators taken out — the exact shape the leak
    // takes once the filter has been through it.
    let squashed: String = project.chars().filter(|c| *c != '/').collect();
    assert!(
        !raw.contains(&squashed),
        "the project's path was sent with its separators stripped: {raw}"
    );
    assert!(
        object.get("apex_project").and_then(|v| v.as_str()).is_some_and(|p| !p.contains('/')),
        "{raw}"
    );
}

#[test]
fn a_model_or_a_gateway_this_project_did_not_bind_never_reaches_cloudflare() {
    // A model cannot even be NAMED as a resource — `@cf/...` is not a
    // `Syntax::Name` — so binding is not a policy choice here, it is the only
    // way to address one. The gateway is bound for a different reason worth
    // stating: Cloudflare creates a gateway on first use if the id is unknown,
    // so a caller that could name one could bill one.
    let f = Fixture::new("unboundai", Mode::Normal, &granted_everything());
    for (operation, resource) in [
        ("cloudflare.workers-ai.run", "somebody-elses-model"),
        ("cloudflare.ai-gateway.run", "somebody-elses-gateway/fast"),
        ("cloudflare.ai-gateway.run", "main/somebody-elses-model"),
        ("cloudflare.ai-gateway.edit", "somebody-elses-gateway"),
    ] {
        let mut rec = f.record(operation, resource);
        if operation.ends_with(".run") {
            rec = rec.param("prompt", "hi");
        } else {
            rec = rec.param("cache-ttl", "300");
        }
        let reply = f.use_it(rec);
        let (error, message) = match reply.as_error() {
            Some(pair) => pair,
            None => panic!("{operation} accepted '{resource}'"),
        };
        assert_eq!(error, ErrorKind::BadRequest, "{operation}: {message}");
        assert!(message.contains("apex.toml"), "{message}");
    }
    assert!(f.fake.seen().is_empty(), "an unbound model or gateway reached the api");

    // ...and the model name itself is refused by the vocabulary, which is the
    // thing that makes the binding unavoidable rather than merely advisable.
    assert!(!operation::valid_name(MODEL), "'{MODEL}' would be nameable as a resource");
}

#[test]
fn a_streamed_response_is_refused_rather_than_delivered_as_framing() {
    // The transport is a `curl` that hands back a finished reply. A streamed
    // answer would arrive as server-sent-event framing in `output`: not a
    // stream, not the JSON the caller asked for, and indistinguishable from a
    // working feature until somebody tries to parse it.
    let f = Fixture::new("aistream", Mode::Normal, &granted_everything());
    std::fs::create_dir_all(f.project.join("ai")).expect("ai");
    std::fs::write(
        f.project.join("ai/streamed.json"),
        r#"{"messages":[{"role":"user","content":"hi"}],"stream":true}"#,
    )
    .expect("write");
    let reply = f.use_it(
        f.record("cloudflare.workers-ai.run", "fast").param("file", "ai/streamed.json"),
    );
    let (_, message) = reply.as_error().expect("a streamed request must be refused");
    assert!(message.contains("streamed response"), "{message}");
    assert!(f.fake.seen().is_empty(), "it was sent anyway");

    // The same document without `stream` goes through, so this is a check on
    // one field rather than a refusal of every file.
    std::fs::write(
        f.project.join("ai/ok.json"),
        r#"{"messages":[{"role":"user","content":"hi"}]}"#,
    )
    .expect("write");
    let ok = f.use_it(f.record("cloudflare.workers-ai.run", "fast").param("file", "ai/ok.json"));
    assert!(matches!(ok, Response::Performed { exit_code: 0, .. }), "{ok:?}");
}

#[test]
fn a_gateway_edit_puts_back_everything_it_did_not_mean_to_change() {
    // `PUT` REPLACES a gateway, and its schema marks five fields required — so
    // a request carrying only the changed field is not a smaller edit, it is a
    // rejected one, and everything the gateway had would go back to a default.
    // Hence the read first, and hence this assertion: the field that was asked
    // for changed, and a field nobody mentioned survived.
    let f = Fixture::new("aiedit", Mode::Normal, &granted_everything());
    let reply = f.use_it(f.record("cloudflare.ai-gateway.edit", "main").param("cache-ttl", "300"));
    assert!(matches!(reply, Response::Performed { exit_code: 0, .. }), "{reply:?}");

    let sent = f.fake.seen();
    assert_eq!(sent.len(), 2, "a replace has to read first: {sent:?}");
    assert_eq!(sent[0].method, "GET");
    let put = &sent[1];
    assert_eq!(put.method, "PUT");
    let body: serde_json::Value = serde_json::from_str(&put.body).expect("json body");
    assert_eq!(body.get("cache_ttl").and_then(|v| v.as_u64()), Some(300), "{}", put.body);

    // Untouched, and still there — the whole reason for the read.
    //
    // **Every value checked here is one the fallbacks could not have supplied.**
    // A mutation that put back nothing at all still passed an earlier version
    // of this assertion, because it asked about `collect_logs` — which the
    // required-field fallback sets to `true` regardless, making a preserved
    // value and a defaulted one indistinguishable. These four are not
    // defaulted: `rate_limiting_limit` falls back to 0 and the gateway says 42,
    // `cache_invalidate_on_update` falls back to false and the gateway says
    // true, and `logpush` and `retry_max_attempts` have no fallback at all. If
    // any of them arrives wrong, the read did not happen or was discarded.
    assert_eq!(
        body.get("rate_limiting_limit").and_then(|v| v.as_u64()),
        Some(42),
        "a fallback overwrote what the gateway already had: {}",
        put.body
    );
    assert_eq!(
        body.get("cache_invalidate_on_update").and_then(|v| v.as_bool()),
        Some(true),
        "{}",
        put.body
    );
    assert_eq!(body.get("logpush").and_then(|v| v.as_bool()), Some(true), "{}", put.body);
    assert_eq!(
        body.get("retry_max_attempts").and_then(|v| v.as_u64()),
        Some(3),
        "a setting this build does not know about was dropped: {}",
        put.body
    );
    assert_eq!(body.get("collect_logs").and_then(|v| v.as_bool()), Some(true), "{}", put.body);
    // The five the schema marks required are all present, whatever was asked.
    for required in [
        "rate_limiting_interval",
        "rate_limiting_limit",
        "collect_logs",
        "cache_ttl",
        "cache_invalidate_on_update",
    ] {
        assert!(body.get(required).is_some(), "{required} is missing: {}", put.body);
    }
    // Read-only fields the API returns are NOT written back.
    for readonly in ["created_at", "modified_at", "id", "is_default"] {
        assert!(body.get(readonly).is_none(), "{readonly} was written back: {}", put.body);
    }
}

#[test]
fn a_gateway_carrying_an_exporter_credential_is_not_rewritten_at_all() {
    // The case a replace cannot do honestly. A gateway's OTel exporter carries
    // an `authorization`, nested inside an ARRAY; putting it back means writing
    // whatever the API chose to show, and leaving it out means deleting it.
    // Refusing is the third option and the only one that cannot silently break
    // somebody's telemetry.
    let f = Fixture::new("aiotel", Mode::Normal, &granted_everything());
    std::fs::write(
        f.project.join("apex.toml"),
        PROJECT_FILE.replace(r#"main = "apex-gateway""#, r#"main = "with-otel""#),
    )
    .expect("rewrite");
    let reply = f.use_it(f.record("cloudflare.ai-gateway.edit", "main").param("cache-ttl", "300"));
    let (error, message) = reply.as_error().expect("a gateway with a credential must be refused");
    assert_eq!(error, ErrorKind::PermissionDenied, "{message}");
    assert!(message.contains("exporter credential"), "{message}");
    assert!(message.contains("Nothing was changed"), "{message}");

    // It read, and it did not write — and the credential is not in the reply.
    let sent = f.fake.seen();
    assert!(sent.iter().all(|s| s.method != "PUT"), "the gateway was rewritten anyway");
    assert!(!message.contains(OTEL_SECRET), "the exporter credential came back: {message}");
    assert!(!f.trail().contains(OTEL_SECRET), "the exporter credential is in the trail");
}

#[test]
fn a_header_this_build_composes_can_never_carry_a_second_line() {
    // `quoted` escapes a backslash and a quote and does nothing about a
    // newline, and a newline in a header value is a second curl configuration
    // line — which is a second header, or an option. Every composed value is
    // held to printable ASCII before it gets there.
    assert!(api::printable("apex-gateway"));
    assert!(api::printable(r#"{"apex_audit":"1a0-1","apex_project":"demo"}"#));
    for evil in [
        "one\ntwo",
        "one\r\nheader: two",
        "one\u{0}two",
        "",
        "caf\u{e9}",
        "one\ttwo",
    ] {
        assert!(!api::printable(evil), "'{}' was accepted", evil.escape_debug());
    }
}

// ── §13.6, the Secrets Store ────────────────────────────────────────────────
//
// Seven mutations were run against the arms below, one at a time, each restored
// by copying the pristine file back so that cargo rebuilt rather than reusing
// the mutant's binary. Every one turns a named test red:
//
// * the lookup trusting `search` instead of checking the exact name, which is
//   the ordinary mistake here because `search` is a SUBSTRING match and a store
//   holding `API_KEY` and `API_KEY_OLD` answers for both —
//   `rotating_a_secret_matches_the_exact_name_and_not_a_prefix_of_it`;
// * a refused credential reported as a missing secret —
//   `a_secret_that_is_not_there_is_told_apart_from_a_credential_…`;
// * "you named no secret" reported as "you may not" —
//   `a_store_this_project_did_not_bind_…`. That one is the THIRD instance of
//   this defect in the Cloudflare block, and the two it joins (R2's
//   bucket-with-no-key and KV's namespace-with-no-key) were found while writing
//   it and fixed in the same commit;
// * a value given twice silently picked, and scopes forwarded unchecked —
//   `a_value_given_twice_or_not_at_all_…`, `a_scope_cloudflare_does_not_…`;
// * the binding replacing the worker's bindings instead of merging into them,
//   which would take every other binding off the worker —
//   `a_binding_is_a_reference_and_the_worker_s_other_bindings_survive_it`;
// * the shell's trailing newline sent as part of the secret —
//   `a_secret_value_travels_as_input_and_never_as_an_option`.

#[test]
fn a_secret_value_travels_as_input_and_never_as_an_option() {
    // **The claim §13.6's block note makes, measured.** A parameter is rendered
    // into `CapabilityRecord::summary` and becomes the `detail` of every
    // refused audit line — so this test proves BOTH halves: the value the
    // caller piped in reached Cloudflare and is nowhere in the trail, while a
    // parameter of the same operation is in the trail in full. The second half
    // is what makes the first a decision rather than a coincidence.
    let f = Fixture::new("secretbody", Mode::Normal, &granted_everything());
    // Sent the way a shell sends one — `echo` puts a newline on the end, and a
    // secret that works from `printf %s` and not from `echo` is an afternoon
    // somebody does not get back. One trailing newline is taken off.
    let reply = f.use_with_input(
        f.record("cloudflare.secret.create", "app/API_KEY").param("scopes", "workers"),
        &format!("{SECRET_VALUE}\n"),
    );
    let Response::Performed { output, exit_code, .. } = &reply else {
        panic!("{reply:?}");
    };
    assert_eq!(*exit_code, 0, "{output}");

    // It arrived, as the value of the one secret being created.
    let sent = f.fake.seen();
    let create = sent
        .iter()
        .find(|s| s.method == "POST" && s.path.contains("/secrets_store/"))
        .expect("nothing was sent");
    assert!(
        create.body.contains(&format!(r#""value":"{SECRET_VALUE}""#)),
        "the value did not arrive, or arrived with the shell's newline on it: {}",
        create.body
    );
    assert!(create.body.contains(r#""name":"API_KEY""#), "{}", create.body);
    assert!(create.body.contains(r#""scopes":["workers"]"#), "{}", create.body);

    // It is not in the trail...
    assert!(!f.trail().contains(SECRET_VALUE), "the secret is in the audit trail");

    // ...and here is what would have happened if it HAD been an option.
    // `AuditLine::from_record` writes `CapabilityRecord::summary` as the detail
    // of every REFUSED line, and `summary` renders each parameter as
    // `name=value`. So one refused request — a store this project does not bind
    // — puts its options in the trail verbatim, on the path where the operation
    // never even happened. That is the argument for the body, measured rather
    // than asserted.
    let refused = f.use_with_input(
        f.record("cloudflare.secret.create", "somebody-elses-store/API_KEY")
            .param("scopes", "workers"),
        SECRET_VALUE,
    );
    assert!(refused.as_error().is_some(), "{refused:?}");
    let trail = f.trail();
    assert!(
        trail.contains("scopes=workers"),
        "a parameter did not reach the trail, so the reason the value is not one \
         no longer holds: {trail}"
    );
    assert!(!trail.contains(SECRET_VALUE), "the secret reached the trail on a refusal");

    // §13.6: "APEX stores only the provider reference/metadata, not fictitious
    // plaintext". Nothing was written to this service's own store.
    assert!(
        Store::new(f.store.clone()).list(me().uid).iter().all(|s| s.service == "cloudflare"),
        "a secret pushed to Cloudflare was also stored here"
    );
}

#[test]
fn no_secret_operation_declares_a_value_option() {
    // The property above, as a property of the DECLARATION rather than of one
    // code path: the framework refuses an option no operation declares, so a
    // vocabulary with no `value` cannot be handed one however the caller asks.
    for op in SPEC.operations.iter().filter(|op| op.id.starts_with("cloudflare.secret.")) {
        assert!(
            !op.params.iter().any(|p| p.name == "value" || p.name == "secret"),
            "'{}' declares an option that would put a secret in the audit trail",
            op.id
        );
    }
    let f = Fixture::new("novalue", Mode::Normal, &granted_everything());
    let reply = f.use_it(
        f.record("cloudflare.secret.create", "app/API_KEY")
            .param("scopes", "workers")
            .param("value", SECRET_VALUE),
    );
    let (error, message) = reply.as_error().expect("an undeclared option must be refused");
    assert_eq!(error, ErrorKind::BadRequest, "{message}");
    assert!(f.fake.seen().is_empty(), "it was sent anyway");
}

#[test]
fn rotating_a_secret_matches_the_exact_name_and_not_a_prefix_of_it() {
    // The endpoint's filter is `search`, and `search` is a SUBSTRING match: a
    // store holding `API_KEY` and `API_KEY_OLD` answers a search for `API_KEY`
    // with both. A build that took the first result would rotate whichever came
    // back first — silently, and the caller would be told it worked.
    let f = Fixture::new("rotate", Mode::Normal, &granted_everything());
    let reply = f.use_with_input(f.record("cloudflare.secret.rotate", "app/API_KEY"), SECRET_VALUE);
    let Response::Performed { output, exit_code, .. } = &reply else {
        panic!("{reply:?}");
    };
    assert_eq!(*exit_code, 0, "{output}");

    let sent = f.fake.seen();
    let patch = sent.iter().find(|s| s.method == "PATCH").expect("nothing was changed");
    assert!(patch.path.ends_with(SECRET_ID), "the wrong secret was rotated: {}", patch.path);
    assert!(!patch.path.contains(OLD_SECRET_ID), "{}", patch.path);
    assert!(patch.body.contains(SECRET_VALUE), "{}", patch.body);
    // A rotate does not rename or re-scope: those are the fields a replace
    // would reset, and this endpoint is a PATCH precisely so it need not.
    assert!(!patch.body.contains(r#""name""#), "{}", patch.body);
    assert!(!f.trail().contains(SECRET_VALUE), "the new value is in the audit trail");
}

#[test]
fn a_secret_that_is_not_there_is_told_apart_from_a_credential_that_was_refused() {
    // The five answers, again, because the alternative is the defect this
    // codebase has now found about fifteen times: a refusal reported as an
    // absence. A caller told "no such secret" creates a second one beside the
    // first.
    let f = Fixture::new("fivewaysecret", Mode::Normal, &granted_everything());
    let reply = f.use_with_input(f.record("cloudflare.secret.rotate", "app/NOT_THERE"), SECRET_VALUE);
    let (error, message) = reply.as_error().expect("a missing secret must be refused");
    assert_eq!(error, ErrorKind::BadRequest, "{message}");
    assert!(message.contains("holds no secret"), "{message}");
    assert!(message.contains("Nothing was changed"), "{message}");
    // Nothing was PATCHed — the lookup ran and stopped there.
    assert!(
        f.fake.seen().iter().all(|s| s.method != "PATCH"),
        "something was changed after a failed lookup"
    );

    // ...and a credential the far side refuses is NOT reported as an absence.
    let denied = Fixture::new("deniedsecret", Mode::EchoUnauthorized, &granted_everything());
    let reply = denied.use_with_input(
        denied.record("cloudflare.secret.rotate", "app/API_KEY"),
        SECRET_VALUE,
    );
    let (error, message) = reply.as_error().expect("a refused credential is not a success");
    assert_ne!(
        error,
        ErrorKind::BadRequest,
        "a refused credential was reported as a missing secret: {message}"
    );
    assert!(message.contains("not the same as the secret not being there"), "{message}");
}

#[test]
fn a_binding_is_a_reference_and_the_worker_s_other_bindings_survive_it() {
    // §13.6: "references/metadata only". What goes onto the worker is a store
    // id and a secret NAME; nothing here ever reads the value, and there is no
    // path by which it could.
    //
    // The settings PATCH replaces the `bindings` list wholesale, so this is a
    // read-modify-write — and the pre-existing binding in the double is what
    // proves the merge happened rather than a replacement that looked fine.
    let f = Fixture::new("bind", Mode::Normal, &granted_everything());
    let reply = f.use_it(
        f.record("cloudflare.secret.bind", "app/API_KEY")
            .param("worker", "project")
            .param("binding", "API_KEY"),
    );
    let Response::Performed { output, exit_code, .. } = &reply else {
        panic!("{reply:?}");
    };
    assert_eq!(*exit_code, 0, "{output}");

    let sent = f.fake.seen();
    let patch = sent
        .iter()
        .find(|s| s.method == "PATCH" && s.path.ends_with("/settings"))
        .expect("the worker's settings were not changed");
    assert!(patch.body.contains(r#""type":"secrets_store_secret""#), "{}", patch.body);
    assert!(patch.body.contains(&format!(r#""store_id":"{STORE_ID}""#)), "{}", patch.body);
    assert!(patch.body.contains(r#""secret_name":"API_KEY""#), "{}", patch.body);
    // The reference carries no value, and there is nowhere for one to come
    // from: Cloudflare never returns it.
    assert!(!patch.body.contains("\"text\":\"" ) || patch.body.contains("GREETING"), "{}", patch.body);
    assert!(!patch.body.contains(SECRET_VALUE), "{}", patch.body);
    // ...and the bindings that were already there are still there, as
    // `inherit` — which is what keeps `OLD_SECRET`'s value. Sending that one
    // back the way it arrived, with no `text`, is how a build silently empties
    // a secret it was never asked to touch.
    assert!(patch.body.contains("GREETING"), "an existing binding was dropped: {}", patch.body);
    assert!(patch.body.contains("OLD_SECRET"), "an existing secret binding was dropped: {}", patch.body);
    assert!(
        patch.body.contains(r#"{"name":"OLD_SECRET","type":"inherit"}"#)
            || patch.body.contains(r#"{"type":"inherit","name":"OLD_SECRET"}"#),
        "an existing secret binding was sent back without its value instead of inherited: {}",
        patch.body
    );
    assert!(
        !patch.body.contains(r#""type":"secret_text""#),
        "a write-only binding was echoed back: {}",
        patch.body
    );
}

#[test]
fn a_store_this_project_did_not_bind_never_reaches_cloudflare() {
    // The §13.1 boundary for §13.6: a project reaches the secrets in the stores
    // its own file lists. The ERROR KIND is asserted — "you did not bind that"
    // must not arrive as "you may not".
    let f = Fixture::new("unboundstore", Mode::Normal, &granted_everything());
    for (resource, why) in [
        ("somebody-elses-store/API_KEY", "a store this project does not bind"),
        ("app", "a store with no secret named in it"),
    ] {
        let reply = f.use_with_input(
            f.record("cloudflare.secret.create", resource).param("scopes", "workers"),
            SECRET_VALUE,
        );
        let (error, message) = match reply.as_error() {
            Some(pair) => pair,
            None => panic!("{why} was accepted"),
        };
        assert_eq!(error, ErrorKind::BadRequest, "{why}: {message}");
    }
    assert!(f.fake.seen().is_empty(), "an unbound store reached the api");
}

#[test]
fn a_scope_cloudflare_does_not_document_is_refused_before_the_credential_is_spent() {
    let f = Fixture::new("scopes", Mode::Normal, &granted_everything());
    for bad in ["everything", "workers,everything", "Workers", ""] {
        let reply = f.use_with_input(
            f.record("cloudflare.secret.create", "app/API_KEY").param("scopes", bad),
            SECRET_VALUE,
        );
        assert!(reply.as_error().is_some(), "'{bad}' was accepted as a scope");
    }
    assert!(f.fake.seen().is_empty(), "a bad scope reached the api");

    // ...and every scope the schema lists is accepted, so the check above is
    // not simply refusing everything.
    let ok = f.use_with_input(
        f.record("cloudflare.secret.create", "app/API_KEY")
            .param("scopes", "workers,ai_gateway,access"),
        SECRET_VALUE,
    );
    assert!(matches!(ok, Response::Performed { exit_code: 0, .. }), "{ok:?}");
}

#[test]
fn a_value_given_twice_or_not_at_all_is_refused_rather_than_guessed() {
    let f = Fixture::new("bothorneither", Mode::Normal, &granted_everything());
    with_files(&f);
    // Neither.
    let neither = f.use_it(
        f.record("cloudflare.secret.create", "app/API_KEY").param("scopes", "workers"),
    );
    let (_, message) = neither.as_error().expect("a secret with no value is not one");
    assert!(message.contains("needs the secret's value"), "{message}");
    // Both.
    let both = f.use_with_input(
        f.record("cloudflare.secret.create", "app/API_KEY")
            .param("scopes", "workers")
            .param("file", "secrets/value.txt"),
        SECRET_VALUE,
    );
    let (_, message) = both.as_error().expect("two values is not one value");
    assert!(message.contains("will not pick"), "{message}");
    assert!(f.fake.seen().is_empty(), "one of them was sent anyway");
}

// ── §13.10, Cloudflare One ──────────────────────────────────────────────────
//
// Seven mutations were run against the arms below, one at a time, each restored
// by copying the pristine file back so that cargo rebuilt rather than reusing
// the mutant's binary. Every one turns a named test red:
//
// * the provider no longer stripping `client_secret` from its own reply —
//   `the_provider_takes_the_token_secret_out_before_the_framework_ever_sees_it`.
//   Worth reading the note in that test: with BOTH layers present this mutation
//   changed nothing any test going through `use_capability` could see, because
//   the framework's scrub caught it. The test was rewritten to call `perform`
//   directly, and only then did the mutation bite. Two layers need two tests;
//   the framework's is `service.rs`'s `SEAM-M1`;
// * the created credential pinned to `api.cloudflare.com` — the API that
//   ISSUED it rather than the host it may be sent to — and the host taken from
//   the caller instead of resolved through the project's binding:
//   `an_access_service_token_is_kept_here_…`,
//   `a_host_this_project_did_not_bind_…`;
// * `access.read` no longer scrubbing a SaaS `client_secret`, and the scrub no
//   longer descending into arrays —
//   `an_access_application_that_carries_a_client_secret_…`,
//   `a_credential_nested_in_an_array_is_still_taken_out`. The second of those
//   also needed its own test: every secret the double nests today is inside an
//   OBJECT, so array descent was untested until something asserted it;
// * the duration forwarded to Cloudflare unchecked —
//   `a_service_token_duration_cloudflare_would_refuse_…`;
// * an Access application id validated as 32 hex alone, which refuses the UUID
//   half of Cloudflare's own `oneOf` —
//   `an_access_service_token_is_kept_here_…`.

#[test]
fn an_access_service_token_is_kept_here_and_the_caller_gets_a_handle() {
    // §13.10, end to end: "Broker service-token/Tunnel/Access credentials
    // directly into protected storage. The agent receives handles/capabilities,
    // not plaintext secrets."
    //
    // Three things are measured, and the third is the one a build gets wrong:
    // the secret did not come back, the secret IS in the store byte for byte,
    // and it is pinned to the HOST the project named rather than to the API
    // that issued it. A credential stored against `api.cloudflare.com` would
    // look stored and be a pin that says the wrong thing.
    let f = Fixture::new("token", Mode::Normal, &granted_everything());
    let reply = f.use_it(
        f.record("cloudflare.access.service-token.create", "admin.example.com")
            .param("name", "ci"),
    );
    let Response::Performed { output, exit_code, .. } = &reply else {
        panic!("{reply:?}");
    };
    assert_eq!(*exit_code, 0, "{output}");
    assert!(!output.contains(CLIENT_SECRET), "the secret came back: {output}");
    // The client id is not a secret and is useless without its other half, so
    // it does come back — an agent that could not name the token it just made
    // would have got a capability it cannot use.
    assert!(output.contains(CLIENT_ID), "{output}");
    assert!(output.contains("cf-access.admin.example.com"), "{output}");

    let store = Store::new(f.store.clone());
    let uid = me().uid;
    let info = store
        .info(uid, "cf-access.admin.example.com")
        .expect("the secret was not kept");
    assert_eq!(info.host, "admin.example.com", "pinned to the wrong host");
    assert_eq!(info.scheme, "https");
    assert_eq!(info.username, CLIENT_ID);
    assert_eq!(
        store.value(uid, "cf-access.admin.example.com").expect("value").as_str(),
        Some(CLIENT_SECRET),
        "a different secret was stored"
    );

    // Not in the trail either, which is the file an administrator greps.
    assert!(!f.trail().contains(CLIENT_SECRET), "the secret is in the audit trail");
}

#[test]
fn a_second_service_token_for_the_same_host_is_refused_before_one_is_issued() {
    // Cloudflare shows a `client_secret` once. So a second token for a host
    // whose secret is already stored has to be refused BEFORE the call, not
    // after: a refusal afterwards leaves a token that exists, is in nobody's
    // hands, and cannot be fetched again. The double's `seen()` is what proves
    // the order — nothing was asked.
    let f = Fixture::new("token2", Mode::Normal, &granted_everything());
    let first = f.use_it(
        f.record("cloudflare.access.service-token.create", "admin.example.com")
            .param("name", "ci"),
    );
    assert!(matches!(first, Response::Performed { exit_code: 0, .. }), "{first:?}");
    let calls = f.fake.seen().len();

    let second = f.use_it(
        f.record("cloudflare.access.service-token.create", "admin.example.com")
            .param("name", "ci-again"),
    );
    let (error, message) = second.as_error().expect("a second token must be refused");
    assert_eq!(error, ErrorKind::BadRequest, "{message}");
    assert!(message.contains("cf-access.admin.example.com"), "{message}");
    assert_eq!(f.fake.seen().len(), calls, "a second token was issued anyway");
}

#[test]
fn a_host_this_project_did_not_bind_cannot_have_a_credential_pinned_to_it() {
    // The pin is only worth having if the project had to be allowed to name the
    // host, so this resolves through the same label-boundary check §13.9 uses:
    // `notexample.com` is not inside `example.com`, however much it ends in the
    // same letters. The ERROR KIND is asserted and not merely that something
    // was refused — "you did not bind that" reported as "you may not" is the
    // defect this Cloudflare block has now produced twice.
    let f = Fixture::new("tokenzone", Mode::Normal, &granted_everything());
    for host in ["notexample.com", "admin.example.com.attacker.test", "elsewhere.test"] {
        let reply = f.use_it(
            f.record("cloudflare.access.service-token.create", host).param("name", "ci"),
        );
        let (error, message) = match reply.as_error() {
            Some(pair) => pair,
            None => panic!("'{host}' was accepted as a host in this project's zone"),
        };
        assert_eq!(error, ErrorKind::BadRequest, "{host}: {message}");
        assert!(message.contains("example.com"), "{message}");
    }
    assert!(f.fake.seen().is_empty(), "an unbound host reached the api");
}

#[test]
fn a_service_token_duration_cloudflare_would_refuse_is_refused_before_the_call() {
    // For every other operation, sending a value the far side rejects wastes a
    // request. For this one it can do worse: a request that fails *after*
    // creating the token leaves a secret nobody will ever see. So the duration
    // is checked here, against the syntax the schema documents.
    let f = Fixture::new("duration", Mode::Normal, &granted_everything());
    for bad in ["8760", "h", "2h45", "forever-and-ever", "-5h", "8760H", ""] {
        let reply = f.use_it(
            f.record("cloudflare.access.service-token.create", "admin.example.com")
                .param("name", "ci")
                .param("duration", bad),
        );
        assert!(
            reply.as_error().is_some(),
            "'{}' was accepted as a duration",
            bad.escape_debug()
        );
    }
    assert!(f.fake.seen().is_empty(), "a bad duration reached the api");

    // ...and the shapes the schema documents are accepted, so the check above
    // is not simply refusing everything.
    for good in ["forever", "8760h", "2h45m", "300ms", "30s"] {
        assert!(valid_duration(good), "'{good}' is a duration Cloudflare accepts");
    }
}

#[test]
fn an_access_application_that_carries_a_client_secret_does_not_hand_it_back() {
    // An Access application is not obviously a place a secret lives, and for a
    // self-hosted one it is not. An OIDC SaaS application is: Cloudflare's own
    // schema gives it a `client_secret`, and this is a READ an agent may hold.
    // The double sends one, so this measures a removal rather than an absence.
    let f = Fixture::new("appsecret", Mode::Normal, &["cloudflare.access.read"]);
    let reply = f.use_it(f.record("cloudflare.access.read", "dashboard"));
    let Response::Performed { output, .. } = &reply else {
        panic!("{reply:?}");
    };
    assert!(!output.contains(APP_SECRET), "the saas client secret came back: {output}");
    assert!(output.contains("has been removed"), "{output}");
    // The rest of the application still arrives — a scrub that returned
    // nothing would pass the assertion above and be useless.
    assert!(output.contains("admin.example.com"), "{output}");
    assert!(output.contains(APP_ID), "{output}");
}

#[test]
fn a_tunnel_is_read_without_ever_asking_for_its_connector_token() {
    // The token endpoint is not declared, so no grant can reach it. The double
    // answers that path anyway — a test that passed because there was nothing
    // to fetch would prove nothing — and the assertion is that this build never
    // asks.
    let f = Fixture::new("tunnel", Mode::Normal, &granted_everything());
    let reply = f.use_it(f.record("cloudflare.tunnel.read", "office"));
    let Response::Performed { output, .. } = &reply else {
        panic!("{reply:?}");
    };
    assert!(output.contains("healthy"), "{output}");
    assert!(!output.contains(TUNNEL_TOKEN), "the connector token came back: {output}");
    assert!(
        f.fake.seen().iter().all(|s| !s.path.ends_with("/token")),
        "this build asked for the connector token"
    );
    // And there is no operation that could: the vocabulary is closed by what is
    // declared, so this is a property of the build and not of one code path.
    assert!(
        !SPEC.operations.iter().any(|op| op.id.contains("tunnel.token")),
        "a tunnel token operation was declared"
    );
}

#[test]
fn the_provider_takes_the_token_secret_out_before_the_framework_ever_sees_it() {
    // **The two layers, told apart.** The framework scrubs a created credential
    // out of anything on its way to the caller — proved in `service.rs` against
    // a provider that deliberately leaks one — and this module takes it out as
    // well. With both in place, removing either one changes nothing a test
    // going through `use_capability` can see, so this one goes around the
    // framework and calls `perform` directly. What it measures is the
    // provider's own half: the value leaves here in `created` and nowhere else.
    let f = Fixture::new("twolayer", Mode::Normal, &["cloudflare.access.service-token.create"]);
    let provider = CloudflareProvider::at(f.fake.port);
    let owner = crate::broker::owner(me().uid).expect("own uid");
    let service = apex_secret_core::store::ServiceInfo {
        service: "cloudflare".into(),
        host: "127.0.0.1".into(),
        scheme: "http".into(),
        username: "x-access-token".into(),
        path: String::new(),
        auth: "bearer".into(),
        port: Some(f.fake.port),
        added: 0,
    };
    let operation = SPEC
        .operations
        .iter()
        .find(|op| op.id == "cloudflare.access.service-token.create")
        .expect("declared");
    let mut params = operation::Params::new();
    params.insert("name".into(), "ci".into());
    let project = f.project.to_string_lossy().into_owned();
    let req = Bind {
        operation,
        resource: "admin.example.com",
        params: &params,
        body: &[],
        project: &project,
        service: &service,
        owner: &owner,
        audit_id: "test-audit-id",
    };
    let bound = provider.bind(&req).expect("binds");
    assert_eq!(
        bound.creates.as_deref(),
        Some("cf-access.admin.example.com"),
        "the name has to be declared BEFORE the call, or a collision cannot be refused in time"
    );
    let performed = provider
        .perform(&req, &bound, &SecretValue::new(TOKEN.as_bytes().to_vec()))
        .expect("performs");
    assert!(
        !performed.output.contains(CLIENT_SECRET),
        "the provider handed the secret to the framework in its output: {}",
        performed.output
    );
    let created = performed.created.expect("the secret must leave here as a credential");
    assert_eq!(created.value.as_str(), Some(CLIENT_SECRET));
    assert_eq!(created.host, "admin.example.com");
    assert_eq!(created.username.as_deref(), Some(CLIENT_ID));
}

#[test]
fn a_credential_nested_in_an_array_is_still_taken_out() {
    // The scrub descends into objects AND arrays. Both matter and neither is
    // hypothetical: a Hyperdrive password is at `result.origin.password`, an
    // Access SaaS secret at `result.saas_app.client_secret`, and an AI
    // Gateway's OTel exporter carries an `authorization` inside a LIST. A
    // top-level scrub, or one that walked objects only, would report success
    // having removed nothing — which is the failure that looks exactly like
    // success.
    let body = r#"{"result":{"top":"kept","inner":{"password":"p1"},"list":[{"password":"p2"},{"keep":"yes"}]}}"#;
    let out = without(body, &["password"], "\nremoved");
    assert!(!out.contains("p1"), "an object-nested secret survived: {out}");
    assert!(!out.contains("p2"), "an array-nested secret survived: {out}");
    assert!(out.contains("kept") && out.contains("yes"), "it removed too much: {out}");
    assert!(out.ends_with("removed"), "it removed something and did not say so: {out}");

    // A reply with nothing to remove is returned unchanged and says nothing.
    let clean = r#"{"result":{"top":"kept"}}"#;
    assert!(!without(clean, &["password"], "\nremoved").contains("removed"));
    // ...and something that is not the envelope at all comes back as it is,
    // rather than becoming the string "null".
    assert_eq!(without("not json", &["password"], "\nremoved"), "not json");
}

#[test]
fn the_cloudflare_one_verbs_are_separate_grants() {
    // §13.2's whole argument, on the surface where it matters most: reading an
    // Access application, revoking its sessions and issuing a credential are
    // three decisions, and an owner gets to make them one at a time.
    let f = Fixture::new("onegrants", Mode::Normal, &["cloudflare.access.read"]);
    let allowed = f.use_it(f.record("cloudflare.access.read", "dashboard"));
    assert!(matches!(allowed, Response::Performed { exit_code: 0, .. }), "{allowed:?}");

    for (operation, resource, option) in [
        ("cloudflare.access.edit", "dashboard", None),
        (
            "cloudflare.access.service-token.create",
            "admin.example.com",
            Some(("name", "ci")),
        ),
        ("cloudflare.tunnel.read", "office", None),
        ("cloudflare.tunnel.edit", "office", Some(("name", "renamed"))),
    ] {
        let mut rec = f.record(operation, resource);
        if let Some((name, value)) = option {
            rec = rec.param(name, value);
        }
        let reply = f.use_it(rec);
        let (error, message) = reply
            .as_error()
            .unwrap_or_else(|| panic!("'{operation}' ran under a grant for access.read"));
        assert_eq!(error, ErrorKind::PermissionDenied, "{operation}: {message}");
    }
}

/// §13.2's list, so the test above compares against the roadmap rather than
/// against itself.
const SECTION_13_2: &[&str] = &[
    "cloudflare.account.read",
    "cloudflare.worker.read",
    "cloudflare.worker.upload-version",
    "cloudflare.worker.deploy",
    "cloudflare.worker.rollback",
    "cloudflare.worker.tail",
    "cloudflare.dns.read",
    "cloudflare.dns.create",
    "cloudflare.dns.update",
    "cloudflare.dns.delete",
    "cloudflare.r2.object.read",
    "cloudflare.r2.object.write",
    "cloudflare.r2.bucket.create",
    "cloudflare.d1.read",
    "cloudflare.d1.query",
    "cloudflare.d1.migrate",
    "cloudflare.kv.read",
    "cloudflare.kv.write",
    "cloudflare.queue.publish",
    "cloudflare.queue.manage",
    "cloudflare.hyperdrive.read",
    "cloudflare.hyperdrive.edit",
    "cloudflare.secret.create",
    "cloudflare.secret.bind",
    "cloudflare.secret.rotate",
    "cloudflare.access.read",
    "cloudflare.access.edit",
    "cloudflare.tunnel.read",
    "cloudflare.tunnel.edit",
    "cloudflare.workers-ai.run",
    "cloudflare.ai-gateway.run",
    "cloudflare.ai-gateway.edit",
];

/// A worker bound to two environments must resolve the same way twice.
#[test]
fn a_worker_bound_twice_resolves_the_same_way_every_time() {
    let f = Fixture::new("stable", Mode::Normal, &["cloudflare.worker.read"]);
    std::fs::write(
        f.project.join("apex.toml"),
        format!(
            "[identity.cloudflare]\naccount_id = \"{ACCOUNT}\"\n\
             [cloudflare.alpha]\nworker = \"twin\"\n\
             [cloudflare.beta]\nworker = \"twin\"\n"
        ),
    )
    .expect("write");
    let mut details = std::collections::BTreeSet::new();
    for _ in 0..5 {
        let lines = {
            f.use_it(f.record("cloudflare.worker.read", "twin"));
            audit::tail(&Store::new(f.store.clone()).audit_path(), 50)
        };
        let used = lines.iter().rev().find(|l| l.event == AuditEvent::Used).expect("used");
        details.insert(used.detail.clone());
    }
    assert_eq!(details.len(), 1, "resolution is not stable: {details:?}");
    assert!(details.iter().next().unwrap().contains("(alpha)"));
}

/// Nothing above depends on this map, but a request that carried one would.
#[test]
fn the_options_a_caller_sends_survive_the_wire_unchanged() {
    let mut params: BTreeMap<String, String> = BTreeMap::new();
    params.insert("version".into(), "1c4dd6be".into());
    let rec = CapabilityRecord::new("cloudflare", "cloudflare.worker.deploy", "project")
        .param("version", "1c4dd6be");
    assert_eq!(rec.params, params);
}

#[test]
fn a_cloudflare_operation_cannot_be_granted_in_every_project_even_though_it_names_nothing() {
    // The collision between P1-002 and P1-018, as a runtime assertion rather
    // than a declaration one.
    //
    // `--everywhere` (the `*` grant key) is gated on the operation reaching the
    // same thing in every project. P1-018 implemented that as
    // `OperationSpec::names_nothing`, which is true here: `cloudflare.account.read`
    // declares no resource and no parameters. But it resolves the account out of
    // the project's own `apex.toml` — bound, `GET /accounts/{id}` for THAT
    // project; unbound, `GET /accounts` for every account the token can see — so
    // it reaches a different thing in a different directory, and a `*` grant
    // would let an agent in a project the owner never approved read that
    // project's account with the one stored token.
    //
    // Two mutations prove this bites, and they fail it for different reasons:
    // make `may_be_granted_everywhere` compute `op.names_nothing()` again
    // instead of reading the declaration, or flip this operation's own
    // `same_everywhere` to true. Both were run, and both turn this red.
    let tag = format!("everywhere-{}", std::process::id());
    let store = std::env::temp_dir().join(format!("apex-cf-store-{tag}"));
    let project = std::env::temp_dir().join(format!("apex-cf-project-{tag}"));
    std::fs::remove_dir_all(&store).ok();
    std::fs::create_dir_all(&project).expect("project");
    std::fs::write(project.join("apex.toml"), PROJECT_FILE).expect("apex.toml");

    let mut registry = Registry::new();
    registry
        .register(Box::new(CloudflareProvider::new()))
        .expect("register");
    let service = Service::new(Store::new(store.clone()), false, registry);
    let peer = me();
    assert_eq!(
        service.add(
            peer,
            NewService {
                service: "cloudflare",
                host: "127.0.0.1",
                scheme: "http",
                username: None,
                path: "",
                auth: None,
                port: None,
            },
            SecretValue::new(TOKEN.as_bytes().to_vec()),
        ),
        Response::Ok
    );

    // Refused everywhere. `*` IS the everywhere key; the trailing bool is
    // `revoke`, not `everywhere`.
    let reply = service.grant(
        peer,
        apex_secret_core::store::ANY_PROJECT,
        "cloudflare",
        "cloudflare.account.read",
        false,
    );
    let (_, message) = reply
        .as_error()
        .expect("`*` must be refused for an operation that resolves per project");
    assert!(
        message.contains("resolves against the project"),
        "the refusal has to say why: {message}"
    );

    // ...and allowed in the project the owner named, which is the whole point
    // of refusing the other one rather than refusing both.
    let named = service.grant(
        peer,
        project.to_str().expect("utf8"),
        "cloudflare",
        "cloudflare.account.read",
        false,
    );
    assert!(
        named.as_error().is_none(),
        "the named project must still be grantable: {named:?}"
    );
    match named {
        Response::Grants { projects } => assert!(
            projects
                .get(project.to_str().expect("utf8"))
                .is_some_and(|caps| caps.iter().any(|c| c.contains("cloudflare.account.read"))),
            "{projects:?}"
        ),
        other => panic!("a grant answers with the grants: {other:?}"),
    }

    std::fs::remove_dir_all(&store).ok();
    std::fs::remove_dir_all(&project).ok();
}
