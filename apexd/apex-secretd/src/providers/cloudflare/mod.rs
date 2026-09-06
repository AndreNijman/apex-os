//! Cloudflare, as one module and one `register` call.
//!
//! P1-001's evidence made a claim about itself: *"P1-002 adds Cloudflare as one
//! module and one register call, with nothing changing in `apex-agent-core`,
//! `apex-agentd` or the CLI."* This directory is the test of it. Nothing in
//! those three crates knows the word Cloudflare, the wire carries an operation
//! id and an option map that were already there, and `apex secret capabilities`
//! prints these operations because the daemon advertises its registry rather
//! than because anyone added them to a list.
//!
//! ## What this supplies, and what it therefore cannot get wrong
//!
//! The four things [`crate::provider`] asks for:
//!
//! * **which operations exist** — [`SPEC`], §13.2's names for the Workers
//!   surface plus the account read that `apex cf status` is built on;
//! * **what a resource name means** — [`binding`], which is §13.1: `project`
//!   is a worker this project bound to an environment, `example.com` is its
//!   zone, and a name it did not bind does not resolve at all;
//! * **how a credential is presented** — [`api`], a `curl` the broker owns,
//!   with the token on the child's stdin and never in `argv`;
//! * **how to mint a short-lived one** — it does not, yet. §13.4's scoped
//!   tokens are P1-011, and [`crate::provider::Provider::mint`]'s default says
//!   "cannot" rather than pretending.
//!
//! Everything else is the framework's, in the order [`crate::provider`] sets
//! out, and this module could not skip a step if it tried: it never sees the
//! stored value until after the grant and the host pin, and the value it does
//! see is scrubbed out of the result by somebody else.
//!
//! ## The pin is real here in a way it is not for git
//!
//! `git`'s endpoint comes out of the caller's own repository, so pinning it
//! catches a repository that points somewhere else. Cloudflare's endpoint is
//! [`api::API_HOST`], a constant. That makes the pin a flat statement: a
//! credential stored for any other host cannot be spent on a Cloudflare
//! operation, and a Cloudflare token stored for `api.cloudflare.com` cannot be
//! spent anywhere else. Neither half needs this module's cooperation.
//!
//! ## Resolving twice, and why
//!
//! [`crate::provider::Bound`] carries an endpoint and a sentence, and nothing a
//! provider defines. So what `bind` worked out cannot be handed to `perform`,
//! and `perform` has to read the project file again. Between the two reads the
//! owner could change it — the owner is the caller — so `perform` rebuilds the
//! sentence and refuses if it is not the one `bind` was pinned on. That closes
//! it here. It is not closed in general, and the git provider has the same gap:
//! `bind` resolves a remote with `git remote get-url` and `perform` runs
//! `git push <remote>`, which resolves it again inside the child.
//!
//! ## What has never run against Cloudflare
//!
//! All of it. There is no account and no token on this machine, so every
//! operation below has been exercised against a loopback server that speaks the
//! same envelope and refuses without an `Authorization` header, and none of it
//! against `api.cloudflare.com`. The paths are the documented ones; that they
//! are the documented ones is not the same as having called them.

pub mod api;
pub mod binding;

use apex_secret_core::operation::{
    self, Effect, OperationSpec, ParamSpec, ProviderSpec, ResourceKind, Syntax,
};
use apex_secret_core::project::{self, MAX_PAYLOAD};
use apex_secret_core::SecretValue;

use crate::provider::{Bind, Bound, Endpoint, Performed, Provider, ProviderError};

use api::{Api, Body, Call, Multipart};
use binding::{Account, Binding, BindingError, Worker, Zone};

/// The worker, zone or bucket a caller names.
const NAMED: ResourceKind = ResourceKind::Name;

/// `message`, the annotation Cloudflare stores against a version or a
/// deployment. `Text`, so it never reaches a command line — it goes in a JSON
/// body this module builds.
const MESSAGE: ParamSpec = ParamSpec {
    name: "message",
    syntax: Syntax::Text,
    required: false,
    summary: "a line recorded with it, for whoever reads the history later",
};

/// `version`, a Worker version id.
const VERSION: ParamSpec = ParamSpec {
    name: "version",
    syntax: Syntax::Name,
    required: true,
    summary: "the version id to put in front of traffic",
};

/// The vocabulary, in §13.2's shape.
///
/// Seven names. §13.2 lists thirty-two, and the other twenty-five belong to
/// P1-005 through P1-017 — R2, D1, KV, DNS, Access, Tunnels, the AI gateway.
/// Declaring one this module cannot perform would put it in
/// `apex secret capabilities`, let an owner grant it, and then fail at use
/// time, which is a worse answer than not offering it.
pub const SPEC: ProviderSpec = ProviderSpec {
    id: "cloudflare",
    summary: "deploy and inspect this project's own Cloudflare workers",
    operations: &[
        OperationSpec {
            id: "cloudflare.account.read",
            summary: "read the Cloudflare account this credential belongs to",
            effect: Effect::Read,
            resource: ResourceKind::None,
            params: &[],
            aliases: &[],
        },
        OperationSpec {
            id: "cloudflare.worker.read",
            summary: "read the settings of one of this project's workers",
            effect: Effect::Read,
            resource: NAMED,
            params: &[],
            aliases: &[],
        },
        OperationSpec {
            id: "cloudflare.worker.upload-version",
            summary: "upload a new version of one of this project's workers, \
                      without putting it in front of traffic",
            effect: Effect::Write,
            resource: NAMED,
            params: &[
                ParamSpec {
                    name: "script",
                    syntax: Syntax::Path,
                    required: true,
                    summary: "the module to upload, as a path inside the project",
                },
                ParamSpec {
                    name: "compatibility-date",
                    syntax: Syntax::Name,
                    required: false,
                    summary: "the runtime date this version is written against",
                },
                MESSAGE,
            ],
            aliases: &[],
        },
        OperationSpec {
            id: "cloudflare.worker.deploy",
            summary: "put an uploaded version of one of this project's workers \
                      in front of all its traffic",
            effect: Effect::Write,
            resource: NAMED,
            params: &[VERSION, MESSAGE],
            aliases: &[],
        },
        OperationSpec {
            id: "cloudflare.worker.rollback",
            summary: "put an earlier version of one of this project's workers \
                      back in front of traffic",
            effect: Effect::Write,
            resource: NAMED,
            params: &[VERSION, MESSAGE],
            aliases: &[],
        },
        OperationSpec {
            id: "cloudflare.worker.tail",
            summary: "start a log session for one of this project's workers",
            effect: Effect::Read,
            resource: NAMED,
            params: &[],
            aliases: &[],
        },
        OperationSpec {
            id: "cloudflare.worker.route.read",
            summary: "read the routes this project's zone sends to its workers",
            effect: Effect::Read,
            resource: NAMED,
            params: &[],
            aliases: &[],
        },
    ],
};

/// What a request resolved to.
///
/// The output of the semantic half. Every variant is a thing the *project*
/// bound, so an operation on something it did not bind has no variant to
/// become and is refused before any call is built.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Target {
    /// No account bound yet: list what this credential can see, so the id can
    /// be put in the file. The only operation that works before §13.1's
    /// binding exists, and `apex cf status` is built on it.
    Accounts,
    Account(Account),
    Worker(Worker),
    Route { zone: Zone, worker: Worker },
}

impl From<BindingError> for ProviderError {
    fn from(e: BindingError) -> ProviderError {
        match e {
            // A name the project does not bind is a resource that does not
            // exist *here*, which is a different answer from "you may not" and
            // from "it broke".
            BindingError::NoWorker { .. }
            | BindingError::NoZone { .. }
            | BindingError::NoBucket { .. } => ProviderError::NoSuchResource(e.to_string()),
            _ => ProviderError::Refused(e.to_string()),
        }
    }
}

/// The Cloudflare provider.
pub struct CloudflareProvider {
    api: Api,
}

impl CloudflareProvider {
    /// The one the daemon serves.
    pub fn new() -> CloudflareProvider {
        CloudflareProvider {
            api: Api::cloudflare(),
        }
    }

    /// One pointed at a loopback server, for the tests that are the only place
    /// any of this has ever run.
    #[cfg(test)]
    pub fn at(port: u16) -> CloudflareProvider {
        CloudflareProvider {
            api: Api::loopback(port),
        }
    }

    /// §13.1, applied: what did this project bind that name to?
    ///
    /// Reads no credential and makes no call, because
    /// [`crate::provider::Provider::bind`] may not. That constraint is the
    /// reason the account and zone **ids** are in the project file at all: the
    /// name-to-id lookup is an authenticated request, so it is an operation of
    /// its own rather than a hidden step inside every other one.
    fn resolve(&self, req: &Bind<'_>) -> Result<Target, ProviderError> {
        let binding = Binding::read(
            std::path::Path::new(req.project),
            req.owner.uid,
            &req.owner.name,
        );

        if req.operation.id == "cloudflare.account.read" {
            // The one that has to work before the file exists, so that there is
            // a way to find out what to put in it.
            return Ok(match binding.and_then(|b| b.account()) {
                Ok(account) => Target::Account(account),
                Err(_) => Target::Accounts,
            });
        }

        let binding = binding?;
        let worker = binding.worker(req.resource)?;
        if req.operation.id == "cloudflare.worker.route.read" {
            let Some(name) = binding.zone.clone() else {
                return Err(BindingError::NoZone {
                    path: binding.path.clone(),
                    named: req.resource.to_string(),
                    bound: None,
                }
                .into());
            };
            let zone = binding.zone(&name)?;
            return Ok(Target::Route { zone, worker });
        }
        Ok(Target::Worker(worker))
    }

    /// The sentence the audit trail records and the reply carries.
    ///
    /// Two jobs, and the second one constrains the first. It is what §11's
    /// trail records, so it has to read as a sentence; and it is the only
    /// thing `perform` can compare against what `bind` was pinned on, so it
    /// has to name **every** value that decides where the request goes. The
    /// account id is in it for that reason and not for decoration: an
    /// `account_id` swapped between the two reads keeps the account's label
    /// and changes the URL, and a sentence that carried only the label would
    /// let that through.
    fn detail(operation: &OperationSpec, target: &Target, params: &operation::Params) -> String {
        let version = params.get("version").map(String::as_str).unwrap_or("");
        match target {
            Target::Accounts => "list the accounts this credential can see".to_string(),
            Target::Account(account) => {
                format!("read account {} [{}]", account.named(), account.id)
            }
            Target::Route { zone, worker } => format!(
                "read the routes zone {} [{}] sends to {} ({})",
                zone.name, zone.id, worker.name, worker.environment
            ),
            Target::Worker(worker) => {
                let where_ = format!(
                    "{} ({}) in account {} [{}]",
                    worker.name,
                    worker.environment,
                    worker.account.named(),
                    worker.account.id
                );
                match operation.id {
                    "cloudflare.worker.read" => format!("read the settings of {where_}"),
                    "cloudflare.worker.upload-version" => {
                        format!("upload a new version of {where_}")
                    }
                    "cloudflare.worker.deploy" => format!("deploy version {version} of {where_}"),
                    "cloudflare.worker.rollback" => {
                        format!("roll {where_} back to version {version}")
                    }
                    "cloudflare.worker.tail" => format!("start a log session for {where_}"),
                    other => format!("{other} on {where_}"),
                }
            }
        }
    }

    /// Turn a resolved target into the call to make.
    ///
    /// Split from [`CloudflareProvider::resolve`] so that `bind` does not read
    /// a Worker bundle off the disk to answer a question about a hostname.
    fn build(&self, req: &Bind<'_>, target: &Target) -> Result<Call, ProviderError> {
        let get = |path: String| Call {
            method: "GET",
            path,
            body: Body::None,
        };
        Ok(match (req.operation.id, target) {
            ("cloudflare.account.read", Target::Accounts) => get("/accounts".to_string()),
            ("cloudflare.account.read", Target::Account(account)) => {
                get(format!("/accounts/{}", account.id))
            }
            ("cloudflare.worker.route.read", Target::Route { zone, .. }) => {
                get(format!("/zones/{}/workers/routes", zone.id))
            }
            ("cloudflare.worker.read", Target::Worker(worker)) => {
                get(format!("{}/settings", script(worker)))
            }
            ("cloudflare.worker.tail", Target::Worker(worker)) => Call {
                method: "POST",
                path: format!("{}/tails", script(worker)),
                body: Body::None,
            },
            ("cloudflare.worker.upload-version", Target::Worker(worker)) => Call {
                method: "POST",
                path: format!("{}/versions", script(worker)),
                body: self.version_body(req)?,
            },
            (id @ ("cloudflare.worker.deploy" | "cloudflare.worker.rollback"), Target::Worker(worker)) => {
                let rollback = id == "cloudflare.worker.rollback";
                Call {
                    method: "POST",
                    // `force` is what tells Cloudflare this is a deliberate
                    // return to an older version rather than a deployment that
                    // lost a race with a secret change. The endpoint is the
                    // same one `deploy` uses; what separates the two is the
                    // grant, which is the whole point of §13.2 naming them
                    // separately.
                    path: format!(
                        "{}/deployments{}",
                        script(worker),
                        if rollback { "?force=true" } else { "" }
                    ),
                    body: self.deployment_body(req)?,
                }
            }
            (id, _) => {
                return Err(ProviderError::Failed(format!(
                    "the cloudflare provider declares '{id}' and does not implement it"
                )))
            }
        })
    }

    /// The multipart body for an upload: the module, and the metadata naming
    /// it.
    fn version_body(&self, req: &Bind<'_>) -> Result<Body, ProviderError> {
        let Some(script) = req.params.get("script") else {
            // The framework checks `required`, so this is unreachable through
            // the pipeline. It is here because a provider that trusted the
            // framework's checks would stop being correct the day they move.
            return Err(ProviderError::Refused(
                "this operation needs a 'script' option naming the module to \
                 upload"
                    .to_string(),
            ));
        };
        let module = script.rsplit('/').next().unwrap_or(script).to_string();
        if !module.ends_with(".js") && !module.ends_with(".mjs") {
            return Err(ProviderError::Refused(format!(
                "'{}' is not a module Cloudflare will accept as a worker's main \
                 module; it has to be a .js or .mjs file",
                module.escape_debug()
            )));
        }

        let bytes = project::read_file(
            std::path::Path::new(req.project),
            script,
            req.owner.uid,
            &req.owner.name,
            MAX_PAYLOAD,
        )
        .map_err(|e| ProviderError::NoSuchResource(e.to_string()))?;

        let mut metadata = serde_json::Map::new();
        metadata.insert("main_module".into(), module.clone().into());
        if let Some(date) = req.params.get("compatibility-date") {
            metadata.insert("compatibility_date".into(), date.clone().into());
        }
        if let Some(message) = req.params.get("message") {
            let mut annotations = serde_json::Map::new();
            annotations.insert("workers/message".into(), message.clone().into());
            metadata.insert("annotations".into(), annotations.into());
        }

        let mut form = Multipart::new().map_err(|e| ProviderError::Failed(e.to_string()))?;
        form.part(
            "metadata",
            None,
            "application/json",
            serde_json::Value::Object(metadata).to_string().as_bytes(),
        );
        form.part(
            &module,
            Some(&module),
            "application/javascript+module",
            &bytes,
        );
        Ok(form.finish())
    }

    /// The JSON body for a deployment: one version, all of the traffic.
    ///
    /// A split between two versions is §13.7's staged rollout and belongs to
    /// P1-013, which needs a health check to decide whether to widen it.
    fn deployment_body(&self, req: &Bind<'_>) -> Result<Body, ProviderError> {
        let Some(version) = req.params.get("version") else {
            return Err(ProviderError::Refused(
                "this operation needs a 'version' option naming the version to \
                 put in front of traffic"
                    .to_string(),
            ));
        };
        let mut entry = serde_json::Map::new();
        entry.insert("version_id".into(), version.clone().into());
        entry.insert("percentage".into(), serde_json::json!(100));

        let mut body = serde_json::Map::new();
        body.insert("strategy".into(), "percentage".into());
        body.insert("versions".into(), serde_json::json!([entry]));
        if let Some(message) = req.params.get("message") {
            let mut annotations = serde_json::Map::new();
            annotations.insert("workers/message".into(), message.clone().into());
            body.insert("annotations".into(), annotations.into());
        }
        Ok(Body::Json(serde_json::Value::Object(body).to_string()))
    }
}

impl Default for CloudflareProvider {
    fn default() -> CloudflareProvider {
        CloudflareProvider::new()
    }
}

/// The path prefix for one worker.
fn script(worker: &Worker) -> String {
    format!(
        "/accounts/{}/workers/scripts/{}",
        worker.account.id, worker.name
    )
}

/// A tail session's reply, with the part of it that is a credential taken out.
///
/// `POST …/tails` answers with `{"result":{"id":…,"url":"wss://…","expires_at":…}}`
/// and that `url` carries its own authorisation: whoever holds it can read the
/// worker's live logs without a token. Handing it back would be handing the
/// agent a credential, which is the one thing this whole service exists not to
/// do — so the session is created, its id and expiry are returned, and the URL
/// is dropped here.
///
/// The consequence is stated rather than hidden: **this build cannot deliver
/// the log stream.** Doing so means the broker holding the WebSocket and
/// relaying it, which is a different shape of operation from every other one in
/// the vocabulary and is named as P1-004's remainder.
fn without_the_tail_url(body: &str) -> String {
    let Ok(mut value) = serde_json::from_str::<serde_json::Value>(body) else {
        return body.to_string();
    };
    let removed = value
        .get_mut("result")
        .and_then(|r| r.as_object_mut())
        .map(|result| result.remove("url"))
        .unwrap_or(None);
    let mut out = serde_json::to_string(&value).unwrap_or_else(|_| body.to_string());
    if removed.is_some() {
        out.push_str(
            "\napex: the session's WebSocket URL is authorisation in itself, so \
             it is not returned. This build starts a tail session; it does not \
             relay the stream.",
        );
    }
    out
}

impl Provider for CloudflareProvider {
    fn spec(&self) -> &'static ProviderSpec {
        &SPEC
    }

    fn bind(&self, req: &Bind<'_>) -> Result<Bound, ProviderError> {
        let target = self.resolve(req)?;
        Ok(Bound {
            endpoint: Endpoint {
                scheme: self.api.scheme.clone(),
                host: self.api.host.clone(),
            },
            detail: CloudflareProvider::detail(req.operation, &target, req.params),
        })
    }

    fn perform(
        &self,
        req: &Bind<'_>,
        bound: &Bound,
        value: &SecretValue,
    ) -> Result<Performed, ProviderError> {
        let target = self.resolve(req)?;
        // The project file is the caller's, and `bind` read it a moment ago.
        // If it says something else now, the pin and the trail describe an
        // operation that is not the one about to happen.
        let detail = CloudflareProvider::detail(req.operation, &target, req.params);
        if detail != bound.detail {
            return Err(ProviderError::Refused(
                "this project's Cloudflare binding changed while the request \
                 was being checked, so the operation that was authorised is \
                 not the one that would run. Try again"
                    .to_string(),
            ));
        }
        let call = self.build(req, &target)?;
        let reply = api::call(&self.api, &call, value, req.owner)
            .map_err(|e| ProviderError::Failed(e.to_string()))?;

        // Everything the far side or the child chose comes back as output, not
        // as an error: the framework scrubs a result and does not scrub a
        // refusal reason.
        let body = if req.operation.id == "cloudflare.worker.tail" && reply.ok() {
            without_the_tail_url(&reply.body)
        } else {
            reply.body.clone()
        };
        let output = if reply.ok() {
            body
        } else if reply.status == 0 {
            format!("apex: the api could not be reached\n{body}")
        } else {
            format!("apex: cloudflare answered HTTP {}\n{body}", reply.status)
        };
        Ok(Performed {
            code: i32::from(!reply.ok()),
            output,
        })
    }
}

#[cfg(test)]
mod tests;
