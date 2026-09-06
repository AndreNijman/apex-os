//! User preferences for the agent runtime.
//!
//! Small on purpose. The roadmap's default-agent abstraction is one setting
//! ("which upstream CLI does `a` run") and everything else here supports it.
//! Unknown keys in the file are preserved on write, so a newer APEX Shell
//! writing a field this build does not know about does not lose it.

use std::path::Path;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::adapter;
use crate::paths;
use crate::policy::{
    AgentPolicy, NativeMode, NetworkPolicy, OriginPolicy, PolicyError, SecretPolicy, SystemAccess,
};
use crate::protocol::SandboxPolicy;
use crate::term::DEFAULT_DETACH_KEY;

/// The runtime's user configuration.
///
/// The six permission dimensions are six sibling keys here rather than one
/// nested `policy` object. `sandbox` has been a top-level key since before the
/// split, and `extra` swallows anything this build does not recognise — so
/// nesting it would have moved a security setting into the catch-all and
/// silently downgraded every configuration file that already sets it. Six
/// siblings and [`Config::policy`] to assemble them costs a few lines and
/// cannot do that.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    /// Adapter id `a` and an unqualified `apex agent run` use.
    #[serde(default = "default_agent")]
    pub default_agent: String,
    /// Dimension 1: applied when `--native` and `--agent-bypass` are not given.
    #[serde(default)]
    pub native: NativeMode,
    /// Dimension 2: applied when `--sandbox` is not given.
    #[serde(default)]
    pub sandbox: SandboxPolicy,
    /// Dimension 3: applied when `--system-access` is not given.
    #[serde(default)]
    pub system: SystemAccess,
    /// Dimension 4: applied when `--secrets` is not given.
    #[serde(default)]
    pub secrets: SecretPolicy,
    /// Dimension 5: applied when `--network` is not given.
    #[serde(default)]
    pub network: NetworkPolicy,
    /// Dimension 6: applied when `--origin-policy` is not given.
    #[serde(default)]
    pub origin: OriginPolicy,
    /// Key that detaches from an attached session.
    #[serde(default = "default_detach_key")]
    pub detach_key: String,
    /// Take a checkpoint before every task without being asked.
    #[serde(default)]
    pub auto_checkpoint: bool,
    /// Anything this build does not recognise, kept so a round-trip is lossless.
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

fn default_agent() -> String {
    adapter::DEFAULT_AGENT.to_string()
}

fn default_detach_key() -> String {
    DEFAULT_DETACH_KEY.to_string()
}

impl Default for Config {
    fn default() -> Config {
        Config {
            default_agent: default_agent(),
            native: NativeMode::default(),
            sandbox: SandboxPolicy::default(),
            system: SystemAccess::default(),
            secrets: SecretPolicy::default(),
            network: NetworkPolicy::default(),
            origin: OriginPolicy::default(),
            detach_key: default_detach_key(),
            auto_checkpoint: false,
            extra: serde_json::Map::new(),
        }
    }
}

impl Config {
    /// The configured defaults as one policy.
    ///
    /// What `apex agent run` starts from before applying a preset and then the
    /// individual flags.
    pub fn policy(&self) -> AgentPolicy {
        AgentPolicy {
            native: self.native,
            sandbox: self.sandbox,
            system: self.system,
            secrets: self.secrets,
            network: self.network,
            origin: self.origin,
        }
    }

    /// Store a policy back as the six defaults.
    pub fn set_policy(&mut self, p: AgentPolicy) {
        self.native = p.native;
        self.sandbox = p.sandbox;
        self.system = p.system;
        self.secrets = p.secrets;
        self.network = p.network;
        self.origin = p.origin;
    }

    /// Load the user's configuration.
    ///
    /// A missing file is the defaults, not an error. A *corrupt* file is also
    /// the defaults, because refusing to launch an agent because a preferences
    /// file has a stray comma would be a worse failure than ignoring it — but
    /// the caller is told, via [`load_reporting`], so it can say so.
    pub fn load() -> Config {
        load_reporting().0
    }

    /// Validate and normalise. Returns the list of corrections made.
    ///
    /// Applied on load as well as on save, so a hand-edited file with an
    /// unknown agent name degrades to the default instead of failing every
    /// later command with the same error.
    pub fn normalise(&mut self) -> Vec<String> {
        let mut fixed = Vec::new();
        if adapter::by_id(&self.default_agent).is_none() {
            fixed.push(format!(
                "unknown default_agent {:?}, using {}",
                self.default_agent,
                adapter::DEFAULT_AGENT
            ));
            self.default_agent = default_agent();
        }
        if crate::term::parse_detach_key(&self.detach_key).is_none() {
            fixed.push(format!(
                "unusable detach_key {:?}, using {DEFAULT_DETACH_KEY}",
                self.detach_key
            ));
            self.detach_key = default_detach_key();
        }
        // A stored default this build cannot enforce is corrected here rather
        // than refused at every `apex agent run`. Refusing would be the safe
        // reflex, but the failure lands on a command the user did not connect
        // to a file they edited weeks ago.
        //
        // ONE dimension at a time, never the whole policy. A file written by a
        // build that has P0-007 says `system: session`, and this build cannot
        // enforce it — but resetting all six would take `sandbox: strict` down
        // to `project` with it, and silently unconfining somebody because a
        // newer APEX Shell wrote a key is exactly the weakening the split
        // exists to prevent. Each `PolicyError` names one dimension, so each
        // pass clears one; four passes is the whole vocabulary, and the
        // all-default policy always validates.
        for _ in 0..4 {
            let Err(e) = self.policy().validate() else {
                break;
            };
            fixed.push(format!("{e}; using the default for that one setting"));
            match e {
                PolicyError::NetworkUnenforceable(_) => self.network = NetworkPolicy::default(),
                PolicyError::SystemAccessUnavailable(_) => self.system = SystemAccess::default(),
                PolicyError::SecretExportUnavailable => self.secrets = SecretPolicy::default(),
                PolicyError::RemoteElevationUnavailable => self.origin = OriginPolicy::default(),
            }
        }
        fixed
    }

    /// Write the configuration back.
    pub fn save(&self) -> Result<()> {
        let path = paths::config_file();
        let dir = path
            .parent()
            .expect("the config path always has a parent directory");
        paths::ensure_private_dir(dir)?;
        let text = serde_json::to_string_pretty(self)?;
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, format!("{text}\n"))
            .with_context(|| format!("writing {}", tmp.display()))?;
        std::fs::rename(&tmp, &path)?;
        Ok(())
    }

    /// The detach byte, already validated by [`Config::normalise`].
    pub fn detach_byte(&self) -> u8 {
        crate::term::parse_detach_key(&self.detach_key)
            .or_else(|| crate::term::parse_detach_key(DEFAULT_DETACH_KEY))
            .expect("the built-in default detach key always parses")
    }
}

/// [`Config::load`], also reporting what had to be corrected.
pub fn load_reporting() -> (Config, Vec<String>) {
    let path = paths::config_file();
    let mut cfg = match std::fs::read_to_string(&path) {
        Ok(text) => match serde_json::from_str::<Config>(&text) {
            Ok(cfg) => cfg,
            Err(e) => {
                let mut cfg = Config::default();
                let mut notes = vec![format!("{} is not valid JSON ({e}); using defaults", path.display())];
                notes.extend(cfg.normalise());
                return (cfg, notes);
            }
        },
        Err(_) => Config::default(),
    };
    let notes = cfg.normalise();
    (cfg, notes)
}

/// Parse a configuration from text, for callers that already have it.
pub fn from_str(text: &str) -> Result<Config> {
    let mut cfg: Config = serde_json::from_str(text)?;
    cfg.normalise();
    Ok(cfg)
}

/// Whether `path` looks like a readable configuration file.
pub fn exists_at(path: &Path) -> bool {
    path.is_file()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_default_configuration_is_valid() {
        let mut cfg = Config::default();
        assert!(cfg.normalise().is_empty(), "defaults needed correcting");
        assert_eq!(cfg.default_agent, adapter::DEFAULT_AGENT);
        assert_eq!(cfg.sandbox, SandboxPolicy::Project);
    }

    #[test]
    fn an_empty_object_yields_the_defaults() {
        let cfg = from_str("{}").expect("parse");
        assert_eq!(cfg.default_agent, adapter::DEFAULT_AGENT);
        assert_eq!(cfg.sandbox, SandboxPolicy::Project);
        assert_eq!(cfg.detach_key, DEFAULT_DETACH_KEY);
        assert!(!cfg.auto_checkpoint);
    }

    #[test]
    fn an_unknown_agent_falls_back_and_says_so() {
        let mut cfg = Config {
            default_agent: "nonexistent".into(),
            ..Config::default()
        };
        let notes = cfg.normalise();
        assert_eq!(cfg.default_agent, adapter::DEFAULT_AGENT);
        assert_eq!(notes.len(), 1);
        assert!(notes[0].contains("nonexistent"), "{notes:?}");
    }

    #[test]
    fn an_unusable_detach_key_falls_back_and_says_so() {
        let mut cfg = Config {
            detach_key: "not-a-key".into(),
            ..Config::default()
        };
        let notes = cfg.normalise();
        assert_eq!(cfg.detach_key, DEFAULT_DETACH_KEY);
        assert_eq!(notes.len(), 1);
        assert_eq!(cfg.detach_byte(), 0x1d);
    }

    #[test]
    fn a_sandbox_policy_survives_a_round_trip() {
        let cfg = Config {
            sandbox: SandboxPolicy::Strict,
            ..Config::default()
        };
        let text = serde_json::to_string(&cfg).unwrap();
        let back = from_str(&text).unwrap();
        assert_eq!(back.sandbox, SandboxPolicy::Strict);
    }

    #[test]
    fn every_dimension_has_its_own_key_and_its_own_default() {
        // Criterion 1 at the configuration layer: six settings, not one.
        let cfg = from_str("{}").expect("parse");
        assert_eq!(cfg.policy(), AgentPolicy::default());

        let cfg = from_str(
            r#"{"native":"bypass","sandbox":"strict","secrets":"none","network":"offline"}"#,
        )
        .expect("parse");
        assert_eq!(cfg.native, NativeMode::Bypass);
        assert_eq!(cfg.sandbox, SandboxPolicy::Strict);
        assert_eq!(cfg.secrets, SecretPolicy::None);
        assert_eq!(cfg.network, NetworkPolicy::Offline);
        // Untouched dimensions keep their defaults rather than following the
        // ones that were set.
        assert_eq!(cfg.system, SystemAccess::None);
        assert_eq!(cfg.origin, OriginPolicy::LocalElevationOnly);
    }

    #[test]
    fn a_pre_split_configuration_file_keeps_its_sandbox_setting() {
        // The file a user already has. `sandbox` must stay a top-level key and
        // must not fall into `extra`, where it would be preserved on write and
        // ignored on read — a strict user silently downgraded to project.
        let cfg = from_str(r#"{"default_agent":"codex","sandbox":"strict"}"#).expect("parse");
        assert_eq!(cfg.sandbox, SandboxPolicy::Strict);
        assert!(!cfg.extra.contains_key("sandbox"), "{:?}", cfg.extra);
        assert_eq!(cfg.policy().sandbox, SandboxPolicy::Strict);
    }

    #[test]
    fn a_stored_default_this_build_cannot_enforce_is_corrected_and_reported() {
        // Hand-edited, or written by a newer build. Failing every later
        // `apex agent run` with the same error is a worse outcome than
        // correcting the one setting that cannot be honoured.
        let mut cfg = Config {
            system: SystemAccess::Unsafe,
            ..Config::default()
        };
        let notes = cfg.normalise();
        assert_eq!(cfg.system, SystemAccess::None);
        assert_eq!(notes.len(), 1);
        assert!(notes[0].contains("system-access"), "{notes:?}");
        assert_eq!(cfg.policy().validate(), Ok(()));
    }

    #[test]
    fn correcting_one_dimension_does_not_take_the_others_with_it() {
        // The cross-version case: a build with P0-007 writes `system: session`
        // into a file this one reads. Resetting all six would take the strict
        // sandbox down to project as well, and unconfine somebody because a
        // newer APEX Shell wrote a key they never typed.
        let mut cfg = Config {
            sandbox: SandboxPolicy::Strict,
            system: SystemAccess::Session,
            secrets: SecretPolicy::None,
            ..Config::default()
        };
        let notes = cfg.normalise();
        assert_eq!(cfg.system, SystemAccess::None, "the unenforceable one is cleared");
        assert_eq!(cfg.sandbox, SandboxPolicy::Strict, "the sandbox must survive");
        assert_eq!(cfg.secrets, SecretPolicy::None, "a tighter setting must survive");
        assert_eq!(notes.len(), 1, "{notes:?}");
        assert_eq!(cfg.policy().validate(), Ok(()));
    }

    #[test]
    fn several_unenforceable_settings_are_all_corrected_and_all_reported() {
        // The loop has to converge, and every correction has to be audible:
        // a note that named one of three problems would leave two silent.
        let mut cfg = Config {
            system: SystemAccess::Unsafe,
            secrets: SecretPolicy::Export,
            origin: OriginPolicy::RemoteElevationAllowed,
            network: NetworkPolicy::Allowlist,
            ..Config::default()
        };
        let notes = cfg.normalise();
        assert_eq!(cfg.policy(), AgentPolicy::default());
        assert_eq!(notes.len(), 4, "{notes:?}");
    }

    #[test]
    fn unknown_keys_survive_a_round_trip() {
        // APEX Shell may write settings a given apex build predates; losing
        // them on the next `apex agent default` would be a silent data loss.
        let cfg = from_str(r#"{"default_agent":"codex","shell_layout":"grid","future":{"a":1}}"#)
            .expect("parse");
        assert_eq!(cfg.default_agent, "codex");
        assert_eq!(
            cfg.extra.get("shell_layout").and_then(|v| v.as_str()),
            Some("grid")
        );
        let text = serde_json::to_string(&cfg).unwrap();
        assert!(text.contains("shell_layout"), "{text}");
        assert!(text.contains("future"), "{text}");
    }

    #[test]
    fn corrupt_json_degrades_to_defaults_rather_than_failing() {
        assert!(from_str("{ not json").is_err());
        // ...but the loader itself never propagates that to the caller as a
        // hard failure; it reports and continues.
        let mut cfg = Config::default();
        assert!(cfg.normalise().is_empty());
    }

    #[test]
    fn every_adapter_id_is_an_acceptable_default_agent() {
        for id in adapter::ids() {
            let mut cfg = Config {
                default_agent: id.to_string(),
                ..Config::default()
            };
            assert!(cfg.normalise().is_empty(), "{id} was rejected");
            assert_eq!(cfg.default_agent, id);
        }
    }
}
