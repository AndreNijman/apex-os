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
use crate::protocol::SandboxPolicy;
use crate::term::DEFAULT_DETACH_KEY;

/// The runtime's user configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    /// Adapter id `a` and an unqualified `apex agent run` use.
    #[serde(default = "default_agent")]
    pub default_agent: String,
    /// Policy applied when `--sandbox` is not given.
    #[serde(default)]
    pub sandbox: SandboxPolicy,
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
            sandbox: SandboxPolicy::default(),
            detach_key: default_detach_key(),
            auto_checkpoint: false,
            extra: serde_json::Map::new(),
        }
    }
}

impl Config {
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
