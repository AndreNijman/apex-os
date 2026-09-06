//! Sealed storage (§11: "support TPM-backed/sealed storage where appropriate").
//!
//! # What sealing buys, stated exactly
//!
//! The store already lives under a uid no managed agent has. Sealing addresses
//! a different attacker: one who has the *bytes* but not the running machine —
//! a stolen disk, a snapshot, a backup, a second OS booted from USB. Under
//! [`Sealing::Tpm2`] the value is encrypted to this machine's TPM, so those
//! bytes are inert anywhere else.
//!
//! It does **not** put the store beyond a local privileged process. The TPM
//! will unseal for anything that can talk to `/dev/tpmrm0`, and root can talk to
//! `/dev/tpmrm0`. Binding to PCRs would narrow that to a particular boot state,
//! at the cost of every sealed value breaking on a firmware or kernel update —
//! a trade worth making later, with a rewrap path, not as part of this task.
//!
//! # Why `systemd-creds` and not a crate
//!
//! Because the alternative is shipping a TPM2 stack and a cipher implementation
//! in a workspace whose entire dependency list is `serde`, `libc` and `clap`.
//! `systemd-creds` is present on every bootc host by construction, is the
//! mechanism systemd itself uses for unit credentials, and takes plaintext on
//! stdin and writes the blob to stdout — so the value is never an argument and
//! never a file on the way through.

use std::io::Write;
use std::process::{Command, Stdio};

use serde::{Deserialize, Serialize};

use crate::value::SecretValue;

/// Longest `systemd-creds` may take before it is abandoned.
///
/// A TPM that is busy or wedged must not turn into a daemon that never answers.
pub const SEAL_TIMEOUT_SECS: u64 = 10;

/// The credential name every blob is bound to.
///
/// `systemd-creds` mixes the name into the encryption, so a blob sealed as one
/// name will not decrypt as another. Fixed rather than per-secret: the name is
/// not a secret and a per-secret name would only mean a rewrap whenever a
/// credential is renamed.
const CREDENTIAL_NAME: &str = "apex-secretd";

/// How a stored value is protected at rest.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum Sealing {
    /// The value as written, in a `0600` file inside a `0700` directory owned by
    /// the service's uid. The default, and on a machine with no TPM the only
    /// option — a store that refuses to work without a TPM is a store nobody
    /// can adopt.
    #[default]
    Plain,
    /// Encrypted to this host's TPM through `systemd-creds`.
    Tpm2,
}

impl Sealing {
    pub fn as_str(&self) -> &'static str {
        match self {
            Sealing::Plain => "plain",
            Sealing::Tpm2 => "tpm2",
        }
    }

    pub fn parse(s: &str) -> Option<Sealing> {
        match s {
            "plain" => Some(Sealing::Plain),
            "tpm2" => Some(Sealing::Tpm2),
            _ => None,
        }
    }
}

/// Why a value could not be sealed or recovered.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SealError {
    /// No usable TPM2 on this machine.
    NoTpm,
    /// There is a TPM, and this account may not use it without an interactive
    /// authentication this service will never answer. Measured on Fedora 43:
    /// `systemd-creds encrypt` succeeds as root and fails for a system account
    /// with `io.systemd.InteractiveAuthenticationRequired`.
    NotPermitted,
    /// `systemd-creds` did not finish in time.
    Timeout,
    /// It ran and failed. The message is its stderr, which describes the TPM
    /// state and never the value — the value only ever reaches its stdin.
    Failed(String),
    /// The blob decrypted to something that is not a credential this service
    /// will hold. In practice: a store written by a different build.
    Corrupt(String),
    Io(String),
}

impl std::fmt::Display for SealError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SealError::NoTpm => write!(
                f,
                "this machine has no usable TPM2, so a credential cannot be \
                 sealed to it; store it with --sealing plain, which is still \
                 confined to the service's own uid"
            ),
            SealError::NotPermitted => write!(
                f,
                "this machine has a TPM but the secret service's account may \
                 not use it: systemd-creds asks for an interactive \
                 authentication, and a service an agent calls must never raise \
                 a prompt nobody is watching. Store it with --sealing plain, or \
                 grant the service that access in the deployment"
            ),
            SealError::Timeout => write!(
                f,
                "the TPM did not answer within {SEAL_TIMEOUT_SECS}s"
            ),
            SealError::Failed(m) => write!(f, "systemd-creds failed: {m}"),
            SealError::Corrupt(m) => write!(f, "the stored blob is unusable: {m}"),
            SealError::Io(m) => write!(f, "{m}"),
        }
    }
}

impl std::error::Error for SealError {}

/// Whether *this process* can seal to a TPM.
///
/// A trial encryption, not `systemd-creds has-tpm2`. The cheap probe answers
/// "does the machine have a TPM", which turned out to be the wrong question:
/// on Fedora 43 it succeeds for every account, while the encryption itself
/// fails for a non-root one with `InteractiveAuthenticationRequired`. A store
/// that advertised sealing and then refused every write would be worse than one
/// that says plainly it cannot.
///
/// Cached: the answer cannot change without the service restarting, and the
/// probe costs a process spawn.
pub fn tpm2_available() -> bool {
    static AVAILABLE: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *AVAILABLE.get_or_init(|| {
        let cheap = Command::new("systemd-creds")
            .arg("has-tpm2")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false);
        if !cheap {
            return false;
        }
        // A fixed, obviously-not-a-credential probe. It goes to stdout and is
        // dropped; nothing is written anywhere.
        match SecretValue::new("apex-secretd-tpm-probe") {
            Ok(probe) => seal(Sealing::Tpm2, &probe).is_ok(),
            Err(_) => false,
        }
    })
}

/// Turn a value into the bytes that go on disk.
pub fn seal(mode: Sealing, value: &SecretValue) -> Result<Vec<u8>, SealError> {
    match mode {
        Sealing::Plain => Ok(value.expose().as_bytes().to_vec()),
        Sealing::Tpm2 => creds(
            &["encrypt", "--with-key=tpm2", &name_arg(), "-", "-"],
            value.expose().as_bytes(),
        ),
    }
}

/// Recover a value from the bytes on disk.
pub fn unseal(mode: Sealing, blob: &[u8]) -> Result<SecretValue, SealError> {
    let plain = match mode {
        Sealing::Plain => blob.to_vec(),
        Sealing::Tpm2 => creds(&["decrypt", &name_arg(), "-", "-"], blob)?,
    };
    let text = String::from_utf8(plain)
        .map_err(|_| SealError::Corrupt("not valid UTF-8".to_string()))?;
    // `systemd-creds decrypt` writes the credential verbatim, but a value that
    // was stored by an older build with looser rules must not sneak past the
    // charset guard on the way back in.
    SecretValue::new(text.trim_end_matches('\n'))
        .map_err(|e| SealError::Corrupt(e.to_string()))
}

fn name_arg() -> String {
    format!("--name={CREDENTIAL_NAME}")
}

/// Run `systemd-creds` with the payload on stdin and the result on stdout.
///
/// Bounded by `timeout(1)` rather than a watchdog thread for the same reason
/// the agent runtime does it: the child has to actually die, or a wedged TPM
/// leaves a process holding the device after the caller has given up.
fn creds(args: &[&str], stdin: &[u8]) -> Result<Vec<u8>, SealError> {
    let mut child = Command::new("timeout")
        .arg(SEAL_TIMEOUT_SECS.to_string())
        .arg("systemd-creds")
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| SealError::Io(format!("running systemd-creds: {e}")))?;

    // Written from a thread: a payload larger than the pipe buffer would
    // otherwise deadlock, this side blocking on write while the child blocks
    // writing output nobody is reading.
    let payload = stdin.to_vec();
    let mut pipe = child.stdin.take().ok_or_else(|| {
        SealError::Io("systemd-creds gave us no stdin".to_string())
    })?;
    let writer = std::thread::spawn(move || {
        let _ = pipe.write_all(&payload);
        drop(pipe);
    });

    let out = child
        .wait_with_output()
        .map_err(|e| SealError::Io(e.to_string()))?;
    let _ = writer.join();

    if out.status.code() == Some(124) {
        return Err(SealError::Timeout);
    }
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr).trim().to_string();
        if err.contains("InteractiveAuthenticationRequired") || err.contains("Access denied") {
            return Err(SealError::NotPermitted);
        }
        if err.contains("TPM2") || err.contains("tpm2") {
            return Err(SealError::NoTpm);
        }
        return Err(SealError::Failed(err));
    }
    Ok(out.stdout)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_plain_value_round_trips() {
        let v = SecretValue::new("not-a-real-token-plain").unwrap();
        let blob = seal(Sealing::Plain, &v).expect("seal");
        assert_eq!(unseal(Sealing::Plain, &blob).unwrap().expose(), v.expose());
    }

    #[test]
    fn sealing_to_an_unusable_tpm_refuses_rather_than_storing_plaintext() {
        // The failure that would be catastrophic: sealing unavailable, and the
        // value written unsealed anyway. It must be an error, so the caller
        // chooses `plain` deliberately rather than getting it by accident.
        if tpm2_available() {
            eprintln!("SKIP: this process can seal, so there is no refusal to check");
            return;
        }
        let v = SecretValue::new("not-a-real-token-refused").unwrap();
        let err = seal(Sealing::Tpm2, &v).expect_err("must refuse");
        assert!(
            matches!(err, SealError::NotPermitted | SealError::NoTpm),
            "{err:?}"
        );
        // And it says what to do instead, without quoting the value.
        let text = err.to_string();
        assert!(text.contains("plain"), "{text}");
        assert!(!text.contains("not-a-real-token-refused"), "{text}");
    }

    #[test]
    fn sealing_names_round_trip_and_an_unknown_one_is_refused() {
        for s in [Sealing::Plain, Sealing::Tpm2] {
            assert_eq!(Sealing::parse(s.as_str()), Some(s));
        }
        assert_eq!(Sealing::parse("keyring"), None);
        assert_eq!(Sealing::default(), Sealing::Plain);
    }

    #[test]
    fn a_corrupt_blob_is_reported_rather_than_returned() {
        // The failure mode this guards: a truncated or foreign blob whose bytes
        // happen to parse, handed to a provider as if it were a credential.
        assert!(matches!(
            unseal(Sealing::Plain, b"has a space"),
            Err(SealError::Corrupt(_))
        ));
        assert!(matches!(
            unseal(Sealing::Plain, &[0xff, 0xfe]),
            Err(SealError::Corrupt(_))
        ));
        assert!(matches!(unseal(Sealing::Plain, b""), Err(SealError::Corrupt(_))));
    }

    #[test]
    fn a_trailing_newline_from_the_credential_tool_is_not_part_of_the_value() {
        // `systemd-creds` round-trips bytes, but a value that arrived through a
        // shell heredoc during an earlier store would come back with a newline
        // and then fail the charset guard for no useful reason.
        assert_eq!(
            unseal(Sealing::Plain, b"not-a-real-token-nl\n").unwrap().expose(),
            "not-a-real-token-nl"
        );
    }

    #[test]
    fn a_tpm_sealed_value_round_trips_when_this_machine_has_a_tpm() {
        // Skipped rather than failed without one: the suite runs in containers
        // and on build hosts that have no TPM, and a test that cannot run must
        // say so rather than turn red.
        if !tpm2_available() {
            eprintln!("SKIP: this process may not seal to a TPM here");
            return;
        }
        let v = SecretValue::new("not-a-real-token-sealed").unwrap();
        let blob = seal(Sealing::Tpm2, &v).expect("seal to the TPM");
        assert_ne!(
            blob, v.expose().as_bytes(),
            "a sealed blob that equals the plaintext is not sealed"
        );
        assert!(
            !String::from_utf8_lossy(&blob).contains(v.expose()),
            "the plaintext survived into the blob"
        );
        assert_eq!(unseal(Sealing::Tpm2, &blob).unwrap().expose(), v.expose());
    }

    #[test]
    fn a_tpm_blob_does_not_decrypt_as_plain_bytes() {
        if !tpm2_available() {
            eprintln!("SKIP: this process may not seal to a TPM here");
            return;
        }
        let v = SecretValue::new("not-a-real-token-mismatch").unwrap();
        let blob = seal(Sealing::Tpm2, &v).expect("seal");
        // Reading a sealed blob with the wrong mode must fail loudly, not hand
        // a caller ciphertext that it then sends to a provider as a token.
        assert!(unseal(Sealing::Plain, &blob).is_err());
    }
}
