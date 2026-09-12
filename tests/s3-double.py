#!/usr/bin/env python3
"""A loopback S3 that verifies SigV4 with an implementation of its own.

Used by tests/test-apex-backup-s3.sh. It is the third implementation of the
signature in this repository, and that is the whole reason it exists:

  * `apexd/apex-secretd/src/providers/s3/sigv4.rs` is the one under test;
  * its known-answer tests are botocore's numbers, pinned;
  * this recomputes the signature from *what actually arrived on the socket*
    with nothing but `hmac` and `hashlib`, and answers 403 when it does not
    match.

So a signer that is wrong in a way that is stable — which every test that
compares it with itself would pass — fails here, and a signer that is right
but wired to the wrong bytes (a path signed that is not the path requested, a
host header that is not the one sent, a digest that is not the body's) fails
here too.

It stores objects in a directory so the backup can be listed and restored.
No credential here is real: the access key id is AWS's documentation example
and the secret beside it authorises nothing anywhere.

    s3-double.py <port> <root-directory> <access-key-id> <secret-key>

It prints one line per request to stderr, which the suite keeps for evidence.
"""

import hashlib
import hmac
import os
import sys
import urllib.parse
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

PORT = int(sys.argv[1])
ROOT = sys.argv[2]
ACCESS_KEY_ID = sys.argv[3]
SECRET_KEY = sys.argv[4]

ALGORITHM = "AWS4-HMAC-SHA256"
SIGNED_HEADERS = "host;x-amz-content-sha256;x-amz-date"


def sign(key, msg):
    return hmac.new(key, msg.encode(), hashlib.sha256).digest()


def signature(method, path, query, host, amz_date, payload_sha, region):
    """The whole SigV4 chain, from the AWS specification and nothing else."""
    canonical = "\n".join(
        [
            method,
            path,
            query,
            f"host:{host}",
            f"x-amz-content-sha256:{payload_sha}",
            f"x-amz-date:{amz_date}",
            "",
            SIGNED_HEADERS,
            payload_sha,
        ]
    )
    date_stamp = amz_date.split("T")[0]
    scope = f"{date_stamp}/{region}/s3/aws4_request"
    to_sign = "\n".join(
        [
            ALGORITHM,
            amz_date,
            scope,
            hashlib.sha256(canonical.encode()).hexdigest(),
        ]
    )
    key = sign(("AWS4" + SECRET_KEY).encode(), date_stamp)
    key = sign(key, region)
    key = sign(key, "s3")
    key = sign(key, "aws4_request")
    return scope, hmac.new(key, to_sign.encode(), hashlib.sha256).hexdigest()


def object_path(bucket, key):
    # Every segment is checked below before this is called.
    return os.path.join(ROOT, bucket, key)


def safe(segment):
    return segment not in ("", ".", "..") and "/" not in segment


class Handler(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"

    def log_message(self, fmt, *args):
        sys.stderr.write("double: " + (fmt % args) + "\n")
        sys.stderr.flush()

    # ── the verification, which is this file's reason to exist ─────────────
    def verified(self, method, body):
        authorization = self.headers.get("Authorization", "")
        amz_date = self.headers.get("x-amz-date", "")
        claimed = self.headers.get("x-amz-content-sha256", "")
        if not authorization.startswith(ALGORITHM + " "):
            return False, "no AWS4-HMAC-SHA256 Authorization header"
        if hashlib.sha256(body).hexdigest() != claimed:
            return False, "x-amz-content-sha256 is not the digest of the body"
        if f"SignedHeaders={SIGNED_HEADERS}," not in authorization:
            return False, "SignedHeaders is not the set this build signs"

        credential = authorization.split("Credential=")[1].split(",")[0]
        key_id, date_stamp, region, service, terminator = credential.split("/")
        if key_id != ACCESS_KEY_ID:
            return False, f"access key id {key_id!r}"
        if service != "s3" or terminator != "aws4_request":
            return False, f"scope {credential!r}"
        if date_stamp != amz_date.split("T")[0]:
            return False, "the credential scope's date is not x-amz-date's"

        split = urllib.parse.urlsplit(self.path)
        host = self.headers.get("Host", "")
        scope, expected = signature(
            method, split.path, split.query, host, amz_date, claimed, region
        )
        sent = authorization.split("Signature=")[1].strip()
        if sent != expected:
            return False, f"signature {sent} != {expected}"
        return True, scope

    def answer(self, status, body=b"", kind="application/xml"):
        self.send_response(status)
        self.send_header("Content-Type", kind)
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def refuse(self, why):
        self.log_message("REFUSED: %s", why)
        self.answer(
            403,
            f"<Error><Code>SignatureDoesNotMatch</Code><Message>{why}</Message></Error>".encode(),
        )

    def do_PUT(self):
        length = int(self.headers.get("Content-Length", "0"))
        body = self.rfile.read(length) if length else b""
        ok, why = self.verified("PUT", body)
        if not ok:
            return self.refuse(why)
        split = urllib.parse.urlsplit(self.path)
        parts = [p for p in urllib.parse.unquote(split.path).split("/") if p]
        if len(parts) < 2 or not all(safe(p) for p in parts):
            return self.answer(400, b"<Error><Code>InvalidRequest</Code></Error>")
        bucket, key = parts[0], "/".join(parts[1:])
        path = object_path(bucket, key)
        os.makedirs(os.path.dirname(path), exist_ok=True)
        with open(path, "wb") as f:
            f.write(body)
        self.log_message("stored %s/%s (%d bytes)", bucket, key, len(body))
        self.answer(200, b"")

    def do_GET(self):
        ok, why = self.verified("GET", b"")
        if not ok:
            return self.refuse(why)
        split = urllib.parse.urlsplit(self.path)
        parts = [p for p in urllib.parse.unquote(split.path).split("/") if p]
        if not parts or not all(safe(p) for p in parts):
            return self.answer(400, b"<Error><Code>InvalidRequest</Code></Error>")
        bucket = parts[0]
        query = urllib.parse.parse_qs(split.query)

        if len(parts) == 1 and query.get("list-type") == ["2"]:
            return self.listing(bucket)
        if len(parts) == 1:
            return self.answer(400, b"<Error><Code>InvalidRequest</Code></Error>")

        key = "/".join(parts[1:])
        path = object_path(bucket, key)
        if not os.path.isfile(path):
            return self.answer(
                404, b"<Error><Code>NoSuchKey</Code></Error>"
            )
        with open(path, "rb") as f:
            self.answer(200, f.read(), "application/octet-stream")

    def listing(self, bucket):
        base = os.path.join(ROOT, bucket)
        keys = []
        for dirpath, _dirs, files in os.walk(base):
            for name in files:
                full = os.path.join(dirpath, name)
                keys.append(os.path.relpath(full, base))
        keys.sort()
        contents = "".join(
            "<Contents><Key>{}</Key></Contents>".format(
                k.replace("&", "&amp;").replace("<", "&lt;")
            )
            for k in keys
        )
        body = (
            '<?xml version="1.0" encoding="UTF-8"?>'
            "<ListBucketResult><Name>{}</Name>"
            "<IsTruncated>false</IsTruncated>{}</ListBucketResult>"
        ).format(bucket, contents)
        self.answer(200, body.encode())


if __name__ == "__main__":
    os.makedirs(ROOT, exist_ok=True)
    ThreadingHTTPServer(("127.0.0.1", PORT), Handler).serve_forever()
