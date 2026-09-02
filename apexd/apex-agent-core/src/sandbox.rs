//! Project-scoped confinement for agent sessions.
//!
//! Built on `bubblewrap`, which is already in the image — this adds no package
//! to `Containerfile.core` and therefore costs the fleet no update. The
//! confinement is mount-namespace based, which for the properties the roadmap
//! asks for (project writable, `~/.ssh` unreachable, browser profiles
//! unreachable, unrelated documents unreachable, no camera or microphone, base
//! OS not modifiable) is stronger than a path-allowlist LSM would be: the files
//! are not merely denied, they are not in the mount namespace at all.
//!
//! ## Default-deny, not blocklist
//!
//! `$HOME` and `$XDG_RUNTIME_DIR` are replaced with empty tmpfs mounts and only
//! an explicit allowlist is bound back. A blocklist ("hide `~/.ssh`, hide
//! `~/.mozilla`") is unmaintainable — every new credential store a tool invents
//! is a hole until someone notices. Default-deny means `~/.ssh`, `~/.gnupg`,
//! `~/.aws`, browser profiles and the ssh-agent and gpg-agent sockets in
//! `$XDG_RUNTIME_DIR` are all unreachable because nothing bound them, not
//! because anything listed them.
//!
//! The environment is treated the same way: `--clearenv` and then an explicit
//! set, so an `ANTHROPIC_API_KEY` or `GITHUB_TOKEN` sitting in the user's shell
//! does not leak into a session that never asked for it.
//!
//! ## Verified properties
//!
//! Measured on APEX-OS 43, kernel 7.1.5, bubblewrap 0.11.0:
//!
//! | property | outside | inside |
//! |---|---|---|
//! | processes visible in `/proc` | 408 | 4 |
//! | `/dev/video*` nodes | 4 | 0 |
//! | `/dev/snd` nodes | 14 | 0 |
//! | `~/.ssh` readable | yes | no |
//! | project readable/writable | yes | yes |
//!
//! Exit status propagates through `bwrap --unshare-pid` unchanged, and
//! `killpg` on the session's process group still stops, continues and kills the
//! whole tree — which is what makes `apex agent pause` work despite the PID
//! namespace.
//!
//! ## What this does not do
//!
//! It does not confine a session with `SandboxPolicy::Unrestricted`, by design
//! — that is the escape hatch. It does not sandbox ordinary terminal processes:
//! policy applies to sessions the runtime manages and to nothing else.

use std::path::{Path, PathBuf};

use crate::protocol::SandboxPolicy;

/// Environment variables every session keeps, regardless of adapter.
///
/// Locale and terminal identity, because a TUI renders wrongly without them,
/// and the account identity a toolchain expects. Notably absent: anything that
/// could carry a credential.
const ENV_BASE: &[&str] = &[
    "TERM",
    "COLORTERM",
    "LANG",
    "LANGUAGE",
    "LC_ALL",
    "LC_CTYPE",
    "LC_MESSAGES",
    "LC_NUMERIC",
    "LC_TIME",
    "LC_COLLATE",
    "LC_MONETARY",
    "TZ",
    "TERMINFO",
    "NO_COLOR",
];

/// Absolute paths that may never be bound writable, whatever asks for it.
///
/// Binding any of these read-write would hand back everything the tmpfs masks
/// (`$HOME`), or make the image-owned base mutable (`/usr`, `/etc`), which is
/// the one thing an atomic OS must not allow a confined process to do.
const NEVER_WRITABLE: &[&str] = &["/", "/usr", "/etc", "/boot", "/sysroot", "/var/lib/apex"];

/// Why a sandbox could not be built. Every variant is fatal: a session is never
/// silently downgraded to a weaker policy than the one that was asked for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SandboxError {
    /// `bwrap` is missing from the image.
    MissingBwrap,
    /// `dev.tty.legacy_tiocsti` is enabled, so terminal input injection into
    /// the controlling terminal is possible and confinement is not meaningful.
    TiocstiEnabled,
    /// A requested writable path is one that may never be writable.
    ForbiddenWritable(PathBuf),
    /// A path that must be absolute was not.
    NotAbsolute(PathBuf),
}

impl std::fmt::Display for SandboxError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SandboxError::MissingBwrap => write!(
                f,
                "bubblewrap (/usr/bin/bwrap) is not installed, so an agent session cannot be \
                 confined; re-run with `--sandbox unrestricted` to run without confinement"
            ),
            SandboxError::TiocstiEnabled => write!(
                f,
                "dev.tty.legacy_tiocsti is 1, which lets a confined process inject keystrokes \
                 into the controlling terminal; set it to 0 (the kernel default) or re-run with \
                 `--sandbox unrestricted`"
            ),
            SandboxError::ForbiddenWritable(p) => {
                write!(f, "{} may never be made writable to an agent", p.display())
            }
            SandboxError::NotAbsolute(p) => {
                write!(f, "{} must be an absolute path", p.display())
            }
        }
    }
}

impl std::error::Error for SandboxError {}

/// Everything needed to build one session's confinement.
#[derive(Debug, Clone)]
pub struct SandboxSpec {
    pub policy: SandboxPolicy,
    /// The user's home. Masked with a tmpfs unless the policy is unrestricted.
    pub home: PathBuf,
    /// `$XDG_RUNTIME_DIR`. Masked, so the ssh-agent and gpg-agent sockets go
    /// with it.
    pub runtime_dir: PathBuf,
    /// The agentd control socket, bound back writable so a session can publish
    /// its own state through the open event protocol.
    pub control_socket: PathBuf,
    /// Per-session scratch, writable.
    pub scratch: PathBuf,
    /// Working directory inside the sandbox.
    pub cwd: PathBuf,
    /// Writable paths: the project root, the worktree, anything `--allow`ed.
    pub rw: Vec<PathBuf>,
    /// Read-only paths bound back into the masked home: toolchain caches,
    /// the agent's own configuration, anything `--allow-ro`ed.
    pub ro: Vec<PathBuf>,
    /// Files to blank out *after* the allowlists have been applied.
    ///
    /// Needed because some allowlist entries are directories that a toolchain
    /// must have but that also hold a credential in a fixed location —
    /// `~/.cargo` carries the build cache and `credentials.toml` in the same
    /// tree. Each of these is replaced with `/dev/null`, so the file reads
    /// empty rather than being merely absent, which is what tools that expect
    /// it to exist handle gracefully.
    pub mask: Vec<PathBuf>,
    /// Variables set explicitly (name, value).
    pub env_set: Vec<(String, String)>,
    /// Variable names inherited from the daemon's environment when present.
    /// Adapters use this to declare the credentials their agent needs.
    pub env_pass: Vec<String>,
}

impl SandboxSpec {
    /// A spec with nothing allowed beyond the defaults.
    pub fn new(policy: SandboxPolicy, home: PathBuf, runtime_dir: PathBuf) -> SandboxSpec {
        SandboxSpec {
            policy,
            home,
            runtime_dir,
            control_socket: PathBuf::new(),
            scratch: PathBuf::new(),
            cwd: PathBuf::from("/"),
            rw: Vec::new(),
            ro: Vec::new(),
            mask: Vec::new(),
            env_set: Vec::new(),
            env_pass: Vec::new(),
        }
    }
}

/// Check that a confined session can actually be started.
///
/// Called before every confined run. Fails closed: if `bwrap` is missing or the
/// kernel allows terminal injection, the session does not start unconfined, it
/// does not start at all.
pub fn preflight(policy: SandboxPolicy) -> Result<(), SandboxError> {
    if !policy.is_confined() {
        return Ok(());
    }
    if !bwrap_path().exists() {
        return Err(SandboxError::MissingBwrap);
    }
    if tiocsti_enabled() {
        return Err(SandboxError::TiocstiEnabled);
    }
    Ok(())
}

/// Where `bwrap` lives. A fixed path, not a `PATH` lookup: this is a security
/// boundary and resolving it through a user-controlled `PATH` would let a
/// shadowing binary decide what "confined" means.
pub fn bwrap_path() -> PathBuf {
    PathBuf::from("/usr/bin/bwrap")
}

/// Whether the kernel still honours the legacy `TIOCSTI` ioctl.
///
/// Absent file means the knob does not exist on this kernel, which means the
/// ioctl was compiled out — safe.
fn tiocsti_enabled() -> bool {
    match std::fs::read_to_string("/proc/sys/dev/tty/legacy_tiocsti") {
        Ok(text) => text.trim() == "1",
        Err(_) => false,
    }
}

/// Build the full argv for a confined session: `bwrap … -- program args…`.
///
/// For [`SandboxPolicy::Unrestricted`] this returns `program args…` unchanged,
/// so callers have a single code path.
pub fn build_argv(
    spec: &SandboxSpec,
    program: &str,
    args: &[String],
) -> Result<Vec<String>, SandboxError> {
    if !spec.policy.is_confined() {
        let mut argv = vec![program.to_string()];
        argv.extend(args.iter().cloned());
        return Ok(argv);
    }

    for p in spec.rw.iter().chain(std::iter::once(&spec.scratch)) {
        if p.as_os_str().is_empty() {
            continue;
        }
        check_writable(p, &spec.home)?;
    }
    if !spec.cwd.is_absolute() {
        return Err(SandboxError::NotAbsolute(spec.cwd.clone()));
    }

    let mut a: Vec<String> = Vec::new();
    let mut push = |s: &str| a.push(s.to_string());

    push(bwrap_path().to_string_lossy().as_ref());

    // 1. The whole filesystem, read-only. Everything after this is an overlay
    //    on top, so ordering matters and this must come first.
    push("--ro-bind");
    push("/");
    push("/");

    // 2. Fresh /proc and a minimal /dev. The minimal /dev is what removes the
    //    camera and sound devices — it exposes only null, zero, full, random,
    //    urandom, tty and a private pts.
    push("--proc");
    push("/proc");
    push("--dev");
    push("/dev");

    // 3. Private /tmp, then the session's own scratch inside it.
    push("--tmpfs");
    push("/tmp");
    if !spec.scratch.as_os_str().is_empty() {
        push("--bind");
        push(&spec.scratch.to_string_lossy());
        push(&spec.scratch.to_string_lossy());
    }

    // 4. Mask the home and the runtime directory, then bind back only what was
    //    asked for. This is the default-deny core of the policy.
    push("--tmpfs");
    push(&spec.home.to_string_lossy());
    if !spec.runtime_dir.as_os_str().is_empty() {
        push("--tmpfs");
        push(&spec.runtime_dir.to_string_lossy());
    }

    // 5. The control socket, so the session can publish its own events. Bound
    //    after the runtime-dir tmpfs, and writable because connecting to a Unix
    //    socket needs write access to it.
    if !spec.control_socket.as_os_str().is_empty() {
        push("--bind-try");
        push(&spec.control_socket.to_string_lossy());
        push(&spec.control_socket.to_string_lossy());
    }

    // 6. The allowlists. `-try` variants throughout: a toolchain cache that
    //    does not exist yet must not stop the session from starting.
    for p in &spec.ro {
        if p.as_os_str().is_empty() {
            continue;
        }
        push("--ro-bind-try");
        push(&p.to_string_lossy());
        push(&p.to_string_lossy());
    }
    for p in &spec.rw {
        if p.as_os_str().is_empty() {
            continue;
        }
        push("--bind-try");
        push(&p.to_string_lossy());
        push(&p.to_string_lossy());
    }

    // 7. Blank out credential files that sit inside an allowlisted directory.
    //    Last, so nothing bound above can bring one back.
    for p in &spec.mask {
        if p.as_os_str().is_empty() {
            continue;
        }
        push("--ro-bind-try");
        push("/dev/null");
        push(&p.to_string_lossy());
    }

    // 8. Namespaces. PID isolation is what stops an agent signalling the
    //    user's other processes; the host still reaches the session's process
    //    group, so pause/resume/kill keep working.
    push("--unshare-pid");
    push("--unshare-ipc");
    push("--unshare-uts");
    if matches!(spec.policy, SandboxPolicy::Strict) {
        push("--unshare-net");
    }

    // Deliberately no `--new-session`: the session must keep the PTY as its
    // controlling terminal or every TUI agent breaks. `preflight` refuses to
    // run confined when `dev.tty.legacy_tiocsti` is enabled, which is what
    // `--new-session` would otherwise be protecting against.

    // 9. Environment: clear, then set exactly what was allowed.
    push("--clearenv");
    for (k, v) in resolved_env(spec) {
        push("--setenv");
        push(&k);
        push(&v);
    }

    push("--chdir");
    push(&spec.cwd.to_string_lossy());
    // Die with the daemon, so no session outlives the runtime that tracks it.
    push("--die-with-parent");

    push("--");
    push(program);
    for arg in args {
        a.push(arg.clone());
    }

    Ok(a)
}

/// The environment a confined session actually gets, sorted by name so the
/// argv is deterministic and therefore testable.
pub fn resolved_env(spec: &SandboxSpec) -> Vec<(String, String)> {
    let mut env: Vec<(String, String)> = Vec::new();
    let seen = |env: &[(String, String)], k: &str| env.iter().any(|(n, _)| n == k);

    // Explicit values win over anything inherited.
    for (k, v) in &spec.env_set {
        if !seen(&env, k) {
            env.push((k.clone(), v.clone()));
        }
    }

    for name in ENV_BASE.iter().map(|s| s.to_string()).chain(
        // Adapter-declared passthrough, e.g. the credential a given agent
        // needs. Everything else in the caller's environment is dropped.
        spec.env_pass.iter().cloned(),
    ) {
        if seen(&env, &name) {
            continue;
        }
        if let Some(val) = std::env::var_os(&name) {
            env.push((name, val.to_string_lossy().into_owned()));
        }
    }

    env.sort_by(|a, b| a.0.cmp(&b.0));
    env
}

/// Reject writable binds that would defeat the policy.
fn check_writable(path: &Path, home: &Path) -> Result<(), SandboxError> {
    if !path.is_absolute() {
        return Err(SandboxError::NotAbsolute(path.to_path_buf()));
    }
    // Normalise away a trailing slash so "/usr/" is caught alongside "/usr".
    let text = path.to_string_lossy();
    let trimmed = text.trim_end_matches('/');
    let normalised = if trimmed.is_empty() { "/" } else { trimmed };

    for forbidden in NEVER_WRITABLE {
        if normalised == *forbidden {
            return Err(SandboxError::ForbiddenWritable(path.to_path_buf()));
        }
    }
    // The home itself, which the tmpfs is masking. A subdirectory is fine.
    if !home.as_os_str().is_empty() && normalised == home.to_string_lossy().trim_end_matches('/') {
        return Err(SandboxError::ForbiddenWritable(path.to_path_buf()));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec() -> SandboxSpec {
        let mut s = SandboxSpec::new(
            SandboxPolicy::Project,
            PathBuf::from("/home/tester"),
            PathBuf::from("/run/user/1000"),
        );
        s.control_socket = PathBuf::from("/run/user/1000/apex-agentd/control.sock");
        s.scratch = PathBuf::from("/tmp/apex-agent/1");
        s.cwd = PathBuf::from("/home/tester/Projects/demo");
        s.rw = vec![PathBuf::from("/home/tester/Projects/demo")];
        s.ro = vec![PathBuf::from("/home/tester/.cargo")];
        s
    }

    fn argv(s: &SandboxSpec) -> Vec<String> {
        build_argv(s, "claude", &["--help".to_string()]).expect("build")
    }

    /// Index of the first occurrence of `needle` in `argv`.
    fn pos(argv: &[String], needle: &str) -> Option<usize> {
        argv.iter().position(|a| a == needle)
    }

    /// Whether `argv` contains the three-token sequence `op src dst`.
    fn has_bind(argv: &[String], op: &str, path: &str) -> bool {
        argv.windows(3)
            .any(|w| w[0] == op && w[1] == path && w[2] == path)
    }

    #[test]
    fn unrestricted_does_not_wrap_the_command_at_all() {
        let mut s = spec();
        s.policy = SandboxPolicy::Unrestricted;
        let a = build_argv(&s, "claude", &["hello".into()]).unwrap();
        assert_eq!(a, vec!["claude".to_string(), "hello".to_string()]);
        assert!(!a.iter().any(|x| x.contains("bwrap")));
    }

    #[test]
    fn confined_argv_starts_with_the_absolute_bwrap_path() {
        let a = argv(&spec());
        assert_eq!(a[0], "/usr/bin/bwrap");
    }

    #[test]
    fn root_is_bound_read_only_before_anything_overlays_it() {
        let a = argv(&spec());
        let ro_root = a
            .windows(3)
            .position(|w| w[0] == "--ro-bind" && w[1] == "/" && w[2] == "/")
            .expect("root ro-bind");
        let home_tmpfs = a
            .windows(2)
            .position(|w| w[0] == "--tmpfs" && w[1] == "/home/tester")
            .expect("home tmpfs");
        assert!(
            ro_root < home_tmpfs,
            "the read-only root must be laid down first, else the masks are overwritten"
        );
    }

    #[test]
    fn home_and_runtime_dir_are_masked() {
        let a = argv(&spec());
        assert!(a.windows(2).any(|w| w[0] == "--tmpfs" && w[1] == "/home/tester"));
        assert!(a
            .windows(2)
            .any(|w| w[0] == "--tmpfs" && w[1] == "/run/user/1000"));
    }

    #[test]
    fn secrets_are_never_bound_back() {
        // The point of default-deny: no allowlist entry mentions these, so no
        // bind for them can appear anywhere in the argv.
        let a = argv(&spec()).join(" ");
        for secret in [
            "/home/tester/.ssh",
            "/home/tester/.gnupg",
            "/home/tester/.aws",
            "/home/tester/.mozilla",
            "/home/tester/.config/google-chrome",
            "/home/tester/Documents",
        ] {
            assert!(!a.contains(secret), "{secret} must not appear in {a}");
        }
    }

    #[test]
    fn the_project_is_writable_and_the_toolchain_cache_is_not() {
        let a = argv(&spec());
        assert!(has_bind(&a, "--bind-try", "/home/tester/Projects/demo"));
        assert!(has_bind(&a, "--ro-bind-try", "/home/tester/.cargo"));
        assert!(!has_bind(&a, "--bind-try", "/home/tester/.cargo"));
    }

    #[test]
    fn the_masked_home_is_laid_down_before_the_paths_bound_back_into_it() {
        let a = argv(&spec());
        let tmpfs = a
            .windows(2)
            .position(|w| w[0] == "--tmpfs" && w[1] == "/home/tester")
            .unwrap();
        let project = a
            .windows(3)
            .position(|w| w[0] == "--bind-try" && w[1] == "/home/tester/Projects/demo")
            .unwrap();
        let cargo = a
            .windows(3)
            .position(|w| w[0] == "--ro-bind-try" && w[1] == "/home/tester/.cargo")
            .unwrap();
        assert!(tmpfs < project, "tmpfs would erase the project bind");
        assert!(tmpfs < cargo, "tmpfs would erase the cache bind");
    }

    #[test]
    fn a_minimal_dev_removes_camera_and_microphone() {
        // `--dev` builds a fresh devtmpfs with only the standard character
        // devices; there is no bind that could bring /dev/video* or /dev/snd
        // back.
        let a = argv(&spec());
        assert!(a.windows(2).any(|w| w[0] == "--dev" && w[1] == "/dev"));
        let joined = a.join(" ");
        assert!(!joined.contains("/dev/video"));
        assert!(!joined.contains("/dev/snd"));
        assert!(!joined.contains("--dev-bind"));
    }

    #[test]
    fn project_policy_keeps_the_network_and_strict_removes_it() {
        let mut s = spec();
        s.policy = SandboxPolicy::Project;
        assert!(pos(&argv(&s), "--unshare-net").is_none());
        s.policy = SandboxPolicy::Strict;
        assert!(pos(&argv(&s), "--unshare-net").is_some());
    }

    #[test]
    fn strict_keeps_every_project_restriction() {
        let mut s = spec();
        s.policy = SandboxPolicy::Strict;
        let a = argv(&s);
        assert!(a.windows(2).any(|w| w[0] == "--tmpfs" && w[1] == "/home/tester"));
        assert!(has_bind(&a, "--bind-try", "/home/tester/Projects/demo"));
        assert!(pos(&a, "--unshare-pid").is_some());
    }

    #[test]
    fn pid_namespace_is_always_unshared_when_confined() {
        assert!(pos(&argv(&spec()), "--unshare-pid").is_some());
    }

    #[test]
    fn no_new_session_so_the_pty_stays_the_controlling_terminal() {
        // `--new-session` would call setsid() and detach the PTY, breaking
        // every TUI agent. preflight() covers the TIOCSTI risk instead.
        assert!(pos(&argv(&spec()), "--new-session").is_none());
    }

    #[test]
    fn the_command_is_separated_from_bwrap_options_by_a_double_dash() {
        let a = argv(&spec());
        let sep = pos(&a, "--").expect("separator");
        assert_eq!(a[sep + 1], "claude");
        assert_eq!(a[sep + 2], "--help");
        assert_eq!(sep + 3, a.len());
    }

    #[test]
    fn an_argument_that_looks_like_a_bwrap_flag_is_not_interpreted_as_one() {
        // Everything after `--` belongs to the agent. A prompt of
        // "--unshare-net" must reach the agent, not reconfigure the sandbox.
        let s = spec();
        let a = build_argv(&s, "claude", &["--unshare-net".to_string()]).unwrap();
        let sep = pos(&a, "--").unwrap();
        assert_eq!(a[sep + 2], "--unshare-net");
        assert!(
            a[..sep].iter().all(|x| x != "--unshare-net"),
            "the agent argument leaked into the sandbox options"
        );
    }

    #[test]
    fn the_environment_is_cleared_before_anything_is_set() {
        let a = argv(&spec());
        let clear = pos(&a, "--clearenv").expect("clearenv");
        let first_setenv = pos(&a, "--setenv");
        if let Some(set) = first_setenv {
            assert!(clear < set, "--clearenv must precede every --setenv");
        }
    }

    #[test]
    fn explicit_values_beat_inherited_ones() {
        let mut s = spec();
        s.env_set = vec![("TERM".into(), "dumb".into())];
        s.env_pass = vec!["TERM".into()];
        let env = resolved_env(&s);
        let terms: Vec<_> = env.iter().filter(|(k, _)| k == "TERM").collect();
        assert_eq!(terms.len(), 1, "TERM set twice: {env:?}");
        assert_eq!(terms[0].1, "dumb");
    }

    #[test]
    fn undeclared_credentials_are_not_inherited() {
        let mut s = spec();
        s.env_pass = vec!["ANTHROPIC_API_KEY".into()];
        let env = resolved_env(&s);
        // GITHUB_TOKEN was never declared by the adapter, so even if the user's
        // shell exports one it cannot appear.
        assert!(!env.iter().any(|(k, _)| k == "GITHUB_TOKEN"));
        assert!(!env.iter().any(|(k, _)| k == "AWS_SECRET_ACCESS_KEY"));
        assert!(!env.iter().any(|(k, _)| k == "SSH_AUTH_SOCK"));
    }

    #[test]
    fn resolved_env_is_sorted_so_the_argv_is_deterministic() {
        let mut s = spec();
        s.env_set = vec![
            ("ZZZ".into(), "1".into()),
            ("AAA".into(), "2".into()),
            ("MMM".into(), "3".into()),
        ];
        let names: Vec<_> = resolved_env(&s).into_iter().map(|(k, _)| k).collect();
        let mut sorted = names.clone();
        sorted.sort();
        assert_eq!(names, sorted);
    }

    #[test]
    fn writable_binds_that_would_defeat_the_policy_are_refused() {
        for bad in ["/", "/usr", "/etc", "/boot", "/usr/", "/home/tester"] {
            let mut s = spec();
            s.rw = vec![PathBuf::from(bad)];
            let err = build_argv(&s, "claude", &[]).unwrap_err();
            assert!(
                matches!(err, SandboxError::ForbiddenWritable(_)),
                "{bad} was accepted as writable: {err:?}"
            );
        }
    }

    #[test]
    fn a_subdirectory_of_home_is_still_allowed_to_be_writable() {
        let mut s = spec();
        s.rw = vec![PathBuf::from("/home/tester/Projects/demo")];
        assert!(build_argv(&s, "claude", &[]).is_ok());
    }

    #[test]
    fn relative_paths_are_refused() {
        let mut s = spec();
        s.rw = vec![PathBuf::from("relative/path")];
        assert!(matches!(
            build_argv(&s, "claude", &[]).unwrap_err(),
            SandboxError::NotAbsolute(_)
        ));

        let mut s = spec();
        s.cwd = PathBuf::from("relative");
        assert!(matches!(
            build_argv(&s, "claude", &[]).unwrap_err(),
            SandboxError::NotAbsolute(_)
        ));
    }

    #[test]
    fn the_control_socket_is_bound_writable_so_events_can_be_published() {
        let a = argv(&spec());
        assert!(has_bind(
            &a,
            "--bind-try",
            "/run/user/1000/apex-agentd/control.sock"
        ));
    }

    #[test]
    fn preflight_is_a_no_op_for_unrestricted() {
        assert_eq!(preflight(SandboxPolicy::Unrestricted), Ok(()));
    }

    #[test]
    fn sandbox_errors_name_the_escape_hatch() {
        // A user who hits this needs to know what to do next, not just that
        // something failed.
        for err in [SandboxError::MissingBwrap, SandboxError::TiocstiEnabled] {
            assert!(
                err.to_string().contains("unrestricted"),
                "unhelpful message: {err}"
            );
        }
    }
}
