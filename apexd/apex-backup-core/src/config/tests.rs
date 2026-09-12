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

/// The two kinds this build refuses, refused where the project names them —
/// reading the file — and not at the first upload.
#[test]
fn an_unimplemented_target_is_refused_at_configuration_time_with_its_reason() {
    for kind in ["ssh", "s3"] {
        let err = parse(&format!(
            "[backup]\n{RECIPIENT}target = \"{kind}\"\n\n[backup.{kind}]\nhost = \"nas.lan\"\n"
        ))
        .expect_err("refuses");
        let ConfigError::Unimplemented { why, .. } = &err else {
            panic!("{kind} was accepted: {err:?}");
        };
        assert!(why.len() > 80, "{kind}'s reason is a stub");
        assert!(err.to_string().contains("will not back up"), "{err}");
    }
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
