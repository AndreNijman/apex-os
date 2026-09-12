//! The device grant, against a server that serves RFC 8628's states in order.
//!
//! Same standard as the provider's: there is no Cloudflare account here, so the
//! flow is exercised against a loopback double rather than asserted about. What
//! the double is for is the states nobody gets to see on a good day —
//! `authorization_pending` three times, then `slow_down`, then a grant; a
//! denial; an expiry; and a body that is neither an error nor a token, which is
//! what a corporate proxy returns and which a loop that checked for a token
//! first would store as a credential.

use std::io::Write;
use std::net::TcpListener;
use std::sync::{Arc, Mutex};

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

/// A token this build will accept, in Cloudflare's shape.
const ACCESS: &str = "apexcf-access-3f9d20b7c1e4a856";
const REFRESH: &str = "apexcf-refresh-77b0e5da91c3";

impl Auth {
    fn start(steps: Vec<Step>) -> Auth {
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
                    (
                        200,
                        // Not `format!`: there is nothing to substitute, and the
                        // doubled braces were only escaping for a template this
                        // never was. The bytes are identical.
                        r#"{"device_code":"dev-abc123","user_code":"WDJB-MJHT",
                                "verification_uri":"https://dash.cloudflare.com/oauth2/device",
                                "verification_uri_complete":"https://dash.cloudflare.com/oauth2/device?code=WDJB-MJHT",
                                "expires_in":300}"#
                            .to_string(),
                    )
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
        Step::Nonsense => (200, r#"{"status":"ok","message":"sign in to continue"}"#.to_string()),
        Step::Granted => (
            200,
            format!(
                r#"{{"access_token":"{ACCESS}","refresh_token":"{REFRESH}",
                    "expires_in":3600,"scope":"account:read workers:read",
                    "token_type":"bearer"}}"#
            ),
        ),
    }
}

fn run(steps: Vec<Step>) -> (Result<Grant, DeviceError>, String, Auth) {
    let auth = Auth::start(steps);
    let mut out: Vec<u8> = Vec::new();
    let result = device_grant(&auth.endpoints(), WRANGLER_CLIENT_ID, &mut out);
    (result, String::from_utf8_lossy(&out).into_owned(), auth)
}

#[test]
fn the_flow_prints_a_url_and_a_code_and_launches_nothing() {
    // The constraint this whole path exists to satisfy. A flow that opened a
    // browser would be unusable on a headless machine and would put a window
    // on somebody's desktop.
    let (grant, printed, _auth) = run(vec![Step::Granted]);
    assert!(grant.is_ok(), "{grant:?}");
    assert!(printed.contains("https://dash.cloudflare.com/oauth2/device"), "{printed}");
    assert!(printed.contains("WDJB-MJHT"), "{printed}");
    assert!(printed.contains("open this on any device"), "{printed}");
    // Nothing that reads as launching anything.
    for evil in ["xdg-open", "www-browser", "opening", "launched"] {
        assert!(!printed.contains(evil), "{printed}");
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
    let grant = device_grant(&ends, WRANGLER_CLIENT_ID, &mut Vec::new());
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
    // And the real one backs off by the five seconds §3.5 asks for.
    assert_eq!(Endpoints::cloudflare().backoff, Duration::from_secs(5));
}

#[test]
fn a_denial_and_an_expiry_are_told_apart() {
    // Two different things to do about them: one is "you said no", the other
    // is "start again". A single "login failed" would send somebody looking
    // for a problem that is not there.
    assert_eq!(run(vec![Step::Denied]).0, Err(DeviceError::Denied));
    assert_eq!(run(vec![Step::Expired]).0, Err(DeviceError::Expired));
    assert!(DeviceError::Denied.to_string().contains("declined"));
    assert!(DeviceError::Expired.to_string().contains("again"));
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
fn the_request_carries_the_client_the_grant_type_and_the_scopes_asked_for() {
    let (_, _, auth) = run(vec![Step::Granted]);
    let sent = auth.requests();
    let device = &sent[0];
    assert!(device.contains(&format!("client_id={WRANGLER_CLIENT_ID}")), "{device}");
    // Scopes are what bound the credential for its whole life, so the set is
    // asserted rather than trusted.
    assert!(device.contains("account:read"), "{device}");
    assert!(device.contains("workers_scripts:write"), "{device}");
    assert!(device.contains("offline_access"), "{device}");
    // ...and the ones no operation in this build needs are NOT asked for.
    for broad in [
        "dns_records:edit",
        "workers_kv:write",
        "cfone:write",
        "access:write",
        "workers_routes:write",
    ] {
        assert!(!device.contains(broad), "asked for {broad}: {device}");
    }
    let poll = &sent[1];
    assert!(poll.contains("grant_type=urn:ietf:params:oauth:grant-type:device_code"), "{poll}");
    assert!(poll.contains("device_code=dev-abc123"), "{poll}");
}

#[test]
fn a_token_the_broker_would_refuse_is_refused_here_first() {
    // The daemon will not put a value holding a quote or a newline in a header,
    // so accepting one here would store a credential that can never be spent
    // and report a problem about Cloudflare instead of about the paste.
    for evil in [
        "tok\"en",
        "tok\\en",
        "Bearer abc123",
        "abc\ndef",
        "abc def",
        "",
    ] {
        assert!(!valid_token(evil), "'{}' was accepted", evil.escape_debug());
    }
    assert!(valid_token(ACCESS));
    assert!(valid_token("v1.0-abcDEF_123-4567890"));
}

#[test]
fn the_refresh_token_is_not_stored_as_though_it_were_an_api_token() {
    // A refresh token is not an API credential and the two hosts are what keep
    // that true: the framework pins a stored credential to the host it was
    // stored for, so one filed under the auth host cannot be sent to the API
    // host by any operation, however it is granted.
    assert_ne!(SERVICE, REFRESH_SERVICE);
    assert_ne!(API_HOST, AUTH_HOST);
    assert_eq!(API_HOST, "api.cloudflare.com");
    assert_eq!(AUTH_HOST, "dash.cloudflare.com");
}

#[test]
fn a_form_value_that_could_be_two_values_is_refused_rather_than_encoded() {
    // Everything sent is a client id, a device code, a grant-type URN or a
    // scope list. A value outside that shape is refused, so there is no
    // encoder here to get wrong — and no way to add a field to a form body.
    for evil in ["a&b=c", "a=b", "a\nb", "a\"b", "a%20b", ""] {
        assert!(!valid_form_value(evil), "'{}' was accepted", evil.escape_debug());
    }
    assert!(valid_form_value(WRANGLER_CLIENT_ID));
    assert!(valid_form_value(DEVICE_GRANT));
    assert!(valid_form_value(&SCOPES.join(" ")));
    // ...and the assembled body, where `&`, `=` and `+` are the separators.
    assert!(valid_form_body("a=b&c=d+e"));
    assert!(!valid_form_body("a=b&c=\"d"));
    assert!(form(&[("client_id", "a&evil=1")]).is_none());
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
    assert_eq!(poll_interval(Some(0), Duration::from_secs(5)), Duration::from_secs(5));
}

#[test]
fn status_looks_up_the_keys_section_thirteen_one_actually_defines() {
    // `status` itself talks to the daemon and is not exercised here — see the
    // report. What IS load-bearing and testable is the key paths it reads: a
    // typo in one of these would print nothing and read as "this project binds
    // nothing", sending somebody to edit a file that is already correct.
    let config = ProjectConfig::parse(
        std::path::Path::new("/p/apex.toml"),
        "[identity.cloudflare]\naccount = \"acme\"\n\
         account_id = \"0123456789abcdef0123456789abcdef\"\n\
         [cloudflare]\nzone = \"example.com\"\n\
         zone_id = \"fedcba9876543210fedcba9876543210\"\n\
         [cloudflare.production]\nworker = \"project\"\n",
    )
    .expect("§13.1's own shape parses");

    let found: Vec<(&str, String)> = identity_keys()
        .into_iter()
        .filter_map(|(label, keys)| {
            config.string(keys).ok().flatten().map(|v| (label, v.to_string()))
        })
        .collect();
    assert_eq!(
        found,
        vec![
            ("account:", "acme".to_string()),
            ("account_id:", "0123456789abcdef0123456789abcdef".to_string()),
            ("zone:", "example.com".to_string()),
            ("zone_id:", "fedcba9876543210fedcba9876543210".to_string()),
        ]
    );
    assert_eq!(config.sections(&["cloudflare"]), vec!["production"]);
    assert_eq!(
        config.string(&["cloudflare", "production", "worker"]).unwrap(),
        Some("project")
    );
}

#[test]
fn the_endpoints_are_the_ones_wrangler_uses() {
    // Read out of workers-sdk rather than reconstructed. The device endpoint
    // has to be on the same auth domain as the token endpoint it is paired
    // with, so a drift between these two is a flow that polls the wrong
    // server.
    let e = Endpoints::cloudflare();
    assert_eq!(e.device, "https://dash.cloudflare.com/oauth2/device/auth");
    assert_eq!(e.token, "https://dash.cloudflare.com/oauth2/token");
    assert_eq!(e.floor, Duration::from_secs(5));
}

#[test]
fn status_json_calls_an_environment_a_section_that_binds_a_worker() {
    // P1-006 gave `[cloudflare]` sub-tables that are not environments — `d1`,
    // `kv`, `queues` and `hyperdrive` hold a project's resource ids. The JSON
    // status listed every sub-table, so it would have called a KV namespace
    // table an environment named `kv`, while the human-readable status — which
    // has always required a `worker` key — did not. The two now agree.
    use apex_secret_core::project::ProjectConfig;
    use std::path::Path;
    let config = ProjectConfig::parse(
        Path::new("/p/apex.toml"),
        "[identity.cloudflare]\naccount_id = \"0123456789abcdef0123456789abcdef\"\n\
         [cloudflare.kv]\ncache = \"00112233445566778899aabbccddeeff\"\n\
         [cloudflare.notes]\nnote = \"not an environment either\"\n\
         [cloudflare.production]\nworker = \"project\"\n",
    )
    .expect("parses");
    let named: Vec<String> = config
        .sections(&["cloudflare"])
        .into_iter()
        .filter(|name| matches!(config.string(&["cloudflare", name, "worker"]), Ok(Some(_))))
        .collect();
    assert_eq!(named, vec!["production"]);
}
