//! `apex cloudflare` — connecting an account, and seeing what a project binds.
//!
//! ```text
//! apex cf connect            # OAuth, by device code, with no browser here
//! apex cf connect --token    # paste a scoped token instead; stdin only
//! apex cf status             # what is stored, and what this project binds
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
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};
use apex_secret_core::client::Client;
use apex_secret_core::project::ProjectConfig;
use apex_secret_core::protocol::{Request, Response};
use apex_secret_core::SecretValue;
use clap::Subcommand;

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

/// Cloudflare's API host — where the access token is valid.
pub const API_HOST: &str = "api.cloudflare.com";

/// The host that issues and refreshes tokens.
pub const AUTH_HOST: &str = "dash.cloudflare.com";

/// Wrangler's OAuth client. See the module note for why it is Wrangler's.
pub const WRANGLER_CLIENT_ID: &str = "54d11594-84e4-41aa-b438-e81b8fa78ee7";

/// RFC 8628's grant type.
const DEVICE_GRANT: &str = "urn:ietf:params:oauth:grant-type:device_code";

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
const SCOPES: &[&str] = &[
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

/// Whether a pasted string is a Cloudflare token.
///
/// Checked here as well as in the daemon, because a token that is only refused
/// at use time is one somebody stored, granted, and then watched fail with a
/// message about the far side rather than about their paste. The commonest bad
/// paste is a trailing newline, which `trim` handles, and the second commonest
/// is the whole `Authorization: Bearer …` line, which this catches.
pub fn valid_token(token: &str) -> bool {
    !token.is_empty()
        && token.len() <= 4096
        && token.bytes().all(|b| {
            b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~' | b'+' | b'/' | b'=')
        })
}

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
    store(SERVICE, API_HOST, "https", value)?;
    println!("stored a Cloudflare token for {API_HOST}");
    whats_next();
    Ok(0)
}

fn store(service: &str, host: &str, scheme: &str, value: &str) -> Result<()> {
    Client::connect()
        .context("apex-secretd is not answering; is it running?")?
        .add(
            service,
            host,
            scheme,
            Some("x-access-token"),
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

// ── the device grant ────────────────────────────────────────────────────────

/// The two endpoints the flow uses.
///
/// A struct rather than constants so the poll loop can be run against a
/// loopback server that serves RFC 8628's states in order. Without that the
/// only way to exercise this code is to have a Cloudflare account.
#[derive(Debug, Clone)]
pub struct Endpoints {
    pub device: String,
    pub token: String,
    /// Shortest gap between polls. Cloudflare's `interval` wins when it is
    /// larger; a test sets this low so the loop does not take a minute.
    pub floor: Duration,
    /// How long to keep asking, whatever the server said.
    pub limit: Duration,
    /// What `slow_down` adds to the interval. RFC 8628 §3.5 says five seconds
    /// and that is what the real one uses; it is a field for the same reason
    /// `floor` is, so a test can exercise the branch without waiting out three
    /// real intervals to do it.
    pub backoff: Duration,
}

impl Endpoints {
    pub fn cloudflare() -> Endpoints {
        Endpoints {
            // Read out of workers-sdk rather than reconstructed: the device
            // endpoint has to live on the same auth domain as the token
            // endpoint it is paired with.
            device: format!("https://{AUTH_HOST}/oauth2/device/auth"),
            token: format!("https://{AUTH_HOST}/oauth2/token"),
            floor: Duration::from_secs(5),
            limit: Duration::from_secs(300),
            backoff: Duration::from_secs(5),
        }
    }
}

/// What the token endpoint gave back.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Grant {
    pub access_token: String,
    pub refresh_token: Option<String>,
    /// Seconds. Printed, because a token that expires this afternoon and a
    /// token that expires next year are different things to be handed.
    pub expires_in: Option<u64>,
    pub scope: Option<String>,
}

/// Why the flow stopped.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeviceError {
    /// The person said no.
    Denied,
    /// The code ran out before it was approved.
    Expired,
    /// The server said something RFC 8628 does not define, or nothing usable.
    Unusable(String),
    /// The endpoint could not be reached.
    Unreachable(String),
}

impl std::fmt::Display for DeviceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DeviceError::Denied => f.write_str(
                "the request was declined in the browser, so nothing was stored",
            ),
            DeviceError::Expired => f.write_str(
                "the code ran out before it was approved. Run `apex cf connect` \
                 again",
            ),
            DeviceError::Unusable(what) => write!(
                f,
                "the authorisation server answered with something this build \
                 cannot use: {what}"
            ),
            DeviceError::Unreachable(what) => {
                write!(f, "the authorisation server could not be reached: {what}")
            }
        }
    }
}

impl std::error::Error for DeviceError {}

/// Run the device grant and store what it produces.
pub fn connect_by_device_code(
    ends: &Endpoints,
    client_id: &str,
    out: &mut impl Write,
) -> Result<i32> {
    if !valid_form_value(client_id) {
        bail!("'{}' is not an OAuth client id", client_id.escape_debug());
    }
    let grant = device_grant(ends, client_id, out)?;

    store(SERVICE, API_HOST, "https", &grant.access_token)?;
    let mut stored = format!("stored a Cloudflare token for {API_HOST}");
    if let Some(refresh) = &grant.refresh_token {
        // Filed against the host that issued it, so the host pin stops it ever
        // being sent to the API as though it were an access token.
        store(REFRESH_SERVICE, AUTH_HOST, "https", refresh)?;
        stored.push_str(&format!(", and its refresh token for {AUTH_HOST}"));
    }
    writeln!(out, "{stored}")?;
    if let Some(seconds) = grant.expires_in {
        writeln!(out, "it stops working in about {}.", roughly(seconds))?;
        // Said plainly because it is the thing that will go wrong: nothing in
        // this build spends the refresh token.
        writeln!(
            out,
            "this build does not refresh it — run `apex cf connect` again when \
             it expires."
        )?;
    }
    if let Some(scope) = &grant.scope {
        writeln!(out, "scopes: {scope}")?;
    }
    whats_next();
    Ok(0)
}

/// How long to wait between polls.
///
/// RFC 8628 §3.5: the server's `interval` is honoured when it is given, and a
/// client that ignored it would be told `slow_down` for the rest of the flow.
/// The floor applies when the server said nothing, or said something smaller
/// than the floor — it is the only value under this build's control, so it is
/// where a test can make the loop run at a speed a test can wait for.
fn poll_interval(server: Option<u64>, floor: Duration) -> Duration {
    match server.map(Duration::from_secs) {
        Some(given) if given > floor => given,
        _ => floor,
    }
}

/// Seconds as something a person reads.
///
/// Plural handled rather than fudged with "(s)", and the boundaries are on the
/// units they name: 3600 seconds is an hour, not sixty minutes.
fn roughly(seconds: u64) -> String {
    let (count, unit) = match seconds {
        0..=90 => (seconds, "second"),
        91..=3599 => (seconds / 60, "minute"),
        3600..=172_799 => (seconds / 3600, "hour"),
        _ => (seconds / 86_400, "day"),
    };
    if count == 1 {
        format!("1 {unit}")
    } else {
        format!("{count} {unit}s")
    }
}

/// Ask for a code, print it, and poll until something decides.
pub fn device_grant(
    ends: &Endpoints,
    client_id: &str,
    out: &mut impl Write,
) -> Result<Grant, DeviceError> {
    let body = form(&[("client_id", client_id), ("scope", &SCOPES.join(" "))])
        .ok_or_else(|| DeviceError::Unusable("that client id cannot be sent".into()))?;
    let reply = post(&ends.device, &body).map_err(DeviceError::Unreachable)?;
    let json = parse(&reply.body)
        .ok_or_else(|| DeviceError::Unusable(describe(reply.status, &reply.body)))?;

    let device_code = text(&json, "device_code")
        .ok_or_else(|| DeviceError::Unusable(describe(reply.status, &reply.body)))?;
    let user_code = text(&json, "user_code")
        .ok_or_else(|| DeviceError::Unusable("no user code".into()))?;
    let uri = text(&json, "verification_uri")
        .ok_or_else(|| DeviceError::Unusable("no verification address".into()))?;
    let expires_in = number(&json, "expires_in").unwrap_or(300);
    let interval = poll_interval(number(&json, "interval"), ends.floor);

    // Printed, never opened. A browser here is the thing the whole flow exists
    // to avoid, and this machine may not have one.
    let _ = writeln!(out, "To connect Cloudflare, open this on any device:");
    let _ = writeln!(out, "  {uri}");
    let _ = writeln!(out, "and enter the code:");
    let _ = writeln!(out, "  {user_code}");
    if let Some(complete) = text(&json, "verification_uri_complete") {
        let _ = writeln!(out, "or open this, which fills the code in:");
        let _ = writeln!(out, "  {complete}");
    }
    let _ = writeln!(
        out,
        "waiting up to {} for you to approve it.",
        roughly(expires_in.min(ends.limit.as_secs()))
    );
    let _ = out.flush();

    poll(ends, client_id, &device_code, interval, expires_in)
}

/// RFC 8628 §3.4: ask the token endpoint until it stops saying "not yet".
fn poll(
    ends: &Endpoints,
    client_id: &str,
    device_code: &str,
    mut interval: Duration,
    expires_in: u64,
) -> Result<Grant, DeviceError> {
    let started = Instant::now();
    let deadline = ends.limit.min(Duration::from_secs(expires_in));
    let Some(body) = form(&[
        ("client_id", client_id),
        ("device_code", device_code),
        ("grant_type", DEVICE_GRANT),
    ]) else {
        // The device code came from the server, so a shape this cannot send is
        // the server's, not the caller's.
        return Err(DeviceError::Unusable(
            "the device code it issued is not a shape this build can send".into(),
        ));
    };

    loop {
        std::thread::sleep(interval);
        if started.elapsed() > deadline {
            return Err(DeviceError::Expired);
        }
        let reply = match post(&ends.token, &body) {
            Ok(reply) => reply,
            // A poll that could not be sent is not a decision. Keep going
            // until the deadline rather than failing a login because one
            // request lost a race with a sleeping wifi card.
            Err(_) => continue,
        };
        let Some(json) = parse(&reply.body) else {
            if reply.status >= 500 || reply.status == 429 {
                continue;
            }
            return Err(DeviceError::Unusable(describe(reply.status, &reply.body)));
        };

        // The error is checked BEFORE the token, because a body carrying both
        // is a body this build does not understand, and reading the token out
        // of it would be reading the half that suits us.
        if let Some(error) = text(&json, "error") {
            match error.as_str() {
                "authorization_pending" => continue,
                // §3.5: add five seconds and carry on, permanently.
                "slow_down" => {
                    interval += ends.backoff;
                    continue;
                }
                "access_denied" => return Err(DeviceError::Denied),
                "expired_token" => return Err(DeviceError::Expired),
                other => {
                    let detail = text(&json, "error_description").unwrap_or_else(|| other.to_string());
                    return Err(DeviceError::Unusable(detail));
                }
            }
        }

        if let Some(access_token) = text(&json, "access_token") {
            if !valid_token(&access_token) {
                return Err(DeviceError::Unusable(
                    "the token it issued is not a shape this build will send".into(),
                ));
            }
            return Ok(Grant {
                access_token,
                refresh_token: text(&json, "refresh_token").filter(|t| valid_token(t)),
                expires_in: number(&json, "expires_in"),
                scope: text(&json, "scope"),
            });
        }

        // Neither an error nor a token. A proxy or a WAF, most likely — and
        // falling through to the success path here is how a login page gets
        // stored as a credential.
        if reply.status >= 500 || reply.status == 429 {
            continue;
        }
        return Err(DeviceError::Unusable(describe(reply.status, &reply.body)));
    }
}

fn describe(status: u16, body: &str) -> String {
    let first: String = body.chars().take(120).filter(|c| !c.is_control()).collect();
    if first.trim().is_empty() {
        format!("HTTP {status}, with an empty body")
    } else {
        format!("HTTP {status}")
    }
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

fn status(json: bool) -> Result<i32> {
    let services = match Client::connect()
        .context("apex-secretd is not answering; is it running?")?
        .call(&Request::List)?
    {
        Response::Services { services } => services,
        other => bail!("unexpected reply: {}", other.variant()),
    };
    let stored = services.iter().find(|s| s.service == SERVICE);
    let refresh = services.iter().any(|s| s.service == REFRESH_SERVICE);

    // Display only. What a name MEANS is decided in the daemon, by the
    // provider, and this cannot reach that code — see the report.
    let project = std::env::current_dir()
        .ok()
        .and_then(|cwd| apex_agent_core::project::detect(&cwd))
        .map(|p| p.root);
    let config = project.as_ref().and_then(|root| {
        ProjectConfig::read(
            std::path::Path::new(root),
            apex_secret_core::paths::uid(),
            "you",
        )
        .ok()
    });

    if json {
        let mut out = serde_json::Map::new();
        out.insert("connected".into(), stored.is_some().into());
        out.insert("refreshable".into(), refresh.into());
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
            if refresh {
                println!("            a refresh token is stored too, and nothing spends it yet");
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

// ── http, the same shape the daemon uses ────────────────────────────────────

/// A reply from the authorisation server.
struct Reply {
    status: u16,
    body: String,
}

/// Whether a value can go in a form body without being escaped.
///
/// Everything this sends is a client id, a device code, a grant-type URN or a
/// space-separated scope list, all of which are already this shape. A value
/// that is not is refused rather than encoded, so there is no encoder here to
/// get wrong — and no `&` or `=` means no way to turn one field into two.
fn valid_form_value(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 4096
        && value.bytes().all(|b| {
            b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~' | b':' | b'/' | b' ')
        })
}

/// The same set once the pairs have been joined: `&` and `=` separate them and
/// `+` is the one substitution [`form`] makes.
fn valid_form_body(body: &str) -> bool {
    !body.is_empty()
        && body.len() <= 8192
        && body.bytes().all(|b| {
            b.is_ascii_alphanumeric()
                || matches!(b, b'-' | b'_' | b'.' | b'~' | b':' | b'/' | b'+' | b'&' | b'=')
        })
}

/// Build a form body, refusing any value that is not [`valid_form_value`].
///
/// `None` rather than an escaped body: a caller that produced an unexpected
/// value has a bug, and encoding around it would hide the bug and send the
/// request anyway.
fn form(pairs: &[(&str, &str)]) -> Option<String> {
    if pairs.iter().any(|(_, v)| !valid_form_value(v)) {
        return None;
    }
    Some(
        pairs
            .iter()
            .map(|(k, v)| format!("{k}={}", v.replace(' ', "+")))
            .collect::<Vec<_>>()
            .join("&"),
    )
}

/// POST a form, through a `curl` this process owns.
///
/// The same arrangement `apex-secretd` uses and for the same reasons: the whole
/// request goes down the child's stdin as a config file, so nothing
/// caller-shaped reaches an option parser, and `-q` first so a `~/.curlrc`
/// cannot choose a proxy for a request that carries an authorisation code.
/// It is forty lines copied rather than shared, because the daemon's copy is
/// inside a binary crate this cannot link.
fn post(url: &str, body: &str) -> Result<Reply, String> {
    if !valid_form_body(body) {
        return Err("that request cannot be sent as written".to_string());
    }
    // The scheme comes from the URL rather than being pinned to https, so a
    // loopback double is reachable. `Endpoints::cloudflare()` is https and
    // nothing but a test ever builds anything else.
    let scheme = url.split("://").next().unwrap_or("https");
    let config = format!(
        "url = \"{url}\"\nrequest = \"POST\"\nproto = \"={scheme}\"\n\
         header = \"Content-Type: application/x-www-form-urlencoded\"\n\
         header = \"Accept: application/json\"\n\
         header = \"Expect:\"\n\
         data = \"{body}\"\n\
         max-time = 30\nconnect-timeout = 15\nsilent\nshow-error\n\
         write-out = \"\\n%{{http_code}}\"\n"
    );

    let mut child = Command::new("/usr/bin/curl")
        .arg("-q")
        .arg("-K")
        .arg("-")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| e.to_string())?;
    if let Some(stdin) = child.stdin.as_mut() {
        stdin.write_all(config.as_bytes()).map_err(|e| e.to_string())?;
    }
    drop(child.stdin.take());
    let out = child.wait_with_output().map_err(|e| e.to_string())?;
    let stdout = String::from_utf8_lossy(&out.stdout);
    let (body, status) = match stdout.rsplit_once('\n') {
        Some((body, tail)) => (body.to_string(), tail.trim().parse::<u16>().ok()),
        None => (String::new(), None),
    };
    match status {
        Some(status) => Ok(Reply { status, body }),
        None => Err(String::from_utf8_lossy(&out.stderr).trim().to_string()),
    }
}

fn parse(body: &str) -> Option<serde_json::Value> {
    serde_json::from_str(body).ok()
}

fn text(value: &serde_json::Value, key: &str) -> Option<String> {
    value
        .get(key)
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

fn number(value: &serde_json::Value, key: &str) -> Option<u64> {
    value.get(key).and_then(|v| v.as_u64()).filter(|n| *n > 0)
}

/// Read a whole HTTP request off a stream and answer it. Test-only, but shaped
/// here so the double and the client agree on framing.
#[cfg(test)]
pub(crate) fn read_request(stream: &mut std::net::TcpStream) -> (String, String) {
    use std::io::{BufRead, BufReader};
    let mut reader = BufReader::new(stream.try_clone().expect("clone"));
    let mut first = String::new();
    let _ = reader.read_line(&mut first);
    let mut length = 0usize;
    loop {
        let mut line = String::new();
        match reader.read_line(&mut line) {
            Ok(0) => break,
            Ok(_) => {}
            Err(_) => break,
        }
        if line.trim().is_empty() {
            break;
        }
        if let Some(value) = line.trim().strip_prefix("Content-Length: ") {
            length = value.trim().parse().unwrap_or(0);
        }
    }
    let mut body = vec![0u8; length];
    if length > 0 {
        let _ = reader.read_exact(&mut body);
    }
    (first, String::from_utf8_lossy(&body).into_owned())
}

#[cfg(test)]
mod tests;
