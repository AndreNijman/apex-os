//! The device grant, against a server that serves RFC 8628's states in order.
//!
//! There is no account at any of the three authorisation servers here, so the
//! flow is exercised against a loopback double rather than asserted about. What
//! the double is for is the states nobody gets to see on a good day —
//! `authorization_pending` three times, then `slow_down`, then a grant; a
//! denial; an expiry; and a body that is neither an error nor a token, which is
//! what a corporate proxy returns and which a loop that checked for a token
//! first would store as a credential.
//!
//! The double is deliberately not Cloudflare-shaped. These tests moved out of
//! `cloudflare/tests.rs` with the code, and a fixture that kept Cloudflare's
//! client id and scopes would have gone on proving the transport works for the
//! one caller it was written for.

use std::io::Write;
use std::net::TcpListener;
use std::sync::{Arc, Mutex};

use apex_secret_core::account::{CLOUDFLARE_OAUTH, GOOGLE_OAUTH, MICROSOFT_OAUTH};

use super::*;

/// What the token endpoint says, in order, one per poll.
#[derive(Clone)]
enum Step {
    Pending,
    SlowDown,
    Denied,
    Expired,
    /// Neither an `error` nor an `access_token` — a proxy, a WAF, a login page.
    Nonsense,
    Granted,
}

struct Auth {
    port: u16,
    /// Every body the client sent, in order.
    seen: Arc<Mutex<Vec<String>>>,
}

/// A token this build will accept.
const ACCESS: &str = "apex-access-3f9d20b7c1e4a856";
const REFRESH: &str = "apex-refresh-77b0e5da91c3";

/// Not any real provider's. The transport takes these as arguments now, and a
/// fixture that used a shipped client id would be asserting about the caller.
const CLIENT: &str = "apex-test-client-id";
const SECRET: &str = "apex-test-client-secret";
const SCOPES: &[&str] = &["openid", "offline_access"];

/// RFC 8628 §3.2's spelling of the address.
const RFC_DEVICE_REPLY: &str = r#"{"device_code":"dev-abc123","user_code":"WDJB-MJHT",
        "verification_uri":"https://auth.example/device",
        "verification_uri_complete":"https://auth.example/device?code=WDJB-MJHT",
        "expires_in":300}"#;

/// Google's, out of its limited-input-device guide: `verification_url`.
const GOOGLE_DEVICE_REPLY: &str = r#"{"device_code":"dev-abc123","user_code":"WDJB-MJHT",
        "verification_url":"https://www.google.com/device",
        "expires_in":300}"#;

impl Auth {
    fn start(steps: Vec<Step>) -> Auth {
        Auth::serving(steps, RFC_DEVICE_REPLY)
    }

    fn serving(steps: Vec<Step>, device_reply: &'static str) -> Auth {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind loopback");
        let port = listener.local_addr().expect("addr").port();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let recorder = Arc::clone(&seen);
        std::thread::spawn(move || {
            let mut polls = 0usize;
            for stream in listener.incoming().flatten() {
                let mut stream = stream;
                let (first, body) = read_request(&mut stream);
                recorder.lock().expect("lock").push(body.clone());
                let device = first.contains("/device/auth");
                let (status, reply) = if device {
                    (200, device_reply.to_string())
                } else {
                    let step = steps.get(polls).cloned().unwrap_or(Step::Pending);
                    polls += 1;
                    answer(&step)
                };
                let _ = write!(
                    stream,
                    "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\n\
                     Content-Length: {}\r\nConnection: close\r\n\r\n{reply}",
                    reply.len()
                );
            }
        });
        Auth { port, seen }
    }

    fn endpoints(&self) -> Endpoints {
        Endpoints {
            device: format!("http://127.0.0.1:{}/oauth2/device/auth", self.port),
            token: format!("http://127.0.0.1:{}/oauth2/token", self.port),
            // A real poll waits five seconds. A test that did would take a
            // minute to prove a retry, and a test nobody runs proves nothing.
            floor: Duration::from_millis(5),
            limit: Duration::from_secs(20),
            backoff: Duration::from_millis(20),
        }
    }

    fn requests(&self) -> Vec<String> {
        self.seen.lock().expect("lock").clone()
    }
}

fn answer(step: &Step) -> (u16, String) {
    match step {
        Step::Pending => (400, r#"{"error":"authorization_pending"}"#.to_string()),
        Step::SlowDown => (400, r#"{"error":"slow_down"}"#.to_string()),
        Step::Denied => (400, r#"{"error":"access_denied"}"#.to_string()),
        Step::Expired => (400, r#"{"error":"expired_token"}"#.to_string()),
        // A proxy's login page, in the JSON a proxy returns.
        Step::Nonsense => (
            200,
            r#"{"status":"ok","message":"sign in to continue"}"#.to_string(),
        ),
        Step::Granted => (
            200,
            format!(
                r#"{{"access_token":"{ACCESS}","refresh_token":"{REFRESH}",
                    "expires_in":3600,"scope":"openid offline_access",
                    "token_type":"bearer"}}"#
            ),
        ),
    }
}

/// A public client: no secret, which is Cloudflare's and Microsoft's case.
fn public() -> OAuthClient<'static> {
    OAuthClient {
        id: CLIENT,
        secret: None,
    }
}

fn prompt() -> Prompt<'static> {
    Prompt {
        label: "the test provider",
        retry: "apex account add test.one",
    }
}

fn run(steps: Vec<Step>) -> (Result<Grant, DeviceError>, String, Auth) {
    let auth = Auth::start(steps);
    let mut out: Vec<u8> = Vec::new();
    let result = device_grant(&auth.endpoints(), &public(), SCOPES, &prompt(), &mut out);
    (result, String::from_utf8_lossy(&out).into_owned(), auth)
}

#[test]
fn the_flow_prints_a_url_and_a_code_and_launches_nothing() {
    // The constraint this whole path exists to satisfy. A flow that opened a
    // browser would be unusable on a headless machine and would put a window
    // on somebody's desktop.
    let (grant, printed, _auth) = run(vec![Step::Granted]);
    assert!(grant.is_ok(), "{grant:?}");
    assert!(printed.contains("https://auth.example/device"), "{printed}");
    assert!(printed.contains("WDJB-MJHT"), "{printed}");
    assert!(printed.contains("open this on any device"), "{printed}");
    // The label is the caller's, so the line names something a person
    // recognises rather than "the provider".
    assert!(printed.contains("To connect the test provider"), "{printed}");
    // Nothing that reads as launching anything.
    for evil in ["xdg-open", "www-browser", "opening", "launched"] {
        assert!(!printed.contains(evil), "{printed}");
    }
}

#[test]
fn googles_spelling_of_the_verification_address_is_read_too() {
    // RFC 8628 §3.2 says `verification_uri`; Google's limited-input-device
    // guide documents `verification_url` and that is what its device endpoint
    // returns. A parser that read only the standard key would fail EVERY real
    // Google sign-in with "no verification address" while this file's double,
    // which spells it correctly, stayed green — the exact shape of defect a
    // loopback double is worst at catching.
    let auth = Auth::serving(vec![Step::Granted], GOOGLE_DEVICE_REPLY);
    let mut out: Vec<u8> = Vec::new();
    let grant = device_grant(&auth.endpoints(), &public(), SCOPES, &prompt(), &mut out);
    assert!(grant.is_ok(), "{grant:?}");
    let printed = String::from_utf8_lossy(&out);
    assert!(printed.contains("https://www.google.com/device"), "{printed}");
}

#[test]
fn a_client_secret_is_sent_when_there_is_one_and_is_absent_when_there_is_not() {
    // Google's flow lists `client_secret` as required on the poll —
    // `ClientSecret::RequiredToObtain` is the table's word for it — and a
    // public client must send NO field rather than an empty one, because a
    // server that reads `client_secret=` literally answers `invalid_client`
    // and says nothing about which of the two it meant.
    let with = Auth::start(vec![Step::Granted]);
    let client = OAuthClient {
        id: CLIENT,
        secret: Some(SECRET),
    };
    let grant = device_grant(
        &with.endpoints(),
        &client,
        SCOPES,
        &prompt(),
        &mut Vec::new(),
    );
    assert!(grant.is_ok(), "{grant:?}");
    let sent = with.requests();
    // §3.1's device request does NOT carry it: neither Google's nor
    // Microsoft's documentation lists one there, and an undocumented field at
    // an authorisation server is a request whose handling nobody has written
    // down.
    assert!(!sent[0].contains("client_secret"), "{}", sent[0]);
    for poll in &sent[1..] {
        assert!(
            poll.contains(&format!("client_secret={SECRET}")),
            "the secret is missing from {poll}"
        );
    }

    let (grant, _, without) = run(vec![Step::Granted]);
    assert!(grant.is_ok(), "{grant:?}");
    for sent in without.requests() {
        assert!(!sent.contains("client_secret"), "a secret appeared: {sent}");
    }
}

#[test]
fn a_pending_authorisation_is_waited_out_rather_than_failed() {
    let (grant, _, auth) = run(vec![
        Step::Pending,
        Step::Pending,
        Step::Pending,
        Step::Granted,
    ]);
    let grant = grant.expect("the grant arrives after the waiting");
    assert_eq!(grant.access_token, ACCESS);
    assert_eq!(grant.refresh_token.as_deref(), Some(REFRESH));
    assert_eq!(grant.expires_in, Some(3600));
    // One device request plus four polls.
    assert_eq!(auth.requests().len(), 5);
}

#[test]
fn slow_down_slows_the_polling_down_and_does_not_end_it() {
    // §3.5. A client that treated `slow_down` as an error would fail a login
    // for being asked to wait, and one that ignored it would be throttled.
    let auth = Auth::start(vec![Step::SlowDown, Step::Pending, Step::Granted]);
    let ends = auth.endpoints();
    let started = std::time::Instant::now();
    let grant = device_grant(&ends, &public(), SCOPES, &prompt(), &mut Vec::new());
    assert_eq!(grant.expect("still finishes").access_token, ACCESS);
    assert_eq!(auth.requests().len(), 4);
    // Three polls: the first at the floor, the two after `slow_down` at the
    // floor plus the backoff. A client that ignored it would finish in three
    // floors and be throttled by a real server for the rest of the flow.
    assert!(
        started.elapsed() >= ends.floor * 3 + ends.backoff * 2,
        "the interval did not grow: {:?}",
        started.elapsed()
    );
    // And a real authorisation server's backs off by the five seconds §3.5
    // asks for — every entry in the shipped table, not one of them.
    for oauth in [&CLOUDFLARE_OAUTH, &GOOGLE_OAUTH, &MICROSOFT_OAUTH] {
        assert_eq!(Endpoints::for_oauth(oauth).backoff, Duration::from_secs(5));
    }
}

#[test]
fn a_denial_and_an_expiry_are_told_apart() {
    // Two different things to do about them: one is "you said no", the other
    // is "start again". A single "login failed" would send somebody looking
    // for a problem that is not there.
    assert_eq!(run(vec![Step::Denied]).0, Err(DeviceError::Denied));
    let expired = run(vec![Step::Expired]).0;
    assert_eq!(
        expired,
        Err(DeviceError::Expired("apex account add test.one".to_string()))
    );
    assert!(DeviceError::Denied.to_string().contains("declined"));
    // And the expiry names the caller's OWN command. "Start again" without one
    // is a dead end, and `apex cf connect` printed at somebody adding a Google
    // account would be worse than nothing.
    let said = expired.unwrap_err().to_string();
    assert!(said.contains("apex account add test.one"), "{said}");
}

#[test]
fn a_body_that_is_neither_an_error_nor_a_token_is_not_read_as_success() {
    // The failure mode worth writing a double for: a proxy or a WAF answers
    // 200 with its own JSON. A loop that looked for `access_token` and fell
    // through on absence would loop forever; one that took `body` as the
    // credential would store a login page.
    let (grant, _, _auth) = run(vec![Step::Nonsense]);
    let Err(DeviceError::Unusable(what)) = grant else {
        panic!("a proxy's answer was accepted: {grant:?}");
    };
    assert!(what.contains("HTTP 200"), "{what}");
}

#[test]
fn the_request_carries_the_client_the_grant_type_and_the_scopes_it_was_given() {
    let (_, _, auth) = run(vec![Step::Granted]);
    let sent = auth.requests();
    let device = &sent[0];
    assert!(device.contains(&format!("client_id={CLIENT}")), "{device}");
    // Scopes are what bound the credential for its whole life, so what reaches
    // the wire is asserted rather than trusted — and it is the caller's list,
    // not a list this module holds.
    assert!(device.contains("openid"), "{device}");
    assert!(device.contains("offline_access"), "{device}");
    let poll = &sent[1];
    assert!(
        poll.contains("grant_type=urn:ietf:params:oauth:grant-type:device_code"),
        "{poll}"
    );
    assert!(poll.contains("device_code=dev-abc123"), "{poll}");
}

#[test]
fn an_empty_scope_list_sends_no_scope_field_at_all() {
    // Cloudflare's table entry asks for none, because the CLI owns that grant
    // and the entry exists for the refresh. `scope=` is a request for a scope
    // named "" at a server that reads it literally, which is not the same
    // request as sending no field.
    let auth = Auth::start(vec![Step::Granted]);
    let grant = device_grant(&auth.endpoints(), &public(), &[], &prompt(), &mut Vec::new());
    assert!(grant.is_ok(), "{grant:?}");
    let device = &auth.requests()[0];
    assert!(!device.contains("scope"), "{device}");
}

#[test]
fn a_token_the_broker_would_refuse_is_refused_here_first() {
    // The daemon will not put a value holding a quote or a newline in a header,
    // so accepting one here would store a credential that can never be spent
    // and report a problem about the provider instead of about the paste.
    for evil in ["tok\"en", "tok\\en", "Bearer abc123", "abc\ndef", "abc def", ""] {
        assert!(!valid_token(evil), "'{}' was accepted", evil.escape_debug());
    }
    assert!(valid_token(ACCESS));
    assert!(valid_token("v1.0-abcDEF_123-4567890"));
    // A JWT, which is what Microsoft issues: three base64url runs and two dots.
    assert!(valid_token("eyJhbGciOiJSUzI1NiJ9.eyJzdWIiOiIxIn0.c2ln-_bmF0dXJl"));
}

#[test]
fn the_cap_on_a_token_is_the_one_the_daemon_uses() {
    // It was 4096, which is a number Cloudflare's tokens never approach and a
    // Microsoft JWT carrying a normal set of claims does. A CLI that refused
    // at 4097 would turn a working sign-in into "that reply was not a token",
    // and the daemon that stores it accepts 8192 — so refusing lower here is
    // refusing something nothing else objects to.
    assert_eq!(MAX_TOKEN, 8192);
    assert!(valid_token(&"a".repeat(MAX_TOKEN)));
    assert!(!valid_token(&"a".repeat(MAX_TOKEN + 1)));
}

#[test]
fn a_form_value_that_could_be_two_values_is_refused_rather_than_encoded() {
    // Everything sent is a client id, a client secret, a device code, a
    // grant-type URN or a scope list. A value outside that shape is refused, so
    // there is no encoder here to get wrong — and no way to add a field to a
    // form body.
    for evil in ["a&b=c", "a=b", "a\nb", "a\"b", "a%20b", ""] {
        assert!(
            !valid_form_value(evil),
            "'{}' was accepted",
            evil.escape_debug()
        );
    }
    assert!(valid_form_value(CLIENT));
    assert!(valid_form_value(DEVICE_GRANT));
    assert!(valid_form_value(&SCOPES.join(" ")));
    // ...and the assembled body, where `&`, `=` and `+` are the separators.
    assert!(valid_form_body("a=b&c=d+e"));
    assert!(!valid_form_body("a=b&c=\"d"));
    assert!(form(&[("client_id", "a&evil=1")]).is_none());
    // A client secret that could open a second field is refused before it is
    // sent, rather than escaped into one that looks fine.
    assert!(form(&[("client_secret", "s3cret&scope=admin")]).is_none());
}

#[test]
fn a_duration_reads_as_something_a_person_would_say() {
    assert_eq!(roughly(45), "45 seconds");
    assert_eq!(roughly(300), "5 minutes");
    assert_eq!(roughly(3600), "1 hour");
    assert_eq!(roughly(7200), "2 hours");
    assert_eq!(roughly(86_400 * 30), "30 days");
    // The boundary that was wrong first time: an hour is an hour.
    assert_eq!(roughly(3599), "59 minutes");
}

#[test]
fn the_servers_polling_interval_is_honoured_when_it_asks_for_a_slower_one() {
    // §3.5. Ignoring it earns a `slow_down` for the rest of the flow; the
    // floor is only a floor. Checked here rather than by waiting a second per
    // poll in every other test in this file.
    let floor = Duration::from_millis(5);
    assert_eq!(poll_interval(Some(5), floor), Duration::from_secs(5));
    assert_eq!(poll_interval(Some(1), floor), Duration::from_secs(1));
    assert_eq!(poll_interval(None, floor), floor);
    // A server asking for less than the floor does not get it.
    assert_eq!(
        poll_interval(Some(0), Duration::from_secs(5)),
        Duration::from_secs(5)
    );
}

#[test]
fn every_authorisation_server_in_the_table_gets_its_own_two_endpoints() {
    // `for_oauth` is the only constructor a shipped caller uses, and a build
    // that rebuilt a device URL from a host would pair one server's device
    // endpoint with another's token endpoint.
    for oauth in [&CLOUDFLARE_OAUTH, &GOOGLE_OAUTH, &MICROSOFT_OAUTH] {
        let ends = Endpoints::for_oauth(oauth);
        assert_eq!(ends.device, oauth.device_url);
        assert_eq!(ends.token, oauth.token_url);
    }
}
