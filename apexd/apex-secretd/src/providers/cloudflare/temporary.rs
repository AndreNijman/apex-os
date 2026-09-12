//! §13.4, applied to Cloudflare: a short-lived token scoped to one operation.
//!
//! > *"When Cloudflare supports scoped/temporary credentials, prefer
//! > short-lived task credentials bound to exact account/resources/operations."*
//!
//! ## What is actually spent, before and after
//!
//! Before this module, every Cloudflare operation spent the token the owner
//! stored. The agent never held it — [`super::api::call`] puts it on a `curl`
//! child's stdin — and every operation was already narrowed to a resource the
//! project's own file names. But the token *itself* was whatever the owner
//! pasted in, which for most people is a token that can reach every product in
//! the account. P1-005's evidence said so in as many words and left the
//! criterion open for this.
//!
//! With this module, the token that reaches the operation is one Cloudflare
//! issued seconds earlier, carrying a single policy, for a single product, at
//! a single scope, expiring in [`LIFETIME_SECS`] and revoked as soon as the
//! operation returns. The stored token is spent on exactly two requests — ask
//! for the permission-group list, ask for the token — and on the revoke.
//!
//! ## The two dead ends, so nobody walks them again
//!
//! **R2's `temp-access-credentials` is not this.** `POST
//! /accounts/{id}/r2/temp-access-credentials` needs a `parentAccessKeyId` —
//! an S3 access key, which this broker does not hold — and it answers with
//! SigV4 credentials, which a `curl` that only ever sends `Authorization:
//! Bearer` cannot spend. Closing that path means giving the broker an S3
//! credential and a SigV4 signing implementation.
//!
//! **An R2 token cannot be narrowed to a bucket and still work here.** R2 does
//! have bucket-scoped tokens, at
//! `com.cloudflare.edge.r2.bucket.<account>_<jurisdiction>_<bucket>`. They
//! carry the *Object* permissions, and Cloudflare's own documentation says:
//! *"The Object Read & Write and Object Read only permissions are only
//! supported by the S3-compatible API, not the Cloudflare REST API."* The
//! permissions a REST transport can spend — Admin Read, Admin Read & Write —
//! are account-scoped and take no bucket. So an R2 token minted here is
//! narrowed by **product and lifetime, not by bucket**; the bucket narrowing
//! stays where P1-005 put it, in [`super::binding`], and is enforced before
//! the request is built rather than by the token. That is a real limit and it
//! is stated here rather than in a plan.
//!
//! DNS is the case where the narrowing reaches the resource as well: DNS
//! permission groups are zone-scoped, so a minted DNS token names the one zone
//! the project bound and no other.
//!
//! ## Why the permission-group ids are looked up and never written down
//!
//! A policy needs permission-group **ids**, and an id is an opaque 32-hex
//! string that means nothing to a reader. The names are stable and documented;
//! the ids are not this build's to know. So [`POLICY`] names the groups, the
//! account's own list turns a name into an id, and a name the account's list
//! does not carry is a [`Minted::CouldNotRun`] — *this build could not
//! assemble a narrow policy*, which is not the same as *there is no narrow
//! policy* and is certainly not the same as *you may not have one*.
//!
//! Each slot in [`POLICY`] lists candidates rather than one name, and the
//! first one the account actually returns wins. Cloudflare spells the write
//! half of a permission group `Edit` for most products and `Write` for Access,
//! and a build that hard-coded the wrong one would mint a token with no
//! permissions at all and only find out when the operation failed. Listing
//! both means a wrong guess degrades to "could not build a policy" — which
//! falls back to the stored token, visibly, in the trail — rather than to a
//! token that silently cannot do the thing it was minted for.
//!
//! ## Pinned, not remembered
//!
//! `cloudflare/api-schemas` `openapi.json` at `d5003a19` (info.version 4.0.0),
//! the same pin P1-005 used for the REST paths:
//!
//! * `POST /accounts/{account_id}/tokens` — `name` and `policies` required,
//!   `expires_on` and `not_before` optional; `result.value` is the token and
//!   `result.id` the handle;
//! * `DELETE /accounts/{account_id}/tokens/{token_id}` — revoke;
//! * `GET /accounts/{account_id}/tokens/permission_groups` — the name-to-id
//!   list, with `name` and `scope` filters;
//! * `policies[].resources` is a map of resource string to `"*"`.
//!
//! One quirk worth knowing: the schema marks `policies[].id` both `required`
//! and `readOnly`, with `x-stainless-terraform-always-send`. It is a code
//! generator's artefact, not a field a creation sends, and this module omits
//! it.
//!
//! **Nothing here has ever run against `api.cloudflare.com`.** There is no
//! Cloudflare account and no token on the machine this was built on. Every
//! test runs against the loopback double in [`super::tests`], which answers
//! nothing at all to a request with no `Authorization` header. Two things are
//! therefore stated rather than measured, and both are in the agent card: the
//! minimum `expires_on` Cloudflare will accept — [`LIFETIME_SECS`] is this
//! build's choice and no documentation gives a floor — and whether a given
//! account's stored token carries the permission to create tokens at all,
//! which Cloudflare documents as requiring Super Administrator on the account.
//! The second is the reason [`Minted::Denied`] is expected to be the common
//! live answer rather than a rare one, and the reason a denial falls back to
//! the stored token instead of refusing the operation.

use apex_secret_core::SecretValue;

use crate::broker::Owner;
use crate::provider::{Lease, Minted};

use super::api::{self, Api, Body, Call};

/// How long a minted token is accepted for.
///
/// A backstop rather than the mechanism: the token is revoked when the
/// operation returns, and this is what limits the damage if the revoke itself
/// could not be done. Five minutes against a request timeout of
/// [`super::api::TIMEOUT_SECS`] leaves room for a slow upload without leaving
/// a spendable credential lying around for an hour.
///
/// **Not verified against Cloudflare.** No documentation this build could find
/// states a minimum `expires_on`, and there is no account here to discover one
/// against. If Cloudflare refuses a five-minute token, the refusal arrives as
/// a 400 and this module answers [`Minted::CouldNotRun`] — the operation still
/// runs, on the stored credential, and the trail says why.
pub const LIFETIME_SECS: u64 = 300;

/// The name a minted token carries at Cloudflare, so a person reading their
/// token list can tell what made it and which request it belonged to.
///
/// The audit id is in it deliberately: a token that outlived its operation
/// because a revoke failed can be traced back to the line that says so.
pub fn token_name(operation: &str, audit_id: &str) -> String {
    let name = format!("apex {operation} {audit_id}");
    // The schema's limit is 120. Truncating on a char boundary rather than a
    // byte one, because every id here is ASCII but nothing enforces that.
    name.chars().take(120).collect()
}

/// Which resource a minted token's policy applies to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Scope {
    /// The whole bound account. What every product but DNS can express.
    Account(String),
    /// One zone in it. What DNS and Workers routes can express, and the only
    /// place the narrowing reaches past the product.
    Zone(String),
}

impl Scope {
    /// The `resources` map, as the policy carries it.
    pub fn resources(&self) -> serde_json::Value {
        let key = match self {
            Scope::Account(id) => format!("com.cloudflare.api.account.{id}"),
            Scope::Zone(id) => format!("com.cloudflare.api.account.zone.{id}"),
        };
        let mut map = serde_json::Map::new();
        map.insert(key, serde_json::Value::String("*".to_string()));
        serde_json::Value::Object(map)
    }
}

/// The narrowest credential that carries one operation.
///
/// [`Narrowest::Nothing`] is a row and not a missing row, deliberately. An
/// operation with no entry at all would go on spending the stored credential
/// because nobody had thought about it; an operation with a `Nothing` row has
/// been thought about and the answer written down, and
/// `every_declared_operation_can_name_the_narrowest_token_that_carries_it`
/// cannot tell the difference between the two unless both exist.
pub enum Narrowest {
    /// A token carrying these permission groups at this scope.
    Token {
        /// One entry per permission group the operation needs, each holding
        /// the spellings this build will accept for it. The first that the
        /// account's own list carries is the one used.
        groups: &'static [&'static [&'static str]],
        /// Whether the policy is scoped to the project's zone rather than its
        /// account.
        zone_scoped: bool,
    },
    /// There is no narrower credential for this operation, and this is why.
    Nothing(&'static str),
}

impl Narrowest {
    /// Whether the policy is zone-scoped, which decides what the caller has to
    /// resolve before minting.
    pub fn zone_scoped(&self) -> bool {
        matches!(self, Narrowest::Token { zone_scoped: true, .. })
    }
}

/// Read-only and write spellings, as Cloudflare's permission-group list gives
/// them. `Edit` for most products, `Write` for Access — both are listed
/// wherever this build is not certain, for the reason in the module note.
const WORKERS_WRITE: &[&str] = &["Workers Scripts Write", "Workers Scripts Edit"];
const WORKERS_READ: &[&str] = &["Workers Scripts Read"];
const ROUTES_READ: &[&str] = &["Workers Routes Read", "Workers Routes Write"];
const R2_READ: &[&str] = &["Workers R2 Storage Read"];
const R2_WRITE: &[&str] = &["Workers R2 Storage Edit", "Workers R2 Storage Write"];
const KV_READ: &[&str] = &["Workers KV Storage Read"];
const KV_WRITE: &[&str] = &["Workers KV Storage Edit", "Workers KV Storage Write"];
const D1_READ: &[&str] = &["D1 Read"];
const D1_WRITE: &[&str] = &["D1 Edit", "D1 Write"];
const QUEUES_WRITE: &[&str] = &["Queues Edit", "Queues Write"];
const HYPERDRIVE_READ: &[&str] = &["Hyperdrive Read"];
const HYPERDRIVE_WRITE: &[&str] = &["Hyperdrive Edit", "Hyperdrive Write"];
const DNS_READ: &[&str] = &["DNS Read"];
const DNS_WRITE: &[&str] = &["DNS Write", "DNS Edit"];
const ACCOUNT_READ: &[&str] = &["Account Settings Read"];
const ACCESS_READ: &[&str] = &["Access: Apps and Policies Read"];
const ACCESS_WRITE: &[&str] = &["Access: Apps and Policies Write", "Access: Apps and Policies Edit"];
const SERVICE_TOKENS_WRITE: &[&str] =
    &["Access: Service Tokens Write", "Access: Service Tokens Edit"];
const TUNNEL_READ: &[&str] = &["Cloudflare Tunnel Read"];
const TUNNEL_WRITE: &[&str] = &["Cloudflare Tunnel Edit", "Cloudflare Tunnel Write"];
const WORKERS_AI_RUN: &[&str] = &["Workers AI Edit", "Workers AI Write", "Workers AI Read"];
const AI_GATEWAY_RUN: &[&str] = &["AI Gateway Run", "AI Gateway Edit"];
const AI_GATEWAY_WRITE: &[&str] = &["AI Gateway Edit", "AI Gateway Write"];
const SECRETS_STORE_WRITE: &[&str] = &["Secrets Store Edit", "Secrets Store Write"];

/// Every operation this provider declares, and the narrowest token that can
/// carry it.
///
/// **Every declared operation must appear here.** An operation with no entry
/// gets no narrowing at all and would go on spending the account-wide token
/// silently, so `every_operation_can_name_the_narrowest_token_that_carries_it`
/// walks [`super::SPEC`] and fails on a missing row rather than letting one be
/// forgotten. Adding an operation means adding a row.
pub const POLICY: &[(&str, Narrowest)] = &[
    ("cloudflare.account.read", Narrowest::Token { groups: &[ACCOUNT_READ], zone_scoped: false }),
    ("cloudflare.worker.read", Narrowest::Token { groups: &[WORKERS_READ], zone_scoped: false }),
    ("cloudflare.worker.upload-version", Narrowest::Token { groups: &[WORKERS_WRITE], zone_scoped: false }),
    ("cloudflare.worker.deploy", Narrowest::Token { groups: &[WORKERS_WRITE], zone_scoped: false }),
    ("cloudflare.worker.rollback", Narrowest::Token { groups: &[WORKERS_WRITE], zone_scoped: false }),
    ("cloudflare.worker.tail", Narrowest::Token { groups: &[WORKERS_WRITE], zone_scoped: false }),
    // A route lives in a zone, not in the account, and is the one Workers name
    // whose token can therefore be narrowed past the product.
    ("cloudflare.worker.route.read", Narrowest::Token { groups: &[ROUTES_READ], zone_scoped: true }),
    ("cloudflare.dns.read", Narrowest::Token { groups: &[DNS_READ], zone_scoped: true }),
    ("cloudflare.dns.create", Narrowest::Token { groups: &[DNS_WRITE], zone_scoped: true }),
    // Update and delete look the record up first, with the same token, so a
    // read is not needed alongside the write: the lookup is a GET on the same
    // zone the write goes to, and DNS Write covers it.
    ("cloudflare.dns.update", Narrowest::Token { groups: &[DNS_WRITE], zone_scoped: true }),
    ("cloudflare.dns.delete", Narrowest::Token { groups: &[DNS_WRITE], zone_scoped: true }),
    ("cloudflare.r2.object.read", Narrowest::Token { groups: &[R2_READ], zone_scoped: false }),
    ("cloudflare.r2.object.write", Narrowest::Token { groups: &[R2_WRITE], zone_scoped: false }),
    ("cloudflare.r2.bucket.create", Narrowest::Token { groups: &[R2_WRITE], zone_scoped: false }),
    ("cloudflare.d1.read", Narrowest::Token { groups: &[D1_READ], zone_scoped: false }),
    ("cloudflare.d1.query", Narrowest::Token { groups: &[D1_WRITE], zone_scoped: false }),
    ("cloudflare.d1.migrate", Narrowest::Token { groups: &[D1_WRITE], zone_scoped: false }),
    ("cloudflare.kv.read", Narrowest::Token { groups: &[KV_READ], zone_scoped: false }),
    ("cloudflare.kv.write", Narrowest::Token { groups: &[KV_WRITE], zone_scoped: false }),
    // Publishing to a queue is a write to it; Cloudflare has no separate
    // publish permission group.
    ("cloudflare.queue.publish", Narrowest::Token { groups: &[QUEUES_WRITE], zone_scoped: false }),
    ("cloudflare.queue.manage", Narrowest::Token { groups: &[QUEUES_WRITE], zone_scoped: false }),
    ("cloudflare.hyperdrive.read", Narrowest::Token { groups: &[HYPERDRIVE_READ], zone_scoped: false }),
    ("cloudflare.hyperdrive.edit", Narrowest::Token { groups: &[HYPERDRIVE_WRITE], zone_scoped: false }),
    ("cloudflare.secret.create", Narrowest::Token { groups: &[SECRETS_STORE_WRITE], zone_scoped: false }),
    ("cloudflare.secret.rotate", Narrowest::Token { groups: &[SECRETS_STORE_WRITE], zone_scoped: false }),
    // Binding changes the WORKER as well as reading the store, so the token
    // needs both groups. A policy with only one of them would mint a token
    // that fails halfway through.
    (
        "cloudflare.secret.bind",
        Narrowest::Token { groups: &[SECRETS_STORE_WRITE, WORKERS_WRITE], zone_scoped: false },
    ),
    ("cloudflare.access.read", Narrowest::Token { groups: &[ACCESS_READ], zone_scoped: false }),
    ("cloudflare.access.edit", Narrowest::Token { groups: &[ACCESS_WRITE], zone_scoped: false }),
    (
        "cloudflare.access.service-token.create",
        Narrowest::Token { groups: &[SERVICE_TOKENS_WRITE], zone_scoped: false },
    ),
    ("cloudflare.tunnel.read", Narrowest::Token { groups: &[TUNNEL_READ], zone_scoped: false }),
    ("cloudflare.tunnel.edit", Narrowest::Token { groups: &[TUNNEL_WRITE], zone_scoped: false }),
    ("cloudflare.workers-ai.run", Narrowest::Token { groups: &[WORKERS_AI_RUN], zone_scoped: false }),
    ("cloudflare.ai-gateway.run", Narrowest::Token { groups: &[AI_GATEWAY_RUN], zone_scoped: false }),
    ("cloudflare.ai-gateway.edit", Narrowest::Token { groups: &[AI_GATEWAY_WRITE], zone_scoped: false }),
    // P1-012's four. `wrangler deploy` is the Workers API with a build step in
    // front of it, so it takes the same token the REST deploy does.
    (
        "cloudflare.wrangler.deploy",
        Narrowest::Token { groups: &[WORKERS_WRITE], zone_scoped: false },
    ),
    (
        "cloudflare.wrangler.versions-upload",
        Narrowest::Token { groups: &[WORKERS_WRITE], zone_scoped: false },
    ),
    // Terraform is the honest `Nothing`. What permissions a plan or an apply
    // needs is decided by the project's own `.tf` files — which product, which
    // resource, read or write — and this build does not read them. It could
    // guess broad, and a token minted broad "because we could not tell" is
    // worse than the stored one: it would look like narrowing in the trail
    // while granting the same reach. So it says there is nothing narrower, in
    // those words, and the stored credential is used with `narrowing` reading
    // `no-narrower-form`.
    //
    // A project that wants terraform on a narrow credential can store a narrow
    // one: §13.1 binds a credential per project, and that is the mechanism
    // that already exists for this.
    (
        "cloudflare.terraform.plan",
        Narrowest::Nothing(
            "what permissions a terraform plan needs is decided by this \
             project's own .tf files, which this build does not read, so it \
             cannot describe a narrower token than the stored one",
        ),
    ),
    (
        "cloudflare.terraform.apply",
        Narrowest::Nothing(
            "what permissions a terraform apply needs is decided by this \
             project's own .tf files, which this build does not read, so it \
             cannot describe a narrower token than the stored one",
        ),
    ),
];

/// The narrowest token that carries `operation`, or nothing if this build has
/// no row for it.
pub fn policy_for(operation: &str) -> Option<&'static Narrowest> {
    POLICY.iter().find(|(id, _)| *id == operation).map(|(_, p)| p)
}

/// Ask the account which permission groups it has, and turn the names in
/// `policy` into the ids a token policy needs.
///
/// The failure shapes are the point. A 401 or 403 is the stored credential
/// being refused — a fact about that token, not about the account — and comes
/// back as [`Minted::Denied`]. Anything else that stops the list arriving, and
/// a list that arrives without a name this build asked for, is
/// [`Minted::CouldNotRun`]: nothing was established about whether a narrower
/// token is possible.
fn group_ids(
    api: &Api,
    account: &str,
    wanted: &'static [&'static [&'static str]],
    value: &SecretValue,
    owner: &Owner,
) -> Result<Vec<String>, Minted> {
    let call = Call::new(
        "GET",
        format!("/accounts/{account}/tokens/permission_groups"),
        Body::None,
    );
    let reply = match api::call(api, &call, value, owner) {
        Ok(reply) => reply,
        Err(e) => return Err(Minted::CouldNotRun(e.to_string())),
    };
    if reply.status == 0 {
        return Err(Minted::CouldNotRun(
            "the api could not be reached to ask which permission groups this \
             account has, so no short-lived token was made and the stored \
             credential was used"
                .to_string(),
        ));
    }
    if reply.status == 401 || reply.status == 403 {
        return Err(Minted::Denied(format!(
            "cloudflare answered HTTP {} when this asked which permission \
             groups the account has. The stored credential is not permitted to \
             read them, so it cannot be exchanged for a narrower one — \
             cloudflare requires Super Administrator on the account to create \
             account-owned tokens. The operation ran on the stored credential",
            reply.status
        )));
    }
    if !reply.ok() {
        return Err(Minted::CouldNotRun(format!(
            "cloudflare answered HTTP {} to the permission-group list",
            reply.status
        )));
    }
    let Ok(body) = serde_json::from_str::<serde_json::Value>(&reply.body) else {
        return Err(Minted::CouldNotRun(
            "the permission-group list was not the json envelope".to_string(),
        ));
    };
    if body.get("success").and_then(|s| s.as_bool()) != Some(true) {
        return Err(Minted::CouldNotRun(
            "the account did not answer the permission-group list successfully".to_string(),
        ));
    }
    let Some(groups) = body.get("result").and_then(|r| r.as_array()) else {
        return Err(Minted::CouldNotRun(
            "the permission-group list carried no groups".to_string(),
        ));
    };

    let mut ids = Vec::with_capacity(wanted.len());
    for slot in wanted {
        let found = slot.iter().find_map(|wanted| {
            groups.iter().find_map(|group| {
                let name = group.get("name").and_then(|n| n.as_str())?;
                let id = group.get("id").and_then(|i| i.as_str())?;
                // An id that is not an id is not one this build will put in a
                // policy: a token created against a malformed policy is a
                // token whose permissions nobody can predict.
                (name == *wanted && super::binding::valid_id(id)).then(|| id.to_string())
            })
        });
        match found {
            Some(id) => ids.push(id),
            None => {
                return Err(Minted::CouldNotRun(format!(
                    "this account's permission-group list carries none of {}, \
                     so a narrow policy for this operation could not be built. \
                     That is not the same as the account being unable to have \
                     one — it is this build not recognising what the account \
                     offers. The operation ran on the stored credential",
                    slot.join(", ")
                )))
            }
        }
    }
    Ok(ids)
}

/// §13.4's exchange: a short-lived token for this operation, or a precise
/// account of why there is not one.
pub fn mint(
    api: &Api,
    operation: &str,
    audit_id: &str,
    account: &str,
    scope: &Scope,
    value: &SecretValue,
    owner: &Owner,
) -> Minted {
    let policy = match policy_for(operation) {
        Some(Narrowest::Token { groups, .. }) => groups,
        Some(Narrowest::Nothing(why)) => return Minted::NoNarrowerForm((*why).to_string()),
        None => {
            return Minted::NoNarrowerForm(format!(
                "this build has no narrower token for '{operation}'"
            ))
        }
    };
    let ids = match group_ids(api, account, policy, value, owner) {
        Ok(ids) => ids,
        Err(answer) => return answer,
    };

    let now = apex_secret_core::store::now_ms() / 1000;
    let expires = now + LIFETIME_SECS;
    let Some(expires_on) = rfc3339(expires) else {
        return Minted::CouldNotRun(
            "this machine's clock is not in a range an expiry can be written \
             from, so no short-lived token was asked for"
                .to_string(),
        );
    };

    let mut policy_object = serde_json::Map::new();
    policy_object.insert("effect".to_string(), serde_json::Value::String("allow".to_string()));
    policy_object.insert(
        "permission_groups".to_string(),
        serde_json::Value::Array(
            ids.into_iter()
                .map(|id| {
                    let mut group = serde_json::Map::new();
                    group.insert("id".to_string(), serde_json::Value::String(id));
                    serde_json::Value::Object(group)
                })
                .collect(),
        ),
    );
    policy_object.insert("resources".to_string(), scope.resources());

    let mut body = serde_json::Map::new();
    body.insert(
        "name".to_string(),
        serde_json::Value::String(token_name(operation, audit_id)),
    );
    body.insert(
        "policies".to_string(),
        serde_json::Value::Array(vec![serde_json::Value::Object(policy_object)]),
    );
    body.insert("expires_on".to_string(), serde_json::Value::String(expires_on));

    let call = Call::new(
        "POST",
        format!("/accounts/{account}/tokens"),
        Body::Json(serde_json::Value::Object(body).to_string()),
    );
    let reply = match api::call(api, &call, value, owner) {
        Ok(reply) => reply,
        Err(e) => return Minted::CouldNotRun(e.to_string()),
    };
    if reply.status == 0 {
        return Minted::CouldNotRun(
            "the api could not be reached to ask for a short-lived token".to_string(),
        );
    }
    if reply.status == 401 || reply.status == 403 {
        return Minted::Denied(format!(
            "cloudflare answered HTTP {} when this asked for a short-lived \
             token scoped to the operation. The stored credential may not \
             create tokens, so it could not be exchanged for a narrower one. \
             The operation ran on the stored credential",
            reply.status
        ));
    }
    if !reply.ok() {
        // A 400 lands here and not in `Denied`, deliberately: a body this
        // build composed that cloudflare will not accept — an expiry it
        // considers too soon, a policy shape it has changed — is this build
        // failing to ask, not the account refusing.
        return Minted::CouldNotRun(format!(
            "cloudflare answered HTTP {} to the request for a short-lived \
             token, so the operation ran on the stored credential",
            reply.status
        ));
    }
    let Ok(parsed) = serde_json::from_str::<serde_json::Value>(&reply.body) else {
        return Minted::CouldNotRun(
            "the reply to the token request was not the json envelope".to_string(),
        );
    };
    if parsed.get("success").and_then(|s| s.as_bool()) != Some(true) {
        return Minted::CouldNotRun(
            "cloudflare did not answer the token request successfully".to_string(),
        );
    }
    let result = parsed.get("result");
    let token = result.and_then(|r| r.get("value")).and_then(|v| v.as_str());
    let handle = result.and_then(|r| r.get("id")).and_then(|i| i.as_str());
    let (Some(token), Some(handle)) = (token, handle) else {
        return Minted::CouldNotRun(
            "cloudflare answered the token request without a token or without \
             an id to revoke it by. A token that cannot be revoked is not one \
             this build will spend"
                .to_string(),
        );
    };
    if !api::valid_token(token) || !super::binding::valid_id(handle) {
        return Minted::CouldNotRun(
            "cloudflare answered the token request with a token or an id this \
             build does not recognise, so it was not spent"
                .to_string(),
        );
    }
    Minted::Narrowed {
        value: SecretValue::new(token.as_bytes().to_vec()),
        lease: Lease {
            handle: handle.to_string(),
            expires_ms: expires * 1000,
        },
    }
}

/// End a minted token's life, with the credential that issued it.
///
/// The stored credential and not the minted one: the minted token carries a
/// single product permission and deliberately cannot delete tokens, so it
/// cannot retire itself.
pub fn revoke(
    api: &Api,
    account: &str,
    lease: &Lease,
    stored: &SecretValue,
    owner: &Owner,
) -> Result<(), String> {
    let call = Call::new(
        "DELETE",
        format!("/accounts/{account}/tokens/{}", lease.handle),
        Body::None,
    );
    let reply = api::call(api, &call, stored, owner).map_err(|e| e.to_string())?;
    if reply.status == 0 {
        return Err("the api could not be reached".to_string());
    }
    if !reply.ok() {
        return Err(format!("cloudflare answered HTTP {}", reply.status));
    }
    Ok(())
}

/// Seconds since the epoch as an RFC 3339 instant in UTC, which is the only
/// spelling of a time this build ever produces.
///
/// Written out rather than taken from a crate because the workspace has no
/// date library and a single field in a single request body is not a reason to
/// add one. `None` for a time outside the range the civil calendar conversion
/// is valid over, which is a clock that is wrong rather than a time that is
/// late — and a wrong clock must not become a token with a nonsense expiry.
pub fn rfc3339(secs: u64) -> Option<String> {
    // Year 10000 would need a fifth digit and the format is fixed-width.
    if secs >= 253_402_300_800 {
        return None;
    }
    let days = (secs / 86_400) as i64;
    let rem = secs % 86_400;
    // Howard Hinnant's civil_from_days, shifted to a 0000-03-01 era.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    Some(format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z",
        rem / 3_600,
        (rem % 3_600) / 60,
        rem % 60
    ))
}
