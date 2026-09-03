//! Assertions for the mode catalogue and its planner (roadmap §11).
//!
//! Everything under test is pure, so none of this can reach the machine it runs
//! on. That is not a happy accident: this is the area where a test suite once
//! shelled out for real, switched the developer's CPU scheduler and then blocked
//! for 177 seconds on a polkit password. `apexd_core::mode` performs no I/O and
//! constructs no writer of any kind, which is why these cases need no fixture
//! root, no sandbox and no daemon.

use apexd_core::mode::{
    diff, identify, plan, ModeId, ModeState, PolicyIntent, ServiceWant, Step, TierPolicy, MODES,
};
use apexd_core::tier::Tier;

/// The state a machine boots into: profile defaults, auto-switch on.
fn fresh() -> ModeState {
    ModeState {
        tier: Tier::Balanced,
        auto_switch: true,
        game_active: false,
    }
}

// ── the catalogue itself ─────────────────────────────────────────────────────

#[test]
fn every_mode_the_roadmap_names_exists_and_is_reachable_by_name() {
    // §11 lists exactly these eight. A missing one is a silently unimplemented
    // product feature, which is the failure mode this asserts against.
    let want = [
        "daily",
        "gaming",
        "development",
        "creator",
        "ai",
        "battery",
        "couch",
        "server",
    ];
    let have: Vec<&str> = ModeId::ALL.iter().map(|m| m.as_str()).collect();
    assert_eq!(have, want);

    for name in want {
        let parsed: ModeId = name.parse().expect("every listed id parses");
        assert_eq!(parsed.as_str(), name);
    }
}

#[test]
fn the_catalogue_has_exactly_one_entry_per_id() {
    assert_eq!(MODES.len(), ModeId::ALL.len());
    for id in ModeId::ALL {
        assert_eq!(id.spec().id, id, "spec() must not return another mode");
    }
    let mut ids: Vec<ModeId> = MODES.iter().map(|m| m.id).collect();
    ids.sort();
    ids.dedup();
    assert_eq!(ids.len(), MODES.len(), "duplicate id in MODES");
}

#[test]
fn aliases_resolve_but_nonsense_is_refused() {
    assert_eq!("GAMING".parse::<ModeId>().unwrap(), ModeId::Gaming);
    assert_eq!("battery-saver".parse::<ModeId>().unwrap(), ModeId::Battery);
    assert_eq!("battery_saver".parse::<ModeId>().unwrap(), ModeId::Battery);
    assert_eq!(" dev ".parse::<ModeId>().unwrap(), ModeId::Development);
    assert!("AI/ML".parse::<ModeId>().is_err());
    // A refusal must name the alternatives, or the user is left guessing.
    let e = "turbo".parse::<ModeId>().unwrap_err();
    let msg = e.to_string();
    assert!(msg.contains("turbo"), "{msg}");
    assert!(msg.contains("gaming") && msg.contains("daily"), "{msg}");
}

#[test]
fn gaming_is_the_only_mode_that_turns_game_mode_on() {
    // Game mode confines work to the P-cores. That is right for a game and
    // actively wrong for a parallel build, so a second mode acquiring the flag
    // would be a real regression rather than a preference.
    let with_game: Vec<ModeId> = MODES.iter().filter(|m| m.game).map(|m| m.id).collect();
    assert_eq!(with_game, vec![ModeId::Gaming]);
}

#[test]
fn daily_is_the_only_mode_that_pins_nothing() {
    let auto: Vec<ModeId> = MODES
        .iter()
        .filter(|m| m.tier == TierPolicy::Auto)
        .map(|m| m.id)
        .collect();
    assert_eq!(auto, vec![ModeId::Daily]);
}

#[test]
fn battery_saver_is_the_frugal_tier_and_ai_is_deliberately_not_performance() {
    assert_eq!(
        ModeId::Battery.spec().tier,
        TierPolicy::Pinned(Tier::PowerSaver)
    );
    // The AI mode's whole documented argument is that local inference is GPU-
    // bound, so pinning every core to `performance` spends package power the
    // GPU wants. If someone "fixes" this to performance the rationale in the
    // catalogue becomes a lie, so it is pinned by a test.
    assert_eq!(ModeId::Ai.spec().tier, TierPolicy::Pinned(Tier::Balanced));
    assert_eq!(ModeId::Ai.spec().intent, Some(PolicyIntent::PreserveVram));
}

#[test]
fn every_mode_explains_itself() {
    // `apex mode show` prints these. An empty rationale renders as a tier with
    // no reason attached, which is indistinguishable from a guess — and §13 is
    // explicit that automatic choices must be visible.
    for m in MODES.iter() {
        assert!(!m.label.is_empty(), "{} has no label", m.id);
        assert!(!m.summary.is_empty(), "{} has no summary", m.id);
        assert!(
            m.rationale.len() > 40,
            "{} needs a real rationale, got {:?}",
            m.id,
            m.rationale
        );
        for s in m.services {
            assert!(!s.unit.is_empty() && !s.why.is_empty());
        }
    }
}

#[test]
fn the_only_service_intent_is_the_one_that_fights_irq_steering() {
    // Service sets are REPORTED, not executed. Keeping the declared set tiny is
    // what stops the report drifting into a promise the CLI never keeps.
    let all: Vec<(&str, ServiceWant)> = MODES
        .iter()
        .flat_map(|m| m.services.iter().map(|s| (s.unit, s.want)))
        .collect();
    assert_eq!(all, vec![("irqbalance.service", ServiceWant::Stopped)]);
    // And no mode declares a sysext yet; merging one on a mode switch is the
    // heavyweight lever the module docs explain is deliberately deferred.
    assert!(MODES.iter().all(|m| m.sysext.is_empty()));
}

// ── diffing and identification ───────────────────────────────────────────────

#[test]
fn a_fresh_machine_is_in_daily() {
    let m = identify(&fresh());
    assert_eq!(m.exact, vec![ModeId::Daily]);
    assert!(!m.is_custom());
    assert!(m.diffs.is_empty());
}

#[test]
fn a_machine_can_be_in_several_modes_at_once_and_says_so() {
    // development, creator and server all pin `performance` with game mode off.
    // Nothing readable off a running machine tells them apart, so reporting one
    // of them would be inventing certainty. This is the honest answer.
    let state = ModeState {
        tier: Tier::Performance,
        auto_switch: false,
        game_active: false,
    };
    let m = identify(&state);
    assert_eq!(
        m.exact,
        vec![ModeId::Development, ModeId::Creator, ModeId::Server]
    );
}

#[test]
fn an_overridden_machine_reports_the_closest_mode_and_what_differs() {
    // The user is in gaming mode and drops the tier by hand. §13: automatic
    // choices must be visible and overrideable — so an override must render as
    // a named difference, not as "unknown".
    let state = ModeState {
        tier: Tier::Balanced,
        auto_switch: false,
        game_active: true,
    };
    let m = identify(&state);
    assert!(m.is_custom(), "no mode should match exactly");
    assert_eq!(m.closest, ModeId::Gaming);
    assert_eq!(m.diffs.len(), 1);
    assert_eq!(m.diffs[0].what, "tier");
    assert_eq!(m.diffs[0].expected, "performance");
    assert_eq!(m.diffs[0].actual, "balanced");
    // And it renders as a sentence a person can act on.
    assert_eq!(
        m.diffs[0].to_string(),
        "tier is balanced (mode wants performance)"
    );
}

#[test]
fn diffs_are_empty_exactly_when_a_mode_matches() {
    // The two halves of ModeMatch must never disagree: a caller printing diffs
    // whenever they are non-empty would otherwise print differences against a
    // mode the machine is actually in.
    for state in [
        fresh(),
        ModeState {
            tier: Tier::Performance,
            auto_switch: false,
            game_active: true,
        },
        ModeState {
            tier: Tier::PowerSaver,
            auto_switch: true,
            game_active: true,
        },
    ] {
        let m = identify(&state);
        assert_eq!(m.exact.is_empty(), !m.diffs.is_empty(), "{state:?}");
    }
}

#[test]
fn an_auto_mode_diffs_on_the_switch_and_never_on_the_tier() {
    // Daily delegates the tier, so ANY tier is correct for it. Reporting
    // "tier is power-saver (mode wants ...)" would be nonsense: Daily wants
    // whatever the profile's defaults produced.
    let state = ModeState {
        tier: Tier::PowerSaver,
        auto_switch: true,
        game_active: false,
    };
    assert!(diff(ModeId::Daily.spec(), &state).is_empty());

    let off = ModeState {
        auto_switch: false,
        ..state
    };
    let d = diff(ModeId::Daily.spec(), &off);
    assert_eq!(d.len(), 1);
    assert_eq!(d[0].what, "auto-switch");
}

// ── the planner: ordering is the whole point ─────────────────────────────────

#[test]
fn entering_a_mode_the_machine_is_already_in_does_nothing() {
    // Idempotence, so `apex mode set daily` twice is not two tier writes and a
    // no-op prints as one.
    for id in ModeId::ALL {
        let spec = id.spec();
        let settled = ModeState {
            tier: match spec.tier {
                TierPolicy::Pinned(t) => t,
                TierPolicy::Auto => Tier::Balanced,
            },
            auto_switch: matches!(spec.tier, TierPolicy::Auto),
            game_active: spec.game,
        };
        assert!(
            plan(spec, &settled).is_empty(),
            "{id} re-plans work on a machine already in it: {:?}",
            plan(spec, &settled)
        );
    }
}

#[test]
fn auto_switch_goes_off_before_the_tier_is_pinned() {
    // With auto-switch on, apexd re-derives the tier from the profile's
    // AC/battery defaults — and enabling it reconciles immediately. Pinning
    // first and disabling second leaves a window in which the pin is undone.
    let steps = plan(ModeId::Battery.spec(), &fresh());
    assert_eq!(
        steps,
        vec![Step::AutoSwitch(false), Step::SetTier(Tier::PowerSaver)]
    );
}

#[test]
fn leaving_game_mode_happens_before_the_new_tier_is_set() {
    // `apex game stop` restores the tier that was active before the session.
    // Setting the new mode's tier first would have that restore overwrite it,
    // so the user asks for Battery Saver and silently lands wherever they were
    // an hour ago. This ordering is the fix, and this test is the proof.
    let in_game = ModeState {
        tier: Tier::Performance,
        auto_switch: false,
        game_active: true,
    };
    let steps = plan(ModeId::Battery.spec(), &in_game);
    assert_eq!(
        steps,
        vec![Step::GameMode(false), Step::SetTier(Tier::PowerSaver)]
    );
    let stop = steps
        .iter()
        .position(|s| *s == Step::GameMode(false))
        .expect("leaving a game session stops game mode");
    let set = steps
        .iter()
        .position(|s| matches!(s, Step::SetTier(_)))
        .expect("a pinned mode sets a tier");
    assert!(stop < set, "game stop must precede the tier write");
}

#[test]
fn the_tier_is_re_asserted_after_leaving_game_mode_even_if_it_looked_right() {
    // The pre-plan reading said `performance` and the target is `performance`,
    // so the naive "skip when equal" rule would emit no SetTier at all — but
    // the game-mode restore in between moves the tier to whatever preceded the
    // session. The write must still happen.
    let in_game = ModeState {
        tier: Tier::Performance,
        auto_switch: false,
        game_active: true,
    };
    let steps = plan(ModeId::Development.spec(), &in_game);
    assert_eq!(
        steps,
        vec![Step::GameMode(false), Step::SetTier(Tier::Performance)]
    );
}

#[test]
fn entering_gaming_turns_the_scheduler_work_on_last() {
    // Game mode migrates every runnable task; it should land once the tier it
    // will run under is settled, not before.
    let steps = plan(ModeId::Gaming.spec(), &fresh());
    assert_eq!(
        steps,
        vec![
            Step::AutoSwitch(false),
            Step::SetTier(Tier::Performance),
            Step::GameMode(true),
        ]
    );
    assert_eq!(steps.last(), Some(&Step::GameMode(true)));
}

#[test]
fn an_auto_mode_hands_control_back_without_naming_a_tier() {
    // Emitting a SetTier alongside SetAutoSwitch(true) would fight the very
    // mechanism being handed control: auto-switch reconciles immediately, so
    // the explicit tier would be overwritten within the same call sequence.
    let pinned = ModeState {
        tier: Tier::Performance,
        auto_switch: false,
        game_active: false,
    };
    let steps = plan(ModeId::Daily.spec(), &pinned);
    assert_eq!(steps, vec![Step::AutoSwitch(true)]);
    assert!(
        !steps.iter().any(|s| matches!(s, Step::SetTier(_))),
        "an Auto mode must never name a tier"
    );
}

#[test]
fn going_from_gaming_to_daily_unwinds_in_the_right_order() {
    let in_game = ModeState {
        tier: Tier::Performance,
        auto_switch: false,
        game_active: true,
    };
    let steps = plan(ModeId::Daily.spec(), &in_game);
    assert_eq!(steps, vec![Step::GameMode(false), Step::AutoSwitch(true)]);
}

#[test]
fn every_plan_lands_the_machine_in_the_mode_it_asked_for() {
    // The property that matters, checked across every start state against every
    // mode: applying a plan must produce a state that mode identifies with.
    // This is what a hand-written per-mode expectation cannot cover.
    let states = [true, false]
        .iter()
        .flat_map(|auto| {
            [true, false].iter().flat_map(move |game| {
                Tier::ALL.iter().map(move |t| ModeState {
                    tier: *t,
                    auto_switch: *auto,
                    game_active: *game,
                })
            })
        })
        .collect::<Vec<_>>();

    for start in states {
        for id in ModeId::ALL {
            let spec = id.spec();
            let mut s = start;
            for step in plan(spec, &start) {
                match step {
                    Step::AutoSwitch(v) => s.auto_switch = v,
                    Step::SetTier(t) => s.tier = t,
                    Step::GameMode(v) => s.game_active = v,
                }
            }
            // An Auto mode's tier is whatever the daemon reconciles to, which
            // this model cannot simulate — its contract is the switch only.
            if spec.tier == TierPolicy::Auto {
                assert!(s.auto_switch, "{id} from {start:?} left auto-switch off");
            }
            assert!(
                diff(spec, &s).is_empty(),
                "{id} from {start:?} ended at {s:?}, differing: {:?}",
                diff(spec, &s)
            );
        }
    }
}

#[test]
fn steps_describe_themselves_for_the_dry_run() {
    // `apex mode set --dry-run` prints exactly these, so an empty or
    // indistinguishable rendering makes the dry run useless.
    let all = [
        Step::AutoSwitch(true),
        Step::AutoSwitch(false),
        Step::SetTier(Tier::Performance),
        Step::GameMode(true),
        Step::GameMode(false),
    ];
    let mut seen = std::collections::HashSet::new();
    for s in all {
        let d = s.describe();
        assert!(d.len() > 8, "{s:?} describes itself as {d:?}");
        assert!(seen.insert(d.clone()), "two steps render identically: {d}");
    }
    assert!(Step::SetTier(Tier::PowerSaver)
        .describe()
        .contains("power-saver"));
}
