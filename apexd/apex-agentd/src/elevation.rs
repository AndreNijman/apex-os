//! Issuing the challenge a security key answers (roadmap §7, P0-014).
//!
//! §7 gives root capability and unsafe-everything "local auth" locally and
//! "local approval required" from everywhere else. `OriginPolicy`'s
//! `remote_elevation_allowed` is the owner's opt-out from the second column,
//! and what the opt-out costs is a touch on a security key. This module is the
//! first half of collecting that touch: the daemon issues a nonce, keeps it in
//! its own memory, and says what the key must sign over. [`redeem`] is the
//! second half, added with its reader in commit 4 — it spends the nonce and
//! hands back a receipt only the verifier can mint.
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
use apex_agent_core::protocol::{ErrorKind, Response, SubmittedFactor};
use apex_agent_core::request;
use apex_agent_core::webauthn::{
    self, b64_encode, AssertionError, CredentialStore, SecondFactor,
};

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

/// Turn what a client sent back into a receipt, or say why not.
///
/// The daemon's half of the sequence, and it is a lock and a call: everything
/// that decides anything is [`webauthn::redeem_and_verify`], which lives beside
/// the only signer in this repository that can produce a valid assertion.
///
/// ## Two things a reader should know
///
/// The challenge lock is held across the `openssl` subprocess that checks the
/// signature. Deliberate, and it costs exactly one thing worth naming: two
/// elevations answered in the same instant are serialised. Nothing else is
/// taken while it is held — not the registry, not the grant authority, not the
/// config — so it cannot deadlock against anything, and issuing a challenge
/// grants nothing, so there is no lock ordering to get wrong.
///
/// The counter is written to disk **only on success**. Commit 3 left that
/// decision here on purpose: `redeem_and_verify` moves the counter in memory
/// and says out loud that when to write a file is a daemon's business and not a
/// pure function's. Only on success, because a refused assertion that could
/// still move the stored counter would be a way to lock the owner out of their
/// own key — present a bad signature carrying a huge counter, and every later
/// genuine touch reads as a replay.
pub fn redeem(
    daemon: &Arc<Daemon>,
    sent: &SubmittedFactor,
) -> Result<SecondFactor, AssertionError> {
    let mut keys = CredentialStore::load();
    let now = request::now_ms();
    let minted = {
        let mut challenges = daemon.challenges.lock().expect("challenge lock");
        webauthn::redeem_and_verify(
            &mut challenges,
            &mut keys,
            &sent.nonce,
            &sent.credential,
            &sent.assertion,
            now,
        )
    };
    if minted.is_ok() {
        if let Err(e) = keys.save() {
            // Not a refusal, and not swallowed either. The elevation this just
            // authorised is sound; what is lost is the replay protection for
            // the NEXT one, and the owner has to be able to find that out.
            eprintln!(
                "apex-agentd: the signature counter for {:?} could not be recorded, so this \
                 assertion could be presented again until the key is used successfully: {e}",
                sent.credential
            );
        }
    }
    minted
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
