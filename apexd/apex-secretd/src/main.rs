//! `apex-secretd` — the APEX protected secret service (roadmap §11).
//!
//! # Why this is a separate daemon
//!
//! §11 asks for a secret service "separate from both `apex-agentd` and broad
//! `apexd`", and the separation is not tidiness — it is the only thing that
//! makes the store mean anything.
//!
//! `apex-agentd` runs as the user, because it launches the user's own programs
//! and handles untrusted model output. That is right for what it does and fatal
//! for a credential store: a managed agent runs under the same uid, so anything
//! `apex-agentd` can read, the agent it started can read. A `0600` file in
//! `$XDG_STATE_HOME` is protected from other accounts, and other accounts are
//! not the threat.
//!
//! `apexd` is root and owns system policy, with a frozen D-Bus surface and
//! polkit actions. Putting credentials there would mean every credential
//! operation crossing a privileged interface, and a polkit prompt in the path
//! of an agent that nobody is watching.
//!
//! So: a third daemon, under its own system account, owning one directory no
//! human user can open. It is not privileged — it has no capabilities, touches
//! no device but the TPM, and can do exactly one thing no other process can,
//! which is read the store.
//!
//! # Two sockets
//!
//! ```text
//! /run/apex-secretd/broker.sock   0666  metadata, and performing operations
//! /run/apex-secretd/admin.sock    0600  storing, rotating, granting, revoking
//! ```
//!
//! The broker socket is open to every local uid because every local uid has its
//! own namespace in the store, keyed on `SO_PEERCRED`. Connecting as somebody
//! else is not possible, and connecting as yourself gets you your own (usually
//! empty) namespace.
//!
//! The admin socket is `0600` and owned by the daemon's account, so in practice
//! root. That is the boundary that makes a grant mean something: the owner
//! authenticates with `sudo` to widen what an agent may do, and the agent —
//! running under the owner's uid but not root — cannot.
//!
//! # What no socket does
//!
//! Return a credential. There is no verb for it, no response variant that could
//! carry one, and the type holding a value implements neither `Serialize` nor
//! `Display`. See `apex_secret_core::protocol`.

mod admin;
mod broker;
mod peer;

use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, Result};
use apex_secret_core::audit;
use apex_secret_core::paths::{self, Layout};
use apex_secret_core::policy::Grants;
use apex_secret_core::protocol::{
    ErrorKind, OperationInfo, ProviderInfo, Request, Response, MAX_LINE_BYTES, PROTOCOL_VERSION,
};
use apex_secret_core::provider::{self, PathShape};
use apex_secret_core::store;
use clap::Parser;

use crate::peer::Peer;

/// Which socket a connection arrived on.
///
/// Carried through dispatch rather than consulted from a global, so a handler
/// cannot forget which one it is answering. A mutating verb on the broker
/// socket is refused here and nowhere else.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Channel {
    Broker,
    Admin,
}

#[derive(Parser, Debug)]
#[command(
    name = "apex-secretd",
    about = "APEX protected secret service: performs credential-backed operations without handing out credentials",
    version
)]
struct Args {
    /// Where credentials, grants and audit logs live.
    ///
    /// Defaults to the packaged path. Overridable so the service can be run by
    /// an ordinary user against a temporary directory, which is how its tests
    /// drive it without root and without touching a running instance.
    #[arg(long, value_name = "DIR")]
    state_dir: Option<PathBuf>,

    /// Where the two sockets are created.
    #[arg(long, value_name = "DIR")]
    runtime_dir: Option<PathBuf>,

    /// Check the store and the sockets can be created, then exit.
    #[arg(long)]
    check: bool,
}

fn main() {
    if let Err(e) = run() {
        eprintln!("apex-secretd: {e:#}");
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    let args = Args::parse();
    // A client that hangs up mid-reply must not kill the daemon. Every write
    // site already handles the error.
    // Safe: setting a signal disposition before any thread exists.
    unsafe { libc::signal(libc::SIGPIPE, libc::SIG_IGN) };

    let default = Layout::default();
    let layout = Arc::new(Layout::new(
        args.state_dir
            .unwrap_or_else(|| default.state_dir().to_path_buf()),
        args.runtime_dir
            .unwrap_or_else(|| default.runtime_dir().to_path_buf()),
    ));

    paths::ensure_private_dir(layout.state_dir())
        .with_context(|| format!("preparing {}", layout.state_dir().display()))?;
    // The runtime directory holds the sockets and must be traversable by the
    // users who connect, so it is 0755 rather than 0700. The sockets inside
    // carry their own modes, which is where the boundary actually is.
    std::fs::create_dir_all(layout.runtime_dir())
        .with_context(|| format!("preparing {}", layout.runtime_dir().display()))?;
    set_mode(layout.runtime_dir(), 0o755)?;

    let broker = bind(&layout.broker_socket(), 0o666)?;
    let admin = bind(&layout.admin_socket(), 0o600)?;

    if args.check {
        eprintln!(
            "apex-secretd: store {} and sockets under {} are usable",
            layout.state_dir().display(),
            layout.runtime_dir().display()
        );
        return Ok(());
    }

    eprintln!(
        "apex-secretd: listening on {} and {} ({} providers, sealing {})",
        layout.broker_socket().display(),
        layout.admin_socket().display(),
        provider::PROVIDERS.len(),
        if apex_secret_core::seal::tpm2_available() {
            "available"
        } else {
            "unavailable, storing plain"
        }
    );

    // The admin listener runs on its own thread; the broker listener on this
    // one. Two threads rather than a poll loop because there are exactly two
    // listeners and the work is blocking I/O either way.
    {
        let layout = Arc::clone(&layout);
        std::thread::Builder::new()
            .name("apex-secretd-admin".into())
            .spawn(move || accept_loop(&layout, admin, Channel::Admin))
            .context("spawning the admin listener")?;
    }
    accept_loop(&layout, broker, Channel::Broker);
    Ok(())
}

fn set_mode(path: &Path, mode: u32) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode))
        .with_context(|| format!("setting the mode of {}", path.display()))
}

/// Bind a socket, replacing one left by a daemon that was killed.
///
/// Probing before unlinking is what stops this stealing the socket from a
/// daemon that is genuinely running — the same guard `apex-agentd` uses.
fn bind(path: &Path, mode: u32) -> Result<UnixListener> {
    if path.exists() {
        if UnixStream::connect(path).is_ok() {
            anyhow::bail!("another apex-secretd is already listening on {}", path.display());
        }
        std::fs::remove_file(path)
            .with_context(|| format!("removing the stale socket {}", path.display()))?;
    }
    let listener = UnixListener::bind(path)
        .with_context(|| format!("binding {}", path.display()))?;
    // Set after binding rather than through the umask, which the daemon
    // inherits and does not control. The admin socket's 0600 is a real boundary
    // and must not depend on inherited state.
    set_mode(path, mode)?;
    Ok(listener)
}

fn accept_loop(layout: &Arc<Layout>, listener: UnixListener, channel: Channel) {
    for stream in listener.incoming() {
        match stream {
            Ok(stream) => {
                let layout = Arc::clone(layout);
                if let Err(e) = std::thread::Builder::new()
                    .name(format!("apex-secretd-{channel:?}").to_lowercase())
                    .spawn(move || {
                        if let Err(e) = serve(&layout, stream, channel) {
                            eprintln!("apex-secretd: connection ended: {e:#}");
                        }
                    })
                {
                    eprintln!("apex-secretd: cannot spawn a connection thread: {e}");
                }
            }
            Err(e) => eprintln!("apex-secretd: accept failed: {e}"),
        }
    }
}

/// Serve one connection: newline-delimited JSON until the client goes away.
fn serve(layout: &Layout, stream: UnixStream, channel: Channel) -> Result<()> {
    // Read the peer credentials ONCE, from the accepted socket, before a single
    // request is parsed. The kernel filled them in at connect(2) and they
    // cannot change for the life of the connection — whereas anything read out
    // of a request line is whatever the client chose to send.
    let peer = Peer::pin(&stream);

    let mut reader = BufReader::new(stream.try_clone().context("cloning the connection")?);
    let mut writer = stream;

    loop {
        let mut line = String::new();
        // Bounded, so a client that never sends a newline cannot make the
        // daemon buffer without limit.
        let n = (&mut reader)
            .take(MAX_LINE_BYTES as u64 + 1)
            .read_line(&mut line)?;
        if n == 0 {
            return Ok(());
        }
        if n > MAX_LINE_BYTES {
            respond(
                &mut writer,
                &Response::error(
                    ErrorKind::BadRequest,
                    format!("a request may be at most {MAX_LINE_BYTES} bytes"),
                ),
            )?;
            return Ok(());
        }
        let line = line.trim_end();
        if line.is_empty() {
            continue;
        }

        // The parse error is serde's, which names a position and never quotes
        // the input. Echoing the line back would put a credential from a
        // malformed `store` into the daemon's own reply.
        let request: Request = match serde_json::from_str(line) {
            Ok(r) => r,
            Err(e) => {
                respond(
                    &mut writer,
                    &Response::error(ErrorKind::BadRequest, format!("unparseable request: {e}")),
                )?;
                continue;
            }
        };

        let response = match peer {
            Some(peer) => dispatch(layout, &peer, channel, request),
            // The kernel would not say who this is, so it is nobody.
            None => Response::error(
                ErrorKind::PermissionDenied,
                "the kernel would not report who is connecting, so nothing can \
                 be authorised for this connection"
                    .to_string(),
            ),
        };
        respond(&mut writer, &response)?;
    }
}

fn respond(writer: &mut UnixStream, response: &Response) -> Result<()> {
    let mut line = serde_json::to_string(response)?;
    line.push('\n');
    writer.write_all(line.as_bytes())?;
    writer.flush().ok();
    Ok(())
}

/// Handle one request.
///
/// `peer` is the connection's credentials and `channel` is the socket it
/// arrived on. Both are passed rather than looked up, so no handler can consult
/// the request for identity or forget which socket it is answering.
fn dispatch(layout: &Layout, peer: &Peer, channel: Channel, request: Request) -> Response {
    // The socket split, enforced once, before any handler runs. Two checks
    // rather than one: the mode on the admin socket keeps other users out, and
    // this keeps a mutating verb from being answered on the broker socket even
    // if the mode were ever wrong.
    if request.admin_only() && channel != Channel::Admin {
        return Response::error(
            ErrorKind::PermissionDenied,
            format!(
                "'{}' changes what is stored, which is only accepted on the \
                 admin socket. Run the command again with sudo",
                request.verb()
            ),
        );
    }
    if channel == Channel::Admin && !peer.may_administer() {
        return Response::error(
            ErrorKind::PermissionDenied,
            "only root may change what this service stores".to_string(),
        );
    }

    match request {
        Request::Hello => Response::Hello {
            version: PROTOCOL_VERSION,
            providers: provider::PROVIDERS.iter().map(|p| p.id.to_string()).collect(),
        },

        Request::Providers => Response::Providers {
            providers: provider::PROVIDERS.iter().map(describe_provider).collect(),
        },

        Request::List => Response::Secrets {
            secrets: store::list(layout, peer.uid),
        },

        Request::Info { secret } => match store::meta(layout, peer.uid, &secret) {
            Ok(meta) => Response::Secret(Box::new(meta)),
            Err(e) => broker::store_error(e),
        },

        Request::Grants => Response::Grants {
            grants: Grants::load(&layout.grants_file(peer.uid)).grants,
        },

        Request::Audit { limit } => Response::Audit {
            entries: audit::tail(layout, peer.uid, limit),
        },

        Request::Use(request) => broker::use_capability(layout, peer, &request),

        // ── admin socket ────────────────────────────────────────────────────
        Request::Store(req) => admin::store(layout, peer, *req),
        Request::Rotate(req) => admin::rotate(layout, peer, *req),
        Request::Remove { owner, secret } => admin::remove(layout, peer, owner, &secret),
        Request::Grant(req) => admin::grant(layout, peer, *req),
    }
}

fn describe_provider(spec: &provider::ProviderSpec) -> ProviderInfo {
    ProviderInfo {
        id: spec.id.to_string(),
        summary: spec.summary.to_string(),
        default_base: spec.default_base.to_string(),
        operations: spec
            .operations
            .iter()
            .map(|op| OperationInfo {
                id: op.id.to_string(),
                summary: op.summary.to_string(),
                method: op.method.as_str().to_string(),
                resource_shape: match op.path {
                    PathShape::Fixed(_) => None,
                    PathShape::OwnerRepo(_) => Some("owner/repo".to_string()),
                },
                fields: op.fields.iter().map(|f| (*f).to_string()).collect(),
            })
            .collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_provider_describes_itself_completely() {
        // What `apex capability providers` prints. A provider with an empty
        // summary or an operation with no fields is a vocabulary entry nobody
        // can decide about.
        for spec in provider::PROVIDERS {
            let info = describe_provider(spec);
            assert!(!info.summary.is_empty(), "{} has no summary", info.id);
            assert!(!info.operations.is_empty(), "{} offers nothing", info.id);
            for op in &info.operations {
                assert!(!op.summary.is_empty(), "{}:{} has no summary", info.id, op.id);
                assert_eq!(op.method, "GET");
                assert!(!op.fields.is_empty(), "{}:{} returns nothing", info.id, op.id);
            }
        }
    }

    #[test]
    fn an_operation_that_takes_a_resource_says_what_shape_it_is() {
        let github = describe_provider(provider::provider("github").unwrap());
        let whoami = github.operations.iter().find(|o| o.id == "whoami").unwrap();
        assert_eq!(whoami.resource_shape, None);
        let repo = github
            .operations
            .iter()
            .find(|o| o.id == "repo-metadata")
            .unwrap();
        assert_eq!(repo.resource_shape.as_deref(), Some("owner/repo"));
    }

    #[test]
    fn a_socket_is_created_with_the_mode_it_was_asked_for() {
        use std::os::unix::fs::PermissionsExt;

        let dir = std::env::temp_dir().join(format!("apex-secretd-bind-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("admin.sock");
        let listener = bind(&path, 0o600).expect("bind");
        let mode = std::fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600, "mode was {:o}", mode & 0o777);

        // A live socket must not be stolen by a second bind.
        let _client = UnixStream::connect(&path).expect("connect");
        assert!(bind(&path, 0o600).is_err(), "a live socket was taken over");

        drop(listener);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_stale_socket_file_is_replaced() {
        let dir = std::env::temp_dir().join(format!("apex-secretd-stale-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("broker.sock");
        // What a killed daemon leaves: a socket file nothing is listening on.
        drop(bind(&path, 0o666).expect("first bind"));
        assert!(path.exists(), "the socket file outlives the listener");
        let _second = bind(&path, 0o666).expect("a stale socket must be replaced");
        std::fs::remove_dir_all(&dir).ok();
    }
}
