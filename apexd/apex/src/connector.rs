//! P1-028's trust planes and P1-027's memory providers.
//!
//! Both items are questions about the same set — every MCP server
//! [`crate::mcp::servers::discover`] finds — so they share a module rather
//! than each re-deriving the set from the four files it lives in.
//!
//! ## P1-028: the planes are already in the data, and were not in the report
//!
//! [`Transport::Stdio`] is a program this machine starts;
//! [`Transport::Endpoint`] is an address something posts to. That distinction
//! already changes behaviour in four places — `apex mcp list` prints a
//! `sandbox` line only for a program, `apex mcp policy` lists only programs,
//! `apex mcp confine` refuses an endpoint, and only a program's credential can
//! be `Brokered`. What was missing is that the report never *named* the split,
//! so a person reading it had to know that "stdio" means on-this-machine and
//! that an `http` line is a connector reaching off it.
//!
//! The two planes get different mitigations, and neither one is the other's:
//!
//! | plane | what confines it | what does not |
//! |---|---|---|
//! | local | bubblewrap, via `apex mcp run` | nothing about its credential |
//! | cloud | the credential, held by `apex-secretd` | no sandbox — there is no local process to confine |
//!
//! ## What a "hardened profile" can and cannot do here — measured
//!
//! P1-028's second criterion is that hardened profiles can reduce cloud-side
//! tools and connectors. There are now two mechanisms, they are not the same
//! mechanism, and the report keeps them apart because a reader who merged them
//! would draw the wrong conclusion from either.
//!
//! **Removing the network removes the whole cloud plane.**
//! `AgentPolicy::effective_network` reads, in full:
//!
//! ```text
//! SandboxPolicy::Strict => NetworkPolicy::Offline,
//! _ => self.network,
//! ```
//!
//! so `--sandbox strict` removes the session's network unconditionally, and
//! `policy_invariants.rs` asserts it as a floor. A session with no network
//! cannot reach any cloud connector. That is all-or-nothing and it is reported
//! as what it is: a consequence of removing the network, not a selection.
//! None of the five named presets does even this — [`PolicyPreset`] has
//! `default`, `agent-bypass`, `unrestricted`, `system-access` and
//! `unsafe-everything`, and every one of them *widens* the default.
//!
//! **Selecting connectors by name is a dimension of its own**, and since
//! `mcpconf` landed it exists: `ConnectorPolicy` is
//! `as_configured | local_only | curated | none`, and `curated` keeps only the
//! names in the runtime's `connector_allow`. `local_only` removes the cloud
//! plane *without* removing the network, which the sandbox route cannot do.
//!
//! ### The qualifier, which is not a footnote
//!
//! That selection happens in exactly one place: `apex-agentd` writing a
//! configuration file and starting the agent with `--strict-mcp-config`. So it
//! applies to a session started through `apex agent`, on an adapter that can
//! be told to ignore every other MCP configuration —
//! [`Adapter::strict_mcp`](apex_agent_core::adapter::Adapter::strict_mcp) says
//! which, and the report names them rather than hard-coding one. A person who
//! types the agent's own name in a terminal gets every connector on this
//! machine, unreduced and unwrapped, because nothing APEX writes is on that
//! path. A readout that said "curated" without saying where would be the label
//! problem one level up.
//!
//! ### Which is why the tables are run, not written
//!
//! [`Selection`] rows are produced by calling the real
//! [`mcpconf::curate`] over the real definitions on this machine, once per
//! `ConnectorPolicy` value. The numbers are therefore a measurement of what a
//! session would be handed, and an arm of the curator that changed would move
//! them. A table written here would be a second copy of the policy, and the
//! thing this module exists to stop is a second copy of a fact.
//!
//! ## P1-027: a provider interface that is not a store
//!
//! The constraint comes first because it decides the design: the memory this
//! machine uses is an Obsidian vault on a NAS, reached over an MCP endpoint,
//! and it is authoritative. A provider interface that became the source of
//! truth, or that needed a migration before it was useful, would fail the item
//! however good the code was. So:
//!
//!   * **Nothing here stores a memory.** There is no APEX-owned memory, no
//!     cache, no index, and no write path to a provider of any kind. The only
//!     files this module reads are the ones that already define MCP servers,
//!     plus an optional declaration described below.
//!   * **Nothing here migrates anything.** The provider list is a view over
//!     definitions that already exist. Removing the declaration file leaves
//!     the machine exactly as it was.
//!   * **A probe is opt-in and read-only.** `initialize` is the one call every
//!     MCP server must answer and answering it changes nothing, which is why
//!     `mcp/connect.rs` already uses it to prove a credential. It still is not
//!     run unless asked: a status command that reached out to somebody's
//!     memory server every time it was typed is a status command that people
//!     stop typing.
//!
//! ### Which server is a memory provider is declared, not guessed
//!
//! APEX must not decide that a server called `memory-pressure-reporter` holds
//! the user's notes. So identification is a declaration —
//! `~/.config/apex/memory.toml` — and when there is none, a name-shaped guess
//! that is **labelled as a guess** in every output. Where two candidates exist
//! and nothing declares which is authoritative, this reports the ambiguity
//! rather than picking; `apex-plugin` refuses to guess which of two plugin
//! directories is real, for the same reason.

use std::path::{Path, PathBuf};

use anyhow::Result;
use serde_json::{json, Value};

use apex_agent_core::adapter;
use apex_agent_core::mcpconf::{self, Approval, Curated, Definition, Wire, Wrap};
use apex_agent_core::policy::{AgentPolicy, ConnectorPolicy, NetworkPolicy, PolicyPreset};
use apex_agent_core::protocol::SandboxPolicy;

use crate::mcp::servers::{self, Server, Surface, Transport};

/// Where the memory providers are declared, under the user's config.
const DECLARATION: &str = "apex/memory.toml";

// ═════════════════════════════════════════════════════════════════════════════
//  P1-028 — the two trust planes
// ═════════════════════════════════════════════════════════════════════════════

/// Which side of the machine boundary a connector is on.
///
/// Three values. `Unknown` is not a tidy-up: a definition with neither a
/// `command` nor a `url` is a connector this cannot place, and calling it local
/// would say something specific and false about what confines it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Plane {
    /// A program on this machine. Bubblewrap can confine it.
    Local,
    /// An address off this machine. Nothing local confines it; its credential
    /// is the boundary, and a session with no network cannot reach it.
    Cloud,
    /// Neither, and the definition is quoted rather than assumed.
    Unknown(String),
}

impl Plane {
    pub fn of(t: &Transport) -> Plane {
        match t {
            Transport::Stdio { .. } => Plane::Local,
            Transport::Endpoint { .. } => Plane::Cloud,
            Transport::Other(what) => Plane::Unknown(what.clone()),
        }
    }

    /// The same three answers, from the launcher's own view of a definition.
    ///
    /// [`Transport`] is what a report reads and [`Wire`] is what the session
    /// launcher decides against; both are derived from the same JSON object.
    /// This exists so the plane of a connector in the *selection* table is the
    /// plane the curator saw, rather than a second reading of the same file
    /// that could disagree with it.
    pub fn of_wire(w: &Wire) -> Plane {
        match w {
            Wire::Program { .. } => Plane::Local,
            Wire::Endpoint { .. } => Plane::Cloud,
            Wire::Unplaceable(what) => Plane::Unknown(what.clone()),
        }
    }

    pub fn tag(&self) -> &'static str {
        match self {
            Plane::Local => "local",
            Plane::Cloud => "cloud",
            Plane::Unknown(_) => "unknown",
        }
    }

    /// The sentence `apex mcp list` prints, which is the whole of P1-028's
    /// first criterion: a reader should not have to know that "stdio" means
    /// on-this-machine.
    pub fn describe(&self) -> String {
        match self {
            Plane::Local => {
                "local — a program on this machine; `apex mcp confine` can sandbox it".to_string()
            }
            Plane::Cloud => {
                "cloud — an endpoint off this machine; no local sandbox applies, its credential \
                 is the boundary"
                    .to_string()
            }
            Plane::Unknown(what) => format!(
                "unknown — defined as {what}, so which side of this machine it is on cannot be said"
            ),
        }
    }

    pub fn is_cloud(&self) -> bool {
        matches!(self, Plane::Cloud)
    }
}

/// What one named policy does to each plane.
#[derive(Debug, Clone)]
pub struct Reduction {
    /// The name a person would type.
    pub name: String,
    /// How it is spelled on the command line.
    pub flag: String,
    /// The network the session actually gets, after `effective_network`.
    pub network: NetworkPolicy,
    /// Whether a cloud connector could be reached at all.
    pub cloud_reachable: bool,
}

impl Reduction {
    fn to_json(&self, cloud: usize, local: usize) -> Value {
        json!({
            "name": self.name,
            "flag": self.flag,
            "effectiveNetwork": self.network.to_string(),
            "cloudReachable": self.cloud_reachable,
            "cloudConnectorsRemoved": if self.cloud_reachable { 0 } else { cloud },
            "localConnectorsKept": local,
        })
    }
}

/// Every named policy this machine has, and what each does to the cloud plane.
///
/// Built from the real types rather than a table written here: the presets come
/// from [`PolicyPreset::ALL`] and the sandbox values from [`SandboxPolicy::ALL`],
/// so a preset added or retightened upstream shows up without this file being
/// edited. A hand-written list would be a second place for the policy to live
/// and would drift, which is the failure `apex-plugin` refuses by asking
/// `manifest.js`.
pub fn reductions() -> Vec<Reduction> {
    let mut out = Vec::new();
    for preset in PolicyPreset::ALL {
        let policy = preset.policy();
        let network = policy.effective_network();
        out.push(Reduction {
            name: format!("preset {}", preset.as_str()),
            flag: match preset {
                PolicyPreset::Default => "(the default)".to_string(),
                p => format!("--{}", p.as_str()),
            },
            cloud_reachable: network != NetworkPolicy::Offline,
            network,
        });
    }
    for sandbox in SandboxPolicy::ALL {
        let policy = AgentPolicy {
            sandbox: *sandbox,
            ..AgentPolicy::default()
        };
        let network = policy.effective_network();
        out.push(Reduction {
            name: format!("sandbox {}", sandbox.as_str()),
            flag: format!("--sandbox {}", sandbox.as_str()),
            cloud_reachable: network != NetworkPolicy::Offline,
            network,
        });
    }
    out
}

// ═════════════════════════════════════════════════════════════════════════════
//  what a session is actually handed — measured by running the curator
// ═════════════════════════════════════════════════════════════════════════════

/// What one `--connectors` value does to the connectors this machine defines.
///
/// Every field is counted from a real [`mcpconf::curate`] run over the real
/// definitions, not from a table written here. That is the difference between
/// a readout and a claim: if the curator's approval rule, its wrapping rule or
/// its plane test changes, these numbers move, and
/// `a_connector_policy_row_is_the_curators_own_answer` goes red when they stop
/// matching.
#[derive(Debug, Clone)]
pub struct Selection {
    pub policy: ConnectorPolicy,
    /// How it is spelled on the command line.
    pub flag: String,
    pub kept_local: usize,
    pub kept_cloud: usize,
    /// Kept, and on neither plane — a transport this build cannot place.
    pub kept_unplaceable: usize,
    pub dropped: usize,
    /// Kept programs bubblewrap confines in such a session.
    pub sandboxed: usize,
    /// Of those, the ones still confined when the agent is started by hand.
    pub sandboxed_everywhere: usize,
    /// Why this value would be refused rather than run, when it would be.
    ///
    /// `curated` with nothing in `connector_allow` is refused by
    /// `AgentPolicy::validate_for`, and a row that printed "0 kept" for it
    /// would describe a session that never starts as one that starts with
    /// everything removed.
    pub refused: Option<String>,
}

impl Selection {
    pub fn kept(&self) -> usize {
        self.kept_local + self.kept_cloud + self.kept_unplaceable
    }

    fn to_json(&self) -> Value {
        json!({
            "policy": self.policy.as_str(),
            "flag": self.flag,
            "kept": self.kept(),
            "keptLocal": self.kept_local,
            "keptCloud": self.kept_cloud,
            "keptUnplaceable": self.kept_unplaceable,
            "removed": self.dropped,
            "sandboxed": self.sandboxed,
            "sandboxedWithoutTheLaunchFile": self.sandboxed_everywhere,
            "refused": self.refused,
        })
    }
}

/// What a session started through `apex agent` would be handed, measured.
///
/// Built once and passed to both the text and the JSON readout, so the two
/// cannot disagree — and built from the same `mcpconf` functions the daemon
/// calls in `install_mcp_config`, so neither can disagree with a real launch.
#[derive(Debug, Clone)]
pub struct Launch {
    /// The default policy's answer, per connector. What the local plane's
    /// confinement column is read out of.
    pub as_configured: Curated,
    /// One row per [`ConnectorPolicy`] value.
    pub selections: Vec<Selection>,
    /// The names in the runtime's `connector_allow`.
    pub allow: Vec<String>,
    /// Adapters that can be told to ignore every other MCP configuration, and
    /// can therefore be handed a curated one at all.
    pub strict_adapters: Vec<&'static str>,
    /// `false` when the runtime could not find its own `apex` binary, which is
    /// a could-not-run rather than a choice and changes what the numbers mean.
    pub wrapper_available: bool,
}

impl Launch {
    /// Run the real curator once per policy value.
    ///
    /// Pure in the same sense [`mcpconf::curate`] is: every filesystem
    /// question is the caller's, so a test can hand it fixtures and assert the
    /// table exhaustively.
    pub fn measure(
        defs: &[Definition],
        approval: &Approval,
        allow: &[String],
        apex: Option<&Path>,
    ) -> Launch {
        let mut selections = Vec::new();
        for policy in ConnectorPolicy::ALL {
            let c = mcpconf::curate(defs, approval, *policy, allow, apex);
            let mut kept_local = 0;
            let mut kept_cloud = 0;
            let mut kept_unplaceable = 0;
            for d in c.decisions.iter().filter(|d| d.kept) {
                // The plane the CURATOR saw, off the definition it decided
                // against. Reading it back off a second walk of the same files
                // is how a report comes to name a plane the launcher did not.
                let wire = defs
                    .iter()
                    .find(|def| def.name == d.name)
                    .map(Definition::wire);
                match wire.as_ref().map(Plane::of_wire) {
                    Some(Plane::Local) => kept_local += 1,
                    Some(Plane::Cloud) => kept_cloud += 1,
                    _ => kept_unplaceable += 1,
                }
            }
            selections.push(Selection {
                policy: *policy,
                flag: match policy {
                    ConnectorPolicy::AsConfigured => "(the default)".to_string(),
                    p => format!("--connectors {}", p.as_str()),
                },
                kept_local,
                kept_cloud,
                kept_unplaceable,
                dropped: c.dropped(),
                sandboxed: c.confined(),
                sandboxed_everywhere: c.confined_everywhere(),
                refused: (*policy == ConnectorPolicy::Curated && allow.is_empty()).then(|| {
                    "refused, not run: connector_allow is empty, and \"nobody filled this\n\
                     in\" is not \"reach nothing\" — `--connectors none` says the second"
                        .to_string()
                }),
            });
        }
        Launch {
            as_configured: mcpconf::curate(
                defs,
                approval,
                ConnectorPolicy::AsConfigured,
                allow,
                apex,
            ),
            selections,
            allow: allow.to_vec(),
            strict_adapters: adapter::ADAPTERS
                .iter()
                .filter(|a| a.strict_mcp)
                .map(|a| a.id)
                .collect(),
            wrapper_available: apex.is_some(),
        }
    }

    /// What confines one connector, and when, as the curator answered it.
    fn wrap_of(&self, name: &str) -> Option<Wrap> {
        self.as_configured
            .decisions
            .iter()
            .find(|d| d.name == name)
            .and_then(|d| d.confined)
    }

    /// The sentence the local plane prints beside a program.
    ///
    /// Three answers, not two. The middle one is the whole reason [`Wrap`]
    /// exists: a plugin's bare definition is confined in a session APEX
    /// started and bare in one anybody else started, and a column that said
    /// "sandboxed" would be describing only half the machine.
    fn confinement_line(&self, name: &str) -> String {
        match self.wrap_of(name) {
            Some(Wrap::InDefinition) => {
                "sandboxed however it is started (its definition names `apex mcp run`)".to_string()
            }
            Some(Wrap::AtLaunch) => {
                "sandboxed only under `apex agent` (the launch file wraps it; running the agent \
                 yourself does not)"
                    .to_string()
            }
            Some(Wrap::Not) => {
                "NOT sandboxed — `apex mcp confine` would change that".to_string()
            }
            // A program with no decision at all means the launcher never saw
            // this definition, which is a disagreement between two readers of
            // the same files and must not be printed as a verdict.
            Some(Wrap::NoProcess) | None => {
                "not classified by the session launcher — report this".to_string()
            }
        }
    }
}

/// The session launcher's verdicts for this `$HOME`, under the default policy.
///
/// The one place three readouts get their confinement answers from —
/// `apex mcp list`, `apex mcp planes` and `apex provenance show` — so none of
/// them can decide a plugin's server is sandboxed while another decides it is
/// not. It is the same [`mcpconf::curate`] call `apex-agentd` makes in
/// `install_mcp_config`, which is what makes these readouts about a real
/// launch rather than about this file's opinion of one.
///
/// [`ConnectorPolicy::AsConfigured`], so `connector_allow` is not consulted;
/// `&[]` is passed rather than this machine's real list precisely so nobody
/// reads a selection into an answer that is only about wrapping. The wrapper is
/// this binary — `None` would be a could-not-run, and `curate` reports the
/// servers unconfined and says why rather than claiming a sandbox it could not
/// build.
pub fn launch_verdicts(home: &Path, cwd: Option<&Path>) -> Curated {
    mcpconf::curate(
        &mcpconf::read(home, cwd),
        &mcpconf::approvals(home, cwd),
        ConnectorPolicy::AsConfigured,
        &[],
        std::env::current_exe().ok().as_deref(),
    )
}

/// The plane readout: every connector by plane, and what each policy removes.
pub fn plane_report(found: &[Server], launch: &Launch) -> String {
    let mut out = String::new();
    let cloud: Vec<&Server> = found.iter().filter(|s| Plane::of(&s.transport).is_cloud()).collect();
    let local: Vec<&Server> = found
        .iter()
        .filter(|s| matches!(Plane::of(&s.transport), Plane::Local))
        .collect();
    let unknown: Vec<&Server> = found
        .iter()
        .filter(|s| matches!(Plane::of(&s.transport), Plane::Unknown(_)))
        .collect();

    out.push_str("LOCAL PLANE — programs this machine starts\n");
    if local.is_empty() {
        out.push_str("  none\n");
    }
    for s in &local {
        out.push_str(&format!(
            "  {:<28}  {}\n",
            s.name,
            launch.confinement_line(&s.name)
        ));
    }

    out.push_str("\nCLOUD PLANE — endpoints off this machine\n");
    if cloud.is_empty() {
        out.push_str("  none\n");
    }
    for s in &cloud {
        let url = match &s.transport {
            Transport::Endpoint { url, .. } => url.clone(),
            _ => String::new(),
        };
        out.push_str(&format!(
            "  {:<28}  {url}\n      credential {}\n",
            s.name,
            if s.credential.agent_readable() {
                "readable by any agent that runs as you"
            } else {
                "not readable by the agent"
            }
        ));
    }

    if !unknown.is_empty() {
        out.push_str("\nNEITHER — a definition with no transport this understands\n");
        for s in &unknown {
            out.push_str(&format!("  {}\n", s.name));
        }
    }

    out.push_str(&format!(
        "\n{} local, {} cloud",
        local.len(),
        cloud.len()
    ));
    if !unknown.is_empty() {
        out.push_str(&format!(", {} neither", unknown.len()));
    }

    // The per-connector table first, because it is the one that selects, and a
    // reader who stops after one table should have read that one.
    out.push_str(".\n\nWHAT `--connectors` DOES TO THESE CONNECTORS — run, not described\n");
    out.push_str(&format!(
        "{:<14}  {:<24}  {:>5}  {:>5}  {:>7}  {:>9}\n",
        "POLICY", "FLAG", "LOCAL", "CLOUD", "REMOVED", "SANDBOXED"
    ));
    for sel in &launch.selections {
        out.push_str(&format!(
            "{:<14}  {:<24}  {:>5}  {:>5}  {:>7}  {:>9}\n",
            sel.policy.as_str(),
            sel.flag,
            sel.kept_local,
            sel.kept_cloud,
            sel.dropped,
            sel.sandboxed
        ));
        if let Some(why) = &sel.refused {
            // Every line of it indented under the row it belongs to, so a
            // refusal is not mistaken for the next policy's row.
            for line in why.lines() {
                out.push_str(&format!("{:<14}  {line}\n", ""));
            }
        }
    }

    out.push_str("\nWHAT A NAMED POLICY DOES TO THE CLOUD PLANE — by removing the network\n");
    out.push_str(&format!(
        "{:<26}  {:<20}  {:<8}  {}\n",
        "POLICY", "NETWORK", "CLOUD", "FLAG"
    ));
    for r in reductions() {
        out.push_str(&format!(
            "{:<26}  {:<20}  {:<8}  {}\n",
            r.name,
            r.network.to_string(),
            if r.cloud_reachable { "reachable" } else { "REMOVED" },
            r.flag
        ));
    }

    // The honesty paragraph, and it is not decoration: without it the tables
    // above read as properties of this machine rather than of one path
    // through it.
    let sandboxed = launch.as_configured.confined();
    let everywhere = launch.as_configured.confined_everywhere();
    out.push_str(&format!(
        "\nTWO DIFFERENT MECHANISMS, and neither is the other. A strict sandbox removes the\n\
         session's network, and a session with no network reaches no endpoint — that takes\n\
         all {} cloud connector(s) at once and selects none of them. None of the five named\n\
         presets reduces anything; every one of them widens the default. `--connectors` is\n\
         the selection: `local_only` removes the cloud plane WITHOUT removing the\n\
         network, and `curated` keeps only the connectors the runtime's own configuration\n\
         names.\n",
        cloud.len()
    ));
    out.push_str(&match launch.allow.len() {
        0 => "\nconnector_allow names nothing on this machine, so `--connectors curated` is\n\
              refused rather than run. Set it in the runtime's configuration — not in the\n\
              request, because a list the confined thing can write is not a boundary.\n"
            .to_string(),
        _ => format!(
            "\nconnector_allow names {}: {}.\n",
            launch.allow.len(),
            launch.allow.join(", ")
        ),
    });
    out.push_str(&format!(
        "\nWHERE ALL OF THAT APPLIES, which is narrower than this machine: only a session\n\
         started through `apex agent`, on an adapter that can be told to ignore every\n\
         other MCP configuration — today {}. Start the agent yourself and you get every\n\
         connector above, unreduced.\n",
        if launch.strict_adapters.is_empty() {
            "none".to_string()
        } else {
            launch.strict_adapters.join(", ")
        },
    ));
    // The number, only where there is one. "0 of the 0 sandboxed" is noise on
    // a machine with nothing wrapped, and noise in a security readout is how
    // the sentence that matters stops being read.
    if sandboxed > everywhere {
        out.push_str(&format!(
            "{} of the {} sandboxed connector(s) above lose their sandbox when you do,\n\
             because that wrapper lives only in the file the daemon writes.\n\
             `apex mcp confine` writes it into the definition instead, which survives\n\
             anything — until the plugin updates and replaces the file.\n",
            sandboxed - everywhere,
            sandboxed
        ));
    }
    if !launch.wrapper_available {
        out.push_str(
            "\nAnd on this run the `apex` binary the wrapper needs could not be found, so the\n\
             sandboxed counts above are what this machine COULD do, not what it would.\n",
        );
    }
    out
}

pub fn plane_json(found: &[Server], launch: &Launch) -> Value {
    let cloud = found.iter().filter(|s| Plane::of(&s.transport).is_cloud()).count();
    let local = found
        .iter()
        .filter(|s| matches!(Plane::of(&s.transport), Plane::Local))
        .count();
    json!({
        "connectors": found.iter().map(|s| {
            let plane = Plane::of(&s.transport);
            let wrap = launch.wrap_of(&s.name);
            json!({
                "name": s.name,
                "plane": plane.tag(),
                "planeDetail": plane.describe(),
                "agentReadableCredential": s.credential.agent_readable(),
                "definedIn": s.surface.describe(),
                // What confines it, and whether that survives somebody
                // starting the agent themselves. A consumer reading only
                // `sandboxed` gets the truth about an APEX-started session;
                // the second field is what stops it being read as a fact about
                // the machine.
                "sandboxed": wrap.and_then(|w| w.confined()),
                "sandboxedBy": wrap.map_or("unknown", |w| w.tag()),
                "sandboxSurvivesAHandRun": wrap.is_some_and(|w| w.survives_a_hand_run()),
            })
        }).collect::<Vec<_>>(),
        "local": local,
        "cloud": cloud,
        "policies": reductions().iter().map(|r| r.to_json(cloud, local)).collect::<Vec<_>>(),
        // The selection table, each row a real run of the curator.
        "connectorPolicies":
            launch.selections.iter().map(Selection::to_json).collect::<Vec<_>>(),
        "perConnectorSwitch": true,
        "perConnectorSwitchIs": "--connectors curated, against connector_allow in the runtime's \
                                 configuration",
        "connectorAllow": launch.allow,
        // The qualifier, as a field rather than only as prose, because the
        // consumer most likely to over-claim is the one that never reads the
        // prose.
        "appliesOnlyToSessionsStartedBy": "apex agent",
        "adaptersThatCanBeCurated": launch.strict_adapters,
        "sandboxedUnderApexAgent": launch.as_configured.confined(),
        "sandboxedWhenStartedByHand": launch.as_configured.confined_everywhere(),
        "howCloudIsReduced": "two ways, and they are different: removing the session's network \
                              (--sandbox strict) takes every cloud connector at once and \
                              selects none, while --connectors local|curated|none selects, and \
                              only for a session apex agent started",
    })
}

// ═════════════════════════════════════════════════════════════════════════════
//  P1-027 — memory providers
// ═════════════════════════════════════════════════════════════════════════════

/// How a server came to be treated as a memory provider.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Identified {
    /// Named in the declaration file. The only answer that is not a guess.
    Declared(PathBuf),
    /// Its name looks like a memory server's. Labelled everywhere it appears.
    Guessed,
}

impl Identified {
    pub fn describe(&self) -> String {
        match self {
            Identified::Declared(p) => format!("declared in {}", p.display()),
            Identified::Guessed => {
                "GUESSED from its name — declare it to be sure (see `apex mcp memory --help`)"
                    .to_string()
            }
        }
    }
}

/// Whether a provider answered, and the two ways of not knowing.
///
/// Four values, the shape `verify.rs` argues for at length: "it said no" and
/// "I could not ask" are different measurements, and a status command that
/// collapsed them would either report a healthy provider as broken whenever
/// the machine was offline, or report an unprobed one as fine.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Health {
    /// An `initialize` came back carrying `result`.
    Answered,
    /// The server answered and the answer was not an acceptance.
    Refused(String),
    /// Nothing answered.
    Unreachable(String),
    /// No probe was made, and why. The default, because probing is opt-in.
    NotProbed(String),
}

impl Health {
    pub fn describe(&self) -> String {
        match self {
            Health::Answered => "answered an MCP handshake".to_string(),
            Health::Refused(why) => format!("answered, and refused: {why}"),
            Health::Unreachable(why) => format!("did not answer: {why}"),
            Health::NotProbed(why) => format!("not probed — {why}"),
        }
    }

    fn to_json(&self) -> Value {
        match self {
            Health::Answered => json!({"state": "answered", "healthy": true}),
            Health::Refused(why) => json!({"state": "refused", "healthy": false, "why": why}),
            Health::Unreachable(why) => {
                json!({"state": "unreachable", "healthy": false, "why": why})
            }
            // `healthy` is null, not false. This is the whole point of the
            // fourth value surviving into the JSON.
            Health::NotProbed(why) => json!({"state": "notProbed", "healthy": null, "why": why}),
        }
    }
}

/// Which projects a provider applies to.
///
/// Read off the definition's own surface rather than invented: Claude resolves
/// `~/.claude.json`'s top-level `mcpServers` in every directory and a
/// `projects.<dir>` block only in that one, so the mapping P1-027 asks to make
/// visible is already a fact about where the definition lives.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Scope {
    /// Every directory on this machine.
    Everywhere,
    /// One directory.
    Directory(String),
    /// Wherever the repository is checked out.
    Repository(String),
    /// Wherever the plugin is enabled, and replaced when it updates.
    Plugin(String),
}

impl Scope {
    pub fn of(surface: &Surface) -> Scope {
        match surface {
            Surface::User => Scope::Everywhere,
            Surface::Directory(d) => Scope::Directory(d.clone()),
            Surface::Repository(f) => Scope::Repository(f.display().to_string()),
            Surface::Plugin { plugin, .. } => Scope::Plugin(plugin.clone()),
        }
    }

    pub fn describe(&self) -> String {
        match self {
            Scope::Everywhere => "every project on this machine".to_string(),
            Scope::Directory(d) => format!("only the project at {d}"),
            Scope::Repository(f) => format!("every checkout of the repository holding {f}"),
            Scope::Plugin(p) => format!("wherever the '{p}' plugin is enabled"),
        }
    }

    fn to_json(&self) -> Value {
        match self {
            Scope::Everywhere => json!({"projects": "all"}),
            Scope::Directory(d) => json!({"projects": "one", "directory": d}),
            Scope::Repository(f) => json!({"projects": "repository", "file": f}),
            Scope::Plugin(p) => json!({"projects": "plugin", "plugin": p}),
        }
    }
}

/// One memory provider.
#[derive(Debug, Clone)]
pub struct Provider {
    pub name: String,
    pub plane: Plane,
    pub address: String,
    pub identified: Identified,
    pub authoritative: bool,
    pub scope: Scope,
    pub health: Health,
    /// Whether the agent itself can read the credential.
    pub agent_readable: bool,
}

impl Provider {
    fn to_json(&self) -> Value {
        json!({
            "name": self.name,
            "plane": self.plane.tag(),
            "address": self.address,
            "identifiedBy": match &self.identified {
                Identified::Declared(_) => "declaration",
                Identified::Guessed => "name",
            },
            "identifiedDetail": self.identified.describe(),
            "authoritative": self.authoritative,
            "scope": self.scope.to_json(),
            "health": self.health.to_json(),
            "agentReadableCredential": self.agent_readable,
        })
    }
}

/// What the declaration file says.
#[derive(Debug, Clone, Default)]
pub struct Declaration {
    pub path: Option<PathBuf>,
    pub authoritative: Option<String>,
    pub providers: Vec<String>,
    /// A file that exists and could not be read or parsed. Never silently an
    /// absent declaration: that would turn a typo into a machine that had
    /// quietly gone back to guessing.
    pub error: Option<String>,
}

/// `$XDG_CONFIG_HOME/apex/memory.toml`, or `~/.config/apex/memory.toml`.
///
/// The same place `mcp/sidecar.rs` puts its per-server policies, so there is
/// one directory holding what the user has decided about MCP.
pub fn declaration_path(home: &Path) -> PathBuf {
    match std::env::var_os("XDG_CONFIG_HOME") {
        Some(x) if !x.is_empty() => PathBuf::from(x).join(DECLARATION),
        _ => home.join(".config").join(DECLARATION),
    }
}

pub fn read_declaration(home: &Path) -> Declaration {
    let path = declaration_path(home);
    let text = match std::fs::read_to_string(&path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Declaration::default(),
        Err(e) => {
            return Declaration {
                path: Some(path),
                error: Some(format!("it exists and could not be read: {e}")),
                ..Declaration::default()
            }
        }
    };
    let doc: toml::Value = match text.parse() {
        Ok(d) => d,
        Err(e) => {
            return Declaration {
                path: Some(path),
                error: Some(format!("it is not valid TOML: {e}")),
                ..Declaration::default()
            }
        }
    };
    let authoritative = doc
        .get("authoritative")
        .and_then(toml::Value::as_str)
        .map(str::to_string);
    let mut providers: Vec<String> = doc
        .get("providers")
        .and_then(toml::Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(toml::Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default();
    // The authoritative one is a provider whether or not it was listed twice.
    if let Some(a) = &authoritative {
        if !providers.iter().any(|p| p == a) {
            providers.push(a.clone());
        }
    }
    Declaration {
        path: Some(path),
        authoritative,
        providers,
        error: None,
    }
}

/// Whether a server's name looks like a memory server's.
///
/// The guess, in one place so it can be read and argued with. Deliberately
/// narrow: a substring match on `memory` and on the two names the MCP ecosystem
/// actually ships, and nothing cleverer. Every provider found this way is
/// labelled [`Identified::Guessed`] wherever it appears.
fn looks_like_memory(name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    n.contains("memory") || n.contains("mem0") || n.contains("knowledge-graph")
}

/// Every memory provider, and which one is authoritative.
pub fn providers(found: &[Server], decl: &Declaration) -> Vec<Provider> {
    let declared = !decl.providers.is_empty();
    let mut out: Vec<Provider> = Vec::new();
    for s in found {
        let identified = if decl.providers.iter().any(|p| p == &s.name) {
            Identified::Declared(decl.path.clone().unwrap_or_default())
        } else if !declared && looks_like_memory(&s.name) {
            Identified::Guessed
        } else {
            continue;
        };
        let address = match &s.transport {
            Transport::Endpoint { url, .. } => url.clone(),
            Transport::Stdio { command, args } => {
                format!("{command} {}", args.join(" ")).trim_end().to_string()
            }
            Transport::Other(what) => what.clone(),
        };
        out.push(Provider {
            name: s.name.clone(),
            plane: Plane::of(&s.transport),
            address,
            identified,
            // Filled in below, once the whole set is known.
            authoritative: false,
            scope: Scope::of(&s.surface),
            health: Health::NotProbed("probing is opt-in; pass --probe".to_string()),
            agent_readable: s.credential.agent_readable(),
        });
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));

    // Authority: the declaration decides. With no declaration and exactly one
    // candidate, that one is it and the report says how that was decided. With
    // two and no declaration, NOTHING is marked authoritative — picking one
    // would be this module deciding where somebody's notes live.
    if let Some(a) = &decl.authoritative {
        for p in out.iter_mut() {
            p.authoritative = &p.name == a;
        }
    } else if out.len() == 1 {
        out[0].authoritative = true;
    }
    out
}

/// The readout.
pub fn memory_report(providers: &[Provider], decl: &Declaration) -> String {
    let mut out = String::new();

    if let Some(e) = &decl.error {
        // A broken declaration must not read as no declaration.
        out.push_str(&format!(
            "the memory declaration at {} could not be used: {e}\n\
             Nothing is being guessed in its place — fix the file or remove it.\n\n",
            decl.path.clone().unwrap_or_default().display()
        ));
        return out;
    }

    if providers.is_empty() {
        out.push_str("no MCP server on this machine is a memory provider.\n\n");
        out.push_str(&format!(
            "Nothing was declared, and no server's name looks like a memory server's.\n\
             To name one:  {}\n    authoritative = \"<the name apex mcp list shows>\"\n",
            declaration_path(Path::new("~")).display()
        ));
        return out;
    }

    for p in providers {
        out.push_str(&format!("{}\n", p.name));
        out.push_str(&format!("  plane         {}\n", p.plane.describe()));
        out.push_str(&format!("  address       {}\n", p.address));
        out.push_str(&format!(
            "  authoritative {}\n",
            if p.authoritative {
                match &decl.authoritative {
                    Some(_) => "yes, by declaration".to_string(),
                    None => "yes — it is the only provider here, which is how that was decided"
                        .to_string(),
                }
            } else {
                "no".to_string()
            }
        ));
        out.push_str(&format!("  projects      {}\n", p.scope.describe()));
        out.push_str(&format!("  health        {}\n", p.health.describe()));
        out.push_str(&format!("  identified    {}\n", p.identified.describe()));
        if p.agent_readable {
            out.push_str("  credential    readable by any agent that runs as you\n");
        }
        out.push('\n');
    }

    if providers.len() > 1 && decl.authoritative.is_none() {
        out.push_str(&format!(
            "{} providers and no declaration says which is authoritative. APEX will not\n\
             guess where your memory lives — name one:\n  {}\n    authoritative = \"…\"\n\n",
            providers.len(),
            declaration_path(Path::new("~")).display()
        ));
    }

    // The constraint, restated in the output rather than only in this file.
    out.push_str(
        "APEX stores no memory of its own. This is a view over MCP servers something\n\
         else already defined; the provider named authoritative stays the source of\n\
         truth, nothing here copies out of it or into it, and removing the declaration\n\
         leaves the machine exactly as it was. There is no migration to do.\n",
    );
    out
}

pub fn memory_json(providers: &[Provider], decl: &Declaration) -> Value {
    json!({
        "providers": providers.iter().map(Provider::to_json).collect::<Vec<_>>(),
        "count": providers.len(),
        "authoritative": providers.iter().find(|p| p.authoritative).map(|p| p.name.clone()),
        "declaration": {
            "path": decl.path.clone().map(|p| p.display().to_string()),
            "present": decl.path.is_some() && decl.error.is_none() && !decl.providers.is_empty(),
            "error": decl.error,
        },
        // Asserted in the JSON so a consumer can check it rather than trust a
        // paragraph of prose.
        "apexOwnedStore": false,
        "migrationRequired": false,
    })
}

// ═════════════════════════════════════════════════════════════════════════════
//  verbs
// ═════════════════════════════════════════════════════════════════════════════

/// `apex mcp planes`
pub fn planes_main(json: bool) -> Result<i32> {
    let home = crate::mcp::home();
    let cwd = std::env::current_dir().ok();
    let found = servers::discover(&home, cwd.as_deref());
    // The same three inputs `apex-agentd`'s `install_mcp_config` uses, read
    // here so the readout is of a launch rather than of a design. `current_exe`
    // rather than a `PATH` lookup, for the reason the daemon resolves its own
    // binary: a wrapper pointing at a different build is the one pairing
    // guaranteed to be wrong.
    let cfg = apex_agent_core::config::Config::load();
    let apex = std::env::current_exe().ok();
    let launch = Launch::measure(
        &mcpconf::read(&home, cwd.as_deref()),
        &mcpconf::approvals(&home, cwd.as_deref()),
        &cfg.connector_allow,
        apex.as_deref(),
    );
    if json {
        println!("{}", serde_json::to_string_pretty(&plane_json(&found, &launch))?);
    } else if found.is_empty() {
        println!("no MCP server is defined for this account");
    } else {
        print!("{}", plane_report(&found, &launch));
    }
    Ok(0)
}

/// `apex mcp memory`
pub fn memory_main(json: bool, probe: bool) -> Result<i32> {
    let home = crate::mcp::home();
    let cwd = std::env::current_dir().ok();
    let found = servers::discover(&home, cwd.as_deref());
    let decl = read_declaration(&home);
    let mut list = providers(&found, &decl);

    if probe {
        for p in list.iter_mut() {
            p.health = probe_one(&found, &p.name);
        }
    }

    if json {
        println!("{}", serde_json::to_string_pretty(&memory_json(&list, &decl))?);
    } else {
        print!("{}", memory_report(&list, &decl));
    }
    // A declaration that could not be read is a failure; an unprobed provider
    // is not.
    if decl.error.is_some() {
        return Ok(1);
    }
    let refused = list
        .iter()
        .any(|p| matches!(p.health, Health::Refused(_) | Health::Unreachable(_)));
    Ok(i32::from(refused))
}

/// The operation a brokered MCP server is used through. `connect.rs`'s
/// `OPERATION`, and it must stay the same string — a probe that asked for a
/// capability nobody grants would report every healthy provider as refused.
const OPERATION: &str = "mcp.request";

/// The message the far end answers without acting on anything.
///
/// Byte-identical to `connect.rs`'s `HANDSHAKE` and for the same reason:
/// `initialize` is the one call every MCP server must answer, and answering it
/// changes nothing anybody has to undo. It is what makes probing somebody's
/// memory server safe — this is a read, not a write, and there is no other
/// request this module can make.
const HANDSHAKE: &[u8] = br#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"apex-mcp-memory","version":"1"}}}"#;

/// Ask one provider to answer a handshake, changing nothing.
///
/// Routed through the broker exactly as `apex mcp connect`'s proof is, because
/// that is where the credential lives and this must not learn a second way to
/// hold one.
///
/// ## Why this never grants
///
/// `apex mcp connect` grants `mcp.request` before it probes and revokes it
/// again if the probe fails, which is right for a verb whose whole job is to
/// change the machine. It is wrong here. `apex mcp memory` is a status command,
/// and a status command that widened a capability in order to report on it
/// would be changing the thing it claims to be observing — and would leave the
/// grant behind on any path that did not reach the revoke. So an ungranted
/// provider is [`Health::NotProbed`] with the command that would grant it, and
/// the machine is left exactly as it was found.
///
/// Every other arm is the same refusal in a different place: a provider whose
/// credential sits in a file is not probed either, because reading a token out
/// of a file to make a request with it is precisely what `apex mcp connect`
/// exists to stop.
fn probe_one(found: &[Server], name: &str) -> Health {
    use apex_secret_core::capability::CapabilityRecord;
    use apex_secret_core::client::Client;
    use apex_secret_core::protocol::{Request, Response};

    let Some(server) = found.iter().find(|s| s.name == name) else {
        return Health::NotProbed("it is no longer defined".to_string());
    };
    let service = match &server.credential {
        servers::Credential::Brokered { service } => service.clone(),
        servers::Credential::None => {
            return Health::NotProbed(
                "its definition carries no credential, so there is nothing here to ask with"
                    .to_string(),
            )
        }
        _ => {
            return Health::NotProbed(
                "its credential is in a file the agent reads rather than in the broker, and this \
                 will not read a token out of a file to make a request with it — \
                 `apex mcp connect` moves it into the broker first"
                    .to_string(),
            )
        }
    };

    let Some(project) = std::env::current_dir()
        .ok()
        .and_then(|cwd| apex_agent_core::project::detect(&cwd).map(|p| p.root))
    else {
        return Health::NotProbed(
            "this directory is not inside a project, and a capability is granted per project"
                .to_string(),
        );
    };

    let mut client = match Client::connect() {
        Ok(c) => c,
        Err(e) => {
            // The secret daemon not answering is not the provider being down.
            return Health::NotProbed(format!("apex-secretd could not be reached: {e:#}"));
        }
    };

    let granted = match client.call(&Request::Grants) {
        Ok(Response::Grants { projects }) => projects.get(&project).is_some_and(|keys| {
            keys.iter().any(|k| k == &format!("{service}:{OPERATION}"))
        }),
        // A daemon that would not answer about grants has told us nothing
        // about grants. Not "no grant".
        _ => {
            return Health::NotProbed(
                "apex-secretd would not say which capabilities are granted here".to_string(),
            )
        }
    };
    if !granted {
        return Health::NotProbed(format!(
            "{OPERATION} on {service} is not granted in this project, and a status command will \
             not grant one to report on it — `apex secret grant {service} {OPERATION}` would"
        ));
    }

    let mut record = CapabilityRecord::new(&service, OPERATION, "");
    record.project = Some(project);
    match client.use_with_body(record, HANDSHAKE) {
        Ok(Response::Performed {
            exit_code, output, ..
        }) => {
            // `connect.rs::accepted`'s rule, and the reasoning is worth
            // repeating because both halves are counter-intuitive: the broker
            // runs curl with fail-with-body, so an HTTP error is a non-zero
            // exit that still carries the server's answer; and a zero exit is
            // not proof either, because a proxy can answer 200 with an HTML
            // page. So the test is the reply — an `initialize` that worked
            // carries `result`, and nothing else counts.
            let messages = crate::mcp::replies(&output);
            let answered_well = exit_code == 0
                && messages.iter().any(|m| {
                    serde_json::from_str::<Value>(m)
                        .ok()
                        .is_some_and(|v| v.get("result").is_some())
                });
            if answered_well {
                Health::Answered
            } else if messages.is_empty() && exit_code != 0 {
                // Nothing came back at all: the network, not the server.
                Health::Unreachable(format!(
                    "exit {exit_code} with no reply: {}",
                    crate::mcp::first_line(&output)
                ))
            } else {
                Health::Refused(
                    messages
                        .first()
                        .cloned()
                        .unwrap_or_else(|| crate::mcp::first_line(&output)),
                )
            }
        }
        Ok(Response::Error { message, .. }) => Health::Refused(message),
        Ok(other) => Health::NotProbed(format!("unexpected reply from the broker: {other:?}")),
        Err(e) => Health::NotProbed(format!("the broker could not be asked: {e:#}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn stdio(name: &str) -> Server {
        Server {
            name: name.to_string(),
            key: name.to_string(),
            surface: Surface::User,
            transport: Transport::Stdio {
                command: "npx".into(),
                args: vec!["-y".into(), "server".into()],
            },
            credential: servers::Credential::None,
        }
    }

    fn endpoint(name: &str, url: &str) -> Server {
        Server {
            name: name.to_string(),
            key: name.to_string(),
            surface: Surface::User,
            transport: Transport::Endpoint {
                kind: "http".into(),
                url: url.to_string(),
            },
            credential: servers::Credential::Brokered {
                service: name.to_string(),
            },
        }
    }

    #[test]
    fn the_two_planes_come_straight_off_the_transport() {
        // P1-028's first criterion: the distinction exists and is named.
        assert_eq!(Plane::of(&stdio("a").transport), Plane::Local);
        assert_eq!(
            Plane::of(&endpoint("b", "https://x/mcp").transport),
            Plane::Cloud
        );
        assert!(matches!(
            Plane::of(&Transport::Other("no transport".into())),
            Plane::Unknown(_)
        ));
        // And the words a person reads are different words.
        assert!(Plane::Local.describe().contains("on this machine"));
        assert!(Plane::Cloud.describe().contains("off this machine"));
        assert_ne!(Plane::Local.tag(), Plane::Cloud.tag());
    }

    #[test]
    fn a_strict_sandbox_removes_the_cloud_plane_and_no_preset_does() {
        // P1-028's second criterion, measured against the real policy types
        // rather than a table in this file. If `effective_network` stops
        // forcing Offline for strict, this fails — which is the point.
        let rs = reductions();
        let strict = rs
            .iter()
            .find(|r| r.name == "sandbox strict")
            .expect("a strict sandbox is a named policy");
        assert!(!strict.cloud_reachable, "strict must remove the cloud plane");
        assert_eq!(strict.network, NetworkPolicy::Offline);

        // Every one of the five presets leaves the cloud plane reachable. This
        // is the honest half: the presets all widen from the default, so none
        // of them is the hardened profile the criterion imagines.
        for preset in PolicyPreset::ALL {
            let r = rs
                .iter()
                .find(|r| r.name == format!("preset {}", preset.as_str()))
                .expect("every preset is reported");
            assert!(
                r.cloud_reachable,
                "preset {} unexpectedly removes the cloud plane",
                preset.as_str()
            );
        }
    }

    fn plugin_stdio(name: &str, plugin: &str) -> Server {
        Server {
            name: name.to_string(),
            key: name.rsplit(':').next().unwrap_or(name).to_string(),
            surface: Surface::Plugin {
                plugin: plugin.to_string(),
                file: PathBuf::from("/tmp/p/.mcp.json"),
            },
            transport: Transport::Stdio {
                command: "node".into(),
                args: vec!["server.js".into()],
            },
            credential: servers::Credential::None,
        }
    }

    /// The same three connectors as definitions, for the launcher's side.
    fn fixture_defs() -> Vec<Definition> {
        vec![
            Definition {
                name: "mine".into(),
                key: "mine".into(),
                origin: mcpconf::Origin::User,
                def: json!({"command": "npx", "args": ["-y", "server"]}),
            },
            Definition {
                name: "plugin:p:srv".into(),
                key: "srv".into(),
                origin: mcpconf::Origin::Plugin {
                    plugin: "p".into(),
                    file: PathBuf::from("/tmp/p/.mcp.json"),
                },
                def: json!({"command": "node", "args": ["server.js"]}),
            },
            Definition {
                name: "cloud-one".into(),
                key: "cloud-one".into(),
                origin: mcpconf::Origin::User,
                def: json!({"type": "http", "url": "https://x/mcp"}),
            },
        ]
    }

    fn fixture_found() -> Vec<Server> {
        vec![
            stdio("mine"),
            plugin_stdio("plugin:p:srv", "p"),
            endpoint("cloud-one", "https://x/mcp"),
        ]
    }

    fn fixture_launch(allow: &[String]) -> Launch {
        Launch::measure(
            &fixture_defs(),
            &Approval::default(),
            allow,
            Some(Path::new("/usr/bin/apex")),
        )
    }

    #[test]
    fn a_connector_policy_row_is_the_curators_own_answer() {
        // The table is RUN, not written. Each row here is checked against what
        // `mcpconf::curate` does with the same inputs, so a table that started
        // describing a policy the launcher no longer applies fails here rather
        // than being printed to somebody making a security decision.
        let launch = fixture_launch(&["cloud-one".to_string()]);
        let row = |p: ConnectorPolicy| {
            launch
                .selections
                .iter()
                .find(|s| s.policy == p)
                .unwrap_or_else(|| panic!("a row for {}", p.as_str()))
                .clone()
        };

        let all = row(ConnectorPolicy::AsConfigured);
        assert_eq!((all.kept_local, all.kept_cloud, all.dropped), (2, 1, 0));
        // One of the two programs is a plugin's, so the launch file wraps it;
        // the user's own is left exactly as they wrote it.
        assert_eq!(all.sandboxed, 1);
        assert_eq!(all.sandboxed_everywhere, 0, "neither is wrapped on disk");

        let local = row(ConnectorPolicy::LocalOnly);
        assert_eq!((local.kept_local, local.kept_cloud, local.dropped), (2, 0, 1));
        assert!(local.refused.is_none());

        let curated = row(ConnectorPolicy::Curated);
        assert_eq!(
            (curated.kept_local, curated.kept_cloud, curated.dropped),
            (0, 1, 2),
            "curated keeps the one connector_allow names, and it is the cloud one — \
             which is the per-connector selection the old readout said did not exist"
        );

        let none = row(ConnectorPolicy::NoConnectors);
        assert_eq!(none.kept(), 0);
        assert_eq!(none.dropped, 3);

        // And every row agrees with a direct run of the curator.
        for sel in &launch.selections {
            let c = mcpconf::curate(
                &fixture_defs(),
                &Approval::default(),
                sel.policy,
                &["cloud-one".to_string()],
                Some(Path::new("/usr/bin/apex")),
            );
            assert_eq!(sel.kept(), c.kept(), "{}", sel.policy.as_str());
            assert_eq!(sel.dropped, c.dropped(), "{}", sel.policy.as_str());
            assert_eq!(sel.sandboxed, c.confined(), "{}", sel.policy.as_str());
        }
    }

    #[test]
    fn an_empty_connector_list_is_a_refusal_and_not_a_row_of_zeroes() {
        // "Nobody filled this in" and "reach nothing" are different statements
        // and `validate_for` refuses the first. A row printing 0 kept would
        // describe a session that never starts as one that starts empty.
        let launch = fixture_launch(&[]);
        let curated = launch
            .selections
            .iter()
            .find(|s| s.policy == ConnectorPolicy::Curated)
            .expect("a curated row");
        let why = curated.refused.as_deref().unwrap_or("");
        assert!(why.contains("refused, not run"), "{why}");
        assert!(why.contains("connector_allow is empty"), "{why}");

        let text = plane_report(&fixture_found(), &launch);
        assert!(text.contains("refused rather than run"), "{text}");
    }

    #[test]
    fn the_report_says_where_the_reduction_applies_and_where_it_stops() {
        // The qualifier, which is the whole difference between a fact about
        // this machine and a fact about one path through it. Every number in
        // the tables above is about a session `apex agent` started; a person
        // who types the agent's own name gets none of it.
        let launch = fixture_launch(&["cloud-one".to_string()]);
        let found = fixture_found();
        let text = plane_report(&found, &launch);

        assert!(text.contains("LOCAL PLANE"), "{text}");
        assert!(text.contains("CLOUD PLANE"), "{text}");
        assert!(
            text.contains("started through `apex agent`"),
            "the readout must say where curation applies:\n{text}"
        );
        assert!(
            text.contains("Start the agent yourself"),
            "and where it stops:\n{text}"
        );
        // The adapter list is read off the adapters, not typed here, so an
        // adapter that grows the flag appears without this file changing.
        assert!(text.contains("claude"), "{text}");

        // The per-connector column is three-way. The plugin's server is
        // sandboxed only under `apex agent`; the user's own is not sandboxed
        // at all. A two-valued column would print the same word for both.
        assert!(
            text.contains("sandboxed only under `apex agent`"),
            "the at-launch answer is missing:\n{text}"
        );
        assert!(
            text.contains("NOT sandboxed — `apex mcp confine`"),
            "the not-sandboxed answer is missing:\n{text}"
        );

        let j = plane_json(&found, &launch);
        assert_eq!(j["perConnectorSwitch"], serde_json::Value::Bool(true));
        assert_eq!(j["local"], 2);
        assert_eq!(j["cloud"], 1);
        assert_eq!(j["sandboxedUnderApexAgent"], 1);
        assert_eq!(j["sandboxedWhenStartedByHand"], 0);
        assert_eq!(j["adaptersThatCanBeCurated"][0], "claude");

        let by: Vec<&str> = j["connectors"]
            .as_array()
            .expect("connectors")
            .iter()
            .map(|c| c["sandboxedBy"].as_str().expect("sandboxedBy"))
            .collect();
        assert_eq!(by, vec!["none", "launch", "no_process"]);
    }

    #[test]
    fn a_declared_provider_is_not_a_guess_and_a_guess_says_so() {
        let found = vec![endpoint("claude-memory", "https://m.example/mcp")];

        // With no declaration the name is a guess, and it is labelled.
        let none = Declaration::default();
        let guessed = providers(&found, &none);
        assert_eq!(guessed.len(), 1);
        assert_eq!(guessed[0].identified, Identified::Guessed);
        assert!(guessed[0].identified.describe().contains("GUESSED"));
        // One candidate and no declaration: authoritative, and the report says
        // how that was decided rather than implying somebody chose it.
        assert!(guessed[0].authoritative);
        let text = memory_report(&guessed, &none);
        assert!(text.contains("it is the only provider here"), "{text}");

        // Declared, and the guess is not used at all.
        let decl = Declaration {
            path: Some(PathBuf::from("/x/memory.toml")),
            authoritative: Some("claude-memory".into()),
            providers: vec!["claude-memory".into()],
            error: None,
        };
        let declared = providers(&found, &decl);
        assert!(matches!(declared[0].identified, Identified::Declared(_)));
        assert!(declared[0].authoritative);
        assert!(memory_report(&declared, &decl).contains("yes, by declaration"));
    }

    #[test]
    fn two_candidates_and_no_declaration_leaves_nothing_authoritative() {
        // APEX deciding where somebody's notes live would be the worst
        // available answer. `apex-plugin` refuses the same way when a plugin
        // is in two trees.
        let found = vec![
            endpoint("claude-memory", "https://m.example/mcp"),
            stdio("memory"),
        ];
        let list = providers(&found, &Declaration::default());
        assert_eq!(list.len(), 2, "{list:?}");
        assert!(
            list.iter().all(|p| !p.authoritative),
            "nothing may be authoritative by accident"
        );
        let text = memory_report(&list, &Declaration::default());
        assert!(text.contains("will not\nguess"), "{text}");
    }

    #[test]
    fn a_declaration_that_cannot_be_read_does_not_fall_back_to_guessing() {
        // The "permission denied is not absence" arm for a config file: a
        // typo'd or unreadable declaration must not silently restore the
        // heuristic, because the machine would then be guessing while its
        // owner believed it had been told.
        let decl = Declaration {
            path: Some(PathBuf::from("/x/memory.toml")),
            error: Some("it is not valid TOML: bad".to_string()),
            ..Declaration::default()
        };
        let text = memory_report(&[], &decl);
        assert!(text.contains("could not be used"), "{text}");
        assert!(
            text.contains("Nothing is being guessed in its place"),
            "{text}"
        );
    }

    #[test]
    fn an_unprobed_provider_is_neither_healthy_nor_unhealthy() {
        // The fourth value, surviving into the JSON. A consumer that read
        // `healthy: false` here would show somebody's working memory server as
        // broken because nobody asked it anything.
        let found = vec![endpoint("claude-memory", "https://m.example/mcp")];
        let list = providers(&found, &Declaration::default());
        assert!(matches!(list[0].health, Health::NotProbed(_)));
        let j = memory_json(&list, &Declaration::default());
        assert_eq!(j["providers"][0]["health"]["healthy"], Value::Null);
        assert_eq!(j["providers"][0]["health"]["state"], "notProbed");
        // And the constraint is machine-checkable, not just prose.
        assert_eq!(j["apexOwnedStore"], Value::Bool(false));
        assert_eq!(j["migrationRequired"], Value::Bool(false));
    }

    #[test]
    fn the_project_mapping_comes_from_the_surface_the_definition_lives_on() {
        // P1-027's "project mapping is visible", and it needed no new state:
        // where a definition lives already decides which projects it applies
        // to.
        assert_eq!(Scope::of(&Surface::User), Scope::Everywhere);
        assert_eq!(
            Scope::of(&Surface::Directory("/home/a/p".into())),
            Scope::Directory("/home/a/p".into())
        );
        assert!(Scope::Everywhere.describe().contains("every project"));
        assert!(Scope::Directory("/home/a/p".into())
            .describe()
            .contains("/home/a/p"));
    }

    #[test]
    fn a_provider_whose_credential_is_not_brokered_is_not_probed_rather_than_failed() {
        // Refusing to read a token out of a file in order to make a request
        // with it, and saying so, rather than reporting the provider as down.
        let mut s = endpoint("claude-memory", "https://m.example/mcp");
        s.credential = servers::Credential::InConfig {
            header: "Authorization".into(),
            shape: servers::Shape::Opaque,
        };
        let found = vec![s];
        let h = probe_one(&found, "claude-memory");
        assert!(matches!(h, Health::NotProbed(_)), "{h:?}");
        assert!(h.describe().contains("apex mcp connect"), "{}", h.describe());
    }

    #[test]
    fn looks_like_memory_is_narrow_and_readable() {
        assert!(looks_like_memory("claude-memory"));
        assert!(looks_like_memory("memory"));
        assert!(looks_like_memory("mem0"));
        assert!(!looks_like_memory("github"));
        assert!(!looks_like_memory("cloudflare"));
    }
}
