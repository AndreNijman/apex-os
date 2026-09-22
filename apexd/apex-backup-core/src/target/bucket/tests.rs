//! No account, no token, no network. These pin the object keys, the base64
//! framing and the listing's parsing; `tests/test-apex-backup.sh` is what runs
//! a real `apex-secretd` against a loopback double that 401s an
//! unauthenticated request.

use super::*;
use crate::target::Target;

/// What a broker was asked to do.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Call {
    operation: String,
    resource: String,
    params: Vec<(String, String)>,
}

struct FakeBroker {
    calls: Vec<Call>,
    /// Answers, in order. A call past the end answers with an empty body.
    answers: Vec<Result<String, TargetError>>,
    /// The project root, so the double can read a staged file the way the real
    /// daemon would — which is what makes "the bytes that went up" measurable.
    project: PathBuf,
    uploaded: Vec<(String, Vec<u8>)>,
}

impl FakeBroker {
    fn new(project: &Path) -> FakeBroker {
        FakeBroker {
            calls: Vec::new(),
            answers: Vec::new(),
            project: project.to_path_buf(),
            uploaded: Vec::new(),
        }
    }

    fn answering(project: &Path, answers: Vec<Result<String, TargetError>>) -> FakeBroker {
        FakeBroker {
            answers,
            ..FakeBroker::new(project)
        }
    }
}

impl Broker for FakeBroker {
    fn perform(
        &mut self,
        operation: &str,
        resource: &str,
        params: &[(&str, &str)],
    ) -> Result<String, TargetError> {
        self.calls.push(Call {
            operation: operation.to_string(),
            resource: resource.to_string(),
            params: params
                .iter()
                .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
                .collect(),
        });
        // Read the staged file exactly as the daemon's `project::read_file`
        // would, so a test can see what really left the machine.
        if let Some((_, file)) = params.iter().find(|(k, _)| *k == "file") {
            if let Ok(bytes) = std::fs::read(self.project.join(file)) {
                self.uploaded.push((resource.to_string(), bytes));
            }
        }
        if self.answers.is_empty() {
            return Ok(String::new());
        }
        self.answers.remove(0)
    }
}

fn work() -> tempfile::TempDir {
    tempfile::Builder::new()
        .prefix("apex-backup-r2-")
        .tempdir_in("/var/tmp")
        .expect("a fixture directory under /var/tmp")
}

const SNAP: &str = "20260912T014233Z-0badc0de";

#[test]
fn an_object_is_addressed_by_the_bucket_the_prefix_and_the_snapshot() {
    let dir = work();
    let target = BucketTarget::r2(
        FakeBroker::new(dir.path()),
        "example-backups",
        "apex-backup",
        dir.path(),
    );
    assert_eq!(
        target.key(SNAP, "data.000000"),
        "apex-backup/20260912T014233Z-0badc0de/data.000000"
    );
    assert_eq!(
        target.resource(SNAP, "data.000000"),
        "example-backups/apex-backup/20260912T014233Z-0badc0de/data.000000"
    );
}

/// The resource string is handed to the capability framework, which refuses a
/// key segment `valid_path` does not accept. A key this build can write but not
/// name is a snapshot that cannot be uploaded at all.
#[test]
fn every_object_key_this_build_produces_is_one_the_framework_will_carry() {
    let dir = work();
    let target = BucketTarget::r2(
        FakeBroker::new(dir.path()),
        "example-backups",
        "apex-backup",
        dir.path(),
    );
    let head = crate::format::Head {
        format: crate::format::FORMAT.to_string(),
        snapshot: SNAP.to_string(),
        created_ms: 1,
        recipient: "r".to_string(),
        ephemeral: "AAAA".to_string(),
        chunk_bytes: 1,
        manifest_chunks: 2,
        data_chunks: 3,
        label: "l".to_string(),
    };
    for object in head.objects() {
        let resource = target.resource(SNAP, &object);
        assert!(
            apex_secret_core::operation::valid_path(&resource),
            "{resource} is not a resource the capability framework accepts"
        );
        let staged = target.staged_relative(SNAP, &object);
        assert!(
            apex_secret_core::operation::valid_path(&staged),
            "{staged} is not a project path the framework accepts, so the \
             upload could never read it"
        );
    }
}

/// `.apex-backup/` cannot be the staging directory: `valid_name` requires the
/// first byte of a segment to be alphanumeric or `_`, so the framework would
/// refuse the `file` parameter before the provider saw it.
#[test]
fn the_staging_directory_is_not_dot_prefixed_because_the_framework_refuses_one() {
    assert!(apex_secret_core::operation::valid_name(STAGING_DIR));
    assert!(!apex_secret_core::operation::valid_name(".apex-backup"));
    assert!(!STAGING_DIR.starts_with('.'));
}

#[test]
fn an_upload_sends_base64_and_not_the_sealed_bytes() {
    let dir = work();
    let target = BucketTarget::r2(
        FakeBroker::new(dir.path()),
        "example-backups",
        "apex-backup",
        dir.path(),
    );
    // Bytes that are not valid UTF-8, which is what every sealed chunk is.
    let sealed: Vec<u8> = vec![0x00, 0xff, 0xfe, 0x80, 0xc3, 0x28, 0x41];
    target.put(SNAP, "data.000000", &sealed).expect("puts");

    let broker = target.broker.borrow();
    let (resource, bytes) = broker.uploaded.first().expect("something was uploaded");
    assert_eq!(
        resource,
        "example-backups/apex-backup/20260912T014233Z-0badc0de/data.000000"
    );
    assert!(
        bytes.iter().all(|b| b.is_ascii()),
        "what went up is not ASCII, so the broker's from_utf8_lossy would eat it"
    );
    assert_eq!(
        data_encoding::BASE64.decode(bytes).expect("is base64"),
        sealed
    );
}

/// The measurement behind the previous test's reason for existing.
#[test]
fn raw_sealed_bytes_would_not_survive_the_brokers_lossy_conversion() {
    let sealed: Vec<u8> = vec![0x00, 0xff, 0xfe, 0x80, 0xc3, 0x28, 0x41];
    // Exactly what apex-secretd's broker::run_curl does to curl's stdout.
    let through = String::from_utf8_lossy(&sealed).into_owned().into_bytes();
    assert_ne!(
        through, sealed,
        "if this ever passes, the base64 framing can go"
    );

    let encoded = data_encoding::BASE64.encode(&sealed).into_bytes();
    let survived = String::from_utf8_lossy(&encoded).into_owned().into_bytes();
    assert_eq!(survived, encoded, "base64 survives it");
}

#[test]
fn a_fetched_object_is_decoded_back_to_the_bytes_that_were_put() {
    let dir = work();
    let sealed: Vec<u8> = (0..=255u8).collect();
    let target = BucketTarget::r2(
        FakeBroker::answering(
            dir.path(),
            vec![Ok(data_encoding::BASE64.encode(&sealed))],
        ),
        "example-backups",
        "apex-backup",
        dir.path(),
    );
    assert_eq!(target.get(SNAP, "data.000000"), Ok(sealed));
}

/// The constraint this module is shaped around.
#[test]
fn a_refused_fetch_is_could_not_run_and_is_never_reported_as_a_missing_object() {
    let dir = work();
    let target = BucketTarget::r2(
        FakeBroker::answering(
            dir.path(),
            vec![Err(TargetError::Unavailable(
                "cloudflare refused; the broker does not hand back the HTTP status".into(),
            ))],
        ),
        "example-backups",
        "apex-backup",
        dir.path(),
    );
    let err = target.get(SNAP, "data.000000").expect_err("refused");
    assert_eq!(err.verdict("the chunk").as_str(), "could-not-run");
}

#[test]
fn a_reply_that_is_not_base64_is_not_treated_as_an_object_or_as_an_absence() {
    let dir = work();
    let target = BucketTarget::r2(
        FakeBroker::answering(
            dir.path(),
            vec![Ok("<html><body>502 Bad Gateway</body></html>".to_string())],
        ),
        "example-backups",
        "apex-backup",
        dir.path(),
    );
    let err = target.get(SNAP, "data.000000").expect_err("refuses");
    assert_eq!(err.verdict("the chunk").as_str(), "could-not-run");
    assert!(err.to_string().contains("not a report that the object is missing"));
}

#[test]
fn a_listing_names_the_snapshots_in_the_bucket_under_this_prefix() {
    let dir = work();
    let body = serde_json::json!({
        "success": true,
        "result": [
            {"key": "apex-backup/20260912T014233Z-0badc0de/head.json"},
            {"key": "apex-backup/20260912T014233Z-0badc0de/data.000000"},
            {"key": "apex-backup/20250101T000000Z-aaaaaaaa/head.json"},
            // Another project's prefix in the same bucket.
            {"key": "website/20270101T000000Z-bbbbbbbb/head.json"},
            // Something that is not a snapshot at all.
            {"key": "apex-backup/notes.txt"},
            {"key": "apex-backup/../escape/head.json"},
        ]
    })
    .to_string();
    let target = BucketTarget::r2(
        FakeBroker::answering(dir.path(), vec![Ok(body)]),
        "example-backups",
        "apex-backup",
        dir.path(),
    );
    let listed = target.list().expect("lists");
    let names: Vec<&str> = listed.iter().map(|i| i.as_str()).collect();
    assert_eq!(
        names,
        vec!["20250101T000000Z-aaaaaaaa", "20260912T014233Z-0badc0de"],
        "a key from another prefix, a stray file or a traversal became a snapshot"
    );
}

/// A listing that did not parse is not an empty bucket. This is the same defect
/// as the unreadable directory, one layer out.
#[test]
fn a_listing_that_did_not_parse_is_could_not_run_and_not_an_empty_bucket() {
    let dir = work();
    for body in [
        "not json".to_string(),
        serde_json::json!({"success": false, "errors": [{"code": 10001}]}).to_string(),
        serde_json::json!({"result": "a string"}).to_string(),
    ] {
        let target = BucketTarget::r2(
            FakeBroker::answering(dir.path(), vec![Ok(body.clone())]),
            "example-backups",
            "apex-backup",
            dir.path(),
        );
        let err = target.list().expect_err("refuses");
        assert_eq!(
            err.verdict("the bucket").as_str(),
            "could-not-run",
            "{body} was reported as {err:?}"
        );
        assert!(err.to_string().contains("not reporting an empty one"), "{err}");
    }
}

#[test]
fn preparing_lists_the_bucket_so_an_unbound_one_refuses_before_anything_is_sealed() {
    let dir = work();
    let target = BucketTarget::r2(
        FakeBroker::answering(
            dir.path(),
            vec![Err(TargetError::Refused(
                "'example-backups' is not one of this project's buckets".into(),
            ))],
        ),
        "example-backups",
        "apex-backup",
        dir.path(),
    );
    let err = target.prepare().expect_err("refuses");
    assert!(err.to_string().contains("not one of this project's buckets"));

    let broker = target.broker.borrow();
    assert_eq!(broker.calls.len(), 1);
    assert_eq!(broker.calls[0].operation, "cloudflare.r2.object.read");
    assert_eq!(
        broker.calls[0].resource, "example-backups",
        "a bucket listing is a read with no key, which is what §13.2's one \
         operation means"
    );
    assert!(broker.uploaded.is_empty(), "something was uploaded anyway");
}

/// A staged chunk is the one plaintext-adjacent file this program leaves in a
/// user's project, so it must not survive the call — including a failed one.
#[test]
fn a_staged_chunk_is_removed_whether_the_upload_worked_or_not() {
    let dir = work();
    let target = BucketTarget::r2(
        FakeBroker::answering(
            dir.path(),
            vec![
                Ok(String::new()),
                Err(TargetError::Unavailable("the far side went away".into())),
            ],
        ),
        "example-backups",
        "apex-backup",
        dir.path(),
    );
    target.put(SNAP, "data.000000", b"one").expect("puts");
    assert!(!target.staged_absolute(SNAP, "data.000000").exists());

    target.put(SNAP, "data.000001", b"two").expect_err("fails");
    assert!(
        !target.staged_absolute(SNAP, "data.000001").exists(),
        "a failed upload left a staged chunk in the project"
    );

    // And the directories go with the last chunk. A project left holding
    // `_apex-backup/<snapshot>/` after every run is litter the owner did not
    // ask for, and the next run's own exclusion would hide it.
    assert!(
        !dir.path().join(STAGING_DIR).exists(),
        "the staging directory was left behind in the project"
    );
}

/// The cleanup must never take anything but empty directories with it.
#[test]
fn the_staging_cleanup_cannot_remove_a_directory_that_still_holds_something() {
    let dir = work();
    let target = BucketTarget::r2(
        FakeBroker::new(dir.path()),
        "example-backups",
        "apex-backup",
        dir.path(),
    );
    // Something else of the owner's, in the way.
    let keep = dir.path().join(STAGING_DIR).join(SNAP);
    std::fs::create_dir_all(&keep).expect("mkdir");
    std::fs::write(keep.join("something-elses.txt"), b"mine").expect("writes");

    target.put(SNAP, "data.000000", b"one").expect("puts");
    assert!(
        keep.join("something-elses.txt").exists(),
        "the cleanup removed a file it did not stage"
    );
    assert!(!target.staged_absolute(SNAP, "data.000000").exists());
}

#[test]
fn the_upload_names_the_staged_file_by_a_path_relative_to_the_project() {
    let dir = work();
    let target = BucketTarget::r2(
        FakeBroker::new(dir.path()),
        "example-backups",
        "apex-backup",
        dir.path(),
    );
    target.put(SNAP, "data.000000", b"bytes").expect("puts");
    let broker = target.broker.borrow();
    let call = broker.calls.first().expect("one call");
    assert_eq!(call.operation, "cloudflare.r2.object.write");
    assert_eq!(
        call.params,
        vec![(
            "file".to_string(),
            "_apex-backup/20260912T014233Z-0badc0de/data.000000".to_string()
        )]
    );
    assert!(
        !call.params[0].1.starts_with('/'),
        "an absolute path is not a path inside a project"
    );
}

/// The target holds no credential and has nowhere to put one.
#[test]
fn nothing_in_this_target_can_hold_a_token() {
    let dir = work();
    let target = BucketTarget::r2(
        FakeBroker::new(dir.path()),
        "example-backups",
        "apex-backup",
        dir.path(),
    );
    // The struct's whole surface, rendered.
    let described = target.describe();
    assert_eq!(described, "r2 bucket example-backups under apex-backup/");
    assert!(!described.contains("Bearer"));
    // And the broker it talks to is asked for an operation by name, never for
    // a value: `Broker::perform` has no return path for one.
    target.put(SNAP, "data.000000", b"x").expect("puts");
    let broker = target.broker.borrow();
    assert!(broker
        .calls
        .iter()
        .all(|c| c.operation.starts_with("cloudflare.r2.")));
}

#[test]
fn a_very_long_error_body_is_squashed_to_one_bounded_line() {
    let sprawling = format!("<html>\n{}\n</html>", "x ".repeat(4000));
    let line = one_line(&sprawling);
    assert!(!line.contains('\n'));
    assert!(line.chars().count() <= 301, "{} characters", line.chars().count());
    assert!(line.ends_with('…'));
}

// ── one target, two providers ──────────────────────────────────────────────

/// The whole of what separates the S3 path from the R2 one: which two
/// capabilities it spends, and the word it calls itself.
///
/// Asserted on the calls a run actually makes rather than on the `Ops`
/// constant, because a constant nobody reads is a constant that can be right
/// while the code spends something else.
#[test]
fn an_s3_target_spends_the_s3_capabilities_and_never_cloudflares() {
    let dir = work();
    let target = BucketTarget::s3(
        FakeBroker::new(dir.path()),
        "example-backups",
        "apex-backup",
        dir.path(),
    );

    target.prepare().expect("prepare");
    target.put(SNAP, "data.000000", b"bytes").expect("put");
    // The get decodes base64, so it is answered with some.
    let _ = target.get(SNAP, "data.000000");
    // The listing wants the envelope; an empty body is `Unavailable`, which is
    // fine here because what is being measured is which operation was spent.
    let _ = target.list();

    let operations: Vec<String> = target
        .broker
        .borrow()
        .calls
        .iter()
        .map(|c| c.operation.clone())
        .collect();
    for operation in &operations {
        assert!(
            operation.starts_with("s3."),
            "an s3 target spent '{operation}'"
        );
    }
    assert!(operations.contains(&"s3.object.read".to_string()));
    assert!(operations.contains(&"s3.object.write".to_string()));

    assert!(
        target.describe().starts_with("s3 bucket "),
        "{}",
        target.describe()
    );
}

#[test]
fn an_r2_target_still_spends_the_cloudflare_capabilities() {
    let dir = work();
    let target = BucketTarget::r2(
        FakeBroker::new(dir.path()),
        "example-backups",
        "apex-backup",
        dir.path(),
    );
    target.prepare().expect("prepare");
    target.put(SNAP, "data.000000", b"bytes").expect("put");

    let operations: Vec<String> = target
        .broker
        .borrow()
        .calls
        .iter()
        .map(|c| c.operation.clone())
        .collect();
    for operation in &operations {
        assert!(
            operation.starts_with("cloudflare.r2."),
            "an r2 target spent '{operation}'"
        );
    }
    assert!(target.describe().starts_with("r2 bucket "), "{}", target.describe());
}
