//! One private CA, trusted by one session's browser and by nothing else
//! (P2-012, gap 5 — `protocol::RunRequest::trust_ca`).
//!
//! The case this exists for is the intranet one: a capsule sent to a site
//! whose certificate is signed by an organisation's own root. That root is not
//! in the machine's trust store, and adding it there for the sake of one
//! automated run would widen what every program on the machine believes,
//! permanently, to close a gap that lasts for one capsule.
//!
//! ## It is the BROWSER's trust, not the session's
//!
//! Said here as well as on the wire field, because the obvious reading is
//! wrong and a reader of this module is the person most likely to act on it:
//! `curl`, `git` and `python` inside the same sandbox keep using the system
//! bundle and still refuse the host. What is installed is a **Firefox
//! enterprise policy**, which is the only CA install route the image has —
//! `nss-tools` is not installed, so there is no `certutil` to add a root to an
//! NSS database, and that absence was once recorded as closing this question
//! altogether.
//!
//! It does not, and the measurement is in `docs/browser-capsule-auth.md`: a
//! fresh profile refused a self-signed loopback server and sat on the refusal
//! until it was killed; the same profile, the same server, with a
//! `Certificates.Install` policy bound over
//! `/etc/firefox/policies/policies.json` inside the namespace only, completed
//! the handshake and rendered the page. The machine's own file was untouched
//! throughout.
//!
//! ## Why the daemon writes the policy rather than the caller
//!
//! The shape this replaced was a wire field naming a file and a path to bind
//! it over. That is a much larger thing than trusting a CA — it would let any
//! client shadow any path inside any session's mount namespace — and it would
//! have arrived as a convenience. So the wire carries a PEM path and nothing
//! else, and the one policy document a session can be given is this one, built
//! here, from the machine's own.
//!
//! ## The machine's policy is MERGED, not replaced
//!
//! `/etc/firefox/policies/policies.json` on an APEX machine carries four
//! preferences at `Status: "default"` — the dark theme and the homepage — and
//! three `//` keys explaining why. A capsule handed a policy document
//! containing only a CA would be a browser with different defaults from every
//! other browser on the machine, for a reason nobody could see. So the file is
//! read, `Certificates.Install` is added to a copy of it, and everything else
//! travels unchanged.
//!
//! ## Every failure here is a refusal, and that is the point
//!
//! Nothing in this module is best-effort. `install_redacted_settings` beside
//! it can afford to warn and carry on, because a session that did not get a
//! redacted copy still runs. A capsule that asked to trust a CA and did not
//! get one does not: Firefox answers an untrusted chain by sitting on it, so
//! the caller waits out `apex browser --timeout`, is told the capsule did not
//! finish, and is given no reason at all. A refusal before the browser starts
//! is the only diagnostic that reaches them.

use std::path::{Path, PathBuf};

use anyhow::{anyhow, bail, Context, Result};
use apex_agent_core::sandbox;

/// The one path Firefox reads an enterprise policy from on this system.
///
/// Named rather than written at its two uses: it is a bind TARGET, and a
/// target that is not there makes `--ro-bind-try` a silent no-op rather than
/// an error. That is why [`install`] refuses when it is absent instead of
/// binding hopefully.
pub const FIREFOX_POLICY: &str = "/etc/firefox/policies/policies.json";

/// What the session's copy of the CA is called inside the scratch directory.
const CA_FILE: &str = "browser-ca.pem";

/// What the merged policy document is called there.
const POLICY_FILE: &str = "firefox-policies.json";

/// Cap on the PEM, in bytes.
///
/// A trust anchor file is a couple of kilobytes and a corporate bundle is tens
/// of them. The cap is not a security boundary — the file is the caller's own
/// and they could copy it into the session's working directory unaided — it is
/// here so that `--trust-ca /dev/zero`, or a path that turns out to be a disk
/// image, fails by name instead of filling the scratch directory.
const MAX_CA_BYTES: usize = 256 * 1024;

/// What was installed, so the caller binds the same paths it wrote.
#[derive(Debug)]
pub struct Installed {
    /// The session's copy of the PEM, inside the scratch directory.
    pub ca: PathBuf,
    /// The merged policy document, inside the scratch directory.
    pub policy: PathBuf,
    /// Where that document is bound inside the namespace — the machine's own
    /// policy path, which is never written to.
    pub at: PathBuf,
}

/// The refusals that can be decided from the request alone, before anything
/// exists to clean up.
///
/// Split from [`install`] for the reason `disposable::check` is split from the
/// capsule it describes: these two are facts about what was asked for, they
/// cost nothing, and a session refused here has not had a scratch directory,
/// a worktree or a grant created for it.
pub fn check(trust_ca: Option<&str>, confined: bool) -> Result<()> {
    let Some(path) = trust_ca else {
        return Ok(());
    };
    if !confined {
        // The same refusal `--ttl` gets on a session with no grant, and for
        // the same reason: an unconfined session has no mount namespace, so
        // there is nothing to install the policy into. Accepting the field and
        // doing nothing would leave a caller believing their capsule trusted a
        // root it had never heard of.
        bail!(
            "trust_ca installs a certificate authority inside the session's mount namespace, \
             and an unconfined session does not have one — the request would be accepted and \
             change nothing. Ask for a confined sandbox, or drop the trust_ca"
        );
    }
    if !Path::new(path).is_absolute() {
        // A relative path would be resolved against the DAEMON's working
        // directory, which is not the caller's. The CLI resolves it before
        // sending; a client that did not is refused rather than served a
        // different file from the one it named.
        bail!(
            "trust_ca must be an absolute path and this one is {path:?}: a relative path arrives \
             here to be resolved against the daemon's working directory, which is not the \
             caller's, so it names a different file or no file at all"
        );
    }
    Ok(())
}

/// How many certificates a PEM holds — refusing anything that is not
/// certificates.
///
/// The refusal that matters is a **private key**. `cat key.pem cert.pem >
/// bundle.pem` is how half the world's TLS configuration is written, so a
/// caller pointing `--trust-ca` at such a file is not a strange event — and
/// this module copies what it is given into the capsule, where the agent can
/// read it. A trust anchor is public; the key beside it is not, and nothing
/// downstream would notice the difference.
pub fn certificates_only(bytes: &[u8]) -> Result<usize> {
    let text = std::str::from_utf8(bytes).map_err(|_| {
        anyhow!(
            "trust_ca names a file that is not PEM text. A DER certificate is the usual reason — \
             convert it once with `openssl x509 -inform DER -in <file> -outform PEM -out ca.pem` \
             and pass that"
        )
    })?;
    let mut certificates = 0usize;
    let mut open: Option<String> = None;
    for line in text.lines() {
        let line = line.trim();
        if let Some(label) = line
            .strip_prefix("-----BEGIN ")
            .and_then(|l| l.strip_suffix("-----"))
        {
            if let Some(already) = open.as_deref() {
                bail!(
                    "trust_ca names a malformed PEM: a `{label}` block opens while `{already}` \
                     is still open"
                );
            }
            if label != "CERTIFICATE" {
                bail!(
                    "trust_ca names a file carrying a `{label}` block, and only certificates may \
                     go into a capsule. A file holding a PRIVATE KEY beside its certificate is \
                     the usual reason and is the one worth refusing: this copies the file where \
                     the session can read it, and a trust anchor is public while the key beside \
                     it is not. Pass the certificate alone"
                );
            }
            open = Some(label.to_string());
        } else if let Some(label) = line
            .strip_prefix("-----END ")
            .and_then(|l| l.strip_suffix("-----"))
        {
            match open.take() {
                Some(ref o) if o == label => certificates += 1,
                Some(o) => bail!(
                    "trust_ca names a malformed PEM: a `{o}` block is closed by an END for \
                     `{label}`"
                ),
                None => bail!(
                    "trust_ca names a malformed PEM: an END for `{label}` with no block open"
                ),
            }
        }
    }
    if let Some(o) = open {
        bail!("trust_ca names a malformed PEM: the `{o}` block is never closed");
    }
    if certificates == 0 {
        // An empty or unrecognised file would install nothing, and Firefox
        // would then refuse the site exactly as it does with no policy at all
        // — the silent five minutes this whole path exists to avoid.
        bail!(
            "trust_ca names a file with no certificate in it, so the capsule's browser would \
             trust nothing extra and would refuse the site with no error anywhere"
        );
    }
    Ok(certificates)
}

/// The machine's policy document with one `Certificates.Install` added.
///
/// A separate function because it is the part worth asserting on its own: it
/// is pure, and what it must keep — the preferences and the `//` keys — is a
/// property of the output rather than of the file system.
pub fn merged_policy(host: &[u8], ca_path: &str) -> Result<Vec<u8>> {
    let mut doc: serde_json::Value = serde_json::from_slice(host).context(
        "the machine's Firefox policy is not valid JSON, so a copy of it cannot be built. \
         Refused rather than replaced with a policy carrying only the CA: that would start the \
         capsule with different browser defaults from every other browser on this machine",
    )?;
    let obj = doc
        .as_object_mut()
        .ok_or_else(|| anyhow!("the machine's Firefox policy is not a JSON object"))?;
    let policies = obj
        .entry("policies")
        .or_insert_with(|| serde_json::Value::Object(serde_json::Map::new()));
    let policies = policies.as_object_mut().ok_or_else(|| {
        anyhow!("the machine's Firefox policy has a `policies` key that is not an object")
    })?;
    if policies.contains_key("Certificates") {
        // The image build asserts this cannot happen on an APEX machine — the
        // shipped file carries `Preferences` and nothing else — so reaching
        // this means somebody has a policy of their own. Merging two trust
        // lists silently is not a decision to take on their behalf, and
        // dropping theirs is worse.
        bail!(
            "the machine's Firefox policy already carries a `Certificates` block, so installing \
             one for this session would either replace the machine's trust decisions or merge \
             with them. Neither is a decision to take silently"
        );
    }
    policies.insert(
        "Certificates".to_string(),
        serde_json::json!({ "Install": [ca_path] }),
    );
    let mut out = serde_json::to_vec_pretty(&doc).context("serialising the session's policy")?;
    out.push(b'\n');
    Ok(out)
}

/// Copy the CA where the session can read it, and build the policy that
/// installs it.
///
/// `host_policy` is a parameter rather than [`FIREFOX_POLICY`] so the whole of
/// this can be exercised against a fixture: a function that read `/etc`
/// directly could only be tested on a machine with Firefox installed, and
/// would be a function nobody tests.
pub fn install(trust_ca: &str, scratch: &Path, host_policy: &Path) -> Result<Installed> {
    let src = Path::new(trust_ca);
    let raw = std::fs::read(src)
        .with_context(|| format!("trust_ca names {trust_ca}, which cannot be read"))?;
    if raw.len() > MAX_CA_BYTES {
        bail!(
            "trust_ca names a {} byte file and the limit is {MAX_CA_BYTES}: a trust anchor is a \
             couple of kilobytes, so this is a path that is not the file it was meant to be",
            raw.len()
        );
    }

    // COPIED FIRST, AND THE COPY IS WHAT IS CHECKED. Validating the source and
    // then copying it would leave a window in which the file changed between
    // the two — the caller's own file, so not an attack on the daemon, but the
    // capsule would end up holding bytes nothing ever inspected. The copy is
    // what the capsule reads, so the copy is what has to be certificates and
    // nothing else.
    let ca = scratch.join(CA_FILE);
    std::fs::write(&ca, &raw)
        .with_context(|| format!("writing the session's copy of the CA to {}", ca.display()))?;
    let copied = std::fs::read(&ca)
        .with_context(|| format!("reading back the session's copy of the CA at {}", ca.display()))?;
    let certificates = certificates_only(&copied)?;

    // The path Firefox will open, which is the path the bind lands on: the
    // scratch directory is canonicalised before `build_argv` sees it, because
    // bwrap will not mount through a symlink and an atomic OS reaches every
    // home through one. A policy naming the pre-canonical path would be a
    // policy naming a file that is not there, and Firefox reports a policy it
    // cannot read by ignoring it.
    let in_namespace = sandbox::real_target(&ca);

    let host = std::fs::read(host_policy).with_context(|| {
        format!(
            "trust_ca needs {} to exist: the CA is installed by binding a copy of that file over \
             it inside the session's namespace, and bwrap's `--ro-bind-try` over a path that is \
             not there is a SILENT no-op. A session would start, trust nothing extra, and sit on \
             the refused handshake until the capsule timed out",
            host_policy.display()
        )
    })?;
    let merged = merged_policy(&host, &in_namespace.to_string_lossy())?;
    let policy = scratch.join(POLICY_FILE);
    std::fs::write(&policy, merged).with_context(|| {
        format!(
            "writing the session's Firefox policy to {}",
            policy.display()
        )
    })?;

    eprintln!(
        "apex-agentd: this session's browser trusts {certificates} certificate{} from {trust_ca}, \
         installed by a policy bound over {} inside its namespace only",
        if certificates == 1 { "" } else { "s" },
        host_policy.display()
    );

    Ok(Installed {
        ca,
        policy,
        at: host_policy.to_path_buf(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A private directory of this test's own, named for the process so
    /// parallel runs cannot collide. The repository's idiom; no
    /// dev-dependency is added for it.
    fn tmpdir(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!(
            "apex-browserca-{tag}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).expect("tempdir");
        d
    }

    fn cert(tag: &str) -> String {
        format!("-----BEGIN CERTIFICATE-----\n{tag}\n-----END CERTIFICATE-----\n")
    }

    /// The shipped APEX policy, verbatim in shape: four preferences at
    /// `Status: "default"` and three `//` keys. Written out rather than read
    /// from `/etc`, so the assertions hold on a runner with no Firefox.
    const APEX_POLICY: &str = r#"{
      "//": "APEX-OS Firefox defaults",
      "policies": {
        "Preferences": {
          "ui.systemUsesDarkTheme": { "Value": 1, "Status": "default" },
          "layout.css.prefers-color-scheme.content-override": { "Value": 0, "Status": "default" },
          "browser.startup.homepage": { "Value": "about:home", "Status": "default" },
          "browser.newtabpage.pinned": { "Value": "[]", "Status": "default" }
        }
      },
      "//2": "Status 'default' means users can still change it.",
      "//3": "Enterprise policy is applied AFTER default prefs."
    }"#;

    #[test]
    fn a_private_key_beside_the_certificate_is_refused() {
        // The refusal this module exists to have. `cat key.pem cert.pem >
        // bundle.pem` is ordinary, the file is copied where the agent can read
        // it, and nothing downstream would notice a key in it.
        let both = format!(
            "-----BEGIN PRIVATE KEY-----\nAAAA\n-----END PRIVATE KEY-----\n{}",
            cert("BBBB")
        );
        let err = certificates_only(both.as_bytes()).expect_err("a key must be refused");
        let said = err.to_string();
        assert!(
            said.contains("PRIVATE KEY"),
            "the refusal has to name what it found: {said}"
        );

        // And the other order, because a check that only looked at the first
        // block would pass one of the two arrangements.
        let other = format!(
            "{}-----BEGIN RSA PRIVATE KEY-----\nAAAA\n-----END RSA PRIVATE KEY-----\n",
            cert("BBBB")
        );
        assert!(certificates_only(other.as_bytes()).is_err());
    }

    #[test]
    fn certificates_are_counted_and_nothing_else_passes() {
        assert_eq!(
            certificates_only(format!("{}{}", cert("AAAA"), cert("BBBB")).as_bytes())
                .expect("two certificates"),
            2
        );
        // Text around the blocks is what `openssl x509 -text` writes and is
        // accepted: it is not a block.
        let annotated = format!("Issuer: CN=Example Root\n{}", cert("AAAA"));
        assert_eq!(certificates_only(annotated.as_bytes()).expect("one"), 1);

        // Nothing at all. The failure mode this refuses is the silent one: a
        // file with no certificate installs nothing, and the capsule then
        // refuses the site exactly as it would with no policy.
        let err = certificates_only(b"").expect_err("an empty file installs nothing");
        assert!(err.to_string().contains("no certificate"), "{err}");
        assert!(certificates_only(b"not a certificate\n").is_err());

        // DER, which is what `--trust-ca ca.crt` usually is, named with the
        // conversion rather than reported as invalid.
        let err = certificates_only(&[0x30, 0x82, 0xff, 0xfe]).expect_err("DER is not PEM");
        assert!(err.to_string().contains("openssl x509"), "{err}");

        // Malformed rather than absent: an unterminated block.
        assert!(certificates_only(b"-----BEGIN CERTIFICATE-----\nAAAA\n").is_err());
    }

    #[test]
    fn the_merge_keeps_the_machines_own_policy() {
        let out = merged_policy(APEX_POLICY.as_bytes(), "/tmp/scratch/browser-ca.pem")
            .expect("the shipped policy merges");
        let doc: serde_json::Value = serde_json::from_slice(&out).expect("valid JSON out");

        // The CA is installed, by the path the bind will land on.
        assert_eq!(
            doc["policies"]["Certificates"]["Install"],
            serde_json::json!(["/tmp/scratch/browser-ca.pem"])
        );

        // And everything the machine had is still there. This is the half a
        // "replace the file" implementation would fail while still trusting
        // the CA — a capsule with the right trust and the wrong defaults,
        // which nothing would report.
        let prefs = doc["policies"]["Preferences"]
            .as_object()
            .expect("the machine's preferences survive the merge");
        assert_eq!(prefs.len(), 4, "a preference was dropped: {prefs:?}");
        assert_eq!(prefs["ui.systemUsesDarkTheme"]["Value"], 1);
        assert_eq!(prefs["ui.systemUsesDarkTheme"]["Status"], "default");
        assert_eq!(prefs["browser.startup.homepage"]["Value"], "about:home");
        for key in ["//", "//2", "//3"] {
            assert!(
                doc.get(key).is_some(),
                "the comment key {key} was dropped, so the file no longer says why it exists"
            );
        }

        // A machine whose file is bare still works: `policies` is created.
        let bare = merged_policy(b"{}", "/tmp/ca.pem").expect("an empty document");
        let doc: serde_json::Value = serde_json::from_slice(&bare).unwrap();
        assert_eq!(
            doc["policies"]["Certificates"]["Install"],
            serde_json::json!(["/tmp/ca.pem"])
        );
    }

    #[test]
    fn a_machine_that_already_installs_certificates_is_refused() {
        let theirs = r#"{"policies":{"Certificates":{"Install":["/etc/pki/corp.pem"]}}}"#;
        let err = merged_policy(theirs.as_bytes(), "/tmp/ca.pem")
            .expect_err("two trust lists is not a silent merge");
        assert!(err.to_string().contains("Certificates"), "{err}");

        // Not JSON at all, and not JSON of the right shape. Both refuse
        // rather than falling back to a policy carrying only the CA, which
        // would silently change the capsule's browser defaults.
        assert!(merged_policy(b"{ \"policies\": }", "/tmp/ca.pem").is_err());
        assert!(merged_policy(b"[]", "/tmp/ca.pem").is_err());
        assert!(merged_policy(b"{\"policies\": 7}", "/tmp/ca.pem").is_err());
    }

    #[test]
    fn the_request_refusals_are_decided_before_anything_is_created() {
        assert!(check(None, false).is_ok(), "no CA asked for, nothing to do");
        assert!(check(Some("/etc/pki/ca.pem"), true).is_ok());

        let err = check(Some("/etc/pki/ca.pem"), false).expect_err("no namespace, no install");
        assert!(err.to_string().contains("unconfined"), "{err}");

        let err = check(Some("ca.pem"), true).expect_err("a relative path names another file");
        assert!(err.to_string().contains("absolute"), "{err}");
    }

    #[test]
    fn an_absent_machine_policy_is_refused_rather_than_bound_hopefully() {
        // The defect this refusal exists for: `--ro-bind-try` over a path that
        // does not exist is a NO-OP, not an error. Without this the session
        // starts, trusts nothing extra, and sits on the refused handshake
        // until the capsule times out — with the CA copied in, so every file
        // the caller could inspect says the install happened.
        let dir = tmpdir("nopolicy");
        let ca = dir.join("root.pem");
        std::fs::write(&ca, cert("AAAA")).unwrap();

        let err = install(
            ca.to_str().unwrap(),
            &dir,
            &dir.join("does-not-exist/policies.json"),
        )
        .expect_err("a missing bind target must refuse");
        let said = err.to_string();
        assert!(said.contains("does-not-exist"), "{said}");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn install_copies_the_ca_and_points_the_policy_at_the_copy() {
        let dir = tmpdir("install");
        // THE SCRATCH PATH IS REACHED THROUGH A SYMLINK, deliberately. The
        // claim being asserted below is that the policy names the path the
        // BIND lands on, which is the canonical one — and on a machine where
        // the scratch directory has no symlink in it the two strings are
        // equal, so the assertion would hold with the canonicalisation
        // removed. An atomic OS reaches every home through a symlink, which
        // is why the daemon canonicalises at all, so the fixture has one.
        let real = dir.join("real-scratch");
        std::fs::create_dir_all(&real).unwrap();
        let scratch = dir.join("via-symlink");
        std::os::unix::fs::symlink(&real, &scratch).unwrap();
        let src = dir.join("corp-root.pem");
        std::fs::write(&src, cert("AAAA")).unwrap();
        let host = dir.join("policies.json");
        std::fs::write(&host, APEX_POLICY).unwrap();

        let got = install(src.to_str().unwrap(), &scratch, &host).expect("installs");

        // The copy is byte-identical and is inside the scratch directory, not
        // the caller's: the capsule reads the copy, and a bind of the original
        // would be a bind of a path outside everything else the session gets.
        assert_eq!(
            std::fs::read(&got.ca).unwrap(),
            std::fs::read(&src).unwrap()
        );
        assert!(got.ca.starts_with(&scratch), "{:?}", got.ca);
        assert!(got.policy.starts_with(&scratch), "{:?}", got.policy);
        assert_eq!(got.at, host, "the bind target is the machine's own path");

        // The policy names the COPY, by the path the bind lands on — which is
        // the canonical one, because the scratch directory is canonicalised
        // before bwrap is built. On a machine where /tmp is a symlink these
        // two strings differ, and the difference is a policy Firefox ignores.
        let doc: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&got.policy).unwrap()).unwrap();
        let canonical = sandbox::real_target(&got.ca);
        assert_ne!(
            canonical, got.ca,
            "the fixture's scratch path has no symlink in it, so the next assertion would hold \
             with the canonicalisation deleted and would be checking nothing"
        );
        assert_eq!(
            doc["policies"]["Certificates"]["Install"],
            serde_json::json!([canonical.to_string_lossy()]),
            "the installed path must be the one the session can open"
        );
        // And the machine's preferences came through the whole path, not just
        // through `merged_policy`'s unit test.
        assert_eq!(
            doc["policies"]["Preferences"]["browser.startup.homepage"]["Value"],
            "about:home"
        );

        // A key in the source is refused by `install`, not only by the
        // function that inspects bytes — the copy exists by then, and what
        // matters is that the session does not start.
        std::fs::write(
            &src,
            format!(
                "{}-----BEGIN PRIVATE KEY-----\nAAAA\n-----END PRIVATE KEY-----\n",
                cert("AAAA")
            ),
        )
        .unwrap();
        assert!(install(src.to_str().unwrap(), &scratch, &host).is_err());

        let _ = std::fs::remove_dir_all(&dir);
    }
}
