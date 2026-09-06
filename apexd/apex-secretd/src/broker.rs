//! Performing the operation, so the credential never has to leave.
//!
//! §3.2's flow ends in a *broker-owned operation*, not in a credential handed
//! back. This module is that operation. The daemon reads the value from its own
//! root-owned store, puts it in the environment of a child it forked itself,
//! and returns the child's output. Nothing on the socket ever carries it.
//!
//! ## Why the child runs as the owner
//!
//! `git push` needs the user's repository, and the daemon is root. It could
//! read that repository as root — and then it would be running git, which
//! executes repository-local configuration, as root. That is a local root
//! escalation handed to whoever can write a `.git/config`. So the child drops
//! to the owner's uid before `exec`, and the operation has exactly the access
//! the owner already had.
//!
//! The cost is stated plainly in the crate note: for the milliseconds that
//! child runs, a same-uid process outside the sandbox can read its environment.
//! A *confined* session cannot — the sandbox uses `--unshare-pid`, so the
//! daemon's children are not in the agent's `/proc` at all — and this is the
//! same exposure the previous in-`apex-agentd` broker had. What P0-002 changes
//! is the credential at rest and the API surface, and it does not claim more.
//!
//! ## What is done about a hostile repository
//!
//! The project path comes from the caller, and git reads that repository's own
//! config. Everything reachable from the command line is closed, because
//! command-line config wins over repository config:
//!
//! * `core.hooksPath` — `pre-push` runs on push and would inherit the
//!   environment the credential is in;
//! * `core.fsmonitor` — a repository-local command git runs on many operations;
//! * `credential.helper` — reset first, then set to ours, so an inherited
//!   helper cannot also be asked for the credential;
//! * `http.proxy` — cleared, so a repository cannot route the request through
//!   somewhere it chose;
//! * `http.sslVerify` — forced on, so a repository cannot turn off the check
//!   that the host is the host.
//!
//! `GIT_CONFIG_GLOBAL` and `GIT_CONFIG_SYSTEM` are `/dev/null`, so the owner's
//! own `~/.gitconfig` is out of the picture too — it is as caller-controlled as
//! the repository is, from this daemon's point of view.
//!
//! This is a mitigation and not a proof. A git configuration key that runs a
//! command and is not on that list is a hole, and the list is maintained by
//! hand.
//!
//! ## Why the URL is resolved and not accepted
//!
//! The capability names a remote by NAME. The URL comes from
//! `git remote get-url`, run in the same configuration environment as the
//! operation itself — the same environment matters, because `get-url` expands
//! `insteadOf` and running the two with different config would pin one URL and
//! contact another. `--push` for a write, because `pushurl` and `pushInsteadOf`
//! can send a push somewhere the fetch URL never mentions.

use std::io::Write;
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::{Command, Stdio};

use apex_secret_core::capability::{self, Capability, CapabilityError};
use apex_secret_core::store::ServiceInfo;
use apex_secret_core::SecretValue;

/// How long a brokered git operation may run.
///
/// A push to an unreachable host otherwise blocks a connection thread
/// indefinitely, and the caller waiting on it never gets an answer.
pub const GIT_TIMEOUT_SECS: u64 = 120;

/// The account an operation runs as.
#[derive(Debug, Clone)]
pub struct Owner {
    pub uid: u32,
    pub gid: u32,
    pub name: String,
    pub home: String,
    /// Supplementary groups, resolved in the parent.
    ///
    /// `initgroups(3)` reads `/etc/group` and allocates, so it is not
    /// async-signal-safe and must not be called between `fork` and `exec`. The
    /// list is built here and only `setgroups(2)` — which is safe — runs in the
    /// child.
    pub groups: Vec<u32>,
}

/// Look up an account by uid.
pub fn owner(uid: u32) -> Option<Owner> {
    use std::ffi::CStr;

    // getpwuid_r, not getpwuid: the plain form returns a pointer into a static
    // buffer shared by the whole process, and this is reached from request
    // handling on several threads at once.
    let mut buf = vec![0 as libc::c_char; 4096];
    let mut pwd: libc::passwd = unsafe { std::mem::zeroed() };
    let mut result: *mut libc::passwd = std::ptr::null_mut();
    // Safe: getpwuid_r writes only into `pwd` and `buf`, both owned here, and
    // reports a too-small buffer rather than overrunning it.
    let rc = unsafe {
        libc::getpwuid_r(
            uid,
            &mut pwd,
            buf.as_mut_ptr(),
            buf.len(),
            &mut result,
        )
    };
    if rc != 0 || result.is_null() || pwd.pw_name.is_null() || pwd.pw_dir.is_null() {
        return None;
    }
    // Safe: both pointers point into `buf`, which is alive until the end of
    // this function, and the bytes are copied out before returning.
    let name = unsafe { CStr::from_ptr(pwd.pw_name) }
        .to_string_lossy()
        .into_owned();
    let home = unsafe { CStr::from_ptr(pwd.pw_dir) }
        .to_string_lossy()
        .into_owned();
    let gid = pwd.pw_gid;

    Some(Owner {
        uid,
        gid,
        groups: group_list(&name, gid),
        name,
        home,
    })
}

/// The supplementary groups of `name`, including its primary gid.
fn group_list(name: &str, gid: u32) -> Vec<u32> {
    let Ok(cname) = std::ffi::CString::new(name) else {
        return vec![gid];
    };
    let mut count: libc::c_int = 32;
    let mut groups = vec![0 as libc::gid_t; count as usize];
    // Safe: `groups` is owned and `count` long; getgrouplist writes at most
    // that many entries and reports the needed size when there are more.
    let rc = unsafe {
        libc::getgrouplist(cname.as_ptr(), gid, groups.as_mut_ptr(), &mut count)
    };
    if rc < 0 {
        // Too small. `count` now holds what is needed; one retry is enough.
        if count <= 0 || count > 1024 {
            return vec![gid];
        }
        groups = vec![0 as libc::gid_t; count as usize];
        // Safe: same contract, with a buffer the kernel just asked for.
        let rc = unsafe {
            libc::getgrouplist(cname.as_ptr(), gid, groups.as_mut_ptr(), &mut count)
        };
        if rc < 0 {
            return vec![gid];
        }
    }
    groups.truncate(count.max(0) as usize);
    if groups.is_empty() {
        groups.push(gid);
    }
    groups
}

/// Whether a project path is one the daemon will act in.
///
/// Absolute, because a relative path would be resolved against the *daemon's*
/// working directory. No NUL and no newline, because the path reaches a command
/// line and an audit line. Existence is not checked here: git says so better,
/// and checking it separately is a race.
pub fn valid_project(path: &str) -> bool {
    !path.is_empty()
        && path.len() <= 4096
        && Path::new(path).is_absolute()
        && !path.contains('\0')
        && !path.contains('\n')
}

/// The URL a remote resolves to, in the same configuration the operation will
/// run in.
pub fn resolve_url(
    project: &str,
    cap: &Capability,
    owner: &Owner,
) -> Result<String, CapabilityError> {
    let remote = cap
        .remote()
        .ok_or_else(|| CapabilityError::NoSuchRemote(cap.name().to_string()))?;
    let mut args = vec!["-C".to_string(), project.to_string(), "remote".into(), "get-url".into()];
    if cap.is_write() {
        args.push("--push".into());
    }
    args.push(remote.to_string());

    let out = run_git(&args, owner, None)
        .map_err(|_| CapabilityError::NoSuchRemote(remote.to_string()))?;
    if out.code != 0 {
        return Err(CapabilityError::NoSuchRemote(remote.to_string()));
    }
    let url = out.text.lines().next().unwrap_or("").trim().to_string();
    if url.is_empty() {
        return Err(CapabilityError::NoSuchRemote(remote.to_string()));
    }
    Ok(url)
}

/// What a git run produced.
pub struct Output {
    pub code: i32,
    pub text: String,
}

/// Perform the capability with the credential attached.
///
/// Returns the child's combined output with the credential scrubbed out of it.
pub fn perform(
    project: &str,
    cap: &Capability,
    info: &ServiceInfo,
    value: &SecretValue,
    owner: &Owner,
) -> Result<Output, String> {
    let token = value
        .as_str()
        .ok_or_else(|| "that credential is not text, so git cannot be given it".to_string())?;

    let mut args: Vec<String> = vec!["-C".to_string(), project.to_string()];
    args.extend(hardening_flags());
    match cap {
        Capability::GitPush { remote, branch } => {
            args.push("push".into());
            args.push(remote.clone());
            args.push(branch.clone().unwrap_or_else(|| "HEAD".into()));
        }
        Capability::GitFetch { remote } => {
            args.push("fetch".into());
            args.push(remote.clone());
        }
        Capability::GitLsRemote { remote } => {
            args.push("ls-remote".into());
            args.push(remote.clone());
        }
        // Not reachable: `use_capability` sends this one to `perform_http`,
        // which is where an endpoint that is not a git remote belongs. Refused
        // rather than defaulted, so a future caller that gets here is told.
        Capability::McpRequest => {
            return Err("an mcp request is not a git operation".to_string())
        }
    }

    let mut out = run_git(&args, owner, Some((&info.username, token)))?;
    out.text = scrub(&out.text, token);
    Ok(out)
}

/// The `-c` flags that close what repository-local config could otherwise open.
///
/// Command-line config wins over repository config, which is why this works at
/// all. See the module note for what each one is for.
fn hardening_flags() -> Vec<String> {
    [
        // Reset any inherited helper list, then install ours. The empty value
        // is git's own way of clearing a multi-valued key.
        "credential.helper=",
        &format!("credential.helper={CREDENTIAL_HELPER}"),
        "core.hooksPath=/nonexistent",
        "core.fsmonitor=false",
        "http.proxy=",
        "http.sslVerify=true",
    ]
    .iter()
    .flat_map(|flag| ["-c".to_string(), flag.to_string()])
    .collect()
}

/// The credential helper git is given.
///
/// It names two environment variables rather than carrying the values, so a
/// credential containing a quote, a `$` or a newline cannot break out of the
/// string — which it could if the value were interpolated in. And it keeps the
/// credential off the command line, which `/proc/<pid>/cmdline` makes
/// world-readable.
const CREDENTIAL_HELPER: &str =
    "!f() { echo \"username=$APEX_GIT_USER\"; echo \"password=$APEX_GIT_TOKEN\"; }; f";

/// Make `cmd` exec as `owner` rather than as this process.
///
/// The daemon is root because the store must be. The child must not be: git
/// runs a caller-controlled repository's own configuration, and curl writes to
/// a caller-named path — either as root is a local escalation handed to whoever
/// can write a `.git/config`. So the drop happens between fork and exec, and is
/// then verified, because a drop that reported success without happening is the
/// one failure this whole arrangement exists to prevent.
fn drop_to(cmd: &mut Command, owner: &Owner) {
    let target_uid = owner.uid;
    let target_gid = owner.gid;
    let groups = owner.groups.clone();
    // Safe: the closure runs between fork and exec in a single-threaded child
    // and calls only async-signal-safe functions. The group list was resolved
    // in the parent for exactly that reason.
    unsafe {
        cmd.pre_exec(move || {
            // Safe: geteuid cannot fail.
            if libc::geteuid() == 0 {
                if libc::setgroups(groups.len(), groups.as_ptr()) != 0 {
                    return Err(std::io::Error::last_os_error());
                }
                if libc::setgid(target_gid) != 0 {
                    return Err(std::io::Error::last_os_error());
                }
                if libc::setuid(target_uid) != 0 {
                    return Err(std::io::Error::last_os_error());
                }
            }
            if libc::getuid() != target_uid || libc::geteuid() != target_uid {
                return Err(std::io::Error::other("could not drop to the owner's uid"));
            }
            Ok(())
        });
    }
}

/// Run git as the owner, with a cleared environment.
///
/// `credential` is `(username, token)` when the operation needs one. Everything
/// else about the environment is built here rather than inherited: the daemon's
/// own environment is root's, and passing it through would hand a git child
/// root's `HOME` and whatever else systemd set.
fn run_git(
    args: &[String],
    owner: &Owner,
    credential: Option<(&str, &str)>,
) -> Result<Output, String> {
    // `timeout` rather than a watchdog thread: the child has to actually die,
    // or a push to an unreachable host leaves git running after the caller has
    // given up.
    let mut cmd = Command::new("timeout");
    cmd.arg(GIT_TIMEOUT_SECS.to_string()).arg("git").args(args);

    cmd.env_clear()
        .env("PATH", "/usr/local/bin:/usr/bin:/bin")
        .env("HOME", &owner.home)
        .env("USER", &owner.name)
        .env("LOGNAME", &owner.name)
        // Stable, parseable messages regardless of the machine's locale.
        .env("LC_ALL", "C")
        // The owner's own gitconfig is as caller-controlled as the repository
        // is, from here.
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        // The alternative to a working credential is git blocking on a prompt
        // that nobody will ever see.
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_ASKPASS", "")
        .env("SSH_ASKPASS", "")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    if let Some((username, token)) = credential {
        cmd.env("APEX_GIT_USER", username)
            .env("APEX_GIT_TOKEN", token);
    }

    drop_to(&mut cmd, owner);

    let out = cmd
        .output()
        .map_err(|e| format!("running git: {e}"))?;

    let mut text = String::from_utf8_lossy(&out.stdout).into_owned();
    let err = String::from_utf8_lossy(&out.stderr);
    if !err.trim().is_empty() {
        if !text.is_empty() && !text.ends_with('\n') {
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
    Ok(Output { code, text })
}

/// How large a brokered HTTP reply may be before it is refused.
///
/// Bounded because the far end is a server the owner chose but the daemon does
/// not control, and an unbounded read is a way for it to exhaust this machine's
/// memory one connection at a time.
pub const HTTP_MAX_BYTES: usize = 3 * 1024 * 1024;

/// The `Mcp-Session-Id` a server issued, if it issued one.
pub struct HttpOutput {
    pub out: Output,
    pub session: Option<String>,
}

/// Carry one message to the service's own endpoint, with the credential
/// attached, and answer with the reply body.
///
/// ## Where the credential goes, and where it does not
///
/// Not on the command line: `/proc/<pid>/cmdline` is world-readable, and the
/// whole point of the store is that this value is not readable by the account
/// the child runs as. Not in a file either, which would leave it at rest for as
/// long as the request takes. It goes down the child's **stdin**, as a curl
/// configuration file — `--config -` — which is the one channel between this
/// process and that one that no third party can read.
///
/// The message goes in a file for the mirror-image reason: only one of the two
/// can have stdin, and the message is the caller's own rather than a
/// credential. `dir` is a root-owned directory the owner may traverse and not
/// write, and the file is created with `O_EXCL` — this process is root, and a
/// root write to a path an ordinary account could have replaced with a symlink
/// first is how an owner turns "carry my message" into "write this anywhere".
///
/// Response headers come back on **stdout**, ahead of the body, rather than
/// through a second file, so there is one fewer path for that argument to
/// apply to. [`strip_http_headers`] separates them again.
///
/// ## Where the request goes
///
/// `info.url()`, built from the host, port, scheme and path pinned when the
/// credential was stored. Nothing the caller sent contributes to it —
/// `Capability::McpRequest` has no fields — so a session cannot aim this at a
/// server of its own choosing. Redirects are not followed, so the far end
/// cannot aim it either.
pub fn perform_http(
    info: &ServiceInfo,
    value: &SecretValue,
    body: &[u8],
    session: Option<&str>,
    owner: &Owner,
    dir: &Path,
) -> Result<HttpOutput, String> {
    let token = value
        .as_str()
        .ok_or_else(|| "that credential is not text, so it cannot become a header".to_string())?;

    let message = TempFile::create(dir, body)?;

    let mut config = String::new();
    config.push_str(&format!("url = {}\n", quote(&info.url())));
    config.push_str("request = \"POST\"\n");
    config.push_str(&format!(
        "header = {}\n",
        quote(&format!("Authorization: {}", info.header_value(token)))
    ));
    config.push_str("header = \"Content-Type: application/json\"\n");
    config.push_str("header = \"Accept: application/json, text/event-stream\"\n");
    // Suppressed so there is exactly one header block on stdout. curl sends
    // `Expect: 100-continue` for a body over a kilobyte, and a server that
    // honours it answers `100 Continue` first — two blocks, and a stripper
    // written for one would hand the caller a reply with HTTP in front of it.
    config.push_str("header = \"Expect:\"\n");
    if let Some(id) = session {
        config.push_str(&format!("header = {}\n", quote(&format!("Mcp-Session-Id: {id}"))));
    }
    config.push_str(&format!("data-binary = {}\n", quote(&format!("@{}", message.path))));
    config.push_str("dump-header = \"-\"\n");
    config.push_str("silent\nshow-error\nfail-with-body\n");
    config.push_str("proto = \"=https,http\"\n");
    config.push_str(&format!("max-filesize = {HTTP_MAX_BYTES}\n"));
    config.push_str(&format!("max-time = {GIT_TIMEOUT_SECS}\n"));

    let mut out = run_curl(&config, owner)?;
    let (headers, rest) = strip_http_headers(&out.text);
    out.text = scrub(&scrub(&rest, token), &info.header_value(token));
    Ok(HttpOutput {
        out,
        session: mcp_session_id(&headers),
    })
}

/// Split curl's stdout into the header blocks it dumped and the body.
///
/// `dump-header = "-"` writes every response's headers before the body, and
/// there can be more than one — a redirect that was not followed still has its
/// own, and an intermediate `100 Continue` is a block of its own. Every leading
/// block is taken, so what is left is the body and only the body.
pub fn strip_http_headers(text: &str) -> (String, String) {
    let mut headers = String::new();
    let mut rest = text;
    while rest.starts_with("HTTP/") {
        let end = match (rest.find("\r\n\r\n"), rest.find("\n\n")) {
            (Some(a), Some(b)) if b < a => (b, 2),
            (Some(a), _) => (a, 4),
            (None, Some(b)) => (b, 2),
            (None, None) => break,
        };
        headers.push_str(&rest[..end.0]);
        headers.push('\n');
        rest = &rest[end.0 + end.1..];
    }
    (headers, rest.to_string())
}

/// The `Mcp-Session-Id` in a header dump, if there is one.
///
/// Bounded and character-checked before it is kept: it is replayed into a later
/// request's headers, and a value carrying a newline would let the far end
/// write headers of its own choosing into the next one.
pub fn mcp_session_id(headers: &str) -> Option<String> {
    for line in headers.lines() {
        let Some((name, value)) = line.split_once(':') else {
            continue;
        };
        if !name.trim().eq_ignore_ascii_case("mcp-session-id") {
            continue;
        }
        let value = value.trim();
        let ok = !value.is_empty()
            && value.len() <= 128
            && value
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | ':'));
        return ok.then(|| value.to_string());
    }
    None
}

/// Run curl as the owner, with its whole configuration on stdin.
fn run_curl(config: &str, owner: &Owner) -> Result<Output, String> {
    let mut cmd = Command::new("curl");
    cmd.arg("--config").arg("-");
    cmd.env_clear()
        .env("PATH", "/usr/local/bin:/usr/bin:/bin")
        .env("HOME", &owner.home)
        .env("LC_ALL", "C")
        // A proxy the environment could name is a destination the caller did
        // not choose and this daemon did not check.
        .env("NO_PROXY", "*")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    drop_to(&mut cmd, owner);

    let mut child = cmd.spawn().map_err(|e| format!("running curl: {e}"))?;
    child
        .stdin
        .take()
        .ok_or_else(|| "curl has no stdin".to_string())?
        .write_all(config.as_bytes())
        .map_err(|e| format!("sending curl its configuration: {e}"))?;
    let out = child
        .wait_with_output()
        .map_err(|e| format!("waiting for curl: {e}"))?;

    let mut text = String::from_utf8_lossy(&out.stdout).into_owned();
    if text.len() > HTTP_MAX_BYTES {
        return Err(format!(
            "that reply is larger than the {HTTP_MAX_BYTES} bytes this service will carry"
        ));
    }
    let err = String::from_utf8_lossy(&out.stderr);
    if !err.trim().is_empty() {
        if !text.is_empty() && !text.ends_with('\n') {
            text.push('\n');
        }
        text.push_str(err.trim_end());
    }
    Ok(Output {
        code: out.status.code().unwrap_or(-1),
        text,
    })
}

/// A curl config value, quoted so nothing in it can be read as syntax.
///
/// curl's parser takes `"…"` with backslash escapes. A credential is never
/// interpolated anywhere else, so this is the only place a `"` or a `\` in one
/// could change what curl does, and it is closed here rather than trusted to
/// the shape of a token.
fn quote(value: &str) -> String {
    let mut out = String::with_capacity(value.len() + 2);
    out.push('"');
    for c in value.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            _ => out.push(c),
        }
    }
    out.push('"');
    out
}

/// A file the child can read, in a directory only root can write, removed when
/// it is dropped.
///
/// `create_new` is the whole of the safety argument. This process is root; the
/// account the child runs as must be able to read the file; and a root write to
/// a name an ordinary account could have created first as a symlink is a root
/// write to wherever that symlink points. `O_EXCL` refuses a name that already
/// exists, and the directory it lives in is not writable by that account
/// anyway, so there are two independent reasons the race cannot be won.
struct TempFile {
    path: String,
}

impl TempFile {
    /// Create the directory brokered messages are written in.
    ///
    /// `0711`: root writes here, and the account the child runs as needs to
    /// traverse it to open a file by name and nothing more. It cannot list the
    /// directory, create a name in it, or replace one.
    pub fn prepare_dir(dir: &Path) -> Result<(), String> {
        use std::os::unix::fs::PermissionsExt;
        std::fs::create_dir_all(dir).map_err(|e| format!("creating {}: {e}", dir.display()))?;
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o711))
            .map_err(|e| format!("securing {}: {e}", dir.display()))
    }

    fn create(dir: &Path, contents: &[u8]) -> Result<TempFile, String> {
        use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
        TempFile::prepare_dir(dir)?;
        let path = dir.join(format!(
            "message-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o644)
            .open(&path)
            .map_err(|e| format!("creating {}: {e}", path.display()))?;
        file.write_all(contents)
            .map_err(|e| format!("writing {}: {e}", path.display()))?;
        // Set again after the write: the mode above is masked by the process
        // umask, and a child that cannot read its own message fails with a
        // curl error about a file rather than anything a reader could act on.
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644))
            .map_err(|e| format!("securing {}: {e}", path.display()))?;
        Ok(TempFile {
            path: path.to_string_lossy().into_owned(),
        })
    }
}

impl Drop for TempFile {
    fn drop(&mut self) {
        std::fs::remove_file(&self.path).ok();
    }
}

/// Remove the credential from anything on its way back to the caller.
///
/// Defence in depth. git does not print credentials, but it does print URLs,
/// and a URL of the form `https://user:token@host/…` appears in some error
/// messages. Since the entire promise of this daemon is that the caller never
/// receives the value, what it gets is scrubbed rather than trusted.
pub fn scrub(text: &str, token: &str) -> String {
    if token.is_empty() {
        return text.to_string();
    }
    text.replace(token, "«redacted»")
}

/// Scheme and host of a resolved URL, for the audit line and the reply.
///
/// The path is deliberately dropped: it is repository detail, and an audit line
/// that carries it invites somebody to grep the trail for private repository
/// names.
pub fn endpoint(url: &str) -> String {
    match capability::http_endpoint(url) {
        Some((scheme, host)) => format!("{scheme}://{host}"),
        None => "unknown".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_token_is_scrubbed_from_anything_returned() {
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
        // A credential containing a quote, a `$` or a newline must not be able
        // to break out of the helper string, which it could if the value were
        // substituted in.
        assert!(CREDENTIAL_HELPER.contains("$APEX_GIT_TOKEN"));
        assert!(!CREDENTIAL_HELPER.contains("{}"));
        assert!(!CREDENTIAL_HELPER.contains('\n'));
    }

    #[test]
    fn the_hardening_flags_close_every_execution_path_named_in_the_note() {
        let flags = hardening_flags().join(" ");
        for key in [
            "core.hooksPath=/nonexistent",
            "core.fsmonitor=false",
            "http.proxy=",
            "http.sslVerify=true",
        ] {
            assert!(flags.contains(key), "{key} is missing from {flags}");
        }
        // The helper list is reset before ours is added, so an inherited helper
        // cannot also be asked for the credential.
        let list = hardening_flags();
        let reset = list.iter().position(|f| f == "credential.helper=").unwrap();
        let ours = list
            .iter()
            .position(|f| f.starts_with("credential.helper=!f()"))
            .unwrap();
        assert!(reset < ours, "ours comes before the reset: {list:?}");
        // Every flag is preceded by its own `-c`.
        assert_eq!(list.len() % 2, 0);
        assert!(list.iter().step_by(2).all(|f| f == "-c"), "{list:?}");
    }

    #[test]
    fn a_project_path_must_be_absolute_and_free_of_framing_characters() {
        assert!(valid_project("/home/x/p"));
        for evil in ["", "relative/p", "../p", "/p\0x", "/p\nx"] {
            assert!(!valid_project(evil), "'{}' was accepted", evil.escape_debug());
        }
        assert!(!valid_project(&"/".repeat(5000)));
    }

    #[test]
    fn the_endpoint_is_the_scheme_and_host_and_nothing_else() {
        // A private repository's name must not end up in a trail an
        // administrator reads.
        assert_eq!(endpoint("https://github.com/acme/secret-plans.git"), "https://github.com");
        assert_eq!(endpoint("http://127.0.0.1:9418/demo.git"), "http://127.0.0.1");
        assert_eq!(endpoint("git@github.com:a/b"), "unknown");
    }

    #[test]
    fn the_owner_of_this_process_resolves_with_a_home_and_a_group() {
        // The lookup the setuid path depends on, against the real passwd
        // database rather than a fixture.
        // Safe: getuid cannot fail.
        let me = unsafe { libc::getuid() };
        let owner = owner(me).expect("this process's own uid must resolve");
        assert_eq!(owner.uid, me);
        assert!(!owner.name.is_empty());
        assert!(Path::new(&owner.home).is_absolute(), "{}", owner.home);
        assert!(
            owner.groups.contains(&owner.gid),
            "the primary group is missing from {:?}",
            owner.groups
        );
    }

    #[test]
    fn an_unknown_uid_does_not_resolve() {
        assert!(owner(4_000_000_000).is_none());
    }

    #[test]
    fn git_runs_as_the_owner_with_an_environment_this_module_built() {
        // The whole child-process path, exercised for real: no credential, no
        // setuid to do (the target is already this uid), but every other part
        // is the shipped one — env_clear, the pre_exec check, `timeout`, and
        // the combined output.
        // Safe: getuid cannot fail.
        let me = unsafe { libc::getuid() };
        let owner = owner(me).expect("own uid");
        let out = run_git(&["--version".to_string()], &owner, None).expect("git --version");
        assert_eq!(out.code, 0, "{}", out.text);
        assert!(out.text.starts_with("git version"), "{}", out.text);
    }

    #[test]
    fn a_failing_git_reports_its_own_message_rather_than_an_empty_success() {
        // Safe: getuid cannot fail.
        let owner = owner(unsafe { libc::getuid() }).expect("own uid");
        let out = run_git(
            &["-C".into(), "/nonexistent-apex-secretd-test".into(), "status".into()],
            &owner,
            None,
        )
        .expect("spawn");
        assert_ne!(out.code, 0);
        assert!(!out.text.trim().is_empty(), "stderr was dropped");
    }
}
