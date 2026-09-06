//! The permission dimensions' acceptance criteria, as tests.
//!
//! P0-004 split the six dimensions and this file was its acceptance suite;
//! P0-008 filled in the network one and P0-013/P0-014 filled in the origin
//! one, both adding their criteria here rather than starting a second file
//! that would assert over the same value sets in a different shape.
//!
//! §3.1 says the six permission layers "must never be collapsed into one
//! switch". Three of those non-collapses are acceptance criteria in their own
//! right, and each one is a claim about what a change to one dimension is
//! allowed to do to another:
//!
//! | criterion | the collapse it forbids |
//! |---|---|
//! | `bypassPermissions` does not disable the APEX sandbox | dimension 1 → dimension 2 |
//! | unrestricted-user does not imply root | dimension 2 → dimension 3 |
//! | a root grant does not imply secret export | dimension 3 → dimension 4 |
//!
//! Each is tested over the *whole* value set of the driving dimension rather
//! than over the one value that names the criterion. "Bypass does not disable
//! the sandbox" is worth little if `Ask` does; the property is that dimension 1
//! cannot reach dimension 2 at all, and iterating proves that where a single
//! assertion would only sample it.
//!
//! A separate integration test rather than a `mod tests` inside `policy.rs`,
//! because these are the acceptance criteria for a roadmap task and should be
//! findable by name — `cargo test --test policy_invariants` runs exactly them.
//! They also exercise the crate through its public API, which is the surface
//! the daemon and the CLI actually use.

use apex_agent_core::adapter;
use apex_agent_core::origin::{may_declare, OriginError, OriginSource, SessionOrigin};
use apex_agent_core::policy::{
    AgentPolicy, NativeMode, NetworkPolicy, OriginPolicy, PolicyPreset, RequestOrigin,
    SandboxPolicy, SecretPolicy, SystemAccess,
};
use apex_agent_core::destination::Allowlist;
use apex_agent_core::request::{Decision, PrivilegeRequest, Verb};
use apex_agent_core::sandbox::{build_argv, EgressBridge, SandboxSpec, BRIDGE_PORT};

use std::path::PathBuf;

/// A spec that can be turned into a real bwrap argv, for the assertions that
/// go past the struct and look at what would actually run.
fn spec(policy: AgentPolicy) -> SandboxSpec {
    let mut s = SandboxSpec::new(
        policy,
        PathBuf::from("/home/tester"),
        PathBuf::from("/run/user/1000"),
    );
    s.control_socket = PathBuf::from("/run/user/1000/apex-agentd/control.sock");
    s.scratch = PathBuf::from("/tmp/apex-agent/1");
    s.cwd = PathBuf::from("/home/tester/Projects/demo");
    s.rw = vec![PathBuf::from("/home/tester/Projects/demo")];
    // Present for every policy, and used only by the one network mode that
    // requires it — so a test can iterate over the whole value set without
    // each case having to know which modes need a route out.
    s.egress = Some(EgressBridge {
        program: PathBuf::from("/usr/bin/apex-agentd"),
        socket: PathBuf::from("/tmp/apex-agent/1/egress.sock"),
        port: BRIDGE_PORT,
    });
    s
}

fn argv(policy: AgentPolicy) -> Vec<String> {
    build_argv(&spec(policy), "claude", &[]).expect("build")
}

/// One dimension's name, and a function that moves it off its default.
type Mutation = (&'static str, fn(&mut AgentPolicy));

/// Whether `argv` contains the two-token sequence `op arg`.
fn has(argv: &[String], op: &str, arg: &str) -> bool {
    argv.windows(2).any(|w| w[0] == op && w[1] == arg)
}

// ── criterion 2 ─────────────────────────────────────────────────────────────

#[test]
fn agent_native_bypass_does_not_disable_the_apex_sandbox() {
    // §3.1: "bypassPermissions changes only layer 1."
    //
    // Layer 2 is checked in both places it exists: the policy field, and the
    // argv the field produces. A test that only compared the field would pass
    // against a build_argv that had learned to special-case the native mode.
    let baseline = argv(AgentPolicy::default());

    for native in NativeMode::ALL {
        let p = AgentPolicy {
            native: *native,
            ..AgentPolicy::default()
        };

        assert_eq!(
            p.sandbox,
            SandboxPolicy::default(),
            "the {native} native mode moved the sandbox dimension"
        );
        assert!(
            p.sandbox.is_confined(),
            "the {native} native mode unconfined the session"
        );

        let a = argv(p);
        assert_eq!(a, baseline, "the {native} native mode changed the sandbox argv");
        // Spelled out as well as compared, so the failure names the property
        // rather than dumping two argv vectors.
        assert!(has(&a, "--tmpfs", "/home/tester"), "the home mask went away");
        assert!(has(&a, "--tmpfs", "/run"), "the /run mask went away");
        assert!(a.iter().any(|x| x == "--unshare-pid"), "PID isolation went away");
        assert!(a.first().is_some_and(|x| x.ends_with("bwrap")), "not confined at all");
    }
}

#[test]
fn the_recommended_high_autonomy_mode_keeps_every_apex_layer() {
    // §4.2's table, read straight down:
    //   agent-native confirmations   OFF
    //   APEX project sandbox         ON
    //   APEX secret broker           ON
    //   APEX root boundary           ON
    let p = PolicyPreset::AgentBypass.policy();
    assert_eq!(p.native, NativeMode::Bypass);
    assert_eq!(p.sandbox, SandboxPolicy::Project);
    assert_eq!(p.secrets, SecretPolicy::Brokered);
    assert_eq!(p.system, SystemAccess::None);
    assert!(p.no_new_privs());
    // And it is a mode a user can actually run today.
    assert_eq!(p.validate(), Ok(()));
}

#[test]
fn the_native_mode_reaches_the_agent_and_nothing_else() {
    // The other half of criterion 2: bypass must genuinely do its own job, or
    // "it does not disable the sandbox" would be true of a flag that did
    // nothing at all.
    let claude = adapter::by_id("claude").expect("the claude adapter");
    assert_eq!(
        claude.build_args(NativeMode::Bypass, None, &[]),
        vec!["--permission-mode".to_string(), "bypassPermissions".to_string()]
    );
    assert!(claude.build_args(NativeMode::Inherit, None, &[]).is_empty());
}

// ── criterion 3 ─────────────────────────────────────────────────────────────

#[test]
fn unrestricted_user_does_not_imply_root() {
    // §3.3: "A managed agent must never become root merely because … the agent
    // is in APEX unrestricted-user mode." §4.3: "still no automatic root;
    // no_new_privs should remain active unless system-access mode explicitly
    // changes it."
    //
    // Over every sandbox value, and over every native mode with it, because
    // §3.3 lists native bypass as a second thing that must not imply root.
    for sandbox in SandboxPolicy::ALL {
        for native in NativeMode::ALL {
            let p = AgentPolicy {
                sandbox: *sandbox,
                native: *native,
                ..AgentPolicy::default()
            };
            assert_eq!(
                p.system,
                SystemAccess::None,
                "{sandbox} + {native} granted a system capability"
            );
            assert!(
                p.no_new_privs(),
                "{sandbox} + {native} dropped no_new_privs, so a setuid binary \
                 inside the session could still escalate"
            );
        }
    }

    // The preset for §4.3's command line, in full.
    let p = PolicyPreset::Unrestricted.policy();
    assert_eq!(p.sandbox, SandboxPolicy::Unrestricted);
    assert_eq!(p.system, SystemAccess::None);
    assert!(p.no_new_privs());
    assert_eq!(p.secrets, SecretPolicy::Brokered);
}

#[test]
fn a_system_grant_is_the_only_thing_that_can_lift_no_new_privs() {
    // The converse, so the assertion above cannot be satisfied by a
    // `no_new_privs` that is hardcoded true and therefore meaningless.
    let mut lifted = Vec::new();
    for system in SystemAccess::ALL {
        let p = AgentPolicy {
            system: *system,
            ..AgentPolicy::default()
        };
        if !p.no_new_privs() {
            lifted.push(*system);
        }
    }
    assert_eq!(
        lifted,
        vec![SystemAccess::Unsafe],
        "only §4.5 break-glass may lift no_new_privs"
    );
}

// ── criterion 4 ─────────────────────────────────────────────────────────────

#[test]
fn a_root_grant_does_not_imply_secret_export() {
    // §3.4's last requirement, which survives even break-glass: "broker
    // secrets are still not conveniently dumped into the agent environment."
    for system in SystemAccess::ALL {
        let p = AgentPolicy {
            system: *system,
            ..AgentPolicy::default()
        };
        assert_eq!(
            p.secrets,
            SecretPolicy::Brokered,
            "the {system} system-access mode moved the secret dimension"
        );
    }

    // Both elevated presets, by name, because they are what a user reaches
    // for and what a future edit would be tempted to make "complete".
    for preset in [
        PolicyPreset::UnsafeSystemAccess,
        PolicyPreset::UnsafeEverything,
    ] {
        assert_eq!(
            preset.policy().secrets,
            SecretPolicy::Brokered,
            "{preset} exported raw secrets"
        );
    }
}

#[test]
fn raw_secret_export_has_no_route_at_all_in_this_build() {
    // §7's default policy table denies "read raw brokered secret" from every
    // origin, local included. Refusing it in `validate` is what makes the
    // criterion above a boundary rather than a default somebody can flip.
    let p = AgentPolicy {
        secrets: SecretPolicy::Export,
        ..AgentPolicy::default()
    };
    assert!(p.validate().is_err());
}

// ── criterion 1 ─────────────────────────────────────────────────────────────

#[test]
fn every_dimension_moves_on_its_own() {
    // "Each policy dimension is independently configurable", stated as the
    // property it has to be: setting any one dimension to any of its values
    // leaves the other five exactly where they were.
    //
    // The one documented exception is the network floor — `strict` forces the
    // network dimension to `offline` — so the comparison is against the
    // normalised baseline for that pair and against the field for everything
    // else.
    let base = AgentPolicy::default();

    // Plain function pointers: every closure here is non-capturing, and a
    // boxed trait object would only add a type nobody needs to read.
    let mutate: [Mutation; 6] = [
        ("native", |p| p.native = NativeMode::Bypass),
        ("sandbox", |p| p.sandbox = SandboxPolicy::Unrestricted),
        ("system", |p| p.system = SystemAccess::Session),
        ("secrets", |p| p.secrets = SecretPolicy::None),
        ("network", |p| p.network = NetworkPolicy::Offline),
        ("origin", |p| p.origin = OriginPolicy::RemoteElevationAllowed),
    ];

    for (name, apply) in &mutate {
        let mut p = base;
        apply(&mut p);
        let moved: Vec<&str> = p
            .dimensions()
            .into_iter()
            .zip(base.dimensions())
            .filter(|((_, got), (_, want))| got != want)
            .map(|((n, _), _)| n)
            .collect();
        assert_eq!(
            moved,
            vec![*name],
            "setting {name} moved {moved:?} instead of only itself"
        );
    }
}

#[test]
fn the_named_modes_are_reachable_and_so_is_everything_between_them() {
    // §4's modes exist as presets, and the presets do not fence the space in:
    // a combination none of them names is still expressible, which is what
    // stops the next requirement from becoming a seventh preset.
    let mut seen: Vec<AgentPolicy> = Vec::new();
    for preset in PolicyPreset::ALL {
        let p = preset.policy();
        assert!(
            !seen.contains(&p),
            "{preset} is an alias of a mode above it, so §4 has one fewer mode \
             than it says it does"
        );
        seen.push(p);
    }

    // A combination no preset produces: confined, offline, no broker, and the
    // agent's own confirmations still on.
    let bespoke = AgentPolicy {
        native: NativeMode::Ask,
        sandbox: SandboxPolicy::Project,
        system: SystemAccess::None,
        secrets: SecretPolicy::None,
        network: NetworkPolicy::Offline,
        origin: OriginPolicy::LocalElevationOnly,
    };
    assert!(
        !PolicyPreset::ALL.iter().any(|p| p.policy() == bespoke),
        "this is supposed to be a combination no preset names"
    );
    assert_eq!(bespoke.validate(), Ok(()));
    assert!(argv(bespoke).iter().any(|x| x == "--unshare-net"));
}

// ── P0-008: the network dimension ───────────────────────────────────────────

#[test]
fn every_network_mode_but_open_takes_the_session_off_the_host_network() {
    // Criterion 1, at the level where it is enforced. §38 asks for four modes;
    // three of them are one kernel fact — a network namespace with nothing in
    // it — and differ in what apex-agentd offers over a Unix socket after
    // that. Asserted over the whole value set so a fifth mode cannot arrive
    // with the host's network by omission.
    for network in NetworkPolicy::ALL {
        let p = AgentPolicy {
            sandbox: SandboxPolicy::Project,
            network: *network,
            ..AgentPolicy::default()
        };
        assert_eq!(
            argv(p).iter().any(|x| x == "--unshare-net"),
            *network != NetworkPolicy::Open,
            "{network} got the wrong network namespace"
        );
    }
}

#[test]
fn strict_is_still_fail_closed_whatever_the_network_dimension_says() {
    // Criterion 2. `strict` is a floor, and the floor is asserted in the argv
    // rather than in `effective_network`, because the argv is what runs. A
    // client that sends `{"sandbox":"strict","network":"open"}` — an older one
    // sends exactly that, since `open` is the deserialisation default — must
    // still get a network-isolated session.
    for network in NetworkPolicy::ALL {
        let p = AgentPolicy {
            sandbox: SandboxPolicy::Strict,
            network: *network,
            ..AgentPolicy::default()
        };
        assert_eq!(p.effective_network(), NetworkPolicy::Offline, "{network}");
        assert!(
            argv(p).iter().any(|x| x == "--unshare-net"),
            "strict kept the network with network={network}"
        );
        // And the record stored for the session says so, so a listing cannot
        // report a network the session does not have.
        assert_eq!(p.normalised().network, NetworkPolicy::Offline, "{network}");
    }
}

#[test]
fn a_network_mode_is_refused_wherever_it_could_not_be_enforced() {
    // The other half of criterion 2. A mode that cannot be enforced must fail
    // the session, never fall through to `open` — an unenforced
    // `--network allowlist` would read as a protection in `apex agent status`,
    // in the Agent Center and in a script, with nothing behind it.
    //
    // No mode may be granted without the namespace it is enforced by.
    for network in NetworkPolicy::ALL {
        let p = AgentPolicy {
            sandbox: SandboxPolicy::Unrestricted,
            network: *network,
            ..AgentPolicy::default()
        };
        assert_eq!(
            p.validate().is_ok(),
            *network == NetworkPolicy::Open,
            "an unconfined session was granted {network}, which it cannot enforce"
        );
        // And the argv builder refuses the same pair, so a caller that skipped
        // the policy check still cannot produce a command running on the
        // host's network under a policy that said otherwise.
        assert_eq!(
            build_argv(&spec(p), "claude", &[]).is_ok(),
            *network == NetworkPolicy::Open,
            "an unconfined {network} session was built anyway"
        );
    }

    // And `allowlist` is refused when there is nothing on the allowlist: a
    // session reporting a destination policy while reaching nothing is an
    // offline session under another name.
    let allowlisted = AgentPolicy {
        sandbox: SandboxPolicy::Project,
        network: NetworkPolicy::Allowlist,
        ..AgentPolicy::default()
    };
    assert!(allowlisted.validate_for(&Allowlist::default()).is_err());
    assert!(allowlisted
        .validate_for(&Allowlist::parse(&["api.example.com"]).expect("parse"))
        .is_ok());

    // The argv builder refuses it a third time when the route the mode
    // depends on was not built.
    let mut without_bridge = spec(allowlisted);
    without_bridge.egress = None;
    assert!(build_argv(&without_bridge, "claude", &[]).is_err());
}

#[test]
fn the_network_dimension_cannot_reach_the_secret_one() {
    // Criterion 4's precondition. Brokered egress is only a way out if the
    // broker answers, and the broker's gate is the secret dimension — so a
    // network mode must not be able to move it, in either direction. A
    // `--network brokered` that quietly turned the broker on would be a
    // network flag granting a credential capability.
    for network in NetworkPolicy::ALL {
        let p = AgentPolicy {
            sandbox: SandboxPolicy::Project,
            network: *network,
            ..AgentPolicy::default()
        };
        assert_eq!(
            p.secrets,
            SecretPolicy::default(),
            "the {network} network mode moved the secret dimension"
        );
        assert!(
            p.secrets.may_use_broker(),
            "the {network} network mode shut the broker"
        );
    }

    // The pair that contradicts is refused rather than resolved: brokered
    // egress with `--secrets none` is a session with no way out at all.
    let shut = AgentPolicy {
        sandbox: SandboxPolicy::Project,
        network: NetworkPolicy::Brokered,
        secrets: SecretPolicy::None,
        ..AgentPolicy::default()
    };
    assert!(shut.validate().is_err());
}

// ── the sixth dimension ─────────────────────────────────────────────────────

#[test]
fn a_remote_origin_cannot_authorise_what_section_seven_reserves_for_a_local_one() {
    // §7's table: root capability and unsafe-everything both need local
    // approval. Dimension 6 is what says so, and it is separate from the
    // origin a request happens to carry.
    let p = AgentPolicy::default();
    assert_eq!(p.origin, OriginPolicy::LocalElevationOnly);
    for origin in RequestOrigin::ALL {
        assert_eq!(
            p.origin.allows_elevation_from(*origin),
            origin.is_local(),
            "{origin}"
        );
    }
    assert!(!RequestOrigin::RemoteControl.is_local());
    assert!(!RequestOrigin::ScheduledJob.is_local());
}

// ── P0-013: remote-origin requests are distinguishable from local ones ──────
//
// roadmap.yaml's security_invariants: "Remote-origin capability requests must
// be distinguishable from local requests." That is one sentence and three
// separate claims, so it is three tests:
//
//   1. the value is carried, and a request that has none does not read as
//      local;
//   2. nothing a client sends can produce a local one;
//   3. the human deciding is shown which it is.
//
// Written against the same public API the daemon and the CLI use, so a change
// that broke any of them for a real caller breaks these too.

#[test]
fn a_remote_origin_request_is_distinguishable_from_a_local_one() {
    // Criterion 1, and the security invariant in its plainest form. Two
    // requests identical in every other field must not compare equal on the
    // only question §7's table branches on.
    let mut local = sample_request();
    local.request_origin = Some(RequestOrigin::LocalTerminal);
    local.origin_source = Some(OriginSource::Observed);

    let mut remote = local.clone();
    remote.request_origin = Some(RequestOrigin::RemoteControl);
    remote.origin_source = Some(OriginSource::Declared);

    assert!(local.is_local());
    assert!(!remote.is_local());
    assert_ne!(local.origin_summary(), remote.origin_summary());
    assert_ne!(local.prompt(), remote.prompt());

    // And every origin in the vocabulary lands on one side or the other, so
    // "distinguishable" holds for the whole value set rather than the pair
    // above.
    for origin in RequestOrigin::ALL {
        let mut r = local.clone();
        r.request_origin = Some(*origin);
        assert_eq!(r.is_local(), origin.is_local(), "{origin}");
        assert!(r.prompt().contains(origin.as_str()), "{origin}");
    }
}

#[test]
fn an_unrecorded_origin_is_not_a_local_one() {
    // The fail-open this invariant is most likely to be broken by, since
    // `RequestOrigin::default()` is `local-terminal`: a record written before
    // origin tracking, or a field a future serde attribute lets default,
    // would silently join the column §7 reserves root for.
    let mut r = sample_request();
    r.request_origin = None;
    r.origin_source = None;
    assert!(!r.is_local());
    assert!(r.origin_summary().contains("unknown"), "{}", r.origin_summary());

    // The same claim about the value the daemon would fall back to.
    assert!(
        RequestOrigin::default().is_local(),
        "if this ever stops being true the test above stops testing anything"
    );
}

#[test]
fn no_declaration_can_turn_a_remote_request_into_a_local_one() {
    // Criterion 2. A field that can be set by the thing being restricted is
    // not a restriction, so this is asserted over the whole 7×7 product of
    // starting points and requested values rather than over the interesting
    // pairs.
    for from in RequestOrigin::ALL {
        for to in RequestOrigin::ALL {
            let after = SessionOrigin::observed(*from).declare(*to);
            if let Ok(after) = after {
                assert!(
                    !after.is_local(),
                    "{from} declared itself {to} and became local"
                );
                assert_eq!(after.source, OriginSource::Declared);
            }
        }
    }
    // Both spellings of local are refused by name, so the failure message
    // says which rule stopped it.
    for local in [RequestOrigin::LocalTerminal, RequestOrigin::ApexShell] {
        assert_eq!(
            may_declare(RequestOrigin::RemoteControl, local),
            Err(OriginError::NotDeclarable(local))
        );
    }
}

#[test]
fn the_approval_prompt_says_which_column_of_section_sevens_table_applies() {
    // Criterion 3. Distinguishable to a program is not distinguishable to the
    // human being asked for root. `prompt()` is the one renderer behind
    // `apex request show`, `pending`, `ask` and the interactive confirm, so
    // asserting it here covers all four.
    for origin in RequestOrigin::ALL {
        let mut r = sample_request();
        r.request_origin = Some(*origin);
        r.origin_source = Some(OriginSource::Observed);
        let p = r.prompt();
        assert!(p.contains("Origin:"), "{origin}: {p}");
        assert!(p.contains(origin.as_str()), "{origin}: {p}");
        assert!(p.contains("observed"), "{origin}: {p}");
    }
}

fn sample_request() -> PrivilegeRequest {
    PrivilegeRequest {
        id: 1,
        verb: Verb::Install {
            packages: vec!["clang".into()],
        },
        reason: "Required to compile the project".into(),
        session: Some(4),
        agent: Some("claude".into()),
        project: Some("/home/tester/Projects/demo".into()),
        request_origin: None,
        origin_source: None,
        decision: Decision::Pending,
        created_ms: 1_700_000_000_000,
        decided_ms: None,
        executed_ms: None,
        exit_code: None,
    }
}
