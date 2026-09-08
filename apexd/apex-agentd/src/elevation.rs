//! Issuing the challenge a security key answers (roadmap §7, P0-014).
//!
//! §7 gives root capability and unsafe-everything "local auth" locally and
//! "local approval required" from everywhere else. `OriginPolicy`'s
//! `remote_elevation_allowed` is the owner's opt-out from the second column,
//! and what the opt-out costs is a touch on a security key. This module is the
//! first half of collecting that touch: the daemon issues a nonce, keeps it in
//! its own memory, and says what the key must sign over.
//!
//! ## Why this is a module of its own and not four lines in `privilege.rs`
//!
//! Everything the sequence actually does lives in
//! [`apex_agent_core::webauthn`], beside the only thing in this repository
//! that can produce a valid assertion — the test signer in that module's own
//! child. What is left here is choosing a credential and turning a refusal
//! into a sentence, which is this file.
//!
//! ## What it does not do
//!
//! Nothing here consults §7's gate, and nothing here grants anything. A
//! challenge is a question, and issuing one to a caller who will be refused
//! later costs nothing and leaks nothing: the nonce is 32 bytes of kernel
//! randomness, it is worthless without a key that was enrolled by the owner,
//! and it expires on its own. Gating the *question* on the answer would mean
//! `may_elevate` running twice with the second run deciding, which is the
//! shape that produced this repository's collection of gates whose only caller
//! could not fail them.

use std::sync::Arc;

use apex_agent_core::grant::GrantKind;
use apex_agent_core::protocol::{ErrorKind, Response};
use apex_agent_core::request;
use apex_agent_core::webauthn::{b64_encode, CredentialStore};

use crate::Daemon;

/// Issue a challenge for one elevation.
///
/// The three fields that scope it — session, kind, ttl — are signed over by
/// the key, so a touch collected here is consent to *this* elevation and not
/// to elevation in general. `webauthn::SecondFactor::may_answer_for` is what
/// enforces that, and `Challenge::binding` is what makes it true.
pub fn challenge(
    daemon: &Arc<Daemon>,
    session: Option<u32>,
    kind: GrantKind,
    ttl_ms: u64,
    label: Option<&str>,
) -> Response {
    // Loaded per call rather than cached on the daemon. Enrolment is
    // `apex agent key add` writing the file directly, so a store cached at
    // startup would mean a key the owner just enrolled did not exist until the
    // daemon restarted.
    let keys = CredentialStore::load();
    let credential = match pick(&keys, label) {
        Ok(c) => c.clone(),
        Err(why) => return Response::error(ErrorKind::PolicyRefused, why),
    };
    let Some(id) = credential.id_bytes() else {
        return Response::error(
            ErrorKind::Internal,
            format!(
                "the credential id stored for {:?} is not base64; re-enrol it with \
                 `apex agent key add`",
                credential.label
            ),
        );
    };

    let now = request::now_ms();
    let challenge = {
        let mut store = daemon.challenges.lock().expect("challenge lock");
        store.issue(session, kind, ttl_ms, now)
    };

    Response::ElevationChallenge {
        nonce: challenge.nonce.clone(),
        binding: b64_encode(&challenge.binding()),
        credential: credential.label.clone(),
        credential_id: b64_encode(&id),
        rp_id: credential.rp_id.clone(),
        expires_ms: challenge.expires_ms,
        instructions: challenge.instructions(&id, &credential.rp_id),
    }
}

/// Which enrolled key has to answer.
///
/// Three cases and three different sentences, because they need three
/// different actions from the owner. An empty store is the one that matters:
/// it is what makes `--origin-policy remote` refuse to start, so the message
/// names the command that fixes it rather than saying no.
///
/// Several enrolled and none named is refused rather than guessed. Picking the
/// first would produce `UnknownCredential` later, when the assertion came back
/// from a key the owner actually used — an error reading "your key is not
/// enrolled" when the true answer is "say which one".
fn pick<'a>(
    keys: &'a CredentialStore,
    label: Option<&str>,
) -> Result<&'a apex_agent_core::webauthn::Credential, String> {
    if let Some(want) = label {
        return keys.by_label(want).ok_or_else(|| {
            format!(
                "no security key is enrolled as {want:?}. Enrolled: {}",
                labels(keys)
            )
        });
    }
    match keys.credentials.len() {
        0 => Err("no security key is enrolled, so a remote elevation cannot be answered. \
                  Enrol one where the key is plugged in: `fido2-cred -M -rk ...`, then \
                  `apex agent key add --label <name> --rp-id <id> --from <file>`"
            .to_string()),
        1 => Ok(&keys.credentials[0]),
        _ => Err(format!(
            "several security keys are enrolled and none was named; say which with \
             --credential. Enrolled: {}",
            labels(keys)
        )),
    }
}

fn labels(keys: &CredentialStore) -> String {
    if keys.is_empty() {
        return "none".to_string();
    }
    keys.credentials
        .iter()
        .map(|c| c.label.clone())
        .collect::<Vec<_>>()
        .join(", ")
}
