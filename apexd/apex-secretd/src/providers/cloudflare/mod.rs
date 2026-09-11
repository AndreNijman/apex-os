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
//! ## What R2 can and cannot carry
//!
//! §13.5 asks for bucket- and object-scoped capabilities, and that is what the
//! three R2 operations are: the bucket is one the project's own file lists, and
//! an object is a key under it. Three limits are in the build rather than in a
//! plan, and each is a consequence of something else here being right:
//!
//! * **an object comes back as text.** [`crate::broker::run_curl`] hands back a
//!   `String`, and every reply in this service travels to the caller as one. A
//!   read of a PNG therefore arrives lossily converted. Text objects — a SQL
//!   dump, a JSON manifest, a log — are exact, and those are what an agent has
//!   any business reading through a broker;
//! * **an object key is a [`apex_secret_core::operation::Syntax::Path`]**, so
//!   `backups/2026-09-12.sql` can be named and `backups/2026-09-12T10:00.sql`
//!   cannot: `:` is refused by the grammar because a resource that could carry
//!   one could be read as a URL. The grammar is load-bearing and the key
//!   restriction is the price;
//! * **ten megabytes**, [`MAX_PAYLOAD`], against the documented endpoint's 300.
//!   A backup larger than that is not an agent operation.
//!
//! §13.5 also calls R2 "the preferred first-party cloud target for encrypted
//! APEX backups". That is P1-010's, and the object write below is the half of
//! it that does not need a backup format to exist first.
//!
//! ## What has never run against Cloudflare
//!
//! All of it. There is no account and no token on this machine, so every
//! operation below has been exercised against a loopback server that speaks the
//! same envelope and refuses without an `Authorization` header, and none of it
//! against `api.cloudflare.com`. The paths are the documented ones — read out
//! of `cloudflare/api-schemas`' `openapi.json` at `d5003a19`, and cross-checked
//! against the rendered reference — and that they are the documented ones is
//! not the same as having called them.

pub mod api;
pub mod binding;

use apex_secret_core::operation::{
    self, Effect, OperationSpec, ParamSpec, ProviderSpec, ResourceKind, Syntax,
};
use apex_secret_core::project::{self, MAX_PAYLOAD};
use apex_secret_core::SecretValue;

use crate::provider::{Bind, Bound, Endpoint, Performed, Provider, ProviderError};

use api::{Api, Body, Call, Multipart};
use binding::{Account, Binding, BindingError, Bucket, Worker, Zone};

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

/// The bucket, database, namespace or object a caller names as a path within
/// something the project bound. `example-assets/backups/db.sql`.
const WITHIN: ResourceKind = ResourceKind::Path;

/// `file`, a path inside the project whose bytes are the request.
///
/// The counterpart to §13.4's rule about credentials, for payloads: the bytes
/// an agent uploads come out of the project it is working in, read by the
/// daemon under [`project::read_file`]'s rules, rather than travelling through
/// the protocol as a parameter. A `Syntax::Path` cannot name `/etc/shadow`,
/// and `read_file` would refuse it anyway — it is not the caller's file.
const FILE: ParamSpec = ParamSpec {
    name: "file",
    syntax: Syntax::Path,
    required: true,
    summary: "the file inside the project whose bytes to send",
};

/// The vocabulary, in §13.2's shape.
///
/// Ten names: **nine of §13.2's thirty-two**, plus `worker.route.read`, which
/// is §13.3's "Worker routes" rather than one of §13.2's examples. So
/// **twenty-three** of §13.2's list are still unimplemented and belong to
/// P1-006 through P1-017 — D1, KV, Queues, Hyperdrive, DNS, Secrets Store,
/// Access, Tunnels, Workers AI and the AI gateway.
///
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
            // **The declaration that forced `same_everywhere` to be a field.**
            //
            // This operation names nothing — no resource, no parameters — and
            // it still resolves against the directory the caller is standing
            // in: `resolve` reads the project's own `apex.toml`, and answers
            // `GET /accounts/{id}` for the account that file binds, or
            // `GET /accounts` for every account the token can see when it
            // binds none. Two projects, two different requests, one stored
            // token. So `*` is refused, and `apex cf status` needs a grant in
            // the project it is run in.
            same_everywhere: false,
        },
        OperationSpec {
            id: "cloudflare.worker.read",
            summary: "read the settings of one of this project's workers",
            effect: Effect::Read,
            resource: NAMED,
            params: &[],
            aliases: &[],
            // Every other Cloudflare operation names a worker, which the
            // project's own `apex.toml` maps to an account and an
            // environment. Per project by construction.
            same_everywhere: false,
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
            same_everywhere: false,
        },
        OperationSpec {
            id: "cloudflare.worker.deploy",
            summary: "put an uploaded version of one of this project's workers \
                      in front of all its traffic",
            effect: Effect::Write,
            resource: NAMED,
            params: &[VERSION, MESSAGE],
            aliases: &[],
            same_everywhere: false,
        },
        OperationSpec {
            id: "cloudflare.worker.rollback",
            summary: "put an earlier version of one of this project's workers \
                      back in front of traffic",
            effect: Effect::Write,
            resource: NAMED,
            params: &[VERSION, MESSAGE],
            aliases: &[],
            same_everywhere: false,
        },
        OperationSpec {
            id: "cloudflare.worker.tail",
            summary: "start a log session for one of this project's workers",
            effect: Effect::Read,
            resource: NAMED,
            params: &[],
            aliases: &[],
            same_everywhere: false,
        },
        OperationSpec {
            id: "cloudflare.worker.route.read",
            summary: "read the routes this project's zone sends to its workers",
            effect: Effect::Read,
            resource: NAMED,
            params: &[],
            aliases: &[],
            same_everywhere: false,
        },
        // ── §13.5, R2 ───────────────────────────────────────────────────────
        OperationSpec {
            id: "cloudflare.r2.object.read",
            summary: "read one object out of one of this project's R2 buckets, \
                      or list what is in it",
            effect: Effect::Read,
            // `example-assets` lists that bucket; `example-assets/db/today.sql`
            // reads that object. One operation and not two, because §13.2
            // names one and because the grant an owner gives is the same
            // either way: this project's buckets, read.
            resource: WITHIN,
            params: &[],
            aliases: &[],
            same_everywhere: false,
        },
        OperationSpec {
            id: "cloudflare.r2.object.write",
            summary: "put a file from this project into one of its R2 buckets",
            effect: Effect::Write,
            resource: WITHIN,
            params: &[FILE],
            aliases: &[],
            same_everywhere: false,
        },
        OperationSpec {
            id: "cloudflare.r2.bucket.create",
            summary: "create one of the R2 buckets this project declares, in \
                      the account it is bound to",
            effect: Effect::Write,
            // A NAME, and one the project already lists under `buckets`. An
            // agent granted this cannot create a bucket the owner never wrote
            // down — the grant says which verb, and §13.1's file says which
            // thing, including a thing that does not exist yet.
            resource: NAMED,
            params: &[
                ParamSpec {
                    name: "location",
                    syntax: Syntax::Name,
                    required: false,
                    summary: "the region hint to create it in: apac, eeur, enam, oc, weur or wnam",
                },
                ParamSpec {
                    name: "storage-class",
                    syntax: Syntax::Name,
                    required: false,
                    summary: "Standard or InfrequentAccess",
                },
            ],
            aliases: &[],
            same_everywhere: false,
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
    /// A bucket this project declares. It is a target before it exists —
    /// `r2.bucket.create` is what makes it — which is why the project file
    /// listing it is what decides the name and not the other way round.
    Bucket(Bucket),
    /// An object in one of this project's buckets, or the bucket's own listing
    /// when the caller named no key.
    Object { bucket: Bucket, key: Option<String> },
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
            // Everything else is a file that is wrong rather than a name that
            // is not bound — a missing id, an id that is not one, a project
            // with no Cloudflare section at all.
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

        // Which noun the operation acts on decides which resolver runs, and
        // getting that wrong is not a silent bug: a build that resolved every
        // resource as a worker would answer "this project does not bind a
        // worker called 'example-assets'" for a bucket — a true sentence about
        // the wrong question, and the reader would go and add an environment.
        match req.operation.id {
            "cloudflare.r2.bucket.create" => {
                return Ok(Target::Bucket(binding.bucket(req.resource)?));
            }
            "cloudflare.r2.object.read" | "cloudflare.r2.object.write" => {
                let (bucket, key) = split_first(req.resource);
                return Ok(Target::Object {
                    bucket: binding.bucket(bucket)?,
                    key,
                });
            }
            _ => {}
        }

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
            Target::Bucket(bucket) => format!(
                "create the bucket {} in account {} [{}]",
                bucket.name,
                bucket.account.named(),
                bucket.account.id
            ),
            Target::Object { bucket, key } => {
                let where_ = format!(
                    "bucket {} in account {} [{}]",
                    bucket.name,
                    bucket.account.named(),
                    bucket.account.id
                );
                match (operation.id, key) {
                    ("cloudflare.r2.object.write", Some(key)) => {
                        // The file is in the sentence because it decides what
                        // is uploaded, and an owner reading the trail afterwards
                        // wants to know what went into the bucket.
                        let file = params.get("file").map(String::as_str).unwrap_or("");
                        format!("write {file} to object {key} in {where_}")
                    }
                    (_, Some(key)) => format!("read object {key} in {where_}"),
                    (_, None) => format!("list the objects in {where_}"),
                }
            }
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
            ("cloudflare.r2.object.read", Target::Object { bucket, key }) => {
                get(objects(bucket, key.as_deref()))
            }
            ("cloudflare.r2.object.write", Target::Object { bucket, key }) => {
                let Some(key) = key else {
                    return Err(ProviderError::Refused(format!(
                        "'{}' names a bucket and not an object. Writing needs a \
                         key as well: {}/<key>",
                        bucket.name, bucket.name
                    )));
                };
                Call {
                    method: "PUT",
                    path: objects(bucket, Some(key)),
                    body: self.object_body(req, key)?,
                }
            }
            ("cloudflare.r2.bucket.create", Target::Bucket(bucket)) => Call {
                method: "POST",
                path: format!("/accounts/{}/r2/buckets", bucket.account.id),
                body: bucket_body(req, bucket)?,
            },
            (id, _) => {
                return Err(ProviderError::Failed(format!(
                    "the cloudflare provider declares '{id}' and does not implement it"
                )))
            }
        })
    }

    /// The bytes of a project file, as the body of an R2 upload.
    ///
    /// Same read as a Worker module's — [`project::read_file`]'s `O_NOFOLLOW`
    /// walk, owned by the caller, capped — because it is the same problem: a
    /// root daemon opening a path a caller chose. What is different is that
    /// nothing here looks at the contents. An R2 object is whatever the project
    /// put in it.
    fn object_body(&self, req: &Bind<'_>, key: &str) -> Result<Body, ProviderError> {
        let Some(file) = req.params.get("file") else {
            return Err(ProviderError::Refused(
                "this operation needs a 'file' option naming the file inside \
                 the project to upload"
                    .to_string(),
            ));
        };
        let bytes = project::read_file(
            std::path::Path::new(req.project),
            file,
            req.owner.uid,
            &req.owner.name,
            MAX_PAYLOAD,
        )
        .map_err(|e| ProviderError::NoSuchResource(e.to_string()))?;
        Ok(Body::Raw {
            content_type: media_type(key),
            bytes,
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

/// The first segment of a resource, and whatever is left.
///
/// `example-assets/db/today.sql` is a bucket and a key, and the key keeps its
/// slashes because R2 object keys have them. The framework has already held
/// the whole string to [`apex_secret_core::operation::valid_path`] — no
/// leading or trailing `/`, no `//`, no `..`, no `:` — so the split cannot
/// produce an empty bucket or a key that climbs.
fn split_first(resource: &str) -> (&str, Option<String>) {
    match resource.split_once('/') {
        Some((first, rest)) => (first, Some(rest.to_string())),
        None => (resource, None),
    }
}

/// The path for one bucket's objects, or for one object in it.
fn objects(bucket: &Bucket, key: Option<&str>) -> String {
    let base = format!(
        "/accounts/{}/r2/buckets/{}/objects",
        bucket.account.id, bucket.name
    );
    match key {
        Some(key) => format!("{base}/{key}"),
        None => base,
    }
}

/// What to call an object's bytes, from the name the caller gave it.
///
/// A `&'static str` out of a fixed table rather than anything the caller sends,
/// because this ends up in a header. The default is deliberately the useless
/// one: an unknown extension is bytes, not a guess.
fn media_type(key: &str) -> &'static str {
    let extension = key.rsplit_once('.').map(|(_, e)| e).unwrap_or("");
    match extension {
        "json" | "map" => "application/json",
        "txt" | "log" => "text/plain; charset=utf-8",
        "md" => "text/markdown; charset=utf-8",
        "csv" => "text/csv; charset=utf-8",
        "html" => "text/html; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "js" | "mjs" => "text/javascript; charset=utf-8",
        "xml" => "application/xml",
        "sql" => "application/sql",
        "wasm" => "application/wasm",
        "pdf" => "application/pdf",
        "zip" => "application/zip",
        "gz" | "tgz" => "application/gzip",
        "tar" => "application/x-tar",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "svg" => "image/svg+xml",
        "ico" => "image/vnd.microsoft.icon",
        "woff2" => "font/woff2",
        _ => "application/octet-stream",
    }
}

/// The JSON body that creates a bucket.
///
/// Both options are closed sets in Cloudflare's own documentation, so a value
/// outside them is refused here rather than sent and refused there. The reason
/// is not politeness: a request that fails at the far side has already spent
/// the credential, and the reply an owner reads then describes an API error
/// instead of a typo.
fn bucket_body(req: &Bind<'_>, bucket: &Bucket) -> Result<Body, ProviderError> {
    const LOCATIONS: &[&str] = &["apac", "eeur", "enam", "oc", "weur", "wnam"];
    const CLASSES: &[&str] = &["Standard", "InfrequentAccess"];

    let mut body = serde_json::Map::new();
    body.insert("name".into(), bucket.name.clone().into());
    if let Some(location) = req.params.get("location") {
        if !LOCATIONS.contains(&location.as_str()) {
            return Err(ProviderError::Refused(format!(
                "'{}' is not an R2 location hint. One of: {}",
                location.escape_debug(),
                LOCATIONS.join(", ")
            )));
        }
        body.insert("locationHint".into(), location.clone().into());
    }
    if let Some(class) = req.params.get("storage-class") {
        if !CLASSES.contains(&class.as_str()) {
            return Err(ProviderError::Refused(format!(
                "'{}' is not an R2 storage class. One of: {}",
                class.escape_debug(),
                CLASSES.join(", ")
            )));
        }
        body.insert("storageClass".into(), class.clone().into());
    }
    Ok(Body::Json(serde_json::Value::Object(body).to_string()))
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

/// The routes in a zone that point at one worker.
///
/// `GET /zones/{id}/workers/routes` answers with every route in the zone, and
/// the caller named a worker. Returning the lot would answer a question nobody
/// asked and would put the rest of the zone's routing in front of an agent
/// scoped to one worker — and the audit line would say "the routes zone X
/// sends to Y" about a list that is not that.
fn only_this_worker_s_routes(body: &str, worker: &str) -> String {
    let Ok(mut value) = serde_json::from_str::<serde_json::Value>(body) else {
        return body.to_string();
    };
    let Some(routes) = value.get_mut("result").and_then(|r| r.as_array_mut()) else {
        return body.to_string();
    };
    routes.retain(|route| route.get("script").and_then(|s| s.as_str()) == Some(worker));
    serde_json::to_string(&value).unwrap_or_else(|_| body.to_string())
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
        let body = match req.operation.id {
            "cloudflare.worker.tail" if reply.ok() => without_the_tail_url(&reply.body),
            "cloudflare.worker.route.read" if reply.ok() => {
                let name = match &target {
                    Target::Route { worker, .. } => worker.name.as_str(),
                    _ => "",
                };
                only_this_worker_s_routes(&reply.body, name)
            }
            _ => reply.body.clone(),
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
