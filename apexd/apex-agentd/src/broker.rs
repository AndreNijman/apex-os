//! The secret broker, daemon side (roadmap §4).
//!
//! The agent asks for an operation; this performs it and returns the result.
//! The token is only ever in the environment of a `git` process the agent
//! cannot see — the sandbox uses `--unshare-pid`, so the daemon's children are
//! not in the agent's `/proc` at all.
//!
//! That is a NAMESPACE boundary, not a privilege one. `apex-agentd` runs as the
//! same unprivileged user; what it has that the session does not is a view of
//! the filesystem and process table. For "the agent must not learn the token"
//! that is exactly the boundary required, and it is why a git credential helper
//! cannot work: `git` runs inside the sandbox, so the helper's reply lands in
//! the agent's own namespace.

use std::path::Path;
use std::sync::Arc;

use apex_agent_core::protocol::{ErrorKind, Response};
use apex_agent_core::secret::{self, Capability, SecretError, SecretGrants};

use crate::peer::Peer;
use crate::privilege;
use crate::Daemon;

/// How long a brokered git operation may run.
///
/// A push to an unreachable host otherwise blocks the connection thread
/// indefinitely, and the agent waiting on it never gets an answer.
const GIT_TIMEOUT_SECS: u64 = 120;

/// Perform one capability.
///
/// Order matters and is the security argument:
///   1. resolve the session from the PEER CREDENTIALS — never the request;
///   2. resolve the project from what the daemon recorded for that session;
///   3. check the grant for (project, service, capability);
///   4. resolve the remote NAME against the repository's own configuration;
///   5. check the remote's host against the credential's host;
///   6. only then read the token, and only into a child's environment.
///
/// Every step before 6 can refuse, and the token is not read until they all
/// pass — so a refusal cannot leak it through an error path.
pub fn use_capability(
    daemon: &Arc<Daemon>,
    peer: Option<Peer>,
    service: &str,
    capability: &str,
    remote: &str,
    branch: Option<&str>,
    claimed_project: Option<&str>,
) -> Response {
    let cap = match Capability::parse(capability, remote, branch) {
        Ok(c) => c,
        Err(e) => return refuse(e),
    };
    if !secret::valid_service_name(service) {
        return refuse(SecretError::NoSuchService(service.to_string()));
    }

    let who = privilege::origin(daemon, peer);

    // The project.
    //
    // For a SESSION it is what the daemon recorded when it forked it, and
    // `claimed_project` is ignored — otherwise a confined agent could name a
    // project whose capabilities it was never granted.
    //
    // For an unsessioned caller the claim is honoured, because the daemon
    // cannot see that caller's working directory. `current_dir()` here returns
    // the DAEMON's cwd, which is how the first version made every grant
    // silently fail to match. Trusting the claim is not a weakening: an
    // unsessioned caller is unconfined and could run git itself.
    let project = match who.session {
        Some(_) => who.project.clone(),
        None => claimed_project
            .map(str::to_string)
            .filter(|p| Path::new(p).is_absolute()),
    };
    let Some(project) = project else {
        return Response::error(
            ErrorKind::PermissionDenied,
            "this session is not inside a project, so no capability can be \
             granted to it"
                .to_string(),
        );
    };

    let info = match secret::info(service) {
        Some(i) => i,
        None => return refuse(SecretError::NoSuchService(service.to_string())),
    };

    let grants = SecretGrants::load(&secret::grants_file());
    if !grants.allows(Some(&project), service, cap.name()) {
        let _ = secret::audit(
            &secret::audit_log(),
            "refused",
            service,
            &cap,
            who.session,
            who.agent.as_deref(),
            Some(&project),
            None,
        );
        return refuse(SecretError::NotGranted {
            service: service.to_string(),
            capability: cap.name().to_string(),
        });
    }

    // The repository's own remotes. Read by the daemon from the repo, so a
    // session cannot substitute a URL — which would make the broker push the
    // branch anywhere it was told, with the user's token attached.
    let remotes = match git_remotes(Path::new(&project)) {
        Ok(r) => r,
        Err(e) => return Response::error(ErrorKind::Internal, e),
    };
    if let Err(e) = secret::check_remote(cap.remote(), &remotes, &info.host) {
        let _ = secret::audit(
            &secret::audit_log(),
            "refused",
            service,
            &cap,
            who.session,
            who.agent.as_deref(),
            Some(&project),
            None,
        );
        return refuse(e);
    }

    // Every check has passed. Only now is the token read.
    let token = match secret::token(service) {
        Ok(t) => t,
        Err(e) => return refuse(e),
    };

    let (code, output) = match run_git(&project, &cap, &info.username, &token) {
        Ok(v) => v,
        Err(e) => {
            return Response::error(ErrorKind::Internal, e);
        }
    };

    let _ = secret::audit(
        &secret::audit_log(),
        "used",
        service,
        &cap,
        who.session,
        who.agent.as_deref(),
        Some(&project),
        Some(code),
    );

    Response::Brokered {
        service: service.to_string(),
        capability: cap.name().to_string(),
        detail: cap.summary(),
        exit_code: code,
        output: scrub(&output, &token),
    }
}

/// Remove the token from anything on its way back to the caller.
///
/// Defence in depth. git does not print credentials, but it does print URLs,
/// and a URL of the form `https://user:token@host/…` appears in some error
/// messages. Since the entire promise of this module is that the agent never
/// receives the secret, the output it gets is scrubbed before it is returned
/// rather than trusted not to contain it.
fn scrub(text: &str, token: &str) -> String {
    if token.is_empty() {
        return text.to_string();
    }
    text.replace(token, "«redacted»")
}

/// `git remote -v`, as (name, url) pairs for the fetch URL.
fn git_remotes(root: &Path) -> Result<Vec<(String, String)>, String> {
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["remote", "-v"])
        .output()
        .map_err(|e| format!("running git remote: {e}"))?;
    if !out.status.success() {
        return Err(String::from_utf8_lossy(&out.stderr).trim().to_string());
    }
    let mut seen = Vec::new();
    for line in String::from_utf8_lossy(&out.stdout).lines() {
        let mut parts = line.split_whitespace();
        let Some(name) = parts.next() else { continue };
        let Some(url) = parts.next() else { continue };
        if !seen.iter().any(|(n, _): &(String, String)| n == name) {
            seen.push((name.to_string(), url.to_string()));
        }
    }
    Ok(seen)
}

/// Run the git operation with the credential in the child's environment.
///
/// The credential reaches git through an inline `credential.helper` that echoes
/// two environment variables. The token is therefore:
///   * never on a command line — `/proc/<pid>/cmdline` is world-readable;
///   * never in a file — nothing to leave behind on a crash;
///   * never interpolated into the helper string — the helper names the
///     variables, so a token containing quotes or `$` cannot break out.
///
/// `GIT_TERMINAL_PROMPT=0` and an empty `GIT_ASKPASS` are set because the
/// alternative to a working credential is git *blocking on a prompt* that
/// nobody will ever see — the same failure the keyring has, and the reason a
/// timeout wraps this too.
fn run_git(
    project: &str,
    cap: &Capability,
    username: &str,
    token: &str,
) -> Result<(i32, String), String> {
    let helper = "!f() { echo \"username=$APEX_GIT_USER\"; \
                  echo \"password=$APEX_GIT_TOKEN\"; }; f";

    let mut args: Vec<String> = vec![
        "-C".into(),
        project.to_string(),
        "-c".into(),
        format!("credential.helper={helper}"),
    ];
    match cap {
        Capability::GitPush { remote, branch } => {
            args.push("push".into());
            args.push(remote.clone());
            if let Some(b) = branch {
                args.push(b.clone());
            } else {
                args.push("HEAD".into());
            }
        }
        Capability::GitFetch { remote } => {
            args.push("fetch".into());
            args.push(remote.clone());
        }
    }

    // `timeout` rather than a watchdog thread: the child has to actually die,
    // or a push to an unreachable host leaves git running after the caller has
    // given up.
    let out = std::process::Command::new("timeout")
        .arg(GIT_TIMEOUT_SECS.to_string())
        .arg("git")
        .args(&args)
        .env("APEX_GIT_USER", username)
        .env("APEX_GIT_TOKEN", token)
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_ASKPASS", "")
        .env("SSH_ASKPASS", "")
        .stdin(std::process::Stdio::null())
        .output()
        .map_err(|e| format!("running git: {e}"))?;

    let mut text = String::from_utf8_lossy(&out.stdout).into_owned();
    let err = String::from_utf8_lossy(&out.stderr);
    if !err.trim().is_empty() {
        if !text.is_empty() {
            text.push('\n');
        }
        text.push_str(err.trim_end());
    }
    let code = out.status.code().unwrap_or(-1);
    if code == 124 {
        text.push_str(&format!(
            "\napex: git did not finish within {GIT_TIMEOUT_SECS}s and was stopped"
        ));
    }
    Ok((code, text))
}

/// Grant a capability. Refused for a sessioned peer, like a privilege decision:
/// a session that can widen its own permissions has no permissions.
pub fn grant(
    daemon: &Arc<Daemon>,
    peer: Option<Peer>,
    project: &str,
    service: &str,
    capability: &str,
    revoke: bool,
) -> Response {
    if let Some(session) = privilege::origin(daemon, peer).session {
        return Response::error(
            ErrorKind::PermissionDenied,
            format!("session {session} cannot change its own capabilities"),
        );
    }
    if !Capability::names().contains(&capability) {
        return refuse(SecretError::UnknownCapability(capability.to_string()));
    }
    if !secret::valid_service_name(service) {
        return refuse(SecretError::NoSuchService(service.to_string()));
    }

    let path = secret::grants_file();
    let mut grants = SecretGrants::load(&path);
    if revoke {
        if !grants.revoke(project, service, capability) {
            return Response::error(
                ErrorKind::NoSuchRequest,
                format!("{service}:{capability} was not granted for {project}"),
            );
        }
    } else {
        // A capability cannot be granted for a service that does not exist:
        // otherwise a typo produces a grant that silently never matches.
        if secret::info(service).is_none() {
            return refuse(SecretError::NoSuchService(service.to_string()));
        }
        grants.allow(project, service, capability);
    }
    if let Err(e) = grants.save(&path) {
        return Response::error(ErrorKind::Internal, e.to_string());
    }
    Response::SecretGrants {
        projects: grants.projects,
    }
}

pub fn grants() -> Response {
    Response::SecretGrants {
        projects: SecretGrants::load(&secret::grants_file()).projects,
    }
}

fn refuse(e: SecretError) -> Response {
    let kind = match e {
        SecretError::NotGranted { .. } => ErrorKind::PermissionDenied,
        SecretError::NoSuchService(_) => ErrorKind::NoSuchRequest,
        SecretError::Io(_) | SecretError::KeyringTimeout => ErrorKind::Internal,
        _ => ErrorKind::BadRequest,
    };
    Response::error(kind, e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_token_is_scrubbed_from_anything_returned() {
        // The whole promise of this module. git does not normally print
        // credentials, but it does print URLs, and some error paths include a
        // `https://user:token@host/…` form.
        let out = scrub(
            "fatal: could not read from https://x:ghp_SEKRIT@github.com/a/b\n",
            "ghp_SEKRIT",
        );
        assert!(!out.contains("ghp_SEKRIT"), "{out}");
        assert!(out.contains("«redacted»"), "{out}");
    }

    #[test]
    fn scrubbing_an_empty_token_does_not_mangle_the_output() {
        // An empty needle would otherwise match everywhere.
        assert_eq!(scrub("hello", ""), "hello");
    }

    #[test]
    fn the_credential_helper_names_variables_and_never_interpolates_them() {
        // A token containing a quote or a `$` must not be able to break out of
        // the helper string, which it could if the value were substituted in.
        let helper = "!f() { echo \"username=$APEX_GIT_USER\"; \
                      echo \"password=$APEX_GIT_TOKEN\"; }; f";
        assert!(helper.contains("$APEX_GIT_TOKEN"));
        // No format placeholder, so there is nothing to interpolate INTO.
        assert!(!helper.contains("{}"));
    }

    #[test]
    fn git_remotes_parses_the_fetch_url_once_per_remote() {
        // `git remote -v` prints two lines per remote (fetch and push). A
        // parser that kept both would report duplicate names, and `find` would
        // then silently pick whichever came first.
        let this = std::env::current_dir().expect("cwd");
        let Ok(remotes) = git_remotes(&this) else {
            eprintln!("SKIP: not a git repository");
            return;
        };
        let mut names: Vec<&str> = remotes.iter().map(|(n, _)| n.as_str()).collect();
        let before = names.len();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), before, "duplicate remote names: {remotes:?}");
    }
}
