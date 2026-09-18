//! `apex cloudflare`'s own half: which scopes it asks for, where each of the
//! two credentials is filed, and what `status` says about renewing.
//!
//! The RFC 8628 transport these used to sit beside moved to
//! `crate::oauth_device` with its loopback double. What stayed is everything
//! that is a decision about CLOUDFLARE rather than about the device grant — a
//! scope set, two hosts that must differ, and the operation name the daemon
//! has to agree with.

use std::time::Duration;

use apex_secret_core::store::Grants;

use super::*;

#[test]
fn the_scopes_asked_for_are_one_per_operation_this_build_can_perform() {
    // §13.5 — no broad token when a narrower scope is possible — applied at
    // the only moment it can be, because a token's scopes are fixed when it is
    // issued. This used to be checked by reading the device request off the
    // wire; the wire is the shared transport's business now, and the list is
    // Cloudflare's, so it is asserted where it is decided.
    assert!(SCOPES.contains(&"account:read"), "{SCOPES:?}");
    assert!(SCOPES.contains(&"workers_scripts:write"), "{SCOPES:?}");
    // `offline_access` is what makes a refresh token come back at all, and
    // without one `apex cf refresh` has nothing to spend.
    assert!(SCOPES.contains(&"offline_access"), "{SCOPES:?}");
    // ...and the ones no operation in this build needs are NOT asked for.
    for broad in [
        "dns_records:edit",
        "workers_kv:write",
        "cfone:write",
        "access:write",
        "workers_routes:write",
    ] {
        assert!(!SCOPES.contains(&broad), "asked for {broad}: {SCOPES:?}");
    }
}

#[test]
fn the_inputs_this_command_hands_the_shared_transport_are_cloudflares() {
    // The one thing the move could quietly lose. `connect_by_device_code` is
    // four arguments and a store, and the store is what makes it untestable
    // here — so the four arguments are a function, and this is it. A build
    // that passed an empty scope list would get a token good for nothing and
    // no test would have minded.
    let (client, scopes, prompt) = grant_inputs(WRANGLER_CLIENT_ID);
    assert_eq!(client.id, WRANGLER_CLIENT_ID);
    // A public client sends no secret at all, which is what the table says
    // Cloudflare is. An empty string here would be a different request.
    assert!(client.secret.is_none());
    assert_eq!(scopes, SCOPES);
    assert_eq!(prompt.label, "Cloudflare");
    assert_eq!(prompt.retry, "apex cf connect");
}

#[test]
fn what_this_command_prints_when_the_code_expires_names_this_command() {
    // The shared transport carries the retry line rather than holding one, so
    // a Google sign-in cannot be told to run `apex cf connect`. This is the
    // half of that contract Cloudflare owns.
    assert_eq!(prompt().label, "Cloudflare");
    assert_eq!(prompt().retry, "apex cf connect");
    let said = crate::oauth_device::DeviceError::Expired(prompt().retry.to_string()).to_string();
    assert!(said.contains("apex cf connect"), "{said}");
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

#[test]
fn the_credential_a_refresh_renews_is_the_one_this_cli_stored() {
    // The whole refresh path rests on a derivation nothing else asserts. The
    // daemon never takes a name from the caller: `oauth.token.refresh` binds
    // by running `account::renewed_service` over the name the request is
    // already against, so `cloudflare-refresh` has to derive `cloudflare` —
    // this file's own `SERVICE`. Rename either constant and a refresh would
    // renew a credential that is not there, or nothing at all, and every test
    // in the provider would still pass because it uses its own fixture names.
    assert_eq!(
        apex_secret_core::account::renewed_service(REFRESH_SERVICE).as_deref(),
        Some(SERVICE),
        "the daemon would not derive '{SERVICE}' from '{REFRESH_SERVICE}'"
    );
    // And the host it is filed under is one the daemon can find a token
    // endpoint for. A refresh token pinned to a host that is not in the table
    // is refused with "not an authorisation server", which reads as though
    // there were no refresh at all.
    let oauth = apex_secret_core::account::oauth_for_auth_host(AUTH_HOST)
        .expect("the host the refresh token is filed under has no token endpoint");
    assert_eq!(oauth.token_url, "https://dash.cloudflare.com/oauth2/token");
}

#[test]
fn the_client_the_grant_was_issued_to_is_recorded_where_the_daemon_reads_it() {
    // RFC 6749 §6 requires a refresh to present the same client the grant was
    // issued to, and `--client-id` lets somebody sign in as one that is not
    // Wrangler's. The daemon reads that client out of the store's non-secret
    // half and falls back to the table only when it holds the placeholder — so
    // storing the placeholder for an overridden grant would renew as Wrangler
    // and earn `invalid_client` with nothing to explain it.
    let overridden = "0f8a1c22-1111-2222-3333-444455556666";
    assert_eq!(username_for(REFRESH_SERVICE, overridden), overridden);
    // The access token has no client half: it is presented as a bearer token,
    // and `x-access-token` is the store's own "there is no name half".
    assert_eq!(
        username_for(SERVICE, overridden),
        apex_secret_core::store::DEFAULT_USERNAME
    );
    assert_eq!(
        username_for(SERVICE, overridden),
        "x-access-token",
        "the placeholder moved; the daemon's fallback reads this exact string"
    );
    // A default connect records Wrangler's id explicitly rather than leaving
    // the daemon to infer it, so the two can never disagree about which client
    // a stored grant belongs to.
    assert_eq!(
        username_for(REFRESH_SERVICE, WRANGLER_CLIENT_ID),
        WRANGLER_CLIENT_ID
    );
    // ...and whatever is recorded has to be something the daemon will put in a
    // form body. `connect_by_device_code` refuses a client id that is not,
    // before any of this runs.
    assert!(crate::oauth_device::valid_form_value(WRANGLER_CLIENT_ID));
    assert!(crate::oauth_device::valid_form_value(overridden));
}

#[test]
fn the_operation_the_cli_asks_for_is_the_one_the_provider_declares() {
    // One constant in the crate the CLI and the daemon both link. The
    // registry-side half of this — that a provider actually offers it — is
    // `providers::tests::the_shipped_registry_builds_and_offers_every_
    // provider_s_vocabulary`, which cannot be run from here.
    assert_eq!(REFRESH_OPERATION, "oauth.token.refresh");
    assert_eq!(
        REFRESH_OPERATION,
        apex_secret_core::account::REFRESH_OPERATION
    );
    // The grant is per credential, and it is the REFRESH credential that is
    // spent — granting the operation on `cloudflare` would allow nothing.
    assert_ne!(SERVICE, REFRESH_SERVICE);
}

// ── what `status` says about renewing ───────────────────────────────────────

#[test]
fn what_status_says_about_renewing_is_the_grant_the_daemon_will_check() {
    // The line this replaces printed `apex cf refresh` whenever a refresh
    // token existed, and a refresh is a `Use`: it needs a per-project grant
    // that `apex cf connect` deliberately does not write. So on every machine
    // that had connected, the status line named a command that was about to be
    // refused and said nothing about why.
    let here = "/p";
    let mut grants = Grants::default();
    assert_eq!(renewal(true, Some(here), &grants), Renewal::NotGranted);

    // No refresh token outranks the grant: writing one would change nothing.
    assert_eq!(renewal(false, Some(here), &grants), Renewal::NoRefreshToken);
    // And a grant is per project, so outside one none can match.
    assert_eq!(renewal(true, None, &grants), Renewal::NotInAProject);

    grants.allow(here, REFRESH_SERVICE, REFRESH_OPERATION);
    assert_eq!(renewal(true, Some(here), &grants), Renewal::Granted);
    // Still nothing to spend, even now that spending it is allowed.
    assert_eq!(renewal(false, Some(here), &grants), Renewal::NoRefreshToken);
    // Granted where it was written and nowhere else.
    assert_eq!(renewal(true, Some("/other"), &grants), Renewal::NotGranted);
}

#[test]
fn renewing_is_not_read_off_a_grant_for_the_other_credential_or_the_other_verb() {
    // The two halves are stored separately on purpose — the access token under
    // `cloudflare` for the API host, the refresh token under
    // `cloudflare-refresh` for the auth host — and the whole value of that
    // split is that a grant on one is not a grant on the other. A status line
    // that matched on the operation alone, or on the service alone, would
    // report "this project may spend it" off a grant to deploy a worker.
    let here = "/p";

    let mut wrong_credential = Grants::default();
    wrong_credential.allow(here, SERVICE, REFRESH_OPERATION);
    assert_eq!(
        renewal(true, Some(here), &wrong_credential),
        Renewal::NotGranted
    );

    let mut wrong_operation = Grants::default();
    wrong_operation.allow(here, REFRESH_SERVICE, "cloudflare.worker.deploy");
    assert_eq!(
        renewal(true, Some(here), &wrong_operation),
        Renewal::NotGranted
    );
}
