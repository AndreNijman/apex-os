//! Known-answer tests for the SigV4 signer.
//!
//! **Every expected value below was produced by botocore 1.43.71's
//! `S3SigV4Auth` on 2026-09-12, not written from memory and not produced by the
//! code under test.** A signer compared only with itself is a signer that will
//! be consistently wrong, and this is the one defect class that cannot be found
//! by running the thing — it fails at a real S3 and nowhere else.
//!
//! The generator, kept here so the vectors can be regenerated and so a reader
//! can see exactly what was asked of the independent implementation:
//!
//! ```python
//! import datetime, hashlib, json
//! import botocore.auth as auth
//! from botocore.awsrequest import AWSRequest
//! from botocore.credentials import Credentials
//!
//! FIXED = datetime.datetime(2026, 9, 12, 10, 11, 12, tzinfo=datetime.timezone.utc)
//! auth.get_current_datetime = lambda: FIXED
//!
//! def sign(method, url, region, body=b""):
//!     creds = Credentials("AKIDEXAMPLE",
//!                         "wJalrXUtnFEMI/K7MDENG+bPxRfiCYEXAMPLEKEY")
//!     req = AWSRequest(method=method, url=url, data=body)
//!     req.headers["X-Amz-Content-SHA256"] = hashlib.sha256(body).hexdigest()
//!     auth.S3SigV4Auth(creds, "s3", region).add_auth(req)
//!     return req.headers["Authorization"]
//! ```
//!
//! `AKIDEXAMPLE` and the secret beside it are AWS's own documentation example
//! credentials. They authorise nothing and are not a credential; the real one
//! never leaves `apex-secretd`, and nothing in this repository holds one.

use super::*;

/// AWS's documented example credentials. Not a secret, and not usable.
const AK: &str = "AKIDEXAMPLE";
const SK: &str = "wJalrXUtnFEMI/K7MDENG+bPxRfiCYEXAMPLEKEY";
const WHEN: &str = "20260912T101112Z";

fn request<'a>(
    method: &'a str,
    host: &'a str,
    uri: &'a str,
    query: &'a str,
    payload: &'a str,
    region: &'a str,
) -> Request<'a> {
    Request {
        method,
        host,
        canonical_uri: uri,
        canonical_query: query,
        payload_sha256: payload,
        amz_date: WHEN,
        region,
    }
}

/// A bucket listing against a loopback endpoint — the exact call the s3 backup
/// target's `list` makes, and the one the shell suite's double verifies.
#[test]
fn a_bucket_listing_signs_the_way_botocore_signs_it() {
    let req = request(
        "GET",
        "127.0.0.1:9000",
        "/example-backups",
        "list-type=2&prefix=apex-backup%2F",
        EMPTY_PAYLOAD_SHA256,
        "us-east-1",
    );
    assert_eq!(
        req.authorization(AK, SK),
        "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260912/us-east-1/s3/aws4_request, \
         SignedHeaders=host;x-amz-content-sha256;x-amz-date, \
         Signature=6e34ead6c6a5dc48f54b84b952928ccbcaaf1551218db8c76499bfa083ded207"
    );
}

/// A real endpoint, a real region, and a nested key — the `get` of a head
/// object.
#[test]
fn an_object_read_signs_the_way_botocore_signs_it() {
    let req = request(
        "GET",
        "s3.ap-southeast-2.amazonaws.com",
        "/example-backups/apex-backup/20260912T101112Z-abcd1234/head.json",
        "",
        EMPTY_PAYLOAD_SHA256,
        "ap-southeast-2",
    );
    assert!(req.authorization(AK, SK).ends_with(
        "Signature=dd862a9fafb9322f4c66d542949a0957acca460d6e523d9a8a16f0df856d5c8c"
    ), "{}", req.authorization(AK, SK));
}

/// A PUT with a body, so the payload hash is not the empty one.
#[test]
fn an_object_write_signs_its_body_the_way_botocore_signs_it() {
    let body = b"sealed bytes, pretend";
    let payload = sha256_hex(body);
    assert_eq!(
        payload,
        "48348efde82fc5d61e43bfe5c3319e107c3e61347bf60d09f5508aff2d59175a",
        "the payload hash itself differs, so nothing below would mean anything"
    );
    let req = request(
        "PUT",
        "s3.ap-southeast-2.amazonaws.com",
        "/example-backups/apex-backup/20260912T101112Z-abcd1234/data.000000",
        "",
        &payload,
        "ap-southeast-2",
    );
    assert!(req.authorization(AK, SK).ends_with(
        "Signature=66ba5a2822a8dd2257b7d30c8cf970d18493684cebbe807454452c1b32eca4ef"
    ), "{}", req.authorization(AK, SK));
}

/// A listing with no query at all, which is a different canonical request from
/// one with an empty query value.
#[test]
fn a_listing_with_no_query_signs_the_way_botocore_signs_it() {
    let req = request(
        "GET",
        "127.0.0.1:9000",
        "/example-backups",
        "",
        EMPTY_PAYLOAD_SHA256,
        "us-east-1",
    );
    assert!(req.authorization(AK, SK).ends_with(
        "Signature=44f553855598aaaa12847f6fa8deeafcd7643bbc1df4d2c3d6f3c24f7c044885"
    ), "{}", req.authorization(AK, SK));
}

/// A key whose bytes are percent-encoded in the path.
///
/// The detail a signer written from the general SigV4 description gets wrong:
/// S3 does **not** encode the path a second time, so the canonical URI is the
/// already-encoded string and not an encoding of it.
///
/// **This vector caught the generator itself.** The first run used botocore's
/// generic `SigV4Auth`, which normalises and re-encodes the path, and it
/// produced `c9b1c33e…` — a different answer for the same request. Every other
/// vector in this file was identical under both classes, because their paths
/// hold nothing that either class would encode, so this is the only one that
/// could have caught it. `S3SigV4Auth` is the right one and gives the value
/// below; a third, hand-written HMAC-SHA256 chain in Python agrees with it.
#[test]
fn an_already_encoded_key_is_not_encoded_a_second_time() {
    let req = request(
        "GET",
        "s3.us-east-1.amazonaws.com",
        "/b/a%20b/c%2Bd",
        "",
        EMPTY_PAYLOAD_SHA256,
        "us-east-1",
    );
    assert!(req.authorization(AK, SK).ends_with(
        "Signature=8caa821f725be5305d8d87a837ca8f226be6bb289e653eb6d83440d97f91da41"
    ), "{}", req.authorization(AK, SK));
}

// ── the parts, so a wrong signature localises ───────────────────────────────

#[test]
fn the_canonical_request_is_the_seven_lines_aws_specifies() {
    let req = request(
        "GET",
        "127.0.0.1:9000",
        "/example-backups",
        "list-type=2",
        EMPTY_PAYLOAD_SHA256,
        "us-east-1",
    );
    assert_eq!(
        req.canonical(),
        format!(
            "GET\n/example-backups\nlist-type=2\n\
             host:127.0.0.1:9000\n\
             x-amz-content-sha256:{EMPTY_PAYLOAD_SHA256}\n\
             x-amz-date:20260912T101112Z\n\n\
             host;x-amz-content-sha256;x-amz-date\n{EMPTY_PAYLOAD_SHA256}"
        )
    );
}

#[test]
fn the_scope_is_the_date_region_service_and_terminator() {
    let req = request("GET", "h", "/", "", EMPTY_PAYLOAD_SHA256, "eu-west-1");
    assert_eq!(req.scope(), "20260912/eu-west-1/s3/aws4_request");
    assert_eq!(req.date_stamp(), "20260912");
}

/// A signing key derived for one day, region or service cannot sign for
/// another. The property that makes SigV4 scoped, asserted rather than assumed.
#[test]
fn the_signature_changes_with_every_part_of_the_scope() {
    let base = request(
        "GET",
        "s3.us-east-1.amazonaws.com",
        "/b/k",
        "",
        EMPTY_PAYLOAD_SHA256,
        "us-east-1",
    );
    let signature = base.signature(SK);

    let other_region = Request {
        region: "eu-west-1",
        ..base.clone()
    };
    assert_ne!(other_region.signature(SK), signature, "region");

    let other_day = Request {
        amz_date: "20260913T101112Z",
        ..base.clone()
    };
    assert_ne!(other_day.signature(SK), signature, "date");

    let other_key = Request { ..base.clone() };
    assert_ne!(
        other_key.signature("wJalrXUtnFEMI/K7MDENG+bPxRfiCYEXAMPLEKEz"),
        signature,
        "secret"
    );

    let other_object = Request {
        canonical_uri: "/b/other",
        ..base.clone()
    };
    assert_ne!(other_object.signature(SK), signature, "path");

    let other_method = Request {
        method: "PUT",
        ..base.clone()
    };
    assert_ne!(other_method.signature(SK), signature, "method");

    let other_payload = Request {
        payload_sha256: &sha256_hex(b"different"),
        ..base.clone()
    };
    assert_ne!(other_payload.signature(SK), signature, "payload");
}

// ── encoding ───────────────────────────────────────────────────────────────

#[test]
fn a_path_keeps_its_separators_and_encodes_everything_else() {
    assert_eq!(encode_path("/bucket/apex-backup/head.json"), "/bucket/apex-backup/head.json");
    assert_eq!(encode_path("/b/a b"), "/b/a%20b");
    assert_eq!(encode_path("/b/a+b"), "/b/a%2Bb");
    assert_eq!(encode_path("/b/~._-"), "/b/~._-");
    // Not encoded twice: a path that is already encoded goes through this once
    // and only once, which is why the caller encodes and the signer does not.
    assert_eq!(encode_path("/b/a%20b"), "/b/a%2520b");
}

#[test]
fn a_query_component_encodes_its_separators_too() {
    assert_eq!(encode_query_component("apex-backup/"), "apex-backup%2F");
    assert_eq!(encode_query_component("a b"), "a%20b");
    assert_eq!(encode_query_component("x=y&z"), "x%3Dy%26z");
}

// ── the clock ──────────────────────────────────────────────────────────────

#[test]
fn a_timestamp_renders_as_the_format_sigv4_requires() {
    // Every expected value here is `date -u -d @<n> +%Y%m%dT%H%M%SZ` on this
    // machine — a second implementation, not this one run twice.
    assert_eq!(amz_date(1_789_207_872), "20260912T101112Z");
    assert_eq!(amz_date(0), "19700101T000000Z");
    // 2000 is a leap year and 2100 is not, which is the rule a civil-calendar
    // conversion most often gets wrong. The two days either side of where
    // 2100-02-29 would be if it existed:
    assert_eq!(amz_date(951_782_400), "20000229T000000Z");
    assert_eq!(amz_date(1_709_164_800), "20240229T000000Z");
    assert_eq!(amz_date(4_102_358_400), "20991231T000000Z");
    assert_eq!(amz_date(4_107_456_000), "21000228T000000Z");
    assert_eq!(amz_date(4_107_542_400), "21000301T000000Z");
}

/// The date stamp and the timestamp must be the same instant.
///
/// A signer that asked the clock twice could straddle midnight and sign a
/// credential scope for one day with a date header from the next — which a far
/// side rejects, intermittently, at midnight UTC.
#[test]
fn the_scope_date_is_taken_from_the_timestamp_and_not_from_a_second_clock_read() {
    let stamp = amz_date(1_789_207_872);
    let req = request("GET", "h", "/", "", EMPTY_PAYLOAD_SHA256, "us-east-1");
    let req = Request {
        amz_date: &stamp,
        ..req
    };
    assert_eq!(req.date_stamp(), &stamp[..8]);
}
