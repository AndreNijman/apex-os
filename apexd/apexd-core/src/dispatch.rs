//! Running work on another APEX device (roadmap §20): the pure decisions.
//!
//! §20 asks for builds, agents and local-model inference to run on a stronger
//! machine while being driven from a laptop:
//!
//! ```text
//! apex agent run --host desktop claude
//! apex build --on desktop
//! apex ai run --on desktop
//! ```
//!
//! [`crate::host`] already owns *reaching* a device. What is left is the part
//! that has nothing to do with ssh: deciding **which directory on the far side
//! is this project**, and deciding whether the answer is trustworthy enough to
//! run a command in.
//!
//! Nothing in this module performs I/O.
//!
//! ── Why the remote path is verified rather than configured or guessed ───────
//!
//! The files are on the laptop; the compute is on the desktop. Three options:
//!
//! | option | rejected because |
//! | --- | --- |
//! | Same absolute path, assumed | Silently building the wrong tree is much worse than refusing. Two machines with the same username and a slightly different layout is the *normal* case, not an exotic one. |
//! | Locate by git identity and clone on demand | Turns a dispatch verb into a repository write on someone else's machine. Uncommitted work is still unhandled, so it does not even buy correctness. |
//! | A configured path map per host | Real configuration complexity for a case that is usually trivial, and one more thing to be out of date. |
//!
//! What this does instead: assume the same absolute path, then **prove it**.
//! The remote directory must exist *and* be the same repository — the `origin`
//! remote URL compared on both ends — before anything runs in it. A mismatch is
//! a refusal that prints both values, and `--remote-path` is the explicit
//! override for a machine that really is laid out differently.
//!
//! For one person with two APEX boxes this needs no configuration at all, and
//! when the assumption is wrong it fails loudly instead of quietly. That is the
//! whole argument: the failure mode of a wrong guess here is a build that
//! *succeeds* against the wrong source.
//!
//! ── Comparing two git URLs is not string equality ──────────────────────────
//!
//! The same repository is spelled several ways, and two checkouts of one
//! project routinely disagree about which:
//!
//! ```text
//! git@github.com:AndreNijman/apex-os.git
//! https://github.com/AndreNijman/apex-os.git
//! https://github.com/AndreNijman/apex-os
//! ssh://git@github.com/AndreNijman/apex-os.git
//! ```
//!
//! All four are one repo. [`same_repo`] normalises to `host/path`, lowercasing
//! the host (DNS is case-insensitive) but **not** the path, because forge paths
//! are case-sensitive on some hosts and treating `apex-OS` as `apex-os` would
//! be inventing an equivalence. A comparison that got this wrong would refuse
//! every legitimate dispatch, so it is worth the care.

/// Why a remote project could not be resolved.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DispatchError {
    /// The local directory is not inside a git repository, and no explicit
    /// remote path was given.
    NotARepo { local: String },
    /// A path that is not absolute cannot be resolved on another machine.
    RelativePath { path: String },
    /// The remote directory does not exist.
    RemoteMissing { host: String, path: String },
    /// The remote directory exists but is a different repository.
    DifferentRepo {
        host: String,
        path: String,
        local_origin: String,
        remote_origin: String,
    },
    /// The remote directory exists and is a git repository, but has no `origin`
    /// — so it cannot be compared, and the safe answer is to say so.
    RemoteNoOrigin { host: String, path: String },
    /// The local repository has no `origin`, so there is nothing to compare
    /// against.
    LocalNoOrigin { local: String },
    /// The local worktree has uncommitted changes and the caller did not say
    /// they were fine with that.
    DirtyWorktree { changed: usize },
}

impl std::fmt::Display for DispatchError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotARepo { local } => write!(
                f,
                "{local} is not inside a git repository, so there is no way to tell which \
                 directory on the remote is the same project. Pass --remote-path to say \
                 explicitly."
            ),
            Self::RelativePath { path } => write!(
                f,
                "remote path {path:?} is relative. A path only means something on the other \
                 machine if it is absolute."
            ),
            Self::RemoteMissing { host, path } => write!(
                f,
                "{host} has no directory at {path}. APEX assumes the project is at the same \
                 absolute path on both machines and then checks — it does not create it. \
                 Either clone it there, or pass --remote-path."
            ),
            Self::DifferentRepo { host, path, local_origin, remote_origin } => write!(
                f,
                "{host}:{path} is a different repository, so nothing was run.\n  \
                 here:  {local_origin}\n  there: {remote_origin}\n\
                 Pass --remote-path if the project really does live somewhere else there."
            ),
            Self::RemoteNoOrigin { host, path } => write!(
                f,
                "{host}:{path} is a git repository with no 'origin' remote, so it cannot be \
                 compared with this one. Nothing was run. Pass --remote-path to say you mean \
                 that directory anyway."
            ),
            Self::LocalNoOrigin { local } => write!(
                f,
                "{local} has no 'origin' remote, so there is nothing to compare the remote \
                 checkout against. Pass --remote-path to dispatch anyway."
            ),
            Self::DirtyWorktree { changed } => write!(
                f,
                "{changed} uncommitted change(s) here. The remote builds its own committed \
                 state, so those would NOT be included. Commit and push them, or pass \
                 --allow-dirty to run against what the remote already has."
            ),
        }
    }
}

impl std::error::Error for DispatchError {}

/// Normalise a git remote URL to `host/path` for comparison.
///
/// Returns `None` for something that does not look like a remote URL at all —
/// including a local filesystem path, which two machines cannot meaningfully
/// compare.
///
/// The host is lowercased because DNS is case-insensitive. The path is **not**,
/// because forge paths are case-sensitive on some hosts, and inventing that
/// equivalence would make this function claim two different repositories are
/// one.
pub fn normalise_git_url(url: &str) -> Option<String> {
    let url = url.trim();
    if url.is_empty() {
        return None;
    }

    // scheme://[user@]host[:port]/path
    let rest = if let Some(i) = url.find("://") {
        let after = &url[i + 3..];
        // Strip credentials, which differ between checkouts of one repo.
        let after = after.rsplit_once('@').map_or(after, |(_, h)| h);
        after
    } else if let Some((before, after)) = url.split_once(':') {
        // scp-like: [user@]host:path. Distinguished from a Windows drive letter
        // or a bare path by requiring a non-empty path and no leading slash.
        if before.is_empty() || after.is_empty() || url.starts_with('/') {
            return None;
        }
        let host = before.rsplit_once('@').map_or(before, |(_, h)| h);
        // `host:path` -> `host/path`
        let path = after.trim_start_matches('/');
        return Some(finish(host, path));
    } else {
        // A bare path, or a hostname with no path. Neither is comparable.
        return None;
    };

    let (host, path) = rest.split_once('/')?;
    // Drop a port: the same repo reached on two ports is the same repo.
    let host = host.split_once(':').map_or(host, |(h, _)| h);
    if host.is_empty() || path.is_empty() {
        return None;
    }
    Some(finish(host, path))
}

fn finish(host: &str, path: &str) -> String {
    let path = path.trim_end_matches('/');
    let path = path.strip_suffix(".git").unwrap_or(path);
    format!("{}/{}", host.to_ascii_lowercase(), path)
}

/// Whether two git remote URLs name the same repository.
///
/// Both must normalise; two URLs this cannot understand are **not** treated as
/// equal even if their strings match, because the caller uses this to decide
/// whether it is safe to run a command, and "I do not understand either of
/// these" is not evidence of sameness.
pub fn same_repo(a: &str, b: &str) -> bool {
    match (normalise_git_url(a), normalise_git_url(b)) {
        (Some(x), Some(y)) => x == y,
        _ => false,
    }
}

/// The remote directory a dispatch should run in.
///
/// `explicit` is `--remote-path`, which skips the identity check entirely: the
/// user has said where, and second-guessing an explicit instruction would be
/// worse than obeying it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RemoteDir {
    /// Use this path, and verify it is the same repository first.
    Verify { path: String, local_origin: String },
    /// Use this path because the user said so. No check.
    AsTold { path: String },
}

impl RemoteDir {
    pub fn path(&self) -> &str {
        match self {
            Self::Verify { path, .. } | Self::AsTold { path } => path,
        }
    }
}

/// Decide which remote directory to use, and whether it needs verifying.
///
/// `local_root` is the local repository root, `local_origin` its `origin` URL
/// (`None` if it has none), and `explicit` the `--remote-path` override.
pub fn plan_remote_dir(
    local_root: &str,
    local_origin: Option<&str>,
    explicit: Option<&str>,
) -> Result<RemoteDir, DispatchError> {
    if let Some(p) = explicit {
        if !p.starts_with('/') {
            return Err(DispatchError::RelativePath { path: p.to_string() });
        }
        return Ok(RemoteDir::AsTold { path: p.to_string() });
    }
    if !local_root.starts_with('/') {
        return Err(DispatchError::RelativePath { path: local_root.to_string() });
    }
    let origin = local_origin.ok_or_else(|| DispatchError::LocalNoOrigin {
        local: local_root.to_string(),
    })?;
    Ok(RemoteDir::Verify {
        path: local_root.to_string(),
        local_origin: origin.to_string(),
    })
}

/// What the remote reported when asked to identify a directory.
///
/// Deliberately three states rather than two: "there is no directory" and
/// "there is a directory but it is not a repository with an origin" lead to
/// different messages, and collapsing them would send the user looking for the
/// wrong problem.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RemoteIdent {
    /// No such directory.
    Missing,
    /// The directory exists but has no `origin` remote.
    NoOrigin,
    /// The directory exists and `origin` is this.
    Origin(String),
}

/// Parse the identity probe's output.
///
/// The probe prints one of three things, chosen so that the states cannot be
/// confused with each other or with an empty answer from a broken connection:
/// `MISSING`, `NO_ORIGIN`, or `ORIGIN <url>`.
pub fn parse_remote_ident(out: &str) -> Option<RemoteIdent> {
    let line = out.lines().map(str::trim).find(|l| !l.is_empty())?;
    if line == "MISSING" {
        return Some(RemoteIdent::Missing);
    }
    if line == "NO_ORIGIN" {
        return Some(RemoteIdent::NoOrigin);
    }
    let url = line.strip_prefix("ORIGIN ")?.trim();
    if url.is_empty() {
        return None;
    }
    Some(RemoteIdent::Origin(url.to_string()))
}

/// Decide whether a verified remote directory may be used.
pub fn check_ident(
    host: &str,
    dir: &RemoteDir,
    ident: &RemoteIdent,
) -> Result<(), DispatchError> {
    let RemoteDir::Verify { path, local_origin } = dir else {
        // `--remote-path` was given; the user has spoken.
        return Ok(());
    };
    match ident {
        RemoteIdent::Missing => Err(DispatchError::RemoteMissing {
            host: host.to_string(),
            path: path.clone(),
        }),
        RemoteIdent::NoOrigin => Err(DispatchError::RemoteNoOrigin {
            host: host.to_string(),
            path: path.clone(),
        }),
        RemoteIdent::Origin(remote) => {
            if same_repo(local_origin, remote) {
                Ok(())
            } else {
                Err(DispatchError::DifferentRepo {
                    host: host.to_string(),
                    path: path.clone(),
                    local_origin: local_origin.clone(),
                    remote_origin: remote.clone(),
                })
            }
        }
    }
}

/// A marker file a build system is recognised by, and the command it implies.
///
/// Ordered, and the order is the policy: a repository with both a
/// `build-local.sh` and a `Cargo.toml` means the script, because someone wrote
/// the script *for this repository* and cargo is a default.
///
/// This list is short on purpose. A dispatch that guessed wrong would run the
/// wrong build on another machine, so anything not recognised here is a refusal
/// that lists what was looked for — not a fallback to something plausible.
const BUILD_MARKERS: &[(&str, &[&str])] = &[
    ("build-local.sh", &["./build-local.sh"]),
    ("Makefile", &["make"]),
    ("Cargo.toml", &["cargo", "build", "--locked"]),
    ("package.json", &["npm", "run", "build"]),
    ("meson.build", &["meson", "compile", "-C", "build"]),
];

/// What `apex build --on` should run, given which marker files exist.
///
/// `present` is the set of names found in the project root. Returns the argv
/// and the marker that chose it, so the caller can *print* which it picked —
/// a detector that silently selects between five possibilities is one the user
/// cannot correct.
pub fn detect_build(present: &[String]) -> Option<(&'static str, Vec<String>)> {
    for (marker, argv) in BUILD_MARKERS {
        if present.iter().any(|p| p == marker) {
            return Some((marker, argv.iter().map(|s| s.to_string()).collect()));
        }
    }
    None
}

/// Every marker `detect_build` knows, for the refusal message.
pub fn build_markers() -> Vec<&'static str> {
    BUILD_MARKERS.iter().map(|(m, _)| *m).collect()
}

/// Whether a dirty local worktree should stop a dispatch.
///
/// It is a refusal rather than a warning because the failure it prevents is
/// silent: the remote builds its own committed state, so a dispatch from a
/// dirty tree produces a result that looks right and does not contain the
/// change being tested. A warning printed above ten minutes of build output is
/// a warning nobody reads.
pub fn check_clean(changed: usize, allow_dirty: bool) -> Result<(), DispatchError> {
    if changed > 0 && !allow_dirty {
        return Err(DispatchError::DirtyWorktree { changed });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── git URL normalisation ────────────────────────────────────────────────

    #[test]
    fn the_four_spellings_of_one_repository_are_one_repository() {
        // Every one of these is github.com/AndreNijman/apex-os, and two
        // checkouts of one project routinely disagree about which they use.
        let forms = [
            "git@github.com:AndreNijman/apex-os.git",
            "https://github.com/AndreNijman/apex-os.git",
            "https://github.com/AndreNijman/apex-os",
            "ssh://git@github.com/AndreNijman/apex-os.git",
            "git://github.com/AndreNijman/apex-os.git",
        ];
        for a in &forms {
            for b in &forms {
                assert!(same_repo(a, b), "{a} != {b}");
            }
        }
    }

    #[test]
    fn the_normal_form_is_host_slash_path() {
        assert_eq!(
            normalise_git_url("git@github.com:AndreNijman/apex-os.git").unwrap(),
            "github.com/AndreNijman/apex-os"
        );
    }

    #[test]
    fn a_different_repository_on_the_same_host_is_different() {
        assert!(!same_repo(
            "git@github.com:AndreNijman/apex-os.git",
            "git@github.com:AndreNijman/apex-shell.git"
        ));
    }

    #[test]
    fn a_different_host_with_the_same_path_is_different() {
        // A fork on another forge is not the same repository.
        assert!(!same_repo(
            "https://github.com/a/b.git",
            "https://gitlab.com/a/b.git"
        ));
    }

    #[test]
    fn the_host_is_case_insensitive_because_dns_is() {
        assert!(same_repo(
            "https://GitHub.com/a/b.git",
            "https://github.com/a/b.git"
        ));
    }

    #[test]
    fn the_path_is_case_sensitive_because_forges_are() {
        // Inventing this equivalence would make the function claim two
        // different repositories are one, which is the failure that matters.
        assert!(!same_repo(
            "https://github.com/a/apex-OS.git",
            "https://github.com/a/apex-os.git"
        ));
    }

    #[test]
    fn credentials_in_a_url_do_not_change_the_repository() {
        assert!(same_repo(
            "https://token:x@github.com/a/b.git",
            "https://github.com/a/b.git"
        ));
    }

    #[test]
    fn a_port_does_not_change_the_repository() {
        assert!(same_repo(
            "ssh://git@example.com:2222/a/b.git",
            "ssh://git@example.com/a/b.git"
        ));
    }

    #[test]
    fn a_trailing_slash_does_not_change_the_repository() {
        assert!(same_repo(
            "https://github.com/a/b/",
            "https://github.com/a/b"
        ));
    }

    #[test]
    fn a_local_path_is_not_comparable_between_two_machines() {
        // `/srv/git/thing` means different things on two hosts, so this must
        // not normalise — and must not compare equal to itself either.
        assert_eq!(normalise_git_url("/srv/git/thing"), None);
        assert_eq!(normalise_git_url("../thing"), None);
        assert!(!same_repo("/srv/git/thing", "/srv/git/thing"));
    }

    #[test]
    fn two_urls_neither_of_which_parses_are_not_equal() {
        // The caller uses this to decide whether running a command is safe.
        // "I understand neither of these" is not evidence of sameness, even
        // when the strings match exactly.
        assert!(!same_repo("nonsense", "nonsense"));
        assert!(!same_repo("", ""));
    }

    #[test]
    fn a_bare_hostname_with_no_path_does_not_normalise() {
        assert_eq!(normalise_git_url("https://github.com"), None);
        assert_eq!(normalise_git_url("https://github.com/"), None);
    }

    // ── planning the remote directory ────────────────────────────────────────

    #[test]
    fn the_default_plan_is_the_same_path_and_it_must_be_verified() {
        let p = plan_remote_dir("/home/a/p", Some("git@h:a/b.git"), None).unwrap();
        assert_eq!(
            p,
            RemoteDir::Verify {
                path: "/home/a/p".into(),
                local_origin: "git@h:a/b.git".into()
            }
        );
    }

    #[test]
    fn an_explicit_remote_path_skips_the_check_entirely() {
        // The user has said where. Second-guessing an explicit instruction
        // would be worse than obeying it.
        let p = plan_remote_dir("/home/a/p", Some("git@h:a/b.git"), Some("/other/place")).unwrap();
        assert_eq!(p, RemoteDir::AsTold { path: "/other/place".into() });
        assert!(check_ident("k", &p, &RemoteIdent::Missing).is_ok());
        assert!(check_ident("k", &p, &RemoteIdent::Origin("git@h:z/z.git".into())).is_ok());
    }

    #[test]
    fn a_relative_remote_path_is_refused() {
        let e = plan_remote_dir("/home/a/p", Some("x"), Some("relative/path")).unwrap_err();
        assert!(matches!(e, DispatchError::RelativePath { .. }));
        assert!(e.to_string().contains("absolute"));
    }

    #[test]
    fn no_local_origin_is_refused_with_the_override_named() {
        let e = plan_remote_dir("/home/a/p", None, None).unwrap_err();
        assert!(matches!(e, DispatchError::LocalNoOrigin { .. }));
        assert!(e.to_string().contains("--remote-path"), "got {e}");
    }

    // ── the identity check ───────────────────────────────────────────────────

    #[test]
    fn a_matching_origin_permits_the_dispatch() {
        let dir = RemoteDir::Verify {
            path: "/p".into(),
            local_origin: "git@github.com:a/b.git".into(),
        };
        // Deliberately a different spelling of the same repo, which is the
        // realistic case: one checkout over ssh, the other over https.
        let ident = RemoteIdent::Origin("https://github.com/a/b".into());
        assert!(check_ident("katana", &dir, &ident).is_ok());
    }

    #[test]
    fn a_different_origin_refuses_and_prints_both() {
        let dir = RemoteDir::Verify {
            path: "/p".into(),
            local_origin: "git@github.com:a/b.git".into(),
        };
        let ident = RemoteIdent::Origin("git@github.com:a/other.git".into());
        let e = check_ident("katana", &dir, &ident).unwrap_err();
        let msg = e.to_string();
        assert!(msg.contains("a/b"), "local origin missing from {msg}");
        assert!(msg.contains("a/other"), "remote origin missing from {msg}");
        assert!(msg.contains("nothing was run"), "got {msg}");
    }

    #[test]
    fn a_missing_remote_directory_says_apex_does_not_create_it() {
        let dir = RemoteDir::Verify { path: "/p".into(), local_origin: "x".into() };
        let e = check_ident("katana", &dir, &RemoteIdent::Missing).unwrap_err();
        assert!(e.to_string().contains("does not create it"), "got {e}");
    }

    #[test]
    fn a_remote_repo_with_no_origin_is_refused_rather_than_assumed_right() {
        let dir = RemoteDir::Verify { path: "/p".into(), local_origin: "x".into() };
        let e = check_ident("katana", &dir, &RemoteIdent::NoOrigin).unwrap_err();
        assert!(matches!(e, DispatchError::RemoteNoOrigin { .. }));
    }

    #[test]
    fn the_three_remote_states_produce_three_different_messages() {
        // Collapsing any two would send the user looking for the wrong problem.
        let dir = RemoteDir::Verify { path: "/p".into(), local_origin: "git@h:a/b".into() };
        let msgs: Vec<String> = [
            RemoteIdent::Missing,
            RemoteIdent::NoOrigin,
            RemoteIdent::Origin("git@h:z/z".into()),
        ]
        .iter()
        .map(|i| check_ident("k", &dir, i).unwrap_err().to_string())
        .collect();
        assert_eq!(msgs.len(), 3);
        for i in 0..3 {
            for j in (i + 1)..3 {
                assert_ne!(msgs[i], msgs[j], "messages {i} and {j} are identical");
            }
        }
    }

    // ── parsing the identity probe ───────────────────────────────────────────

    #[test]
    fn the_probe_states_parse() {
        assert_eq!(parse_remote_ident("MISSING\n"), Some(RemoteIdent::Missing));
        assert_eq!(parse_remote_ident("NO_ORIGIN\n"), Some(RemoteIdent::NoOrigin));
        assert_eq!(
            parse_remote_ident("ORIGIN git@github.com:a/b.git\n"),
            Some(RemoteIdent::Origin("git@github.com:a/b.git".into()))
        );
    }

    #[test]
    fn an_empty_answer_is_not_a_state() {
        // A broken connection must not read as "MISSING", which would send the
        // user to clone a directory that is already there.
        assert_eq!(parse_remote_ident(""), None);
        assert_eq!(parse_remote_ident("\n\n"), None);
        assert_eq!(parse_remote_ident("ORIGIN "), None);
    }

    #[test]
    fn a_login_banner_before_the_answer_does_not_break_it() {
        // Login shells print things. The first non-empty line is the answer
        // only because the probe is the only thing that prints these tokens —
        // and a banner line that is not one of them yields None rather than a
        // wrong state.
        assert_eq!(parse_remote_ident("\n\nMISSING\n"), Some(RemoteIdent::Missing));
        assert_eq!(parse_remote_ident("Welcome!\nMISSING\n"), None);
    }

    #[test]
    fn an_unknown_token_is_not_silently_treated_as_a_state() {
        assert_eq!(parse_remote_ident("MAYBE\n"), None);
        assert_eq!(parse_remote_ident("missing\n"), None);
    }

    // ── real URLs from the machine this was written on ───────────────────────

    #[test]
    fn the_two_apex_repositories_are_not_confused_for_each_other() {
        // Exactly what `git remote get-url origin` prints in each checkout
        // here. Note they disagree about the .git suffix all on their own,
        // which is the case the normaliser exists for — and it is not a case
        // anyone contrived.
        let os = "https://github.com/AndreNijman/apex-os.git";
        let shell = "https://github.com/AndreNijman/apex-shell";
        assert!(!same_repo(os, shell), "two different repos compared equal");
        assert!(same_repo(os, "https://github.com/AndreNijman/apex-os"));
        assert!(same_repo(shell, "https://github.com/AndreNijman/apex-shell.git"));
        assert!(same_repo(os, "git@github.com:AndreNijman/apex-os.git"));
    }

    // ── the build detector ───────────────────────────────────────────────────

    #[test]
    fn a_repository_script_beats_a_generic_build_system() {
        // Someone wrote build-local.sh for this repository; cargo is a default.
        let present = vec!["Cargo.toml".to_string(), "build-local.sh".to_string()];
        let (marker, argv) = detect_build(&present).unwrap();
        assert_eq!(marker, "build-local.sh");
        assert_eq!(argv, vec!["./build-local.sh"]);
    }

    #[test]
    fn each_marker_selects_its_own_command() {
        for (marker, want) in [
            ("Makefile", vec!["make"]),
            ("Cargo.toml", vec!["cargo", "build", "--locked"]),
            ("package.json", vec!["npm", "run", "build"]),
            ("meson.build", vec!["meson", "compile", "-C", "build"]),
        ] {
            let (got_marker, argv) = detect_build(&[marker.to_string()]).unwrap();
            assert_eq!(got_marker, marker);
            assert_eq!(argv, want, "for {marker}");
        }
    }

    #[test]
    fn an_unrecognised_project_is_a_refusal_not_a_guess() {
        // A wrong guess runs the wrong build on another machine.
        assert!(detect_build(&["setup.py".to_string()]).is_none());
        assert!(detect_build(&[]).is_none());
    }

    #[test]
    fn every_marker_is_listable_for_the_refusal_message() {
        // The refusal must be able to say what it looked for, so this list and
        // the table cannot drift apart.
        let m = build_markers();
        assert_eq!(m.len(), BUILD_MARKERS.len());
        assert!(m.contains(&"Cargo.toml"));
    }

    #[test]
    fn detection_is_by_exact_name_not_substring() {
        // `Cargo.toml.orig` or `my-Makefile` must not select a build.
        assert!(detect_build(&["Cargo.toml.orig".to_string()]).is_none());
        assert!(detect_build(&["my-Makefile".to_string()]).is_none());
    }

    // ── the dirty-worktree gate ──────────────────────────────────────────────

    #[test]
    fn a_dirty_worktree_refuses_by_default() {
        let e = check_clean(3, false).unwrap_err();
        assert!(matches!(e, DispatchError::DirtyWorktree { changed: 3 }));
        // The message has to say what will actually happen, because the whole
        // point is that the wrong outcome looks like success.
        assert!(e.to_string().contains("would NOT be included"), "got {e}");
    }

    #[test]
    fn allow_dirty_permits_it() {
        assert!(check_clean(3, true).is_ok());
    }

    #[test]
    fn a_clean_worktree_needs_no_flag() {
        assert!(check_clean(0, false).is_ok());
    }
}
