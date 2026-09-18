//! `apex cloudflare` — connecting an account, and seeing what a project binds.
//!
//! ```text
//! apex cf connect            # OAuth, by device code, with no browser here
//! apex cf connect --token    # paste a scoped token instead; stdin only
//! apex cf status             # what is stored, and what this project binds
//! apex cf refresh            # renew the access token without signing in again
//! ```
//!
//! §13.1 asks for exactly these three things: an OAuth or scoped-token
//! connection path, a manual token fallback, and a project that binds the
//! account, zone and environment it means.
//!
//! ## This is a CLI change, and P1-002's was not
//!
//! P1-001 claimed a provider could be added without touching `apex-agent-core`,
//! `apex-agentd` or this CLI, and P1-002 did not touch any of them. This file
//! is not a counter-example to that: `apex cf connect` is not how a capability
//! is performed — that is still `apex secret use`, which knows no provider
//! names. It is how a credential gets *into* the broker in the first place,
//! which is a thing a person does once, at a terminal, and it needs a verb
//! somebody can find. `apex secret add cloudflare --host api.cloudflare.com`
//! already does the manual half; what it cannot do is run an OAuth flow or tell
//! you whether this project's `apex.toml` says anything sensible.
//!
//! ## No browser, and no keyring
//!
//! The device grant (RFC 8628) is the only Cloudflare flow that fits: the other
//! one runs a callback server on `localhost:8976` and needs a browser on this
//! machine to reach it. Here the URL and the code are printed and somebody
//! types them wherever they already have a browser. **Nothing is launched.**
//!
//! The token lands in `apex-secretd`'s root-owned store, never in a dotfile.
//! Wrangler writes `~/.wrangler/config/default.toml`, or a keyring; both are
//! readable by the agent, which is the entire thing P0-002 exists to stop.
//!
//! ## Whose OAuth client this is
//!
//! The default `client_id` is Wrangler's, `54d11594-…`, read from
//! `workers-sdk`'s own source rather than guessed. Every third-party tool that
//! speaks this flow does the same, because the alternative is registering an
//! OAuth application, which needs a Cloudflare account nobody here has. The
//! consequence is honest and printed: the consent screen says Wrangler, and the
//! grant a person approves is Wrangler's. `--client-id` overrides it, and
//! whether APEX should have its own registration is a decision for the person
//! with the account.

use std::io::{Read, Write};

use anyhow::{bail, Context, Result};
use apex_secret_core::account::CLOUDFLARE_OAUTH;
use apex_secret_core::capability::CapabilityRecord;
use apex_secret_core::client::Client;
use apex_secret_core::project::ProjectConfig;
use apex_secret_core::protocol::{Request, Response};
use apex_secret_core::store::Grants;
use apex_secret_core::SecretValue;
use clap::Subcommand;

use crate::oauth_device::{
    self, device_grant, roughly, valid_token, Endpoints, OAuthClient, Prompt,
};

/// The service name the credential is stored under, and the one the provider's
/// operations are granted against.
pub const SERVICE: &str = "cloudflare";

/// Where the refresh token goes.
///
/// A second service, on the host that issued it. That is not tidiness: the
/// framework pins a credential to the host it was stored for, so a refresh
/// token filed under `dash.cloudflare.com` **cannot be spent as an API token**
/// against `api.cloudflare.com`, whatever asks for it.
pub const REFRESH_SERVICE: &str = "cloudflare-refresh";

/// The operation that spends it, read off the crate the daemon declares it in.
///
/// Not a string spelled here. `apex-secretd`'s `oauth` provider declares its
/// one operation with this same constant, so `apex cf refresh` cannot ask for
/// an operation no provider offers.
pub const REFRESH_OPERATION: &str = apex_secret_core::account::REFRESH_OPERATION;

/// Cloudflare's API host — where the access token is valid.
pub const API_HOST: &str = "api.cloudflare.com";

/// The host that issues and refreshes tokens.
///
/// Read off the shared table rather than spelled again. `apex-secretd` finds a
/// token endpoint by looking up the host a credential is PINNED to, so this
/// string and `CLOUDFLARE_OAUTH.auth_host` are the same fact — and if they
/// drifted, the CLI would file the refresh token under a host the daemon's
/// lookup does not know, which reads as "there is no refresh for this".
pub const AUTH_HOST: &str = CLOUDFLARE_OAUTH.auth_host;

/// Wrangler's OAuth client. See the module note for why it is Wrangler's.
///
/// RFC 6749 §6 requires a refresh to present the same `client_id` the grant
/// was issued to, and the refresh runs in the daemon while the grant runs
/// here. One constant, in the crate both link.
pub const WRANGLER_CLIENT_ID: &str = apex_secret_core::account::WRANGLER_CLIENT_ID;

/// The scopes asked for, and no more.
///
/// One per operation this build's provider can actually perform, which is the
/// §13.5 rule — *no broad token when narrower scope is possible* — applied at
/// the only moment it can be: a token's scopes are fixed when it is issued, so
/// asking for the whole list here would make every later grant a formality.
///
/// Two deliberate omissions. `workers_routes:write` is the only route scope
/// Cloudflare defines, and `cloudflare.worker.route.read` only reads — so this
/// asks for `zone:read` and lets a route read fail with Cloudflare's own
/// message rather than holding a write scope for a read operation. And nothing
/// for DNS, R2, D1, KV or Access, because no operation in this build touches
/// them; P1-005 onwards each add their own.
pub(crate) const SCOPES: &[&str] = &[
    "account:read",
    "user:read",
    "workers:read",
    "workers_scripts:write",
    "workers_deployments:read",
    "workers_tail:read",
    "zone:read",
    // Without this there is no refresh token, and the access token simply
    // stops working with nothing to do about it but connect again.
    "offline_access",
];

pub mod preview;

/// `apex cloudflare <verb>`.
#[derive(Subcommand)]
pub enum CloudflareCmd {
    /// Connect a Cloudflare account to this machine's broker.
    ///
    /// Prints a URL and a short code. Open them on any device you already have
    /// a browser on; nothing is launched here. The token is stored by
    /// `apex-secretd`, which no agent can read.
    Connect {
        /// Paste a scoped API token instead, read from stdin.
        ///
        /// Use this when you would rather make a token in the dashboard and
        /// scope it by hand. `printf %s "$TOKEN" | apex cf connect --token`.
        #[arg(long)]
        token: bool,
        /// The OAuth client to ask as. Defaults to Wrangler's.
        #[arg(long, value_name = "UUID")]
        client_id: Option<String>,
    },
    /// What is connected, and what this project binds.
    Status {
        #[arg(long)]
        json: bool,
    },
    /// Renew the stored access token with the refresh token beside it.
    ///
    /// The refresh runs in `apex-secretd`: nothing here ever sees either
    /// token, and the request names no host, no resource and no parameters —
    /// where it goes is decided by the host the refresh token was pinned to
    /// when `apex cf connect` stored it.
    ///
    /// It is a capability, so it is granted per project like every other one.
    Refresh,
    /// §13.13: what this worktree owns at Cloudflare, and how to let it go.
    ///
    /// Ownership is read out of the audit trail rather than a manifest, so it
    /// describes what this worktree actually did. `apex cf preview plan` shows
    /// it; `apex cf preview destroy` shows the same plan and destroys nothing
    /// until you add `--yes`.
    Preview {
        #[command(subcommand)]
        cmd: PreviewCmd,
    },
}

/// `apex cloudflare preview <verb>`.
#[derive(Subcommand)]
pub enum PreviewCmd {
    /// What this worktree owns, and what destroying it would and would not do.
    Plan {
        #[arg(long)]
        json: bool,
    },
    /// Destroy what this build can destroy, after printing the plan.
    ///
    /// Prints the plan and stops unless `--yes` is given. That is the whole of
    /// §13.13's *"after showing a clear destroy plan"*, and it is a flag rather
    /// than a prompt because this has to work in a session nobody is watching.
    Destroy {
        /// Actually destroy. Without it nothing is touched.
        #[arg(long)]
        yes: bool,
        #[arg(long)]
        json: bool,
    },
}

pub fn main(cmd: CloudflareCmd) -> i32 {
    let result = match cmd {
        CloudflareCmd::Connect { token, client_id } => {
            if token {
                connect_with_a_pasted_token()
            } else {
                connect_by_device_code(
                    &Endpoints::cloudflare(),
                    client_id.as_deref().unwrap_or(WRANGLER_CLIENT_ID),
                    &mut std::io::stdout(),
                )
            }
        }
        CloudflareCmd::Status { json } => status(json),
        CloudflareCmd::Refresh => refresh(),
        CloudflareCmd::Preview { cmd } => match cmd {
            PreviewCmd::Plan { json } => preview_plan(json),
            PreviewCmd::Destroy { yes, json } => preview_destroy(yes, json),
        },
    };
    match result {
        Ok(code) => code,
        Err(e) => {
            eprintln!("apex cloudflare: {e:#}");
            1
        }
    }
}

// ── the manual half ─────────────────────────────────────────────────────────

fn connect_with_a_pasted_token() -> Result<i32> {
    let mut value = String::new();
    std::io::stdin().read_to_string(&mut value)?;
    let value = value.trim();
    if value.is_empty() {
        bail!(
            "nothing on stdin. Pipe the token in, so it never reaches a command \
             line:\n  printf %s \"$TOKEN\" | apex cf connect --token"
        );
    }
    if !valid_token(value) {
        bail!(
            "that does not look like a Cloudflare API token. One is a run of \
             letters, digits and `-` `_` `.`; paste the token on its own, not \
             the whole `Authorization:` line"
        );
    }
    // A pasted token is a dashboard-scoped API token, not an OAuth grant.
    // There is no client to record and no refresh token beside it.
    store(
        SERVICE,
        API_HOST,
        "https",
        username_for(SERVICE, WRANGLER_CLIENT_ID),
        value,
    )?;
    println!("stored a Cloudflare token for {API_HOST}");
    whats_next();
    Ok(0)
}

/// The non-secret half each of the two credentials is filed with.
///
/// The access token has none, and the store's placeholder says so. The refresh
/// token's is **the OAuth client the grant was issued to**, because RFC 6749
/// §6 requires a refresh to present the same client — and `--client-id` lets
/// somebody sign in as a client that is not Wrangler's. Recording it is the
/// whole of making that work: the daemon reads
/// `providers::oauth::OAuthProvider::client_id` out of exactly this field and
/// falls back to the table's default only when it holds the placeholder. A
/// build that stored the placeholder here would renew an overridden grant as
/// Wrangler and get `invalid_client` from Cloudflare with nothing to explain
/// it.
fn username_for<'a>(service: &str, client_id: &'a str) -> &'a str {
    if service == REFRESH_SERVICE {
        client_id
    } else {
        apex_secret_core::store::DEFAULT_USERNAME
    }
}

fn store(service: &str, host: &str, scheme: &str, username: &str, value: &str) -> Result<()> {
    Client::connect()
        .context("apex-secretd is not answering; is it running?")?
        .add(
            service,
            host,
            scheme,
            Some(username),
            "",
            // A Cloudflare API token is presented as `Authorization: Bearer
            // <token>`, which is what the provider builds and what `bearer`
            // means to `EndpointRecord::header_value`. `raw` would store the
            // token as a whole header value and send it unprefixed.
            "bearer",
            None,
            &SecretValue::new(value.as_bytes().to_vec()),
        )?;
    Ok(())
}

fn whats_next() {
    println!("nothing is allowed yet. Bind this project to an account:");
    println!("  apex cf status");
    println!("then allow an operation for it:");
    println!("  apex secret grant {SERVICE} cloudflare.worker.deploy");
}

impl Endpoints {
    /// Cloudflare's, read out of workers-sdk rather than reconstructed and now
    /// taken off the shared table rather than rebuilt from a host: the device
    /// endpoint has to live on the same auth domain as the token endpoint it is
    /// paired with, and `Provider::validate` is what holds the table to that.
    pub fn cloudflare() -> Endpoints {
        Endpoints::for_oauth(&CLOUDFLARE_OAUTH)
    }
}

/// What the device grant prints and what it tells somebody to run again.
///
/// Cloudflare's half of a flow that is now shared. `apex cf connect` rather
/// than `apex cloudflare connect` because that is the spelling the rest of
/// this file's output uses.
fn prompt() -> Prompt<'static> {
    Prompt {
        label: "Cloudflare",
        retry: "apex cf connect",
    }
}

/// Everything the shared transport needs in order to run as Cloudflare.
///
/// One function rather than four arguments written out at the call site,
/// because the call site cannot be tested — `connect_by_device_code` stores
/// what it gets, and storing needs the daemon — while this can. Before the
/// transport moved, the scope list was asserted off the wire; now the wire
/// belongs to `oauth_device`'s double and this is where Cloudflare's own
/// inputs are pinned.
fn grant_inputs(client_id: &str) -> (OAuthClient<'_>, &'static [&'static str], Prompt<'static>) {
    (
        // No `client_secret`: Cloudflare's device flow is a public client, and
        // `CLOUDFLARE_OAUTH.client_secret` is the `ClientSecret::None` that
        // says so.
        OAuthClient {
            id: client_id,
            secret: None,
        },
        SCOPES,
        prompt(),
    )
}

/// Run the device grant and store what it produces.
pub fn connect_by_device_code(
    ends: &Endpoints,
    client_id: &str,
    out: &mut impl Write,
) -> Result<i32> {
    if !oauth_device::valid_form_value(client_id) {
        bail!("'{}' is not an OAuth client id", client_id.escape_debug());
    }
    let (client, scopes, prompt) = grant_inputs(client_id);
    let grant = device_grant(ends, &client, scopes, &prompt, out)?;

    store(
        SERVICE,
        API_HOST,
        "https",
        username_for(SERVICE, client_id),
        &grant.access_token,
    )?;
    let mut stored = format!("stored a Cloudflare token for {API_HOST}");
    if let Some(refresh) = &grant.refresh_token {
        // Filed against the host that issued it, so the host pin stops it ever
        // being sent to the API as though it were an access token — and with
        // the client the grant was issued to in the non-secret half, which is
        // where the daemon's refresh reads it from.
        store(
            REFRESH_SERVICE,
            AUTH_HOST,
            "https",
            username_for(REFRESH_SERVICE, client_id),
            refresh,
        )?;
        stored.push_str(&format!(", and its refresh token for {AUTH_HOST}"));
    }
    writeln!(out, "{stored}")?;
    if let Some(seconds) = grant.expires_in {
        writeln!(out, "it stops working in about {}.", roughly(seconds))?;
        // What to do about it, which is now a thing rather than "connect
        // again". Said with the grant it needs, because a refresh is a
        // capability like any other and is not granted by connecting.
        if grant.refresh_token.is_some() {
            writeln!(
                out,
                "renew it before then with `apex cf refresh`, once per project:"
            )?;
            writeln!(out, "  apex secret grant {REFRESH_SERVICE} {REFRESH_OPERATION}")?;
            writeln!(out, "  apex cf refresh")?;
        } else {
            // No `offline_access` in the grant, so there is nothing to spend.
            writeln!(
                out,
                "this grant carries no refresh token — run `apex cf connect` \
                 again when it expires."
            )?;
        }
    }
    if let Some(scope) = &grant.scope {
        writeln!(out, "scopes: {scope}")?;
    }
    whats_next();
    Ok(0)
}

// ── status ──────────────────────────────────────────────────────────────────

/// The project this command acts for: the worktree you are standing in.
///
/// `git rev-parse --show-toplevel` inside a worktree answers the worktree, not
/// the main tree — which is what makes "this worktree's resources" a question
/// the audit trail can answer at all, and is also why grants are already
/// per-worktree.
fn here() -> Result<String> {
    let cwd = std::env::current_dir().context("reading the current directory")?;
    let project = apex_agent_core::project::detect(&cwd).ok_or_else(|| {
        anyhow::anyhow!(
            "this directory is not inside a git repository, and a Cloudflare \
             resource is owned by a project"
        )
    })?;
    Ok(project.root)
}

fn preview_plan(json: bool) -> Result<i32> {
    let plan = preview::plan(&here()?)?;
    if json {
        println!("{}", serde_json::to_string_pretty(&preview::to_json(&plan))?);
    } else {
        preview::print(&plan);
    }
    Ok(0)
}

fn preview_destroy(yes: bool, json: bool) -> Result<i32> {
    let plan = preview::plan(&here()?)?;
    if json {
        println!("{}", serde_json::to_string_pretty(&preview::to_json(&plan))?);
    } else {
        preview::print(&plan);
    }
    if !yes {
        if !json {
            println!();
            println!("Nothing was destroyed. Run the same command with --yes to do it.");
        }
        return Ok(0);
    }
    let failed = preview::destroy(&plan)?;
    if failed > 0 {
        eprintln!(
            "apex cf preview: {failed} thing{} could not be destroyed and {} still there",
            if failed == 1 { "" } else { "s" },
            if failed == 1 { "is" } else { "are" }
        );
        return Ok(1);
    }
    Ok(0)
}

/// The project a capability would be granted for, if this is inside one.
///
/// The same answer `apex cf status` prints, so a refusal that names a project
/// and the line that told you which project you were in cannot disagree.
fn project_root() -> Option<String> {
    std::env::current_dir()
        .ok()
        .and_then(|cwd| apex_agent_core::project::detect(&cwd))
        .map(|p| p.root)
}

/// `apex cf refresh` — RFC 6749 §6, performed in the daemon.
///
/// Nothing here sees either token. The request carries a service name, an
/// operation and a project and **nothing else**: `oauth.token.refresh`
/// declares no resource and no parameters, so there is no field on this
/// request that could aim a refresh at a host other than the one the refresh
/// token was pinned to.
///
/// A grant is required and is deliberately not written by `apex cf connect`.
/// Connecting stores a credential; deciding which project may spend it is the
/// owner's, and a grant silently keyed on whatever directory `connect`
/// happened to run in would be a capability nobody chose. So the refusal below
/// is a normal outcome the first time, and the daemon's own message already
/// spells the `apex secret grant` line that fixes it.
fn refresh() -> Result<i32> {
    let Some(root) = project_root() else {
        bail!(
            "this directory is not inside a git repository, and a capability is \
             granted per project. Run this from inside the project the account \
             is bound to."
        );
    };
    let mut record = CapabilityRecord::new(REFRESH_SERVICE, REFRESH_OPERATION, "");
    record.project = Some(root);
    let reply = Client::connect()
        .context("apex-secretd is not answering; is it running?")?
        .request(&Request::Use {
            record: Box::new(record),
            body_len: 0,
        })?;
    match reply {
        // A non-zero exit code is the authorisation server refusing, which is
        // not this command failing to run: the output carries `invalid_grant`
        // or whatever else it said, and that is the only thing that says
        // whether the grant was revoked or the token had already been rotated.
        Response::Performed {
            exit_code, output, ..
        } => {
            println!("{}", output.trim_end());
            if exit_code != 0 {
                println!("connect again to get a new one:");
                println!("  apex cf connect");
            }
            Ok(exit_code)
        }
        Response::Error { message, .. } => bail!("{message}"),
        other => bail!("the secret service answered a refresh with {}", other.variant()),
    }
}

/// What `apex cf status` can honestly say about renewing.
///
/// Four states rather than the bool the line used to print, because three of
/// them send the reader somewhere different and the one it printed was the
/// answer to a question nobody asked. `apex cf refresh` is a `Use`, a `Use`
/// needs a per-project grant, and **`apex cf connect` deliberately records
/// none** — connecting stores a credential; which project may spend it is the
/// owner's decision, and a grant keyed on whatever directory `connect` ran in
/// would be a capability nobody chose. So "a refresh token is stored" and
/// "this project may spend it" are different facts and the status line has to
/// carry both.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Renewal {
    /// The grant carried no `offline_access`, so there is nothing to spend.
    NoRefreshToken,
    /// A grant is per project and this directory is not in one, so no grant
    /// can match here and writing one would not help.
    NotInAProject,
    /// `apex cf refresh` will get as far as the authorisation server.
    Granted,
    /// It would be refused before the token is read.
    NotGranted,
}

/// Decide it with the daemon's own rule.
///
/// [`Grants::allows`] rather than a lookup written again here: it is what
/// `use_capability` will consult, including the [`apex_secret_core::store::
/// ANY_PROJECT`] fallback, and a second copy of that rule is a status line
/// that says "granted" about a request that is about to be refused.
fn renewal(refresh_stored: bool, project: Option<&str>, grants: &Grants) -> Renewal {
    if !refresh_stored {
        return Renewal::NoRefreshToken;
    }
    let Some(project) = project else {
        return Renewal::NotInAProject;
    };
    if grants.allows(Some(project), REFRESH_SERVICE, REFRESH_OPERATION) {
        Renewal::Granted
    } else {
        Renewal::NotGranted
    }
}

fn status(json: bool) -> Result<i32> {
    let mut client =
        Client::connect().context("apex-secretd is not answering; is it running?")?;
    let services = match client.call(&Request::List)? {
        Response::Services { services } => services,
        other => bail!("unexpected reply: {}", other.variant()),
    };
    // Asked of the daemon rather than assumed, because the answer changes what
    // the next line tells the reader to type.
    let granted = match client.call(&Request::Grants)? {
        Response::Grants { projects } => Grants { projects },
        other => bail!("unexpected reply: {}", other.variant()),
    };
    let stored = services.iter().find(|s| s.service == SERVICE);
    let refresh = services.iter().any(|s| s.service == REFRESH_SERVICE);

    // Display only. What a name MEANS is decided in the daemon, by the
    // provider, and this cannot reach that code — see the report.
    let project = project_root();
    let config = project.as_ref().and_then(|root| {
        ProjectConfig::read(
            std::path::Path::new(root),
            apex_secret_core::paths::uid(),
            "you",
        )
        .ok()
    });

    let renewal = renewal(refresh, project.as_deref(), &granted);

    if json {
        let mut out = serde_json::Map::new();
        out.insert("connected".into(), stored.is_some().into());
        out.insert("refreshable".into(), refresh.into());
        out.insert(
            "refresh_granted".into(),
            match renewal {
                Renewal::Granted => true.into(),
                Renewal::NotGranted => false.into(),
                // Neither of these is "not granted", and `false` here would
                // send a reader to write a grant that changes nothing: there
                // is no refresh token to spend, or no project to spend it in.
                Renewal::NoRefreshToken | Renewal::NotInAProject => serde_json::Value::Null,
            },
        );
        out.insert(
            "project".into(),
            project.clone().map(Into::into).unwrap_or(serde_json::Value::Null),
        );
        for (label, keys) in identity_keys() {
            let value = config
                .as_ref()
                .and_then(|c| c.string(keys).ok().flatten())
                .map(Into::into)
                .unwrap_or(serde_json::Value::Null);
            out.insert(label.into(), value);
        }
        out.insert(
            "environments".into(),
            config
                .as_ref()
                // An environment is a `[cloudflare.<name>]` that binds a
                // WORKER, which is what the human-readable output below has
                // always printed. The JSON said "every sub-table", and the two
                // disagreed the moment a project had a sub-table that was not
                // an environment — `[cloudflare.kv]` and its three siblings are
                // now exactly that, so `apex cf status --json` would have
                // reported a KV namespace table as an environment called `kv`.
                .map(|c| {
                    let named: Vec<String> = c
                        .sections(&["cloudflare"])
                        .into_iter()
                        .filter(|name| {
                            matches!(c.string(&["cloudflare", name, "worker"]), Ok(Some(_)))
                        })
                        .collect();
                    serde_json::json!(named)
                })
                .unwrap_or(serde_json::Value::Null),
        );
        println!("{}", serde_json::to_string_pretty(&out)?);
        return Ok(0);
    }

    match stored {
        Some(info) => {
            println!("credential: stored for {}://{}", info.scheme, info.host);
            match renewal {
                // Worth saying: without `offline_access` there is nothing to
                // renew, and the only way to get one is to connect again.
                Renewal::NoRefreshToken => {
                    println!("            no refresh token — this one cannot be renewed");
                }
                Renewal::Granted => {
                    println!("            a refresh token is stored too, for {AUTH_HOST}");
                    println!("            this project may spend it — renew without signing in again:");
                    println!("              apex cf refresh");
                }
                Renewal::NotGranted => {
                    println!("            a refresh token is stored too, for {AUTH_HOST}");
                    println!("            this project may NOT spend it yet. Connecting stored a");
                    println!("            credential; which project may renew with it is yours to say:");
                    println!("              apex secret grant {REFRESH_SERVICE} {REFRESH_OPERATION}");
                    println!("              apex cf refresh");
                }
                Renewal::NotInAProject => {
                    println!("            a refresh token is stored too, for {AUTH_HOST}");
                    println!("            renewing spends it, and that is granted per project — run");
                    println!("            `apex cf refresh` from inside the project that should renew");
                }
            }
        }
        None => {
            println!("credential: none. Connect one:");
            println!("              apex cf connect");
            println!("            or paste a scoped token:");
            println!("              printf %s \"$TOKEN\" | apex cf connect --token");
        }
    }

    let Some(root) = project else {
        println!("project:    this directory is not inside a git repository, and a");
        println!("            capability is granted per project");
        return Ok(0);
    };
    println!("project:    {root}");

    let Some(config) = config else {
        println!("binding:    no readable {root}/apex.toml");
        print_example();
        return Ok(0);
    };
    let mut bound = false;
    for (label, keys) in identity_keys() {
        if let Ok(Some(value)) = config.string(keys) {
            println!("{label:<12}{value}");
            bound = true;
        }
    }
    let environments = config.sections(&["cloudflare"]);
    for name in &environments {
        if let Ok(Some(worker)) = config.string(&["cloudflare", name, "worker"]) {
            // §13.8's opt-out, reported as the FILE states it and never
            // re-derived. The rule that decides which environments are
            // unattended without a line lives in the provider, inside the
            // daemon, and a second copy of it here would be a second copy of
            // it here — the defect this repository already has a name for.
            // What this can honestly say is what is written down.
            let opted_out = match config.boolean(&["cloudflare", name, "unattended"]) {
                Ok(Some(true)) => "  [unattended = true]",
                Ok(Some(false)) => "  [unattended = false]",
                Ok(None) => "",
                // A value that is not a boolean is a file the daemon will
                // refuse to read at all, so saying nothing here would hide the
                // reason every operation in this project is about to fail.
                Err(_) => "  [unattended is not true or false — the daemon will refuse this file]",
            };
            println!("{:<12}{worker}{opted_out}", format!("{name}:"));
            bound = true;
        }
    }
    if !bound {
        println!("binding:    {} says nothing about Cloudflare", config.path().display());
        print_example();
        return Ok(0);
    }
    if config.string(&["identity", "cloudflare", "account_id"]).ok().flatten().is_none() {
        println!();
        println!("`account_id` is not set, and the API is addressed by id rather than");
        println!("by name. The ids this credential can see:");
        println!("  apex secret grant {SERVICE} cloudflare.account.read");
        println!("  apex secret use {SERVICE} cloudflare.account.read");
    }
    Ok(0)
}

/// The single-valued keys `status` prints, with the label for each.
fn identity_keys() -> Vec<(&'static str, &'static [&'static str])> {
    vec![
        ("account:", &["identity", "cloudflare", "account"]),
        ("account_id:", &["identity", "cloudflare", "account_id"]),
        ("zone:", &["cloudflare", "zone"]),
        ("zone_id:", &["cloudflare", "zone_id"]),
    ]
}

fn print_example() {
    println!();
    println!("Bind this project to what it should deploy to, in apex.toml:");
    println!("  [identity.cloudflare]");
    println!("  account_id = \"…\"");
    println!();
    println!("  [cloudflare.production]");
    println!("  worker = \"my-worker\"");
}

#[cfg(test)]
mod tests;
