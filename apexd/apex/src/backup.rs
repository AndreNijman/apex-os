//! `apex backup` — encrypted backups (roadmap P2-001, P2-002).
//!
//! ```text
//! sudo apex backup key init            # the keypair; prints the recipient line
//! apex backup key show                 # the recipient this machine registered
//! apex backup targets                  # the five kinds, and which this build carries
//! apex backup init                     # mark the target, so an unmounted one is caught
//! apex backup run                      # take a snapshot
//! apex backup list                     # the versions on the target
//! apex backup verify latest            # presence; --deep also opens it, and needs root
//! sudo apex backup restore <id> --into /var/tmp/somewhere-new
//! ```
//!
//! Which verbs need root, and why it is not arbitrary:
//!
//! * **`run` does not.** A snapshot is sealed to a public key, so making one
//!   needs no secret at all. That is what lets a backup run as the user, or as
//!   an agent, without either of them being able to read what they wrote.
//! * **`key init` and `restore` do.** The private half is root-owned 0600, and
//!   opening a snapshot is the only thing that needs it.
//!
//! The split is P2-002's second criterion in its strongest form: the backup
//! path never touches the private key, so there is nothing on that path to
//! leak.
//!
//! `APEX_BACKUP_KEYS` moves the key directory. It exists for
//! `tests/test-apex-backup.sh`, which must not write to `/var/lib`, and it is
//! not a supported way to run the real thing — a key directory that is not
//! root-owned is a key anybody can read, and the suite says so where it sets
//! the variable.

use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use apex_backup_core::config::{BackupConfig, Where};
use apex_backup_core::crypto::Identity;
use apex_backup_core::format::SnapshotId;
use apex_backup_core::keys::KeyStore;
use apex_backup_core::session::{self, RestoreOptions, RunOptions};
use apex_backup_core::target::fs::{FsTarget, Marker};
use apex_backup_core::target::r2::{R2Target, SecretdBroker};
use apex_backup_core::target::ssh::{SshCommand, SshTarget};
use apex_backup_core::target::{Kind, Target};
use clap::Subcommand;

/// `apex backup <verb>`.
#[derive(Subcommand)]
pub enum BackupCmd {
    /// The backup keypair for this account.
    Key {
        #[command(subcommand)]
        cmd: KeyCmd,
    },
    /// The target kinds this build knows, and which of them it carries.
    Targets,
    /// Mark a target directory, so an unmounted one is caught rather than
    /// quietly written to.
    Init {
        /// The project whose apex.toml describes the backup.
        #[arg(long)]
        project: Option<PathBuf>,
    },
    /// Take a snapshot.
    Run {
        #[arg(long)]
        project: Option<PathBuf>,
        /// What to back up. Defaults to the project itself.
        #[arg(long)]
        source: Option<PathBuf>,
        /// A name for this snapshot in a listing. Never a path.
        #[arg(long, default_value = "")]
        label: String,
        /// Carry on past anything that cannot be read, recording each one in
        /// the manifest.
        #[arg(long)]
        skip_unreadable: bool,
        #[arg(long)]
        json: bool,
    },
    /// The snapshots on the target, oldest first.
    List {
        #[arg(long)]
        project: Option<PathBuf>,
        #[arg(long)]
        json: bool,
    },
    /// Check a snapshot. Presence always; contents with --deep, which needs the
    /// private key and so needs root.
    Verify {
        /// A snapshot id, or `latest`.
        snapshot: String,
        #[arg(long)]
        project: Option<PathBuf>,
        /// Open every chunk and check every digest.
        #[arg(long)]
        deep: bool,
        #[arg(long)]
        json: bool,
    },
    /// Restore a snapshot into a directory. Needs root.
    Restore {
        /// A snapshot id, or `latest`.
        snapshot: String,
        /// Where to put it. Must be empty or not exist.
        #[arg(long)]
        into: PathBuf,
        #[arg(long)]
        project: Option<PathBuf>,
        /// Write into a destination that already holds something.
        #[arg(long)]
        into_non_empty: bool,
        #[arg(long)]
        json: bool,
    },
}

#[derive(Subcommand)]
pub enum KeyCmd {
    /// Make this account's keypair. Needs root, and refuses to replace one.
    Init,
    /// Print the recipient this machine has registered.
    Show,
}

/// Exit code, the way every other verb in this CLI reports one: a `Result`
/// inside, one number out, and a message on stderr for the error arm.
pub fn main(cmd: BackupCmd) -> i32 {
    let result = match cmd {
        BackupCmd::Key { cmd } => match cmd {
            KeyCmd::Init => key_init(),
            KeyCmd::Show => key_show(),
        },
        BackupCmd::Targets => {
            targets();
            Ok(0)
        }
        BackupCmd::Init { project } => init(project.as_deref()),
        BackupCmd::Run {
            project,
            source,
            label,
            skip_unreadable,
            json,
        } => run(
            project.as_deref(),
            source.as_deref(),
            &label,
            skip_unreadable,
            json,
        ),
        BackupCmd::List { project, json } => list(project.as_deref(), json),
        BackupCmd::Verify {
            snapshot,
            project,
            deep,
            json,
        } => verify(&snapshot, project.as_deref(), deep, json),
        BackupCmd::Restore {
            snapshot,
            into,
            project,
            into_non_empty,
            json,
        } => restore(&snapshot, &into, project.as_deref(), into_non_empty, json),
    };
    match result {
        Ok(code) => code,
        Err(e) => {
            eprintln!("apex backup: {e:#}");
            1
        }
    }
}

// ── the key ─────────────────────────────────────────────────────────────────

fn store() -> KeyStore {
    store_at(std::env::var_os("APEX_BACKUP_KEYS"))
}

/// The key directory an environment chooses.
///
/// Split from [`store`] so a test can ask the question without calling
/// `set_var`: the environment is process-wide and this binary's unit tests run
/// in parallel threads, so a test that moved `APEX_BACKUP_KEYS` would be
/// moving it under every other test at the same time.
fn store_at(override_path: Option<std::ffi::OsString>) -> KeyStore {
    match override_path {
        Some(path) => KeyStore::new(PathBuf::from(path)),
        None => KeyStore::system(),
    }
}

/// Whose key this is.
///
/// The account that invoked the command, which under `sudo` is `SUDO_UID` and
/// not 0. Read from the environment because that is where it is — the caller
/// already holds root by the time this runs, so the variable does not grant
/// anything; all it selects is which of that person's own keys is meant. A
/// value that is not a number is a refusal rather than a silent fall back to
/// root's.
fn owner_uid() -> Result<u32> {
    owner_uid_from(std::env::var("SUDO_UID").ok().as_deref())
}

/// The same question, asked of a value rather than of the process.
fn owner_uid_from(sudo_uid: Option<&str>) -> Result<u32> {
    match sudo_uid {
        Some(text) => text.trim().parse::<u32>().with_context(|| {
            format!(
                "SUDO_UID is '{}', which is not a user id",
                text.escape_debug()
            )
        }),
        None => Ok(unsafe { libc::geteuid() }),
    }
}

fn key_init() -> Result<i32> {
    let uid = owner_uid()?;
    // No `geteuid` gate here, and that is deliberate. What protects the key is
    // the directory's ownership, not a check in this binary — so the
    // filesystem is left to answer, and `KeyError::Denied` says "a refusal and
    // not an absence, try it with sudo" in the one place that sentence lives.
    // A second copy of the policy here would be a second thing to get wrong,
    // and it would be wrong immediately for anyone who has pointed
    // APEX_BACKUP_KEYS at a directory they own.
    let recipient = store()
        .generate(uid)
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    println!("A backup key for uid {uid} is made.\n");
    println!("Add this to the project's apex.toml:\n");
    println!("[backup]");
    println!("recipient = \"{}\"", recipient.to_string_value());
    println!();
    println!(
        "Keep a copy of {} somewhere other than this machine. It is the only \
         thing that opens these backups, and a machine that is gone takes it \
         with it.",
        store().identity_path(uid).display()
    );
    Ok(0)
}

fn key_show() -> Result<i32> {
    let uid = owner_uid()?;
    let recipient = store()
        .registered(uid)
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    println!("{}", recipient.to_string_value());
    Ok(0)
}

fn targets() {
    println!("Target kinds this build knows:\n");
    for kind in Kind::ALL {
        match kind.unimplemented_reason() {
            None => println!("  {:<6} carried", kind.as_str()),
            Some(why) => {
                println!("  {:<6} REFUSED", kind.as_str());
                for line in wrap(why, 68) {
                    println!("         {line}");
                }
            }
        }
    }
}

/// Wrap at word boundaries. `apex backup targets` is the one place this CLI
/// prints a paragraph, and a paragraph on one line is a paragraph nobody reads.
fn wrap(text: &str, width: usize) -> Vec<String> {
    let mut lines = Vec::new();
    let mut line = String::new();
    for word in text.split_whitespace() {
        if !line.is_empty() && line.len() + 1 + word.len() > width {
            lines.push(std::mem::take(&mut line));
        }
        if !line.is_empty() {
            line.push(' ');
        }
        line.push_str(word);
    }
    if !line.is_empty() {
        lines.push(line);
    }
    lines
}

// ── the project, the config and the target ──────────────────────────────────

struct Setup {
    project: PathBuf,
    config: BackupConfig,
    uid: u32,
}

fn setup(project: Option<&Path>) -> Result<Setup> {
    let project = match project {
        Some(p) => p.to_path_buf(),
        None => std::env::current_dir().context("finding the current directory")?,
    };
    let project = project
        .canonicalize()
        .with_context(|| format!("{} could not be resolved", project.display()))?;
    let uid = owner_uid()?;
    let name = user_name(uid);
    let config = BackupConfig::read(&project, uid, &name).map_err(|e| anyhow::anyhow!("{e}"))?;
    Ok(Setup {
        project,
        config,
        uid,
    })
}

fn user_name(uid: u32) -> String {
    // Only ever used in a message, so a uid that has no passwd entry is its own
    // answer rather than a failure.
    std::env::var("SUDO_USER")
        .or_else(|_| std::env::var("USER"))
        .unwrap_or_else(|_| format!("uid {uid}"))
}

/// Build the target a project's config names.
///
/// Boxed rather than an enum because a caller only ever calls [`Target`]'s four
/// methods, and the R2 one is generic over its broker.
fn open_target(setup: &Setup) -> Result<Box<dyn Target>> {
    match &setup.config.where_to {
        Where::Directory { path, marker } => {
            let target = match setup.config.kind {
                Kind::Nas => FsTarget::nas(path.clone(), &setup.config.prefix, marker.clone()),
                _ => FsTarget::local(path.clone(), &setup.config.prefix, marker.clone()),
            };
            Ok(Box::new(target))
        }
        Where::Bucket { bucket, service } => Ok(Box::new(R2Target::new(
            SecretdBroker::new(service, &setup.project.to_string_lossy()),
            bucket,
            &setup.config.prefix,
            &setup.project,
        ))),
        Where::Remote { endpoint, root } => Ok(Box::new(SshTarget::new(
            SshCommand,
            endpoint.clone(),
            root,
            &setup.config.prefix,
        ))),
    }
}

fn init(project: Option<&Path>) -> Result<i32> {
    let setup = setup(project)?;
    // An ssh target has no marker to write — `apex backup init` on one is a
    // connection test, which is the thing an operator actually wants before
    // the first run: it proves the key is accepted, the host key matches the
    // pinned one, and the remote directory can be created. A failure here is
    // the ssh refusal in full, which is where the useful message is.
    if let Where::Remote { endpoint, root } = &setup.config.where_to {
        let target = SshTarget::new(
            SshCommand,
            endpoint.clone(),
            root,
            &setup.config.prefix,
        );
        target.prepare().map_err(|e| anyhow::anyhow!("{e}"))?;
        println!("{} is ready.\n", target.describe());
        println!(
            "The host key was the pinned one, the key was accepted, and \
             {root}/{} exists.",
            setup.config.prefix
        );
        println!(
            "\nThere is no marker file on an ssh target, so nothing catches a \
             path typo the way it does on a directory target: the host key pin \
             says WHICH MACHINE, and the path is taken as written."
        );
        return Ok(0);
    }
    let Where::Directory { path, marker } = &setup.config.where_to else {
        bail!(
            "`apex backup init` marks a directory target so that an unmounted \
             one is caught. An r2 target needs no marker: the bucket either \
             answers or it does not, and `apex backup list` says which"
        );
    };
    let target = match setup.config.kind {
        Kind::Nas => FsTarget::nas(path.clone(), &setup.config.prefix, None),
        _ => FsTarget::local(path.clone(), &setup.config.prefix, None),
    };
    if let Ok(existing) = target.read_marker() {
        println!("{} is already target {}.", path.display(), existing.id);
        if marker.as_deref() != Some(existing.id.as_str()) {
            println!("\nThis project does not record it. Add to apex.toml:\n");
            println!("[backup.{}]", setup.config.kind);
            println!("id = \"{}\"", existing.id);
        }
        return Ok(0);
    }
    target.prepare().map_err(|e| anyhow::anyhow!("{e}"))?;
    let marker = Marker::new(apex_backup_core::now_ms())?;
    target
        .write_marker(&marker)
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    println!("{} is now a backup target.\n", path.display());
    println!("Add this to the project's apex.toml:\n");
    println!("[backup.{}]", setup.config.kind);
    println!("path = \"{}\"", path.display());
    println!("id = \"{}\"", marker.id);
    Ok(0)
}

fn run(
    project: Option<&Path>,
    source: Option<&Path>,
    label: &str,
    skip_unreadable: bool,
    json: bool,
) -> Result<i32> {
    let setup = setup(project)?;
    // The declared recipient is checked against the one this machine
    // registered, BEFORE anything is read or written. See keys.rs for the
    // one-line attack this refuses.
    let recipient = store()
        .check_declared(setup.uid, &setup.config.recipient)
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    let target = open_target(&setup)?;
    let source = source.unwrap_or(&setup.project);

    let written = session::write(
        source,
        target.as_ref(),
        &recipient,
        &setup.config,
        &RunOptions {
            skip_unreadable,
            label: label.to_string(),
        },
        apex_backup_core::now_ms(),
    )
    .map_err(|e| anyhow::anyhow!("{e}"))?;

    if json {
        println!(
            "{}",
            serde_json::json!({
                "snapshot": written.id.as_str(),
                "files": written.files,
                "bytes": written.bytes,
                "dataChunks": written.head.data_chunks,
                "manifestChunks": written.head.manifest_chunks,
                "skipped": written.skipped.len(),
                "target": target.describe(),
            })
        );
    } else {
        println!(
            "snapshot {} — {} file(s), {} byte(s) to {}",
            written.id,
            written.files,
            written.bytes,
            target.describe()
        );
        for skipped in &written.skipped {
            println!("  NOT BACKED UP  {} — {}", skipped.path, skipped.why);
        }
    }
    Ok(0)
}

fn list(project: Option<&Path>, json: bool) -> Result<i32> {
    let setup = setup(project)?;
    let target = open_target(&setup)?;
    let listed = session::list(target.as_ref()).map_err(|e| anyhow::anyhow!("{e}"))?;

    if json {
        let rows: Vec<serde_json::Value> = listed
            .iter()
            .map(|l| {
                serde_json::json!({
                    "snapshot": l.id.as_str(),
                    "verdict": l.verdict.to_json(),
                    "label": l.head.as_ref().map(|h| h.label.clone()),
                    "createdMs": l.head.as_ref().map(|h| h.created_ms),
                    "recipient": l.head.as_ref().map(|h| h.recipient.clone()),
                })
            })
            .collect();
        println!("{}", serde_json::json!({ "snapshots": rows }));
        return Ok(0);
    }

    if listed.is_empty() {
        // Said precisely. The target answered and holds nothing — which is a
        // different sentence from the one an unreachable target gets, and that
        // one is an error rather than this.
        println!("{} answered, and holds no snapshots.", target.describe());
        return Ok(0);
    }
    for entry in &listed {
        let label = entry
            .head
            .as_ref()
            .map(|h| h.label.clone())
            .unwrap_or_default();
        println!(
            "{}  {:<12} {}",
            entry.id,
            label,
            if entry.verdict.is_intact() {
                String::new()
            } else {
                entry.verdict.to_string()
            }
        );
    }
    Ok(0)
}

/// Resolve `latest`, or a snapshot id, against what the target holds.
fn resolve(target: &dyn Target, want: &str) -> Result<SnapshotId> {
    if want == "latest" {
        let ids = target.list().map_err(|e| anyhow::anyhow!("{e}"))?;
        return ids.into_iter().next_back().ok_or_else(|| {
            anyhow::anyhow!(
                "that target answered and holds no snapshots, so there is no \
                 latest one"
            )
        });
    }
    SnapshotId::parse(want).ok_or_else(|| {
        anyhow::anyhow!(
            "'{}' is not a snapshot id. `apex backup list` prints them, and \
             `latest` names the most recent",
            want.escape_debug()
        )
    })
}

/// The private half, or the reason there is not one.
///
/// `what` names the verb that wanted it, so a refusal says which command is
/// being refused rather than only which file.
fn identity(uid: u32, what: &str) -> Result<Identity> {
    store()
        .identity(uid)
        .map_err(|e| anyhow::anyhow!("{what} needs the private half of the backup key: {e}"))
}

fn verify(snapshot: &str, project: Option<&Path>, deep: bool, json: bool) -> Result<i32> {
    let setup = setup(project)?;
    let target = open_target(&setup)?;
    let id = resolve(target.as_ref(), snapshot)?;
    let key = if deep {
        Some(identity(setup.uid, "--deep")?)
    } else {
        None
    };
    let verification = session::verify(target.as_ref(), &id, key.as_ref());

    if json {
        println!("{}", verification.to_json());
    } else {
        println!("snapshot {}", verification.id);
        println!("  objects   {}", verification.presence);
        println!("  contents  {}", verification.contents);
    }
    // A verdict that is not `intact` is a non-zero exit, so a timer that runs
    // `apex backup verify` notices. `could-not-run` exits non-zero too: it is
    // not a pass.
    if verification.presence.is_intact() && verification.contents.is_intact() {
        return Ok(0);
    }
    Ok(1)
}

fn restore(
    snapshot: &str,
    into: &Path,
    project: Option<&Path>,
    into_non_empty: bool,
    json: bool,
) -> Result<i32> {
    let setup = setup(project)?;
    let target = open_target(&setup)?;
    let id = resolve(target.as_ref(), snapshot)?;
    let key = identity(setup.uid, "restoring")?;

    let restored = session::restore(
        target.as_ref(),
        &id,
        &key,
        into,
        &RestoreOptions { into_non_empty },
    )
    .map_err(|e| anyhow::anyhow!("{e}"))?;

    if json {
        println!(
            "{}",
            serde_json::json!({
                "snapshot": restored.id.as_str(),
                "files": restored.files,
                "directories": restored.directories,
                "symlinks": restored.symlinks,
                "bytes": restored.bytes,
                "verdict": restored.verdict.to_json(),
                "skippedAtBackup": restored.skipped_at_backup.len(),
            })
        );
    } else {
        println!(
            "snapshot {} restored into {} — {} file(s), {} director(ies), {} \
             symlink(s)",
            restored.id,
            into.display(),
            restored.files,
            restored.directories,
            restored.symlinks
        );
        println!("  {}", restored.verdict);
        for skipped in &restored.skipped_at_backup {
            println!(
                "  MISSING FROM THE BACKUP  {} — {}",
                skipped.path, skipped.why
            );
        }
    }
    if restored.verdict.is_intact() {
        return Ok(0);
    }
    // A restore that did not verify is not a restore that succeeded, and it
    // must not exit zero into a script that then deletes the source.
    Ok(1)
}

#[cfg(test)]
mod backup_tests;
