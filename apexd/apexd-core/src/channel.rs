//! Update channels, staged rollout, and the health signal that stops one.
//! Roadmap §26.
//!
//! Pure: no files, no processes, no network. The CLI measures the machine and
//! hands the measurements in, the same division `blueprint` and `task` use.
//!
//! ## What a channel is here
//!
//! A tag on one repository. `ghcr.io/andrenijman/apex-os:stable` and
//! `…:edge` name different digests of the same image, and which one a machine
//! follows is the `container-image-reference` in its deployment origin —
//! written at install time, changed by `bootc switch`. So a channel is not a
//! new build tier, a new edition, or a second update mechanism: it is which
//! tag `bootc upgrade` resolves.
//!
//! ## Every APEX machine is already on edge, and has never been told so
//!
//! `apex`, `daily`, `gaming-mesa` and `gaming-nvidia` are four names for one
//! digest, and `build-image.yml` moves all four on every successful build of
//! `main`. That is the definition of an edge channel. Andre's L16 follows
//! `:daily`. [`from_tag`] therefore maps those four to [`Channel::Edge`] and
//! keeps the alias, so `apex channel status` can say "tracking `daily`, which
//! moves with every build — that is the edge channel under an older name"
//! instead of "unknown". Reporting a machine's real state is the entire value
//! of this surface, and those four tags must go on meaning what they mean:
//! they are what installed machines track, and a tag that stops moving does
//! not error, it silently stops delivering security updates.
//!
//! ## Which direction is dangerous
//!
//! Moving toward `edge` takes a newer image, which is an ordinary update.
//! Moving toward `stable` usually takes an OLDER one, and that is the direction
//! that needs care for the reason §25 exists: `/usr` goes back and the state
//! the newer build migrated does not. [`Direction::Behind`] is the flag for
//! that, and the CLI pins the current deployment and prints §25's rollback
//! table before it switches.
//!
//! ## Staged rollout, without inventing a server
//!
//! A machine derives a stable bucket from its own `/etc/machine-id`
//! ([`bucket`]) and takes a release only when its bucket falls inside the
//! release's percentage ([`admits`]). The bucket never leaves the machine — it
//! is one of a hundred values, and it is not in the report `apex channel
//! report` would send.
//!
//! What is missing, and named rather than faked: the percentage has to live
//! somewhere that can change between builds — 1, then 5, then 25 — and an OCI
//! label is baked once per image. So the gate is implemented and tested, the
//! percentage is read from the image's own label when it carries one, and the
//! default is 100. Every image published so far is at 100, which is the truth.

use crate::recover::Health;

/// The four channels §26 names, ordered from most cautious to least.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Channel {
    Stable,
    Candidate,
    Beta,
    Edge,
}

impl Channel {
    pub const ALL: [Channel; 4] = [
        Channel::Stable,
        Channel::Candidate,
        Channel::Beta,
        Channel::Edge,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Channel::Stable => "stable",
            Channel::Candidate => "candidate",
            Channel::Beta => "beta",
            Channel::Edge => "edge",
        }
    }

    /// How far from `stable`. 0 is stable, 3 is edge.
    pub fn rank(self) -> u8 {
        match self {
            Channel::Stable => 0,
            Channel::Candidate => 1,
            Channel::Beta => 2,
            Channel::Edge => 3,
        }
    }

    /// What a person is choosing when they choose this channel.
    ///
    /// Written for somebody deciding, not for somebody who already knows the
    /// release process: what arrives, how often, and what it costs.
    pub fn summary(self) -> &'static str {
        match self {
            Channel::Stable => {
                "only builds that have run on the other channels first. The fewest updates, and the longest wait for a fix"
            }
            Channel::Candidate => {
                "a build being considered for stable. It has already been on beta"
            }
            Channel::Beta => "a build that has been on edge and looks sound",
            Channel::Edge => {
                "every successful build of main, as soon as it is published. New work arrives first here, and so do its faults"
            }
        }
    }
}

impl std::str::FromStr for Channel {
    type Err = String;

    fn from_str(s: &str) -> Result<Channel, String> {
        Channel::ALL
            .into_iter()
            .find(|c| c.as_str() == s)
            .ok_or_else(|| {
                format!(
                    "{s:?} is not a channel. The channels are: {}",
                    Channel::ALL
                        .iter()
                        .map(|c| c.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                )
            })
    }
}

/// The four tags that predate channels.
///
/// They are not editions any more — they are four names for one digest, kept
/// alive because they are what installed machines track. All four move on every
/// successful build of `main`, so a machine following one of them is on edge.
pub const LEGACY_TAGS: [&str; 4] = ["apex", "daily", "gaming-mesa", "gaming-nvidia"];

/// What a machine is following.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tracking {
    /// The channel, when the tag names one.
    pub channel: Option<Channel>,
    /// The tag as written in the deployment origin.
    pub tag: String,
    /// Set when the tag is one of [`LEGACY_TAGS`]: the same content as `edge`,
    /// under the name it had before channels existed.
    pub alias: Option<&'static str>,
}

/// Read a channel out of an image reference or a bare tag.
///
/// Accepts either `ghcr.io/owner/repo:tag` or `tag`. A tag nobody recognises is
/// not an error — a machine can legitimately follow a digest, a revision tag or
/// somebody's own build, and calling that "unknown channel" is more useful than
/// refusing to answer.
pub fn from_tag(reference: &str) -> Tracking {
    // Split off the tag: the last colon, but only when it comes after the last
    // slash. A colon before that is a port on the registry host.
    let tag = match (reference.rfind(':'), reference.rfind('/')) {
        (Some(c), Some(s)) if c > s => &reference[c + 1..],
        (Some(c), None) => &reference[c + 1..],
        _ => reference,
    };
    let channel = tag.parse::<Channel>().ok();
    let alias = LEGACY_TAGS.into_iter().find(|t| *t == tag);
    Tracking {
        // A legacy tag IS edge: same digest, moved by the same job, on every
        // build. Reporting it as no channel at all would leave every machine
        // that exists today unable to see where it stands.
        channel: channel.or(alias.map(|_| Channel::Edge)),
        tag: tag.to_string(),
        alias,
    }
}

/// Which way a channel change moves, in terms of what it does to the image.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    /// Toward `edge`. Takes a newer image — an ordinary update.
    Ahead,
    /// Toward `stable`. Usually takes an OLDER image, which is the direction
    /// §25 cares about: `/usr` goes back and the state a newer build migrated
    /// stays where it is.
    Behind,
    Same,
}

pub fn direction(from: Channel, to: Channel) -> Direction {
    match to.rank().cmp(&from.rank()) {
        std::cmp::Ordering::Greater => Direction::Ahead,
        std::cmp::Ordering::Less => Direction::Behind,
        std::cmp::Ordering::Equal => Direction::Same,
    }
}

// ── staged rollout ───────────────────────────────────────────────────────────

/// This machine's rollout bucket, 0 to 99.
///
/// FNV-1a over the machine id, because it needs no dependency and the
/// distribution only has to be even across a hundred buckets rather than
/// cryptographically uniform. Stable across reboots and across updates, which
/// is the property that matters: a machine must not creep into a 1% cohort by
/// re-rolling on every check.
///
/// It never leaves the machine. `apex channel report` does not carry it: one of
/// a hundred values is a hundredth of a machine id, and a supply-chain report
/// has no use for it.
pub fn bucket(machine_id: &str) -> u8 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for b in machine_id.trim().as_bytes() {
        hash ^= u64::from(*b);
        hash = hash.wrapping_mul(0x1000_0000_01b3);
    }
    (hash % 100) as u8
}

/// Whether a machine in `bucket` takes a release published at `percent`.
///
/// `percent` above 100 is clamped rather than refused: it arrives from an image
/// label, and a typo there must not stop a machine updating.
pub fn admits(bucket: u8, percent: u8) -> bool {
    u32::from(bucket) < u32::from(percent.min(100))
}

/// What every image published so far is at, and what an image with no rollout
/// label means.
pub const FULL_ROLLOUT: u8 = 100;

/// The OCI label a build stamps to hold a release back.
///
/// Named here because the reader and the writer are in different repositories'
/// worth of tooling, and a label whose spelling drifts is a gate that silently
/// stops gating.
pub const ROLLOUT_LABEL: &str = "org.apexos.rollout.percent";

// ── the health signal that stops a rollout ───────────────────────────────────

/// Whether the machine came back healthy from an update.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Verdict {
    pub healthy: bool,
    /// One line per regression, naming the component and what is wrong. Empty
    /// when healthy.
    pub reasons: Vec<String>,
}

/// The `apex recover status` rows whose failure is an update regression.
///
/// Not every row: a machine with no network route has a problem an update did
/// not cause, and refusing to update because the wifi is off would strand the
/// user on the release that broke them. These four are the ones an image
/// change can break and a rollback can fix.
///
/// §26 lists boot failures, shell crash loops, GPU initialisation failures,
/// portal failures and network regressions. Boot failure is not observable from
/// a machine that booted, so it is covered from the other side — the previous
/// deployment row is what says whether there is anything to go back TO.
pub const REGRESSION_ROWS: [&str; 4] = [
    // The driver that did not bind after the update. §26's "GPU initialisation
    // failures".
    "gpu-driver",
    // The user interface, vendored in the image, so an image change is the only
    // thing that moves it. §26's "APEX Shell crash loops".
    "apex-shell",
    // A read-only /usr that is not read-only, or a root that is not the
    // deployment. An image that half-installed.
    "filesystem",
    // Extensions built for the previous OS release, which systemd refuses to
    // merge — the user's own software gone after an update.
    "package-extensions",
];

/// Turn one measurement of the machine into a verdict.
///
/// `rows` are `apex recover status`'s rows: id, health, and the detail line.
/// `failed_units` is `systemctl --failed`.
///
/// [`Health::Unavailable`] is NOT a regression, and that asymmetry is the point.
/// A row that could not be measured is a fact about the reader, and treating it
/// as a failure would refuse a user's update because a file was unreadable —
/// which is the same mistake, in the opposite direction, as reporting an
/// unreadable file as fine. It is reported in `reasons` so the user knows the
/// verdict is partial, without counting against it.
pub fn verdict(rows: &[(String, Health, String)], failed_units: &[String]) -> Verdict {
    let mut reasons = Vec::new();
    let mut healthy = true;

    for (id, health, detail) in rows {
        if !REGRESSION_ROWS.contains(&id.as_str()) {
            continue;
        }
        match health {
            Health::Attention => {
                healthy = false;
                reasons.push(format!("{id}: {}", first_line(detail)));
            }
            Health::Unavailable => {
                reasons.push(format!(
                    "{id}: could not be checked ({}), so this verdict is partial",
                    first_line(detail)
                ));
            }
            Health::Verified | Health::Available => {}
        }
    }

    // A unit that failed to start is the most direct evidence an image change
    // broke something, and it is the one signal that works on every boot path —
    // APEX's boot-health unit is conditioned on systemd-boot's
    // LoaderBootCountPath and every published image boots GRUB, so it never
    // runs and its verdict file does not exist.
    for unit in failed_units {
        healthy = false;
        reasons.push(format!("systemd unit failed: {unit}"));
    }

    Verdict { healthy, reasons }
}

fn first_line(s: &str) -> String {
    s.lines().next().unwrap_or("").trim().to_string()
}

/// The record `apex update` leaves so the NEXT update can tell whether this one
/// went well.
///
/// Kept deliberately small. It answers one question — "is the image I am
/// running the one the last update staged" — and it answers it by digest,
/// because a version string moves for reasons that are not an update.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct LastUpdate {
    /// Schema version, on the §25 rule: absent means 1.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub schema: Option<u32>,
    /// The digest that was booted when the update ran. What the machine is
    /// running now, if the update has not been rebooted into yet.
    pub from_digest: String,
    /// The tag the machine follows.
    pub tag: String,
    /// Unix seconds.
    pub at: u64,
}

/// Schema version of [`LastUpdate`].
pub const LAST_UPDATE_SCHEMA: u32 = 1;

/// Whether the machine has rebooted into whatever the last `apex update`
/// staged.
///
/// True when the booted digest differs from the one recorded before the update
/// ran. Comparing against the digest we came FROM rather than the one we went
/// TO is what makes this work without a second registry round trip: `bootc
/// upgrade` does not report the digest it staged in a form worth parsing, and
/// the machine can see its own.
pub fn rebooted_into_new(record: &LastUpdate, booted_digest: &str) -> bool {
    !booted_digest.is_empty() && booted_digest != record.from_digest
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_four_channels_are_ordered_from_cautious_to_not() {
        assert!(Channel::Stable < Channel::Candidate);
        assert!(Channel::Candidate < Channel::Beta);
        assert!(Channel::Beta < Channel::Edge);
        let ranks: Vec<u8> = Channel::ALL.iter().map(|c| c.rank()).collect();
        assert_eq!(ranks, vec![0, 1, 2, 3]);
    }

    #[test]
    fn every_channel_round_trips_and_an_invented_one_is_refused_by_name() {
        for c in Channel::ALL {
            assert_eq!(c.as_str().parse::<Channel>().unwrap(), c);
            assert!(!c.summary().is_empty());
        }
        // The refusal has to name the four that exist. "unknown channel" on its
        // own leaves somebody guessing at the one word they must not guess at.
        let e = "nightly".parse::<Channel>().unwrap_err();
        for c in Channel::ALL {
            assert!(e.contains(c.as_str()), "{e}");
        }
    }

    #[test]
    fn the_legacy_tags_are_edge_under_an_older_name() {
        // Every machine in the field follows one of these, and they move on
        // every build of main. Reporting them as "no channel" would leave every
        // existing machine unable to see where it stands.
        for tag in LEGACY_TAGS {
            let t = from_tag(&format!("ghcr.io/andrenijman/apex-os:{tag}"));
            assert_eq!(t.channel, Some(Channel::Edge), "{tag}");
            assert_eq!(t.alias, Some(tag));
            assert_eq!(t.tag, tag);
        }
        // The exact string on Andre's L16.
        let t = from_tag("ghcr.io/andrenijman/apex-os:daily");
        assert_eq!(t.channel, Some(Channel::Edge));
        assert_eq!(t.alias, Some("daily"));
    }

    #[test]
    fn a_channel_tag_is_not_reported_as_an_alias() {
        for c in Channel::ALL {
            let t = from_tag(&format!("ghcr.io/andrenijman/apex-os:{}", c.as_str()));
            assert_eq!(t.channel, Some(c));
            assert_eq!(t.alias, None, "{} is not a legacy alias", c.as_str());
        }
    }

    #[test]
    fn a_tag_nobody_recognises_is_answered_rather_than_refused() {
        // A revision tag, a fork, somebody's local build. All legitimate.
        let t = from_tag("ghcr.io/andrenijman/apex-os:apex-57f593a");
        assert_eq!(t.channel, None);
        assert_eq!(t.tag, "apex-57f593a");
        assert_eq!(t.alias, None);
    }

    #[test]
    fn a_registry_port_is_not_mistaken_for_a_tag() {
        let t = from_tag("registry.example:5000/apex/apex-os");
        assert_eq!(t.tag, "registry.example:5000/apex/apex-os");
        assert_eq!(t.channel, None);
        let t = from_tag("registry.example:5000/apex/apex-os:stable");
        assert_eq!(t.tag, "stable");
        assert_eq!(t.channel, Some(Channel::Stable));
    }

    #[test]
    fn moving_toward_stable_is_flagged_as_the_backwards_direction() {
        assert_eq!(direction(Channel::Edge, Channel::Stable), Direction::Behind);
        assert_eq!(direction(Channel::Beta, Channel::Candidate), Direction::Behind);
        assert_eq!(direction(Channel::Stable, Channel::Edge), Direction::Ahead);
        assert_eq!(direction(Channel::Beta, Channel::Beta), Direction::Same);
    }

    #[test]
    fn a_bucket_is_stable_for_a_machine_and_spread_across_a_hundred() {
        // Stable, because a machine that re-rolled on every check would creep
        // into a 1% cohort it was never in.
        let id = "c03b1fb9a1e24d0b8f3c5e7a9d2b4c60";
        assert_eq!(bucket(id), bucket(id));
        assert!(bucket(id) < 100);
        // Even enough: 1000 distinct ids must land in most of the hundred
        // buckets. A hash that collapsed would silently make every machine take
        // or refuse every staged release together.
        let mut seen = [false; 100];
        for i in 0..1000 {
            seen[bucket(&format!("machine-{i:04}")) as usize] = true;
        }
        let hit = seen.iter().filter(|b| **b).count();
        assert!(hit > 90, "only {hit} of 100 buckets were reached");
    }

    #[test]
    fn a_release_at_full_rollout_admits_every_machine() {
        for b in 0..100u8 {
            assert!(admits(b, FULL_ROLLOUT), "bucket {b} was held back at 100%");
        }
        // And nothing is admitted at 0.
        for b in 0..100u8 {
            assert!(!admits(b, 0), "bucket {b} took a release at 0%");
        }
    }

    #[test]
    fn a_one_percent_release_reaches_exactly_one_bucket() {
        let taken = (0..100u8).filter(|b| admits(*b, 1)).count();
        assert_eq!(taken, 1);
        assert_eq!((0..100u8).filter(|b| admits(*b, 25)).count(), 25);
        // A label with a typo must not stop a machine updating.
        assert!(admits(99, 255));
    }

    #[test]
    fn a_healthy_machine_has_no_reasons() {
        let rows = vec![
            ("gpu-driver".to_string(), Health::Verified, "1 — AMD via amdgpu".to_string()),
            ("apex-shell".to_string(), Health::Verified, "vendored".to_string()),
            ("filesystem".to_string(), Health::Verified, "read-only".to_string()),
            ("package-extensions".to_string(), Health::Verified, "none".to_string()),
        ];
        let v = verdict(&rows, &[]);
        assert!(v.healthy);
        assert!(v.reasons.is_empty());
    }

    #[test]
    fn a_row_that_could_not_be_measured_does_not_fail_the_verdict() {
        // The asymmetry that matters. Refusing somebody's update because a file
        // was unreadable is the same mistake as reporting an unreadable file as
        // fine, from the other side — and this one strands them on the release
        // that broke them.
        let rows = vec![(
            "package-extensions".to_string(),
            Health::Unavailable,
            "/var/lib/apex/pkg: Permission denied".to_string(),
        )];
        let v = verdict(&rows, &[]);
        assert!(v.healthy, "an unmeasurable row failed the verdict");
        // But it is still SAID, so the user knows the verdict is partial.
        assert_eq!(v.reasons.len(), 1);
        assert!(v.reasons[0].contains("could not be checked"), "{:?}", v.reasons);
        assert!(v.reasons[0].contains("partial"), "{:?}", v.reasons);
    }

    #[test]
    fn a_broken_gpu_driver_stops_the_rollout_and_says_which_row() {
        let rows = vec![(
            "gpu-driver".to_string(),
            Health::Attention,
            "no driver is bound to the discrete GPU".to_string(),
        )];
        let v = verdict(&rows, &[]);
        assert!(!v.healthy);
        assert_eq!(v.reasons.len(), 1);
        assert!(v.reasons[0].starts_with("gpu-driver: "), "{:?}", v.reasons);
    }

    #[test]
    fn a_row_an_image_change_cannot_break_is_not_a_regression() {
        // The wifi being off is not a reason to refuse somebody the update that
        // fixes their machine.
        let rows = vec![
            ("network".to_string(), Health::Attention, "no default route".to_string()),
            ("secure-boot".to_string(), Health::Attention, "disabled".to_string()),
        ];
        let v = verdict(&rows, &[]);
        assert!(v.healthy, "{:?}", v.reasons);
        assert!(v.reasons.is_empty());
    }

    #[test]
    fn a_failed_unit_stops_the_rollout_on_every_boot_path() {
        // The one signal that works under GRUB, which is every published image:
        // apex-boot-health is conditioned on systemd-boot's LoaderBootCountPath
        // and never runs, so its verdict file does not exist to read.
        let v = verdict(&[], &["apex-shell.service".to_string()]);
        assert!(!v.healthy);
        assert!(v.reasons[0].contains("apex-shell.service"), "{:?}", v.reasons);
    }

    #[test]
    fn the_detail_of_a_multi_line_row_does_not_wrap_into_the_reason() {
        let rows = vec![(
            "apex-shell".to_string(),
            Health::Attention,
            "not provisioned for this account\nrun apex shell firstrun".to_string(),
        )];
        let v = verdict(&rows, &[]);
        assert_eq!(v.reasons, vec!["apex-shell: not provisioned for this account"]);
    }

    #[test]
    fn the_machine_knows_it_has_rebooted_into_the_update_it_staged() {
        let r = LastUpdate {
            schema: Some(LAST_UPDATE_SCHEMA),
            from_digest: "sha256:5e206de5".into(),
            tag: "daily".into(),
            at: 1788700000,
        };
        assert!(rebooted_into_new(&r, "sha256:308127d9"));
        assert!(!rebooted_into_new(&r, "sha256:5e206de5"));
        // A digest nobody could read is not evidence of a reboot, and acting on
        // it would run the health gate against an update that never happened.
        assert!(!rebooted_into_new(&r, ""));
    }

    #[test]
    fn the_rollout_label_is_spelled_once() {
        // The writer is a Containerfile and the reader is this crate. A label
        // whose spelling drifts is a gate that silently stops gating, and
        // nothing else in the system would notice.
        assert_eq!(ROLLOUT_LABEL, "org.apexos.rollout.percent");
        assert_eq!(FULL_ROLLOUT, 100);
    }
}
