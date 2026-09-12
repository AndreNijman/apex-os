use super::*;
use apex_secret_core::project::ProjectConfig;
use std::path::Path;

fn parse(text: &str) -> Result<BackupConfig, ConfigError> {
    let config = ProjectConfig::parse(Path::new("/p/apex.toml"), text).expect("valid toml");
    BackupConfig::from_project(&config)
}

const RECIPIENT: &str = "recipient = \"apexbk1AAAA\"\n";

#[test]
fn a_project_with_no_backup_section_is_told_what_to_do_and_not_told_off() {
    let err = parse("[identity.cloudflare]\naccount = \"acme\"\n").expect_err("refuses");
    assert_eq!(err, ConfigError::Absent);
    assert!(err.to_string().contains("apex backup init"), "{err}");
}

#[test]
fn a_local_target_reads_its_path_and_its_marker() {
    let config = parse(&format!(
        "[backup]\n{RECIPIENT}target = \"local\"\n\n\
         [backup.local]\npath = \"/var/backups/apex\"\nid = \"9f2c4a1b8e7d6503\"\n"
    ))
    .expect("parses");
    assert_eq!(config.kind, Kind::Local);
    assert_eq!(config.prefix, DEFAULT_PREFIX);
    assert_eq!(
        config.where_to,
        Where::Directory {
            path: PathBuf::from("/var/backups/apex"),
            marker: Some("9f2c4a1b8e7d6503".to_string()),
        }
    );
}

#[test]
fn an_r2_target_reads_its_bucket_and_defaults_its_credential_name() {
    let config = parse(&format!(
        "[backup]\n{RECIPIENT}target = \"r2\"\n\n\
         [backup.r2]\nbucket = \"example-backups\"\n"
    ))
    .expect("parses");
    assert_eq!(config.kind, Kind::R2);
    assert_eq!(
        config.where_to,
        Where::Bucket {
            bucket: "example-backups".to_string(),
            service: "cloudflare".to_string(),
        }
    );
}

/// Every kind this build refuses, refused where the project names it — reading
/// the file — and not at the first upload.
///
/// Driven off `Kind::ALL` rather than a list written here, so that the day a
/// kind stops being carried this test covers it without anybody remembering to
/// add it. `s3` is the only one today; `ssh` was the other until P2-001's
/// second round.
#[test]
fn an_unimplemented_target_is_refused_at_configuration_time_with_its_reason() {
    let mut checked = 0;
    for kind in Kind::ALL.into_iter().filter(|k| !k.is_implemented()) {
        let err = parse(&format!(
            "[backup]\n{RECIPIENT}target = \"{kind}\"\n\n[backup.{kind}]\nhost = \"nas.lan\"\n"
        ))
        .expect_err("refuses");
        let ConfigError::Unimplemented { why, .. } = &err else {
            panic!("{kind} was accepted: {err:?}");
        };
        assert!(why.len() > 80, "{kind}'s reason is a stub");
        assert!(err.to_string().contains("will not back up"), "{err}");
        checked += 1;
    }
    assert_eq!(
        checked, 1,
        "the set of refused kinds changed; a loop over an empty set would have \
         passed having asserted nothing"
    );
}

#[test]
fn a_target_kind_this_build_never_heard_of_names_the_ones_it_knows() {
    let err = parse(&format!("[backup]\n{RECIPIENT}target = \"dropbox\"\n"))
        .expect_err("refuses");
    let text = err.to_string();
    for kind in ["local", "nas", "ssh", "s3", "r2"] {
        assert!(text.contains(kind), "{kind} is missing from: {text}");
    }
}

#[test]
fn a_directory_target_without_a_path_says_which_key_is_missing() {
    for kind in ["local", "nas"] {
        let err = parse(&format!("[backup]\n{RECIPIENT}target = \"{kind}\"\n"))
            .expect_err("refuses");
        assert!(matches!(err, ConfigError::Missing { .. }), "{err:?}");
        assert!(err.to_string().contains(kind), "{err}");
    }
}

/// A relative target path means a different directory depending on where the
/// command was run, which for a backup means snapshots scattered across the
/// filesystem.
#[test]
fn a_relative_target_path_is_refused() {
    let err = parse(&format!(
        "[backup]\n{RECIPIENT}target = \"local\"\n\n[backup.local]\npath = \"backups\"\n"
    ))
    .expect_err("refuses");
    assert!(err.to_string().contains("absolute"), "{err}");
}

#[test]
fn a_missing_recipient_says_how_to_make_one() {
    let err = parse("[backup]\ntarget = \"local\"\n\n[backup.local]\npath = \"/b\"\n")
        .expect_err("refuses");
    assert!(err.to_string().contains("apex backup key init"), "{err}");
}

/// A prefix becomes one segment of an object key, and the capability framework
/// refuses a segment `valid_name` does not accept.
#[test]
fn a_prefix_that_could_not_be_an_object_key_segment_is_refused() {
    for bad in ["a/b", "..", ".hidden", "", "with space", "-leading"] {
        let err = parse(&format!(
            "[backup]\n{RECIPIENT}target = \"local\"\nprefix = \"{bad}\"\n\n\
             [backup.local]\npath = \"/b\"\n"
        ))
        .expect_err("refuses");
        assert!(matches!(err, ConfigError::Bad { .. }), "{bad:?}: {err:?}");
    }
    for good in ["apex-backup", "backups", "_b", "a.b", "Team1"] {
        let config = parse(&format!(
            "[backup]\n{RECIPIENT}target = \"local\"\nprefix = \"{good}\"\n\n\
             [backup.local]\npath = \"/b\"\n"
        ))
        .expect("parses");
        assert_eq!(config.prefix, good);
    }
}

/// Whole segments, never substrings: `exclude = ["target"]` must not also skip
/// `src/targeting.rs`.
#[test]
fn exclusions_match_whole_path_segments_and_not_substrings() {
    let config = parse(&format!(
        "[backup]\n{RECIPIENT}target = \"local\"\nexclude = [\"target\", \"node_modules\"]\n\n\
         [backup.local]\npath = \"/b\"\n"
    ))
    .expect("parses");

    assert!(config.is_excluded("target"));
    assert!(config.is_excluded("target/debug/x"));
    assert!(config.is_excluded("crates/a/target/x"));
    assert!(config.is_excluded("node_modules"));

    assert!(!config.is_excluded("src/targeting.rs"));
    assert!(!config.is_excluded("targets"));
    assert!(!config.is_excluded("my-target"));
    assert!(!config.is_excluded("src/main.rs"));
}

/// A run that backed up the chunks it was in the middle of uploading would
/// grow without bound, snapshot on snapshot.
#[test]
fn the_staging_directory_is_always_excluded_whatever_the_project_says() {
    let config = parse(&format!(
        "[backup]\n{RECIPIENT}target = \"local\"\n\n[backup.local]\npath = \"/b\"\n"
    ))
    .expect("parses");
    assert!(config.exclude.is_empty());
    assert!(config.is_excluded(crate::target::r2::STAGING_DIR));
    assert!(config.is_excluded("_apex-backup/20260912T014233Z-0badc0de/data.000000"));
    assert!(config.is_excluded("nested/_apex-backup/x"));
}

#[test]
fn a_file_that_is_not_toml_is_a_position_and_never_its_contents() {
    let err = ProjectConfig::parse(Path::new("/p/apex.toml"), "[backup\nsecret = \"hunter2\"\n")
        .expect_err("refuses");
    let text = err.to_string();
    assert!(!text.contains("hunter2"), "the file's contents are in: {text}");
    assert!(text.contains("line"), "{text}");
}

// ── the ssh target, and §36's host group, which is enforced here ────────────

/// A complete `[backup.ssh]`, plus a bound host group that contains the host.
const SSH_OK: &str = r#"
[backup]
recipient = "apexbk1AAAA"
target = "ssh"

[backup.ssh]
host = "backup.example"
user = "apex"
port = 2222
path = "/srv/backups"
identity = "/var/lib/apex-backup/ssh/id"
known_hosts = "/var/lib/apex-backup/ssh/known_hosts"

[identity.ssh]
host_group = "robotics"

[ssh.host_groups]
robotics = ["nas.robotics.example", "backup.example"]
"#;

#[test]
fn an_ssh_target_reads_every_field_of_its_endpoint() {
    let config = parse(SSH_OK).expect("parses");
    assert_eq!(config.kind, Kind::Ssh);
    assert_eq!(
        config.where_to,
        Where::Remote {
            endpoint: SshEndpoint {
                host: "backup.example".to_string(),
                user: Some("apex".to_string()),
                port: Some(2222),
                identity: PathBuf::from("/var/lib/apex-backup/ssh/id"),
                known_hosts: PathBuf::from("/var/lib/apex-backup/ssh/known_hosts"),
            },
            root: "/srv/backups".to_string(),
        }
    );
}

/// §36's refusal, at the moment the file is read.
///
/// This is P2-013's ssh half: an ssh target whose host is not in the group the
/// project bound never gets as far as a connection.
#[test]
fn a_host_outside_the_bound_group_is_refused_before_anything_connects() {
    let text = SSH_OK.replace("host = \"backup.example\"", "host = \"elsewhere.example\"");
    let err = parse(&text).expect_err("refused");
    let ConfigError::Identity(inner) = &err else {
        panic!("the wrong refusal: {err:?}");
    };
    assert!(
        matches!(inner, IdentityError::WrongHost { .. }),
        "{inner:?}"
    );
    let why = err.to_string();
    assert!(why.contains("elsewhere.example"), "{why}");
    assert!(why.contains("robotics"), "{why}");
    // The group's members are named, so the remedy is visible rather than a
    // thing to go and look up.
    assert!(why.contains("nas.robotics.example"), "{why}");
    assert!(why.contains("apex.toml"), "{why}");
}

/// A group with no members has not said that any host is allowed.
#[test]
fn a_bound_group_that_is_not_defined_refuses_every_host() {
    let text = SSH_OK.replace("robotics = [", "something_else = [");
    let err = parse(&text).expect_err("refused");
    let ConfigError::Identity(IdentityError::EmptyHostGroup { group }) = &err else {
        panic!("the wrong refusal: {err:?}");
    };
    assert_eq!(group, "robotics");
    assert!(err.to_string().contains("[ssh.host_groups]"), "{err}");

    // Defined and empty is the same answer, for the same reason.
    let text = SSH_OK.replace(
        "robotics = [\"nas.robotics.example\", \"backup.example\"]",
        "robotics = []",
    );
    let err = parse(&text).expect_err("refused");
    assert!(
        matches!(err, ConfigError::Identity(IdentityError::EmptyHostGroup { .. })),
        "{err:?}"
    );
}

/// A project that binds no group is unconstrained, exactly as one that binds no
/// GitHub account is.
///
/// The regression this file could most easily introduce: a check that refused
/// every ssh target would pass both tests above.
#[test]
fn a_project_that_binds_no_host_group_may_use_any_host() {
    let text = SSH_OK
        .replace("[identity.ssh]\nhost_group = \"robotics\"\n", "")
        .replace("[ssh.host_groups]\nrobotics = [\"nas.robotics.example\", \"backup.example\"]\n", "");
    assert!(!text.contains("host_group"), "the fixture still binds one");
    let config = parse(&text).expect("an unbound project is unconstrained");
    assert_eq!(config.kind, Kind::Ssh);
}

/// Host names are host names; case is not a different machine.
#[test]
fn the_host_group_is_matched_without_regard_to_case() {
    let text = SSH_OK.replace("host = \"backup.example\"", "host = \"BACKUP.Example\"");
    parse(&text).expect("the same host in different case");
}

#[test]
fn an_ssh_target_without_a_known_hosts_file_is_refused_and_says_why() {
    let text = SSH_OK.replace(
        "known_hosts = \"/var/lib/apex-backup/ssh/known_hosts\"\n",
        "",
    );
    let err = parse(&text).expect_err("refused");
    let ConfigError::Missing { key, hint } = &err else {
        panic!("{err:?}");
    };
    assert!(key.contains("known_hosts"), "{key}");
    assert!(
        hint.contains("whoever answered"),
        "the hint must say what accepting any host key would mean: {hint}"
    );
}

/// Values that would stop being data once they reach an ssh command line.
///
/// A host name reaches `ssh` as its own argv element, so there is no shell to
/// escape it for — but one beginning with a dash is an OPTION, and
/// `-oProxyCommand=…` on that line would send the backup through a program of
/// the file's choosing. The `-o` values are worse: the whole argument is parsed
/// by ssh as one configuration line, so a space or a quote in a path does not
/// mean what the file says it means.
#[test]
fn a_host_user_or_path_that_would_not_survive_an_ssh_command_line_is_refused() {
    // No `[identity.ssh]` in these fixtures, so the ONLY thing that can refuse
    // them is the command-line check. With a binding present, every case would
    // be refused as a host outside the group and this would prove nothing.
    fn build(host: &str, user: &str, identity: &str, known_hosts: &str) -> String {
        format!(
            "[backup]\nrecipient = \"apexbk1AAAA\"\ntarget = \"ssh\"\n\n\
             [backup.ssh]\nhost = '{host}'\nuser = '{user}'\n\
             path = \"/srv/backups\"\nidentity = '{identity}'\n\
             known_hosts = '{known_hosts}'\n"
        )
    }
    const HOST: &str = "backup.example";
    const USER: &str = "apex";
    const ID: &str = "/var/lib/apex-backup/ssh/id";
    const KH: &str = "/var/lib/apex-backup/ssh/known_hosts";

    // The control: the same builder with nothing hostile in it parses. Without
    // it, a builder that produced invalid TOML would make every case below
    // "refused" for a reason that has nothing to do with ssh.
    parse(&build(HOST, USER, ID, KH)).expect("the honest fixture parses");

    let cases = [
        build("-oProxyCommand=id", USER, ID, KH),
        build("back up.example", USER, ID, KH),
        build("backup\"example", USER, ID, KH),
        build("", USER, ID, KH),
        build(HOST, "-lroot", ID, KH),
        build(HOST, USER, "/var/lib/apex backup/id", KH),
        build(HOST, USER, ID, "/var/lib/apex-backup/\"kh\""),
        // Relative. This is a path on THIS machine, so it is absolute.
        build(HOST, USER, "ssh/id", KH),
    ];
    for text in cases {
        let err = parse(&text).unwrap_err();
        assert!(
            matches!(err, ConfigError::Bad { .. }),
            "accepted:\n{text}\ngot {err:?}"
        );
    }
}

#[test]
fn a_relative_remote_path_is_refused_because_it_would_depend_on_the_login() {
    let text = SSH_OK.replace("path = \"/srv/backups\"", "path = \"backups\"");
    let err = parse(&text).expect_err("refused");
    assert!(matches!(err, ConfigError::Bad { .. }), "{err:?}");
    assert!(err.to_string().contains("absolute"), "{err}");
}

/// `port = "2222"` is somebody getting it wrong, not a port.
#[test]
fn a_quoted_port_is_refused_rather_than_read_as_a_number() {
    let text = SSH_OK.replace("port = 2222", "port = \"2222\"");
    let err = parse(&text).expect_err("refused");
    assert!(matches!(err, ConfigError::Project(_)), "{err:?}");

    let text = SSH_OK.replace("port = 2222", "port = 70000");
    let err = parse(&text).expect_err("refused");
    assert!(matches!(err, ConfigError::Bad { .. }), "{err:?}");
    assert!(err.to_string().contains("1 to 65535"), "{err}");
}

#[test]
fn the_user_and_the_port_are_optional() {
    let text = SSH_OK
        .replace("user = \"apex\"\n", "")
        .replace("port = 2222\n", "");
    let config = parse(&text).expect("parses");
    let Where::Remote { endpoint, .. } = &config.where_to else {
        panic!("{:?}", config.where_to);
    };
    assert_eq!(endpoint.user, None);
    assert_eq!(endpoint.port, None);
}
