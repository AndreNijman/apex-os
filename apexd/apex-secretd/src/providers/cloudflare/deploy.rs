//! §13.7, the part that needs a credential: what is serving traffic now.
//!
//! ## Why a staged rollout has to ask first
//!
//! Cloudflare's deployment payload is not "move this much traffic to the new
//! version". It is the **whole split**, every time: a list of versions and the
//! share each one gets, which must add to 100. So a caller who says *"send 30%
//! to this version"* is asking for something the API cannot express without
//! also naming where the other 70% goes — and the only honest answer to that
//! is the version that has it now.
//!
//! Which means a staged rollout is two requests, the way `dns.update` is two
//! requests, and for the same reason: the thing the caller named is not the
//! thing the API takes, and the translation costs a credentialled lookup.
//!
//! ## Why it refuses more than it guesses
//!
//! [`Current`] has five values and three of them are refusals. That is
//! deliberate, and it is the shape [`super::dns::Lookup`] established:
//!
//! * a worker that has **never** been deployed has no traffic to keep, so
//!   there is no remainder to assign and a partial rollout is not a thing that
//!   can be done to it. Deploying at 100% is — and that is a different
//!   operation from the caller's point of view, so it is refused rather than
//!   quietly promoted;
//! * a worker that is **already** split between two versions has no single
//!   "the rest of the traffic". Picking one would silently retire the other.
//!   Refused, with both named;
//! * a lookup that was **refused** is not a worker with no deployments, and a
//!   lookup that **could not run** is not either. Cloudflare documents no code
//!   that separates "not authorised" from "not found" — see the note in
//!   [`super::dns`] — so this keys on the HTTP status and calls what it cannot
//!   place `CouldNotRun`.
//!
//! The alternative to all of this is a build that reads the first version out
//! of the first deployment and hopes. That build works until the first time
//! somebody runs a canary, and then it ends the canary without mentioning it.

use apex_secret_core::SecretValue;

use crate::broker::Owner;

use super::api::{self, Api, Body, Call};
use super::binding::Worker;

/// What is serving traffic for one worker.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Current {
    /// One version has all of it, and this is its id.
    One(String),
    /// The worker has no deployments at all.
    None,
    /// More than one version is serving. Their ids, for the refusal.
    Split(Vec<String>),
    /// The credential was refused. **Not** an absence.
    Denied(u16),
    /// The question was not answered. Neither an absence nor a refusal.
    CouldNotRun(String),
}

/// Ask which version is in front of traffic.
pub fn current(
    api: &Api,
    worker: &Worker,
    value: &SecretValue,
    owner: &Owner,
) -> Current {
    let call = Call::new("GET", format!("{}/deployments", super::script(worker)), Body::None);
    let reply = match api::call(api, &call, value, owner) {
        Ok(reply) => reply,
        Err(e) => return Current::CouldNotRun(e.to_string()),
    };
    if reply.status == 0 {
        return Current::CouldNotRun("the api could not be reached".to_string());
    }
    if reply.status == 401 || reply.status == 403 {
        return Current::Denied(reply.status);
    }
    if !reply.ok() {
        return Current::CouldNotRun(format!(
            "cloudflare answered HTTP {} when this asked which version is \
             serving traffic",
            reply.status
        ));
    }
    let Ok(body) = serde_json::from_str::<serde_json::Value>(&reply.body) else {
        return Current::CouldNotRun("the reply was not the json envelope".to_string());
    };
    if body.get("success").and_then(|s| s.as_bool()) != Some(true) {
        return Current::CouldNotRun("the worker did not answer successfully".to_string());
    }
    let Some(deployments) = body
        .get("result")
        .and_then(|r| r.get("deployments"))
        .and_then(|d| d.as_array())
    else {
        return Current::CouldNotRun("the reply carried no list of deployments".to_string());
    };
    // **The most recent is the one serving, and this build has not verified
    // that cloudflare lists it first.** The schema says nothing about the
    // order of `deployments`, there is no account here to ask, and it is the
    // one assumption in this module that a single live request would settle.
    // If the list came back oldest-first, a staged rollout would keep traffic
    // on the ORIGINAL version and retire the one actually serving — the exact
    // canary-ending failure the refusals below exist to prevent. It would be
    // visible in the trail immediately: the wrong `version_id` in the second
    // half of the split. Stated here rather than assumed silently.
    //
    // An empty list is a worker whose code exists and has never been put in
    // front of anything.
    let Some(latest) = deployments.first() else {
        return Current::None;
    };
    let Some(versions) = latest.get("versions").and_then(|v| v.as_array()) else {
        return Current::CouldNotRun(
            "the deployment cloudflare answered with names no versions".to_string(),
        );
    };
    // A share of zero is not serving. The documented minimum is 0.01, so this
    // is a guard against a shape rather than a rounding rule.
    let serving: Vec<String> = versions
        .iter()
        .filter(|v| {
            v.get("percentage")
                .and_then(serde_json::Value::as_f64)
                .is_some_and(|p| p > 0.0)
        })
        .filter_map(|v| v.get("version_id").and_then(|i| i.as_str()))
        .filter(|id| super::binding::valid_uuid(id))
        .map(str::to_string)
        .collect();
    match serving.len() {
        0 => Current::CouldNotRun(
            "the deployment cloudflare answered with has no version serving \
             traffic, which is not a shape this build can add to"
                .to_string(),
        ),
        1 => Current::One(serving[0].clone()),
        _ => Current::Split(serving),
    }
}

/// The smallest share Cloudflare will accept, and the largest.
///
/// From the pinned schema: `versions[].percentage` is `minimum: 0.01`,
/// `maximum: 100`.
pub const MIN_SHARE: f64 = 0.01;
pub const MAX_SHARE: f64 = 100.0;

/// Read a `percentage` option, or say why it is not one.
///
/// Refused rather than clamped. A caller who wrote `150` meant something, and
/// deploying at 100 because 150 was out of range is this service deciding what
/// they meant.
pub fn share(raw: &str) -> Result<f64, String> {
    let Ok(value) = raw.parse::<f64>() else {
        return Err(format!(
            "'{raw}' is not a share of traffic. Give a number between \
             {MIN_SHARE} and {MAX_SHARE}"
        ));
    };
    if !value.is_finite() || !(MIN_SHARE..=MAX_SHARE).contains(&value) {
        return Err(format!(
            "a share of traffic is between {MIN_SHARE} and {MAX_SHARE}, and \
             '{raw}' is not"
        ));
    }
    Ok(value)
}

/// The remainder, rounded the way the far side will accept.
///
/// Two decimal places because `0.01` is the documented minimum, and a
/// remainder carrying more precision than that is a body cloudflare refuses
/// for a reason nobody reading the error would connect to this line.
pub fn remainder(share: f64) -> f64 {
    ((MAX_SHARE - share) * 100.0).round() / 100.0
}
