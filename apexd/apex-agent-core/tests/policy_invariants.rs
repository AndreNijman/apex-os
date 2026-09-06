//! P0-004's acceptance criteria, as tests.
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
use apex_agent_core::policy::{
    AgentPolicy, NativeMode, NetworkPolicy, OriginPolicy, PolicyPreset, RequestOrigin,
    SandboxPolicy, SecretPolicy, SystemAccess,
};
use apex_agent_core::sandbox::{build_argv, SandboxSpec};

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
