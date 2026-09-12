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
use apex_agent_core::origin::{
    may_declare, Capability, OriginError, OriginSource, Ruling, SessionOrigin,
};
use apex_agent_core::policy::{
    AgentPolicy, ConnectorPolicy, NativeMode, NetworkPolicy, OriginPolicy, PolicyPreset, RequestOrigin,
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
        connectors: ConnectorPolicy::LocalOnly,
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
    assert!(allowlisted.validate_for(&Allowlist::default(), &[]).is_err());
    assert!(allowlisted
        .validate_for(&Allowlist::parse(&["api.example.com"]).expect("parse"), &[])
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

// ── P0-014: the Remote Control policy ───────────────────────────────────────

#[test]
fn a_remote_session_may_still_edit_run_tests_and_push() {
    // P0-014's first criterion, and the one most easily lost by being
    // careful: §7 opens with "Remote Control is a normal workflow, not an
    // edge case", and four of its eight rows are `allow` from both columns.
    // A build that tightened them would be safer and wrong.
    for origin in RequestOrigin::ALL {
        for cap in [
            Capability::EditProject,
            Capability::RunTests,
            Capability::GitHubPush,
            Capability::CloudflarePreviewDeploy,
        ] {
            assert!(
                cap.ruling(*origin).is_unattended(),
                "{cap} needed a decision from {origin}"
            );
        }
    }

    // The three places that could have refused a remote session and must not:
    //
    // - starting one. No dimension is derived from the origin, so a session
    //   runs under the same six wherever it is driven from.
    for origin in RequestOrigin::ALL {
        let p = AgentPolicy::default();
        assert_eq!(p.validate(), Ok(()), "{origin}");
        assert_eq!(p.sandbox, SandboxPolicy::Project);
    }
    // - reaching the broker, which is dimension 4's question and takes no
    //   origin at all. `may_use_broker(&self)` has no parameter to pass one
    //   to, which is the strongest form this claim can take.
    assert!(SecretPolicy::Brokered.may_use_broker());
    // - the secret dimension itself, which a remote origin does not move.
    for origin in RequestOrigin::ALL {
        assert_eq!(
            AgentPolicy::default().secrets,
            SecretPolicy::Brokered,
            "{origin}"
        );
    }
}

#[test]
fn a_root_request_is_never_decided_without_a_local_human() {
    // P0-014's second criterion, as a property of the request rather than of
    // the daemon that enforces it. Every verb in the vocabulary is a root
    // capability, so there is no request in this build that §7 lets happen
    // unattended.
    for name in Verb::names() {
        let args = if matches!(*name, "install" | "remove") {
            vec!["clang".to_string()]
        } else {
            vec![]
        };
        let verb = Verb::parse(name, &args).expect("a real verb");
        assert_eq!(
            verb.capability(),
            Capability::RootCapability,
            "{name} is not treated as a root operation"
        );

        for origin in RequestOrigin::ALL {
            let mut r = sample_request();
            r.verb = verb.clone();
            r.request_origin = Some(*origin);
            let ruling = r.ruling();
            assert!(!ruling.is_unattended(), "{name} from {origin} ran unasked");
            assert!(ruling.needs_local_human(), "{name} from {origin}");
            assert_eq!(
                ruling == Ruling::LocalAuth,
                origin.is_local(),
                "{name} from {origin}"
            );
        }

        // And a request with no recorded origin gets the remote column, not
        // the local one.
        let mut r = sample_request();
        r.verb = verb;
        r.request_origin = None;
        assert_eq!(r.ruling(), Ruling::LocalApproval, "{name} with no origin");
    }
}

#[test]
fn remote_elevation_is_configurable_and_is_now_authenticated_rather_than_refused() {
    // P0-014's third criterion, closed. This test used to assert that the
    // setting parsed and was then REFUSED, because §7 allows an owner to opt
    // into remote elevation "with strong WebAuthn/FIDO2 authentication" and
    // nothing in the build could ask for a security key. That refusal was the
    // honest answer for as long as it was true.
    //
    // It is no longer true, so the assertion is inverted rather than deleted:
    // the verifier, the challenge round trip and the gate all exist, and
    // `apex-agentd`'s `privilege::decide_origin` reads this very value.
    assert_eq!(
        OriginPolicy::parse("remote"),
        Some(OriginPolicy::RemoteElevationAllowed)
    );
    let p = AgentPolicy {
        origin: OriginPolicy::RemoteElevationAllowed,
        ..AgentPolicy::default()
    };
    assert_eq!(p.validate(), Ok(()));

    // What the setting does NOT do, which is the half worth asserting now
    // that it is accepted: it moves one dimension and nothing else. It is an
    // opt-in to a second authentication path, not a preset, so it must not
    // quietly bring root, a looser sandbox or exported secrets with it.
    let d = AgentPolicy::default();
    assert_eq!(p.system, d.system);
    assert_eq!(p.sandbox, d.sandbox);
    assert_eq!(p.secrets, d.secrets);
    assert_eq!(p.native, d.native);
    assert_eq!(p.effective_network(), d.effective_network());

    // And it is still not the default: an owner has to ask for it per
    // session. §7's second column stays closed unless somebody opens it.
    assert_eq!(
        AgentPolicy::default().origin,
        OriginPolicy::LocalElevationOnly
    );
    assert_ne!(AgentPolicy::default().origin, p.origin);
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
        actor: None,
        decision: Decision::Pending,
        created_ms: 1_700_000_000_000,
        decided_ms: None,
        executed_ms: None,
        exit_code: None,
        system_grant: None,
    }
}

// ── P0-005: the agent's own permission mode is inherited, not overridden ────
//
// §4.1 is three sentences and each is a separate claim:
//
//   1. a Claude profile that defaults to `bypassPermissions` keeps it under
//      `a` — APEX must pass nothing and must not shadow the setting;
//   2. APEX does not duplicate the dangerous-mode warning Claude already
//      prints;
//   3. the agent-native mode is visible in the Agent Center.
//
// Criterion 3 has a trap in it, and it is the one this task actually turned
// on: `policy.native` reads `inherit` for the normal case, which describes
// what APEX *did* — pass no flag — and not what the agent is doing. A shell
// rendering "inherit" beside a session running `bypassPermissions` would have
// satisfied the words and answered nothing. `native_observed` is the field
// that answers it.

#[test]
fn a_claude_profile_that_defaults_to_bypass_keeps_it_under_the_shortcut() {
    // Criterion 1. `a` and an unqualified `apex agent run` both resolve to
    // the default policy, whose dimension 1 is `inherit`, and `inherit` is
    // the ABSENCE of a flag rather than a flag meaning "default" — so
    // `~/.claude/settings.json` is the only thing deciding, which is what
    // §4.1 asks for.
    let p = AgentPolicy::default();
    assert_eq!(p.native, NativeMode::Inherit);

    let claude = adapter::by_id("claude").expect("the claude adapter");
    let args = claude.build_args(NativeMode::Inherit, Some("go"), &[]);
    assert!(
        !args.iter().any(|a| a == "--permission-mode"),
        "APEX passed a permission mode under the default policy: {args:?}"
    );
    assert_eq!(args, vec!["go".to_string()], "{args:?}");

    // And it is true of every adapter, not only the one the criterion names:
    // an adapter that expressed `inherit` as a flag would be overriding a
    // profile in exactly the way §4.1 forbids, and it would do it silently.
    for a in adapter::ADAPTERS {
        assert_eq!(
            a.native_mode_args(NativeMode::Inherit),
            Some(Vec::new()),
            "{} spells inherit as a flag",
            a.id
        );
    }
}

#[test]
fn the_settings_file_apex_injects_cannot_move_the_agents_permission_mode() {
    // The other half of criterion 1, and the one that could have broken it
    // without anybody typing a flag. P0-011 starts a managed Claude with
    // `--settings <scratch>/claude-hooks.json`, which is a higher-precedence
    // settings source than `~/.claude/settings.json`. A `permissions` key in
    // that document — even an innocuous-looking `defaultMode` — would
    // override the user's own default on every managed launch, and the only
    // symptom would be Claude asking for confirmations it had been told not
    // to ask for.
    //
    // So: an ALLOWLIST, not a free hand. Two keys, each here for a stated
    // reason, and a third would have to be argued for in this test before it
    // could ship.
    //
    //   hooks       §6.1's lifecycle bridge. A list key, so Claude combines it
    //               across sources and the user's own hooks keep running.
    //   statusLine  §P1-021's telemetry. An OBJECT key, so this document
    //               replaces the user's — which is exactly why
    //               `apex agent statusline` runs the user's own command and
    //               copies its output through. It is presentation and a
    //               measurement; nothing about it touches a permission.
    let doc = apex_agent_core::hook::settings_json(std::path::Path::new("/usr/bin/apex"), None);
    let obj = doc.as_object().expect("an object");
    let mut keys: Vec<&String> = obj.keys().collect();
    keys.sort();
    assert_eq!(
        keys,
        vec!["hooks", "statusLine"],
        "APEX's settings document carries a key nobody has argued for: {doc}"
    );
    for forbidden in ["permissions", "defaultMode", "permissionMode", "allowedTools"] {
        assert!(
            !doc.to_string().contains(forbidden),
            "{forbidden} appears in the settings APEX injects: {doc}"
        );
    }

    // And the status line APEX names is APEX's own. A document that carried
    // the user's command instead would leave the terminal looking right and
    // the Agent Center empty, which is the failure that looks like success.
    let status = doc["statusLine"].as_object().expect("a statusLine object");
    assert_eq!(status["type"], "command");
    assert_eq!(status["command"], "/usr/bin/apex agent statusline");
    assert_eq!(
        status.len(),
        2,
        "with no user status line to copy from, the overlay invents nothing: {doc}"
    );
}

#[test]
fn apex_states_its_own_layers_and_does_not_restate_the_agents_warning() {
    // Criterion 2. Claude prints its own banner in `bypassPermissions`, every
    // launch. A user who set that as their profile default has already agreed
    // to see it, and a second APEX warning saying the same thing would be
    // noise on every run — which is how a warning stops being read, including
    // the one warning here that is worth reading.
    //
    // Stated as a property of the vocabulary: nothing APEX prints about
    // dimension 1 is a caution. The words below are the ones a duplicated
    // warning would use.
    const CAUTION: [&str; 6] = [
        "dangerous",
        "danger",
        "be careful",
        "at your own risk",
        "are you sure",
        "warning",
    ];
    for native in NativeMode::ALL {
        let p = AgentPolicy {
            native: *native,
            ..AgentPolicy::default()
        };
        // Everything APEX says about the six dimensions, in every renderer
        // that shows them: the six labels and values, which is what the
        // launch line and `apex agent status` are both built from.
        let said = p
            .dimensions()
            .iter()
            .map(|(n, v)| format!("{n} {v}"))
            .collect::<Vec<_>>()
            .join(", ")
            .to_lowercase();
        for word in CAUTION {
            assert!(!said.contains(word), "{native}: APEX warned about the agent's own mode: {said}");
        }
        // And APEX's layers ARE stated, which is the other half: not warning
        // must not become not saying anything.
        assert!(said.contains("sandbox project"), "{said}");
        assert!(said.contains("secrets brokered"), "{said}");
        assert!(said.contains("system none"), "{said}");
    }

    // The one mode that DOES get a caution is break-glass, and it is APEX's
    // own boundary being removed rather than the agent's. §3.4 asks for it in
    // as many words.
    assert_eq!(
        PolicyPreset::UnsafeEverything.policy().system,
        SystemAccess::Unsafe
    );
}

#[test]
fn the_agent_center_is_shown_the_mode_the_agent_is_actually_in() {
    // Criterion 3, and the trap in it. `policy.native` is `inherit` for the
    // case that matters, which says what APEX did and not what the agent is
    // doing; a shell rendering "inherit" beside a session running
    // `bypassPermissions` would have satisfied the words and answered
    // nothing.
    //
    // Claude puts `permission_mode` on every hook payload, and the bridge
    // carries it to `SessionInfo.native_observed`. That is the field the
    // Agent Center reads.
    use apex_agent_core::hook::{observe, HookEvent, Payload};

    for mode in ["bypassPermissions", "acceptEdits", "plan", "default"] {
        let payload = Payload {
            permission_mode: Some(mode.to_string()),
            tool_name: Some("Bash".into()),
            ..Payload::default()
        };
        let o = observe(HookEvent::PreToolUse, &payload);
        assert_eq!(o.native.as_deref(), Some(mode), "{mode} was not carried");
    }

    // Passed through, not mapped onto `NativeMode`. Three of those four have
    // no APEX vocabulary at all, and folding them into "not ask" would answer
    // "what mode is this agent in" with a summary of what APEX did about it —
    // which is the thing the user can already see.
    assert_eq!(NativeMode::parse("acceptEdits"), None);
    assert_eq!(NativeMode::parse("plan"), None);

    // An agent that says nothing leaves it absent, which reads as "not
    // reported" rather than as a mode.
    assert_eq!(observe(HookEvent::Stop, &Payload::default()).native, None);

    // And it is bounded and stripped, because it is read off a document the
    // agent writes into a record a client renders in a table.
    for hostile in [
        "bypass\nPRIORITY=7",
        "\x1b[31mroot\x1b[0m",
        &"x".repeat(64),
        "  ",
    ] {
        let payload = Payload {
            permission_mode: Some(hostile.to_string()),
            ..Payload::default()
        };
        assert_eq!(
            observe(HookEvent::Stop, &payload).native,
            None,
            "{hostile:?} was carried through"
        );
    }
}

// ── P0-006 and P0-007: the two system-access grants ─────────────────────────
//
// Six criteria and five, and they overlap: both grants are time-limited, both
// are audited, both take a local password. What separates them is what the
// session gets — a bounded window in which named privilege verbs need no
// second decision, versus `no_new_privs` off and real root — and the tests
// below are written to fail if that ever collapses into one thing.
//
// Two criteria are NOT here, because they are not properties of these types:
// "the authentication prompt occurs outside the agent PTY" is a property of
// which process polkit is asked about, which lives in `apex-agentd`'s
// `privilege::authorise_grant` and is tested there and in
// `tests/test-agent-grants.sh`; and "the agent cannot renew its own grant" is
// the same function's first refusal. Both are about a connection, and a
// connection is not something this file has.

#[test]
fn a_grant_is_bound_to_one_session_and_a_bounded_window() {
    // P0-007 criteria 1 and 2, and P0-006 criteria 2 and 4, which are the
    // same two properties of the same type.
    use apex_agent_core::grant::{ttl_for, GrantKind, MAX_BREAK_GLASS_MS, MAX_SESSION_ACCESS_MS};

    // Every window is bounded, in both modes, and zero is refused as a grant
    // that has already expired.
    for kind in GrantKind::ALL {
        assert!(ttl_for(*kind, Some(0)).is_err(), "{kind} issued a zero window");
        let cap = match kind {
            GrantKind::BreakGlass => MAX_BREAK_GLASS_MS,
            GrantKind::SystemAccess => MAX_SESSION_ACCESS_MS,
        };
        assert!(ttl_for(*kind, Some(cap)).is_ok(), "{kind}");
        assert!(
            ttl_for(*kind, Some(cap + 1)).is_err(),
            "{kind} issued an unbounded window"
        );
    }

    // §3.4's "explicit short TTL", read as written: break-glass will not
    // default its own window. That its cap is the shorter of the two is a fact
    // about two constants and is asserted where the compiler can settle it,
    // beside them in `grant.rs`.
    assert!(ttl_for(GrantKind::BreakGlass, None).is_err());
    assert!(ttl_for(GrantKind::SystemAccess, None).is_ok());
}

#[test]
fn a_grant_that_outlived_a_reboot_is_reported_rather_than_forgotten() {
    // P0-006 criterion 5, and the whole of what it means. "Does not silently
    // persist across reboot" is not "is forgotten on reboot": an owner who
    // authorised fifteen minutes of break-glass and then rebooted has no way
    // to tell whether the window is still open, and a machine that simply
    // loses the grant has answered them with silence.
    //
    // So: the grant is stamped with the boot it was issued under, it is not
    // in force on any other boot, and the machine says which of the two
    // things happened — the window ran out on its own, or the reboot ended it.
    use apex_agent_core::grant::{BootStamp, ClosureReason, GrantKind, SystemGrant};

    let issued = 1_000_000_000_000u64;
    let g = SystemGrant {
        id: 1,
        kind: GrantKind::BreakGlass,
        session: 4,
        agent: "claude".into(),
        project: None,
        capabilities: Vec::new(),
        issued_ms: issued,
        expires_ms: issued + 900_000,
        boot_id: "the-boot-it-was-issued-on".into(),
        request_origin: RequestOrigin::LocalTerminal,
        authenticated_by: "org.apexos.agent.break-glass".into(),
        closed: None,
    };

    // Same boot, inside the window: in force.
    let same = BootStamp {
        id: "the-boot-it-was-issued-on".into(),
        booted_ms: issued - 60_000,
    };
    assert!(g.state_at(issued + 1, &same).is_active());

    // A different boot: not in force, whatever the clock says — including a
    // clock that has not yet reached the expiry.
    let next = BootStamp {
        id: "a-different-boot".into(),
        booted_ms: issued + 300_000,
    };
    assert!(!g.state_at(issued + 1, &next).is_active());
    assert!(!g.state_at(issued + 899_999, &next).is_active());

    // And it is SAID, with the reason, which is the part that makes this
    // criterion different from "it is gone".
    assert_eq!(
        g.state_at(issued + 400_000, &next).reason(),
        Some(ClosureReason::Reboot)
    );
    let said = g.describe(issued + 400_000, &next);
    assert!(said.contains("rebooted"), "{said}");
    assert!(said.contains("nothing has been re-authorised"), "{said}");

    // The distinction is kept rather than collapsed: a window that ran out
    // before the machine went down expired on its own, and calling that
    // "ended at the reboot" would misdescribe it.
    let much_later = BootStamp {
        id: "a-different-boot".into(),
        booted_ms: issued + 900_000,
    };
    assert_eq!(
        g.state_at(issued + 9_000_000, &much_later).reason(),
        Some(ClosureReason::Expired)
    );
}

#[test]
fn every_elevation_takes_a_local_password_and_giving_it_up_takes_none() {
    // P0-006 criterion 1 and P0-007's half of the same thing, as the pure
    // decision they are. The prompt itself cannot be unit-tested; this is the
    // rule that decides whether there is one, and it is asserted over the
    // whole 3×3 product rather than over the interesting pairs.
    use apex_agent_core::auth::{required_for, ACTION_BREAK_GLASS, ACTION_SYSTEM_ACCESS};

    for from in SystemAccess::ALL {
        for to in SystemAccess::ALL {
            let r = required_for(*from, *to);
            assert_eq!(
                r.needs_a_human(),
                *to != SystemAccess::None,
                "{from} -> {to}"
            );
        }
    }
    // Two actions, not one: allowing the smaller thing must not allow
    // break-glass.
    assert_eq!(
        required_for(SystemAccess::None, SystemAccess::Session).action(),
        Some(ACTION_SYSTEM_ACCESS)
    );
    assert_eq!(
        required_for(SystemAccess::None, SystemAccess::Unsafe).action(),
        Some(ACTION_BREAK_GLASS)
    );
    // A renewal is a step toward privilege and is not free. That is the
    // policy half of "the agent cannot renew its own grant": even for a
    // caller who is allowed to ask, there is no standing yes to inherit.
    assert!(required_for(SystemAccess::Unsafe, SystemAccess::Unsafe).needs_a_human());
}

#[test]
fn a_grant_covers_named_verbs_and_break_glass_covers_none_of_them() {
    // P0-007 criterion 3. Capability-scoped means a whitelist by name: a verb
    // added to the vocabulary tomorrow is not covered by a grant issued
    // today, which is what stops the scope from widening under a grant that
    // is already in force.
    use apex_agent_core::grant::{normalise_capabilities, GrantKind, SystemGrant};

    let g = SystemGrant {
        id: 1,
        kind: GrantKind::SystemAccess,
        session: 4,
        agent: "claude".into(),
        project: Some("/p".into()),
        capabilities: normalise_capabilities(["install", "update"]),
        issued_ms: 0,
        expires_ms: 1,
        boot_id: "b".into(),
        request_origin: RequestOrigin::LocalTerminal,
        authenticated_by: "org.apexos.agent.system-access".into(),
        closed: None,
    };
    assert!(g.covers("install"));
    assert!(g.covers("update"));
    for name in Verb::names() {
        if *name != "install" && *name != "update" {
            assert!(!g.covers(name), "{name} was covered by a grant that did not name it");
        }
    }

    // Break-glass carries no capabilities at all, and that is the difference
    // between the two modes rather than an omission: break-glass is sudo
    // inside the session, not a pre-approval of the request vocabulary. So an
    // expired break-glass grant cannot leave behind a session whose
    // `apex request install` goes through unasked.
    let bg = SystemGrant {
        kind: GrantKind::BreakGlass,
        capabilities: Vec::new(),
        ..g.clone()
    };
    for name in Verb::names() {
        assert!(!bg.covers(name), "break-glass covered {name}");
    }
}

#[test]
fn the_two_grants_lift_different_boundaries_and_only_one_lifts_the_kernels() {
    // §4.5: "deliberately different from system-access mode". The difference
    // is a kernel fact, which is also why only one of them can be expired by
    // anything short of ending the session.
    use apex_agent_core::grant::GrantKind;

    let session = AgentPolicy {
        system: SystemAccess::Session,
        sandbox: SandboxPolicy::Unrestricted,
        ..AgentPolicy::default()
    };
    let breakglass = AgentPolicy {
        system: SystemAccess::Unsafe,
        sandbox: SandboxPolicy::Unrestricted,
        ..AgentPolicy::default()
    };
    assert!(session.no_new_privs(), "§4.4 does not hand back setuid");
    assert!(!breakglass.no_new_privs(), "§4.5 is the mode that does");
    assert!(!GrantKind::SystemAccess.expiry_ends_the_session());
    assert!(GrantKind::BreakGlass.expiry_ends_the_session());

    // Neither moves the secret dimension. §3.4 keeps the broker even through
    // break-glass, in as many words.
    assert_eq!(session.secrets, SecretPolicy::Brokered);
    assert_eq!(breakglass.secrets, SecretPolicy::Brokered);

    // And each names its own grant, so nothing arrives at one by omission.
    assert_eq!(AgentPolicy::default().needs_grant(), None);
    assert_eq!(session.needs_grant(), Some(GrantKind::SystemAccess));
    assert_eq!(breakglass.needs_grant(), Some(GrantKind::BreakGlass));
}

#[test]
fn a_grant_that_lifts_no_new_privs_cannot_be_asked_for_inside_a_sandbox() {
    // The finding this task turned up, as a boundary. `bwrap` sets
    // PR_SET_NO_NEW_PRIVS for everything it wraps and no process can clear
    // it, so `--unsafe-everything --sandbox project` would run with the flag
    // ON while every reader of `no_new_privs()` — the sandbox builder, the
    // hook policy point, the Agent Center — reported it off.
    for sandbox in [SandboxPolicy::Project, SandboxPolicy::Strict] {
        let p = AgentPolicy {
            system: SystemAccess::Unsafe,
            sandbox,
            ..AgentPolicy::default()
        };
        assert!(p.validate().is_err(), "{sandbox}");
    }
    // The session grant keeps no_new_privs, so it is at home in any sandbox —
    // which is the point of it being the smaller mode.
    for sandbox in SandboxPolicy::ALL {
        let p = AgentPolicy {
            system: SystemAccess::Session,
            sandbox: *sandbox,
            ..AgentPolicy::default()
        };
        assert_eq!(p.validate(), Ok(()), "{sandbox}");
    }
}

#[test]
fn no_elevation_is_reachable_from_a_stored_default() {
    // §3.4: "no remember forever". Dimension 3 is a grant, not a setting, and
    // a configuration file naming an elevated default would make every later
    // `apex agent run` arrive asking for one. Asserted through the loader
    // every caller actually goes through.
    for stored in ["session", "unsafe"] {
        let cfg = apex_agent_core::config::from_str(&format!(
            r#"{{"system":"{stored}","sandbox":"unrestricted"}}"#
        ))
        .expect("a file naming an elevated default must still load");
        assert_eq!(cfg.policy().system, SystemAccess::None, "{stored}");
        assert_eq!(cfg.policy().needs_grant(), None, "{stored}");
    }
}

#[test]
fn a_grant_event_carries_everything_an_audit_is_read_for() {
    // P0-006 criterion 6. The record has to answer who was granted what, when,
    // for how long, on whose authority, from which origin, and how it ended —
    // and it has to be a value, not a sentence, so the two trails cannot
    // disagree about it.
    use apex_agent_core::grant::{GrantKind, SystemGrant};

    let g = SystemGrant {
        id: 3,
        kind: GrantKind::BreakGlass,
        session: 9,
        agent: "claude".into(),
        project: Some("/home/t/p".into()),
        capabilities: Vec::new(),
        issued_ms: 1_757_000_000_000,
        expires_ms: 1_757_000_900_000,
        boot_id: "abc".into(),
        request_origin: RequestOrigin::LocalTerminal,
        authenticated_by: "org.apexos.agent.break-glass".into(),
        closed: None,
    };
    let json = serde_json::to_value(&g).expect("serialise");
    for key in [
        "id",
        "kind",
        "session",
        "agent",
        "project",
        "capabilities",
        "issued_ms",
        "expires_ms",
        "boot_id",
        "request_origin",
        "authenticated_by",
    ] {
        assert!(json.get(key).is_some(), "{key} missing from the record: {json}");
    }
    // The authority is named, not merely implied: "a human authenticated"
    // without saying against which action leaves an auditor unable to tell a
    // session grant from break-glass.
    assert_eq!(
        json["authenticated_by"],
        apex_agent_core::auth::action_for(GrantKind::BreakGlass)
    );
    // And the record survives the wire intact, because the shell and the CLI
    // both read it.
    let back: SystemGrant = serde_json::from_value(json).expect("parse");
    assert_eq!(back, g);
}
