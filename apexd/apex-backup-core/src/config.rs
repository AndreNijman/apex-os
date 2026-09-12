//! The `[backup]` section of a project's `apex.toml`.
//!
//! Read through `apex_secret_core::project::ProjectConfig`, which is the same
//! reader `[identity.cloudflare]` and `[cloudflare] buckets` go through and has
//! the properties a root daemon needs: `O_NOFOLLOW` per component, owner
//! checked, size capped, and a parse error reported as a position and never as
//! the text at it.
//!
//! ```toml
//! [backup]
//! recipient = "apexbk1…"        # checked against the registered one
//! target    = "nas"             # local | nas | ssh | s3 | r2
//! prefix    = "apex-backup"     # the directory or key prefix under the target
//! exclude   = ["target", "node_modules"]
//!
//! [backup.nas]
//! path = "/mnt/nas/backups"
//! id   = "9f2c4a1b8e7d6503"     # the marker `apex backup init` wrote there
//!
//! [backup.r2]
//! bucket  = "example-backups"   # must also be in [cloudflare] buckets
//! service = "cloudflare"        # the stored credential, by name
//! ```
//!
//! Everything here is a *declaration*. Two of them are checked against
//! something the project cannot write — the recipient against
//! [`crate::keys::KeyStore::registered`], and the bucket against the
//! project's `[cloudflare] buckets` by the daemon itself — and the rest are
//! taken at face value because they are the owner describing their own
//! machine.

use std::path::PathBuf;

use apex_secret_core::project::{ProjectConfig, ProjectError};

use crate::target::Kind;

/// What a project says about its backups.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BackupConfig {
    pub recipient: String,
    pub kind: Kind,
    pub prefix: String,
    pub exclude: Vec<String>,
    pub where_to: Where,
}

/// The part that depends on which kind it is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Where {
    Directory { path: PathBuf, marker: Option<String> },
    Bucket { bucket: String, service: String },
}

/// The default key prefix, and the default directory under a target root.
pub const DEFAULT_PREFIX: &str = "apex-backup";

/// Why a project's `[backup]` section is not usable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConfigError {
    /// No `[backup]` at all.
    Absent,
    /// A key is missing or empty.
    Missing { key: String, hint: String },
    /// A value is there and is not usable.
    Bad { key: String, why: String },
    /// The target kind is one this build refuses, with its reason.
    Unimplemented { kind: Kind, why: &'static str },
    /// The file itself could not be read or parsed.
    Project(ProjectError),
}

impl std::fmt::Display for ConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ConfigError::Absent => write!(
                f,
                "this project has no [backup] section. `apex backup init` \
                 writes one, and `apex backup targets` lists the kinds"
            ),
            ConfigError::Missing { key, hint } => {
                write!(f, "[backup] does not set {key}. {hint}")
            }
            ConfigError::Bad { key, why } => write!(f, "[backup] {key} {why}"),
            ConfigError::Unimplemented { kind, why } => write!(
                f,
                "this build will not back up to a '{kind}' target: {why}"
            ),
            ConfigError::Project(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for ConfigError {}

impl BackupConfig {
    /// Read `<project>/apex.toml` as the account the operation runs as.
    pub fn read(
        root: &std::path::Path,
        owner_uid: u32,
        owner_name: &str,
    ) -> Result<BackupConfig, ConfigError> {
        let config =
            ProjectConfig::read(root, owner_uid, owner_name).map_err(ConfigError::Project)?;
        BackupConfig::from_project(&config)
    }

    pub fn from_project(config: &ProjectConfig) -> Result<BackupConfig, ConfigError> {
        let string = |keys: &[&str]| -> Result<Option<String>, ConfigError> {
            config
                .string(keys)
                .map(|v| v.map(str::to_string))
                .map_err(ConfigError::Project)
        };

        let Some(target) = string(&["backup", "target"])? else {
            // No `target` and no `recipient` means no [backup] worth speaking
            // of. Saying "absent" rather than "you did not set target" is the
            // difference between a first run being told what to do and being
            // told off.
            if string(&["backup", "recipient"])?.is_none() {
                return Err(ConfigError::Absent);
            }
            return Err(ConfigError::Missing {
                key: "target".to_string(),
                hint: format!(
                    "One of: {}",
                    Kind::ALL
                        .iter()
                        .map(|k| k.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
            });
        };

        let Some(kind) = Kind::parse(&target) else {
            return Err(ConfigError::Bad {
                key: "target".to_string(),
                why: format!(
                    "is '{}', which is not a target kind. One of: {}",
                    target.escape_debug(),
                    Kind::ALL
                        .iter()
                        .map(|k| k.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
            });
        };
        // Refused here — at configuration time, reading the file — and not at
        // the first `put`. See `target::Kind::unimplemented_reason`.
        if let Some(why) = kind.unimplemented_reason() {
            return Err(ConfigError::Unimplemented { kind, why });
        }

        let Some(recipient) = string(&["backup", "recipient"])? else {
            return Err(ConfigError::Missing {
                key: "recipient".to_string(),
                hint: "`sudo apex backup key init` makes a key and prints the \
                       line to add"
                    .to_string(),
            });
        };

        let prefix = string(&["backup", "prefix"])?.unwrap_or_else(|| DEFAULT_PREFIX.to_string());
        if !apex_secret_core::operation::valid_name(&prefix) {
            return Err(ConfigError::Bad {
                key: "prefix".to_string(),
                why: format!(
                    "is '{}', which cannot be one segment of an object key. \
                     Letters, digits, '_', '.' and '-', starting with a letter, \
                     a digit or '_'",
                    prefix.escape_debug()
                ),
            });
        }

        let exclude = config
            .strings(&["backup", "exclude"])
            .map_err(ConfigError::Project)?;

        let where_to = match kind {
            Kind::Local | Kind::Nas => {
                let section = if kind == Kind::Local { "local" } else { "nas" };
                let Some(path) = string(&["backup", section, "path"])? else {
                    return Err(ConfigError::Missing {
                        key: format!("{section}] path (in [backup.{section}"),
                        hint: "The directory the snapshots go in.".to_string(),
                    });
                };
                if !path.starts_with('/') {
                    return Err(ConfigError::Bad {
                        key: format!("{section}] path (in [backup.{section}"),
                        why: format!(
                            "is '{}', and a target path is absolute — a \
                             relative one would mean a different directory \
                             depending on where the command was run",
                            path.escape_debug()
                        ),
                    });
                }
                Where::Directory {
                    path: PathBuf::from(path),
                    marker: string(&["backup", section, "id"])?,
                }
            }
            Kind::R2 => {
                let Some(bucket) = string(&["backup", "r2", "bucket"])? else {
                    return Err(ConfigError::Missing {
                        key: "r2] bucket (in [backup.r2".to_string(),
                        hint: "The R2 bucket, which must also be in \
                               [cloudflare] buckets — the broker resolves it \
                               there and refuses anything else."
                            .to_string(),
                    });
                };
                Where::Bucket {
                    bucket,
                    service: string(&["backup", "r2", "service"])?
                        .unwrap_or_else(|| "cloudflare".to_string()),
                }
            }
            // Refused above, before any of this ran.
            Kind::Ssh | Kind::S3 => unreachable!("an unimplemented kind is refused above"),
        };

        Ok(BackupConfig {
            recipient,
            kind,
            prefix,
            exclude,
            where_to,
        })
    }

    /// Whether a path relative to the source root is one this run skips.
    ///
    /// Matched on whole path segments, never as a substring: `exclude =
    /// ["target"]` must not also skip `src/targeting.rs`. The staging
    /// directory is always excluded — a run that backed up the chunks it was
    /// in the middle of uploading would grow without bound.
    pub fn is_excluded(&self, relative: &str) -> bool {
        relative.split('/').any(|segment| {
            segment == crate::target::r2::STAGING_DIR
                || self.exclude.iter().any(|e| e == segment)
        })
    }
}

#[cfg(test)]
mod tests;
