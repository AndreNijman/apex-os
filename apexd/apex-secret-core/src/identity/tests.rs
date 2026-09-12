use super::*;
use std::path::PathBuf;

fn parse(text: &str) -> Identities {
    let config = ProjectConfig::parse(Path::new("/p/apex.toml"), text).expect("valid toml");
    Identities::from_project(&config).expect("reads")
}

/// §36's own example, read field for field. The names are the roadmap's, not
/// this build's invention, which is why they are pinned here.
#[test]
fn section_thirty_sixs_own_example_reads_as_the_four_bindings_it_describes() {
    let identities = parse(
        r#"
[identity.github]
account = "example"

[identity.cloudflare]
account = "example"

[identity.ssh]
host_group = "robotics"

[identity.agent]
default = "claude"
"#,
    );
    assert_eq!(
        identities.github,
        Some(GitHubIdentity {
            account: "example".to_string(),
            host: GITHUB_HOST.to_string(),
        })
    );
    assert_eq!(identities.cloudflare.as_deref(), Some("example"));
    assert_eq!(identities.ssh_host_group.as_deref(), Some("robotics"));
    assert_eq!(identities.agent_default.as_deref(), Some("claude"));
}

#[test]
fn a_project_that_binds_nothing_binds_nothing() {
    let identities = parse("[cloudflare]\nzone = \"example.com\"\n");
    assert_eq!(identities, Identities::default());
    assert!(identities.github.is_none());
}

#[test]
fn a_github_binding_may_name_a_host_that_is_not_github_com() {
    let identities = parse("[identity.github]\naccount = \"acme\"\nhost = \"git.acme.example\"\n");
    let github = identities.github.expect("bound");
    assert_eq!(github.account, "acme");
    assert_eq!(github.host, "git.acme.example");
}

// ── the check, which is §36's whole purpose ─────────────────────────────────

fn github(account: &str, host: &str) -> GitHubIdentity {
    GitHubIdentity {
        account: account.to_string(),
        host: host.to_string(),
    }
}

#[test]
fn a_remote_belonging_to_the_bound_account_is_allowed() {
    let id = github("acme", GITHUB_HOST);
    for url in [
        "https://github.com/acme/widgets.git",
        "https://github.com/acme/widgets",
        "https://github.com/ACME/widgets.git",
        "https://x-access-token:secret@github.com/acme/widgets.git",
        "https://github.com:443/acme/widgets.git",
    ] {
        assert_eq!(id.check(url), Ok(()), "{url} was refused");
    }
}

/// The one-line exfiltration §36 exists to stop: an agent adds a remote and
/// pushes the project's code to an account the owner never named.
#[test]
fn a_remote_belonging_to_another_account_is_refused_before_the_credential_is_read() {
    let id = github("acme", GITHUB_HOST);
    let err = id
        .check("https://github.com/someone-else/widgets.git")
        .expect_err("refused");
    assert_eq!(
        err,
        IdentityError::WrongAccount {
            got: "someone-else".to_string(),
            bound: "acme".to_string(),
            host: GITHUB_HOST.to_string(),
        }
    );
    let text = err.to_string();
    assert!(text.contains("someone-else"), "{text}");
    assert!(text.contains("acme"), "{text}");
    assert!(text.contains("wrong account"), "{text}");
}

/// Where a push goes is the credential's host pin. What this binding checks is
/// which account on its OWN host — a second, weaker copy of the pin would be
/// worse than none.
#[test]
fn a_remote_on_another_host_is_left_to_the_credentials_pin() {
    let id = github("acme", GITHUB_HOST);
    for url in [
        "https://gitlab.com/someone-else/widgets.git",
        "https://git.acme.example/someone-else/widgets.git",
        "https://127.0.0.1:9/anything/widgets.git",
    ] {
        assert_eq!(id.check(url), Ok(()), "{url} was refused by the identity");
    }
}

/// A URL on the bound host that no account can be read out of is refused
/// rather than allowed. "I could not tell" is not "it is fine".
#[test]
fn a_url_on_the_bound_host_with_no_account_in_it_is_refused_and_not_waved_through() {
    let id = github("acme", GITHUB_HOST);
    for url in ["https://github.com", "https://github.com/", "https://github.com//"] {
        let err = id.check(url).expect_err(&format!("{url} was allowed"));
        assert!(matches!(err, IdentityError::Unreadable { .. }), "{err:?}");
        assert!(err.to_string().contains("will not guess"), "{err}");
    }
}

/// An ssh remote reaches this function only if something else let it; it is
/// not an http URL, so no host can be read and it is left alone. The git
/// provider refuses ssh remotes for its own reason a few lines later.
#[test]
fn a_url_that_is_not_http_is_not_this_bindings_business() {
    let id = github("acme", GITHUB_HOST);
    for url in [
        "git@github.com:someone-else/widgets.git",
        "ssh://git@github.com/someone-else/widgets.git",
        "",
        "file:///tmp/repo",
    ] {
        assert_eq!(id.check(url), Ok(()), "{url}");
    }
}

#[test]
fn a_host_is_read_out_of_a_url_the_way_git_would_write_one() {
    assert_eq!(
        host_and_owner("https://github.com/acme/widgets.git"),
        Some(("github.com".to_string(), Some("acme".to_string())))
    );
    assert_eq!(
        host_and_owner("http://user:pw@127.0.0.1:8080/x/y.git"),
        Some(("127.0.0.1".to_string(), Some("x".to_string())))
    );
    assert_eq!(
        host_and_owner("https://github.com"),
        Some(("github.com".to_string(), None))
    );
    assert_eq!(host_and_owner("git@github.com:acme/widgets.git"), None);
    assert_eq!(host_and_owner("https:///acme/widgets.git"), None);
}

// ── declared is not enforced, and the report says which ─────────────────────

/// A report that listed four bindings without saying that two of them are
/// checked and two are not would tell an operator their ssh host group
/// protects something.
#[test]
fn the_report_says_of_every_binding_whether_anything_checks_it() {
    let identities = parse(
        r#"
[identity.github]
account = "acme"

[identity.ssh]
host_group = "robotics"
"#,
    );
    let report = identities.report();
    assert_eq!(report.len(), 4, "§36 names four sections");

    let github = &report[&Kind::GitHub];
    assert_eq!(github.binds.as_deref(), Some("account = acme on github.com"));
    assert!(github.enforced);
    assert!(github.enforced_by.contains("git provider"));

    let ssh = &report[&Kind::Ssh];
    assert_eq!(ssh.binds.as_deref(), Some("host_group = robotics"));
    assert!(!ssh.enforced, "nothing in this build checks an ssh host group");
    assert!(ssh.enforced_by.contains("nothing yet"), "{}", ssh.enforced_by);

    // A section the project does not have is still reported, so the surface is
    // the same four every time.
    assert!(report[&Kind::Agent].binds.is_none());
    assert!(report[&Kind::Cloudflare].binds.is_none());
}

/// Exactly two of §36's four are checked by this build. Pinned, so that adding
/// an enforcement point without saying so fails a test, and so that claiming
/// one that does not exist does too.
#[test]
fn exactly_github_and_cloudflare_are_enforced() {
    let enforced: Vec<&str> = Kind::ALL
        .into_iter()
        .filter(|k| k.is_enforced())
        .map(Kind::as_str)
        .collect();
    assert_eq!(enforced, vec!["github", "cloudflare"]);

    for kind in Kind::ALL {
        let where_ = kind.enforced_by();
        assert!(where_.len() > 40, "{kind:?}'s answer is a stub: {where_}");
        if kind.is_enforced() {
            assert!(
                !where_.starts_with("nothing"),
                "{kind:?} claims enforcement and names nowhere"
            );
        } else {
            assert!(
                where_.starts_with("nothing yet"),
                "{kind:?} is not enforced and does not say so: {where_}"
            );
        }
    }
}

#[test]
fn the_four_names_are_section_thirty_sixs_own() {
    let names: Vec<&str> = Kind::ALL.into_iter().map(Kind::as_str).collect();
    assert_eq!(names, vec!["github", "cloudflare", "ssh", "agent"]);
}

// ── a file that could not be read is not a file that says nothing ───────────

fn fixture() -> tempfile::TempDir {
    tempfile::Builder::new()
        .prefix("apex-identity-")
        .tempdir_in("/var/tmp")
        .expect("a fixture under /var/tmp")
}

#[test]
fn a_project_with_no_apex_toml_binds_nothing() {
    let dir = fixture();
    let identities = Identities::read_or_unbound(dir.path(), uid(), &name()).expect("no file");
    assert_eq!(identities, Identities::default());
}

/// The distinction this whole function exists for, and the one this repository
/// keeps having to make. Treating an unreadable `apex.toml` as "binds nothing"
/// is how an agent that can `chmod 000 apex.toml` takes the check away.
#[test]
fn an_unreadable_apex_toml_is_an_error_and_never_binds_nothing() {
    use std::os::unix::fs::PermissionsExt;
    if unsafe { libc::geteuid() } == 0 {
        eprintln!(
            "SKIP  an_unreadable_apex_toml_is_an_error_and_never_binds_nothing: \
             running as root, which mode bits do not stop. NOT ASSERTED: that a \
             project file the kernel refuses is an error rather than a project \
             that binds nothing."
        );
        return;
    }
    let dir = fixture();
    let path = dir.path().join("apex.toml");
    std::fs::write(&path, "[identity.github]\naccount = \"acme\"\n").expect("writes");
    // It really did bind something a moment ago.
    assert!(Identities::read_or_unbound(dir.path(), uid(), &name())
        .expect("reads")
        .github
        .is_some());

    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o000)).expect("chmod");
    let got = Identities::read_or_unbound(dir.path(), uid(), &name());
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).expect("chmod");

    let err = got.map(|_| ()).expect_err("an unreadable file is an error");
    assert!(
        !matches!(err, ProjectError::Absent { .. }),
        "an unreadable apex.toml was reported as absent: {err:?}"
    );
}

/// The same rule, for the other ways a project file can fail to be one.
#[test]
fn a_malformed_apex_toml_is_an_error_and_never_binds_nothing() {
    let dir = fixture();
    std::fs::write(dir.path().join("apex.toml"), "[identity.github\n").expect("writes");
    let err = Identities::read_or_unbound(dir.path(), uid(), &name())
        .map(|_| ())
        .expect_err("malformed");
    assert!(matches!(err, ProjectError::Malformed { .. }), "{err:?}");
    // Position only. This message reaches the audit trail.
    assert!(!err.to_string().contains("identity.github"), "{err}");
}

#[test]
fn an_apex_toml_owned_by_somebody_else_is_an_error() {
    if unsafe { libc::geteuid() } == 0 {
        eprintln!(
            "SKIP  an_apex_toml_owned_by_somebody_else_is_an_error: running as \
             root, so every file is owned by an account this can read. NOT \
             ASSERTED: that a project file the operation's account could not \
             have written is refused."
        );
        return;
    }
    let dir = fixture();
    std::fs::write(dir.path().join("apex.toml"), "[identity.github]\naccount=\"a\"\n")
        .expect("writes");
    // Ask as a uid that is not the file's owner.
    let err = Identities::read_or_unbound(dir.path(), uid() + 1, "somebody-else")
        .map(|_| ())
        .expect_err("not owned");
    assert!(matches!(err, ProjectError::NotOwned { .. }), "{err:?}");
}

#[test]
fn a_symlinked_apex_toml_is_an_error() {
    let dir = fixture();
    let elsewhere = dir.path().join("elsewhere.toml");
    std::fs::write(&elsewhere, "[identity.github]\naccount = \"acme\"\n").expect("writes");
    std::os::unix::fs::symlink(&elsewhere, dir.path().join("apex.toml")).expect("symlink");
    let err = Identities::read_or_unbound(dir.path(), uid(), &name())
        .map(|_| ())
        .expect_err("a symlink is not followed");
    assert!(matches!(err, ProjectError::Symlink { .. }), "{err:?}");
}

fn uid() -> u32 {
    unsafe { libc::getuid() }
}

fn name() -> String {
    std::env::var("USER").unwrap_or_else(|_| "tester".to_string())
}

/// `PathBuf` is used by the fixture helpers above; this keeps the import
/// honest rather than silencing it.
#[test]
fn a_fixture_path_is_a_path() {
    let dir = fixture();
    let as_buf: PathBuf = dir.path().to_path_buf();
    assert!(as_buf.is_dir());
}
