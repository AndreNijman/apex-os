//! §13.9: what an ordinary project grant may do to a zone, and what it may not.
//!
//! §13.9 is two sentences and they pull in opposite directions:
//!
//! > Ordinary project grants may operate only on bound zones/records.
//! >
//! > Registrar, nameserver, DNSSEC-root, ownership, and destructive
//! > account-level operations belong in elevated/break-glass capability
//! > classes.
//!
//! The first is a narrowing this build can enforce, and [`Record::bind`] is it.
//! The second names a class that does not exist yet — `Effect` is `Read` and
//! `Write`, and nothing in `apex-secret-core` knows what "elevated" would mean.
//! So this module takes the only honest reading available to a build with one
//! class: **the shapes §13.9 reserves are refused outright**, by name, with a
//! message saying which class they belong to. An ordinary grant cannot reach
//! them, which is the half that matters for safety; the class itself is still
//! owed, and [`elevated`] says so where the next person will read it.
//!
//! ## Why a record is looked up rather than named by id
//!
//! Cloudflare addresses a record by a 32-hex id, and the obvious design hands
//! that id to the caller as a parameter. It is the wrong design here, and not
//! by a little: an id is opaque, so a project that narrowed its grant to
//! `www.example.com` would be narrowing nothing — any id in the zone would be
//! accepted, including the one behind the apex. So the caller names a record
//! the way a person does, the broker turns that into an id with a credential it
//! already holds, and the narrowing survives.
//!
//! That costs a second request, and the second request is where this gets
//! interesting.
//!
//! ## The lookup has five answers, not two
//!
//! This codebase has found roughly fifteen defects of one shape: *a refusal
//! reported as an absence.* A `stat` that returns `EACCES` is not a file that
//! is not there. A registry that answers 403 is not a registry with nothing in
//! it. The DNS lookup is the same shape with real consequences — if "the zone
//! did not answer" collapses into "there is no such record", then a caller told
//! "no such record" creates a second one beside the first, and a caller told
//! the same thing on delete believes the record is gone.
//!
//! So [`Lookup`] has five values, modelled on
//! [`crate::providers::cloudflare`]'s sibling in `apex/src/verify.rs`, where
//! `CouldNotRun` is neither a pass nor a failure:
//!
//! * [`Lookup::Found`] — exactly one record answers to that name and type;
//! * [`Lookup::Absent`] — **the zone answered** and holds none. This is the
//!   only value that means the record is not there;
//! * [`Lookup::Denied`] — the credential was refused. Cloudflare documents no
//!   way to tell this from "not found" by error code (403 with a
//!   `documentation_url` is the one documented denial signal, and code 7003
//!   "No route for the URI" is returned for both a bad identifier and an
//!   out-of-scope one), so this build keys on the HTTP status and calls
//!   everything it cannot place `CouldNotRun` rather than guessing;
//! * [`Lookup::Ambiguous`] — more than one record answers. Two `A` records at
//!   `www` is ordinary round-robin, not a fault, and picking the first would
//!   change or delete an arbitrary one of them;
//! * [`Lookup::CouldNotRun`] — no answer, or one this build cannot read.
//!
//! Only `Found` builds a second request. The other four are the end of the
//! operation, and each says which one it was.
//!
//! ## A filter that was sent is not a filter that was applied
//!
//! The lookup sends `name.exact` and `type`, and then checks every record it
//! got back against the name and type anyway. That is not belt and braces: a
//! query parameter this build spells wrongly, or one the far side stops
//! honouring, would silently widen the lookup from one record to the whole
//! zone — and the first of those would then be modified. Filtering here makes
//! the widening show up as [`Lookup::Ambiguous`], which refuses.

use apex_secret_core::SecretValue;

use crate::broker::Owner;

use super::api::{self, Api, Body, Call};
use super::binding::Zone;

/// The record types Cloudflare's own schema lists for a zone's records.
///
/// A closed set, so a type this build has not heard of is refused before the
/// credential is spent rather than after. `SOA`, `DNSKEY` and the rest of
/// [`ELEVATED`] are deliberately checked *before* this list, so that asking to
/// change one is answered with the class it belongs to instead of "that is not
/// a record type".
pub const TYPES: &[&str] = &[
    "A", "AAAA", "CAA", "CERT", "CNAME", "DNSKEY", "DS", "HTTPS", "LOC", "MX", "NAPTR", "NS",
    "OPENPGPKEY", "PTR", "SMIMEA", "SRV", "SSHFP", "SVCB", "TLSA", "TXT", "URI",
];

/// The record types §13.9 reserves, and why each one is reserved.
///
/// Every entry is a change to who is authoritative for a name or to how that
/// authority is proved — the two things a zone's owner cannot get back by
/// editing a record afterwards.
pub const ELEVATED: &[(&str, &str)] = &[
    ("NS", "a nameserver record delegates a name away from this zone"),
    ("DS", "a DS record is the DNSSEC chain of trust from the parent zone"),
    ("DNSKEY", "a DNSKEY record is this zone's DNSSEC signing key"),
    ("SOA", "the SOA record is the zone's own statement of authority"),
];

/// Why a record type may not be changed by an ordinary grant, if it may not.
///
/// Read-only operations are not asked this question: reading a zone's `NS`
/// records tells an agent where the zone is delegated and changes nothing,
/// and §13.9 reserves *changes*.
pub fn elevated(kind: &str) -> Option<&'static str> {
    ELEVATED
        .iter()
        .find(|(name, _)| *name == kind)
        .map(|(_, why)| *why)
}

/// A record this project may act on: a name inside the bound zone, and a type.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Record {
    pub zone: Zone,
    /// The fully qualified name, as it appears in the zone.
    pub name: String,
    /// The record type, upper case. `None` for a read that names no type.
    pub kind: Option<String>,
}

impl Record {
    /// The path the zone's records live under.
    pub fn path(&self) -> String {
        format!("/zones/{}/dns_records", self.zone.id)
    }
}

/// Whether `name` is the zone itself or a name inside it.
///
/// The label boundary is the whole point: `example.com` is the apex and
/// `www.example.com` is inside it, but `notexample.com` is a different
/// registration that merely ends in the same letters. A suffix test without
/// the dot would hand it over.
pub fn inside(zone: &str, name: &str) -> bool {
    name == zone || name.ends_with(&format!(".{zone}"))
}

/// What the lookup found, in the five values it can honestly report.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Lookup {
    /// Exactly one record, and its id.
    Found(String),
    /// The zone answered and holds no such record. The only value that means
    /// the record is not there.
    Absent,
    /// The credential was refused. Not an absence, however much the reply
    /// resembles one.
    Denied(u16),
    /// More than one record answers to that name and type.
    Ambiguous(usize),
    /// No answer, or an answer this build cannot read. Never a pass and never
    /// a failure — it is the absence of a measurement.
    CouldNotRun(String),
}

/// Ask the zone which record a name means.
///
/// Spends the credential, so it lives here and not in `bind`.
pub fn look_up(
    api: &Api,
    record: &Record,
    kind: &str,
    value: &SecretValue,
    owner: &Owner,
) -> Lookup {
    let call = Call {
        method: "GET",
        // Both filters are documented query parameters. Neither is trusted —
        // see the module note — and neither can carry anything odd: the name
        // has been through `valid_name` and the type through [`TYPES`].
        path: format!("{}?name.exact={}&type={}", record.path(), record.name, kind),
        body: Body::None,
    };
    let reply = match api::call(api, &call, value, owner) {
        Ok(reply) => reply,
        // A sentence composed in `api.rs`, carrying no credential and no body.
        Err(e) => return Lookup::CouldNotRun(e.to_string()),
    };
    if reply.status == 0 {
        return Lookup::CouldNotRun("the api could not be reached".to_string());
    }
    // The one distinction this whole module exists to keep. A credential that
    // is refused is not a zone that is empty, and Cloudflare gives no error
    // code that separates the two, so the status is what this keys on.
    if reply.status == 401 || reply.status == 403 {
        return Lookup::Denied(reply.status);
    }
    if !reply.ok() {
        return Lookup::CouldNotRun(format!(
            "cloudflare answered HTTP {} to the lookup",
            reply.status
        ));
    }
    let Ok(body) = serde_json::from_str::<serde_json::Value>(&reply.body) else {
        return Lookup::CouldNotRun("the reply was not the json envelope".to_string());
    };
    // `success: false` with a 200 is not a shape Cloudflare documents, and a
    // build that read the result array anyway would be reading a field that
    // may not mean what it looks like.
    if body.get("success").and_then(|s| s.as_bool()) != Some(true) {
        return Lookup::CouldNotRun("the zone did not answer successfully".to_string());
    }
    let Some(results) = body.get("result").and_then(|r| r.as_array()) else {
        return Lookup::CouldNotRun("the reply carried no list of records".to_string());
    };
    let matching: Vec<&serde_json::Value> = results
        .iter()
        .filter(|r| {
            r.get("name").and_then(|n| n.as_str()) == Some(record.name.as_str())
                && r.get("type").and_then(|t| t.as_str()) == Some(kind)
        })
        .collect();
    match matching.len() {
        0 => Lookup::Absent,
        1 => match matching[0].get("id").and_then(|i| i.as_str()) {
            Some(id) if is_record_id(id) => Lookup::Found(id.to_string()),
            _ => Lookup::CouldNotRun("the record it answered with has no usable id".to_string()),
        },
        n => Lookup::Ambiguous(n),
    }
}

/// Whether a string is a Cloudflare record id.
///
/// Checked because it is interpolated into the URL of the request that
/// *changes* something, and it came off the wire rather than out of the
/// project's file. Everything else in a path here has been through the
/// vocabulary's own grammar; this is the one value that has not.
pub fn is_record_id(id: &str) -> bool {
    id.len() == 32 && id.bytes().all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_name_is_inside_a_zone_only_on_a_label_boundary() {
        assert!(inside("example.com", "example.com"), "the apex is in its own zone");
        assert!(inside("example.com", "www.example.com"));
        assert!(inside("example.com", "a.b.example.com"));
        // The one that a `ends_with` without the dot would hand over: a
        // different registration that happens to end in the same letters.
        assert!(!inside("example.com", "notexample.com"));
        assert!(!inside("example.com", "example.com.attacker.test"));
        assert!(!inside("example.com", "com"));
        assert!(!inside("example.com", ""));
    }

    #[test]
    fn every_type_section_thirteen_nine_reserves_is_one_cloudflare_knows_about() {
        // Except SOA, which Cloudflare does not accept as an editable record
        // type at all. It is in the reserved list anyway: a build that dropped
        // it would answer "that is not a record type" to somebody asking to
        // rewrite a zone's authority, which is the right refusal for the wrong
        // reason and would stop being right if the type were ever accepted.
        for (kind, why) in ELEVATED {
            assert!(!why.is_empty(), "{kind} is reserved with no reason given");
            if *kind != "SOA" {
                assert!(TYPES.contains(kind), "{kind}");
            }
        }
        assert_eq!(elevated("NS"), Some(ELEVATED[0].1));
        assert_eq!(elevated("A"), None);
        assert_eq!(elevated("TXT"), None);
        // This answers about the string it is given and nothing else. The
        // caller's `ns` becomes `NS` in `mod.rs::record_type` before it gets
        // here, so THAT is where case is handled and
        // `changing_a_delegation_or_a_dnssec_record_is_refused_as_an_elevated_shape`
        // is what proves it end to end — this line only pins the narrow
        // contract, which is that the lookup is exact.
        assert_eq!(elevated("ns"), None);
    }

    #[test]
    fn a_record_id_off_the_wire_is_checked_before_it_becomes_a_url() {
        assert!(is_record_id("0123456789abcdef0123456789abcdef"));
        for evil in [
            "",
            "0123456789abcdef0123456789abcde",
            "0123456789ABCDEF0123456789ABCDEF",
            "../../zones/somebody-else00000000",
            "0123456789abcdef0123456789abcde/",
        ] {
            assert!(!is_record_id(evil), "'{}' was accepted", evil.escape_debug());
        }
    }
}
