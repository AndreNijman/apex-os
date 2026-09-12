//! `apex vm` — P2-008's first-party virtualization UX, as a clap surface over
//! the shipped engine.
//!
//! A typed enum rather than a raw argument passthrough, for the reason
//! `DisposableCmd` and `EnvCmd` are: `apex vm create --help` documents the real
//! thing, and a typo is caught before a domain is defined. The engine
//! (`/usr/libexec/apex-vm`) still owns every decision; this only builds its
//! argv.
//!
//! ## Why that argv is worth pinning with tests
//!
//! Four of the flags below are not conveniences, they are the difference
//! between a VM that enforces something and one that reports that it does:
//!
//!   * `--secure-boot` / `--no-secure-boot` decides whether the guest's
//!     firmware verifies what it boots.
//!   * `--tpm` / `--no-tpm` decides whether a guest has a root of trust to
//!     seal anything to.
//!   * `--network` decides whether a machine that may be hostile can reach the
//!     network at all — and `bridge` is refused by the engine rather than
//!     accepted, because it would need a host bridge that outlives the VM.
//!   * `--share-ro` versus `--share` decides whether the guest can write to a
//!     host directory.
//!
//! A dropped flag here is not a compile error; it is a silent policy change
//! that produces a weaker VM than the user asked for. The tests at the bottom
//! assert each of those reaches the engine, and that the defaults are the safe
//! ones.
//!
//! ## Defaults live in the engine, not here
//!
//! `create` does not pass `--secure-boot` when the user did not type it: the
//! engine's default is already enforcing, and duplicating a default in two
//! files is how the two stop agreeing. `--no-secure-boot` IS passed, because
//! it is a departure from the default and a departure has to be explicit in
//! the argv for `apex vm info` to record it.

use clap::{Args, Subcommand};

#[derive(Subcommand)]
pub enum VmCmd {
    /// What the virtualization stack has, and what it is missing.
    ///
    /// The stack is userspace — qemu, libvirt, OVMF, swtpm, virtiofsd — and is
    /// deliberately not in the image; the KVM kernel modules are, because a
    /// kernel module cannot be added at runtime under Secure Boot and
    /// userspace can. This prints the one `apex install` line that adds the
    /// rest.
    Doctor,
    /// Define a VM and its disk.
    ///
    /// Secure Boot enforcing and an emulated TPM 2.0 by default, because the
    /// guest people actually want to run in 2026 refuses to install without
    /// both, and because a VM that quietly verifies nothing is a worse default
    /// than one that needs a flag.
    Create(CreateArgs),
    /// Every VM and the state it is in.
    List {
        #[arg(long)]
        json: bool,
    },
    /// What a VM was made from — the record, not the running domain.
    Info {
        #[arg(value_name = "NAME")]
        name: String,
    },
    /// Power a VM on.
    Start {
        #[arg(value_name = "NAME")]
        name: String,
    },
    /// Ask a VM to shut down.
    Stop {
        #[arg(value_name = "NAME")]
        name: String,
        /// Pull the plug instead of asking. The guest's filesystem may need a
        /// check afterwards.
        #[arg(long)]
        force: bool,
    },
    /// Attach to a VM's serial console.
    ///
    /// Serial, and only serial. There is no viewer and no `--graphics` to
    /// reach: a VM viewer is a second product — a window, a clipboard channel,
    /// a USB redirection path — and half of one is worse than none.
    Console {
        #[arg(value_name = "NAME")]
        name: String,
        /// Print what the VM has written to its console so far and exit,
        /// instead of attaching.
        #[arg(long)]
        log: bool,
    },
    /// Snapshots: the disk and the UEFI variable store, together.
    Snapshot {
        #[command(subcommand)]
        cmd: SnapshotCmd,
    },
    /// Host directories inside the guest, over virtiofs.
    Share {
        #[command(subcommand)]
        cmd: ShareCmd,
    },
    /// Pass a host USB device through to a guest.
    Usb {
        #[command(subcommand)]
        cmd: UsbCmd,
    },
    /// Run one task in a throwaway VM and delete the whole thing (P2-009).
    ///
    /// A different and stronger boundary than `apex disposable`, which is a
    /// container that can reach your home through `/run/host` and says so.
    /// Here the guest is another kernel with no view of the host filesystem:
    /// it sees a read-only volume of what you copied in, a blank volume to
    /// write to, and nothing else. It has NO network and there is no flag to
    /// give it one.
    ///
    /// Egress is by NAME. Only the filenames passed to `--egress` are read
    /// back out of the guest's volume, and only if `--egress-to` says where to
    /// put them; everything else the guest wrote is deleted with the VM.
    Run(RunArgs),
    /// Undefine a VM and delete its disk.
    Rm {
        #[arg(value_name = "NAME")]
        name: String,
        /// Keep the disk image. The domain is still undefined.
        #[arg(long)]
        keep_disk: bool,
    },
}

#[derive(Args)]
pub struct CreateArgs {
    #[arg(value_name = "NAME")]
    pub name: String,
    /// Memory in MiB.
    #[arg(long, value_name = "MIB")]
    pub memory: Option<u32>,
    /// Virtual CPUs.
    #[arg(long, value_name = "N")]
    pub cpus: Option<u32>,
    /// Disk size, like 20G.
    #[arg(long, value_name = "SIZE")]
    pub disk: Option<String>,
    /// Start from an existing qcow2 image instead of a blank disk. It is
    /// COPIED, so the original is never what the VM runs on.
    #[arg(long, value_name = "FILE")]
    pub import: Option<String>,
    /// Start from this UEFI variable store, so the guest enforces Secure Boot
    /// against the keys IT carries rather than the stock ones.
    ///
    /// This is how a kernel signed with a key that is not in the shipped
    /// firmware gets tested before it is published.
    #[arg(long, value_name = "FILE")]
    pub uefi_vars: Option<String>,
    /// Do not enforce Secure Boot. For a guest whose kernel is not signed.
    #[arg(long, conflicts_with = "secure_boot")]
    pub no_secure_boot: bool,
    /// Enforce Secure Boot. Already the default; accepted so a script can say
    /// so out loud.
    #[arg(long)]
    pub secure_boot: bool,
    /// No emulated TPM.
    #[arg(long, conflicts_with = "tpm")]
    pub no_tpm: bool,
    /// Give the guest an emulated TPM 2.0. Already the default.
    #[arg(long)]
    pub tpm: bool,
    /// `user` for outbound NAT with no host bridge, or `none` for a guest with
    /// no network at all.
    ///
    /// `bridge` is refused by the engine: it needs libvirt's system daemon and
    /// its `default` network, which is a host bridge and a dnsmasq that
    /// outlive the VM that asked for them.
    #[arg(long, value_name = "MODE")]
    pub network: Option<String>,
    /// Share a host directory with the guest, as HOSTPATH:TAG. Repeatable.
    #[arg(long, value_name = "PATH:TAG")]
    pub share: Vec<String>,
    /// Share a host directory read-only. Repeatable.
    #[arg(long, value_name = "PATH:TAG")]
    pub share_ro: Vec<String>,
    /// Pass a host USB device through, as `lsusb` prints it: VENDOR:PRODUCT.
    /// Repeatable. The HOST loses the device while the VM holds it.
    #[arg(long, value_name = "VENDOR:PRODUCT")]
    pub usb: Vec<String>,
}

#[derive(Args)]
pub struct RunArgs {
    /// The guest disk to run the task in. It is COPIED, so the original is
    /// never what runs.
    ///
    /// Required, and the engine builds no guest for you: the image must
    /// already carry what the task needs, must mount the filesystem labelled
    /// APEXIN and run `task.sh` from it, and must write anything it wants to
    /// hand back to the filesystem labelled APEXOUT.
    #[arg(long, value_name = "DISK")]
    pub image: String,
    /// A host path copied into the read-only volume. Repeatable. Nothing is
    /// copied in by default.
    #[arg(long, value_name = "PATH")]
    pub copy_in: Vec<String>,
    /// A filename the task is allowed to hand back. Repeatable.
    ///
    /// A plain filename: no directory component and no wildcard. A wildcard
    /// would let the guest decide what leaves by choosing names, which is the
    /// defect this whole verb exists not to have.
    #[arg(long, value_name = "NAME")]
    pub egress: Vec<String>,
    /// Where the nominated files land. WITHOUT THIS, NOTHING LEAVES,
    /// whatever `--egress` says.
    #[arg(long, value_name = "DIR")]
    pub egress_to: Option<String>,
    /// Memory in MiB.
    #[arg(long, value_name = "MIB")]
    pub memory: Option<u32>,
    /// Virtual CPUs.
    #[arg(long, value_name = "N")]
    pub cpus: Option<u32>,
    /// How long to wait for the guest to power off, in seconds.
    #[arg(long, value_name = "SECS")]
    pub timeout: Option<u32>,
    /// Name the VM instead of generating one. Prefixed `run-` either way.
    #[arg(long, value_name = "NAME")]
    pub name: Option<String>,
    /// The command the guest runs. Omit nothing: this is the task.
    #[arg(trailing_var_arg = true, allow_hyphen_values = true, required = true)]
    pub command: Vec<String>,
}

#[derive(Subcommand)]
pub enum SnapshotCmd {
    /// Take a snapshot of the disk and the UEFI variables together.
    Create {
        #[arg(value_name = "NAME")]
        name: String,
        #[arg(value_name = "SNAPSHOT")]
        snapshot: String,
        #[arg(long, value_name = "TEXT")]
        description: Option<String>,
    },
    /// Snapshots a VM has.
    List {
        #[arg(value_name = "NAME")]
        name: String,
    },
    /// Put a VM back at a snapshot — disk and UEFI variables both.
    Revert {
        #[arg(value_name = "NAME")]
        name: String,
        #[arg(value_name = "SNAPSHOT")]
        snapshot: String,
    },
    /// Delete a snapshot.
    Delete {
        #[arg(value_name = "NAME")]
        name: String,
        #[arg(value_name = "SNAPSHOT")]
        snapshot: String,
    },
}

#[derive(Subcommand)]
pub enum ShareCmd {
    /// Add a host directory to a VM, as HOSTPATH:TAG.
    Add {
        #[arg(value_name = "NAME")]
        name: String,
        #[arg(value_name = "PATH:TAG")]
        spec: String,
        /// The guest cannot write to it.
        #[arg(long)]
        readonly: bool,
    },
    /// Remove a share by its tag.
    Remove {
        #[arg(value_name = "NAME")]
        name: String,
        #[arg(value_name = "TAG")]
        tag: String,
    },
    /// Shares a VM has.
    List {
        #[arg(value_name = "NAME")]
        name: String,
    },
}

#[derive(Subcommand)]
pub enum UsbCmd {
    /// Give a VM a host USB device. The HOST loses it until the VM gives it
    /// back.
    Attach {
        #[arg(value_name = "NAME")]
        name: String,
        #[arg(value_name = "VENDOR:PRODUCT")]
        id: String,
        /// Attach it to a running VM as well as to its definition.
        #[arg(long)]
        live: bool,
    },
    /// Take a host USB device back from a VM.
    Detach {
        #[arg(value_name = "NAME")]
        name: String,
        #[arg(value_name = "VENDOR:PRODUCT")]
        id: String,
    },
    /// USB devices a VM holds.
    List {
        #[arg(value_name = "NAME")]
        name: String,
    },
}

/// Build the engine argv.
pub fn argv(cmd: VmCmd) -> Vec<String> {
    match cmd {
        VmCmd::Doctor => vec!["doctor".to_string()],
        VmCmd::Create(a) => create_argv(a),
        VmCmd::List { json } => {
            let mut v = vec!["list".to_string()];
            if json {
                v.push("--json".to_string());
            }
            v
        }
        VmCmd::Info { name } => vec!["info".to_string(), name],
        VmCmd::Start { name } => vec!["start".to_string(), name],
        VmCmd::Stop { name, force } => {
            let mut v = vec!["stop".to_string(), name];
            if force {
                v.push("--force".to_string());
            }
            v
        }
        VmCmd::Console { name, log } => {
            let mut v = vec!["console".to_string(), name];
            if log {
                v.push("--log".to_string());
            }
            v
        }
        VmCmd::Run(a) => run_argv(a),
        VmCmd::Snapshot { cmd } => snapshot_argv(cmd),
        VmCmd::Share { cmd } => share_argv(cmd),
        VmCmd::Usb { cmd } => usb_argv(cmd),
        VmCmd::Rm { name, keep_disk } => {
            let mut v = vec!["rm".to_string(), name];
            if keep_disk {
                v.push("--keep-disk".to_string());
            }
            v
        }
    }
}

fn create_argv(a: CreateArgs) -> Vec<String> {
    let mut v = vec!["create".to_string(), a.name];
    if let Some(m) = a.memory {
        v.push("--memory".to_string());
        v.push(m.to_string());
    }
    if let Some(c) = a.cpus {
        v.push("--cpus".to_string());
        v.push(c.to_string());
    }
    if let Some(d) = a.disk {
        v.push("--disk".to_string());
        v.push(d);
    }
    if let Some(i) = a.import {
        v.push("--import".to_string());
        v.push(i);
    }
    if let Some(u) = a.uefi_vars {
        v.push("--uefi-vars".to_string());
        v.push(u);
    }
    // Only the departures are passed. The engine's defaults are enforcing
    // Secure Boot and an emulated TPM; re-stating them here would be a second
    // place for the default to live, and the two would eventually disagree.
    if a.no_secure_boot {
        v.push("--no-secure-boot".to_string());
    } else if a.secure_boot {
        v.push("--secure-boot".to_string());
    }
    if a.no_tpm {
        v.push("--no-tpm".to_string());
    } else if a.tpm {
        v.push("--tpm".to_string());
    }
    if let Some(n) = a.network {
        v.push("--network".to_string());
        v.push(n);
    }
    for s in a.share {
        v.push("--share".to_string());
        v.push(s);
    }
    for s in a.share_ro {
        v.push("--share-ro".to_string());
        v.push(s);
    }
    for u in a.usb {
        v.push("--usb".to_string());
        v.push(u);
    }
    v
}

/// Build `apex vm run`'s argv.
///
/// The `--` before the command is not cosmetic: without it the engine cannot
/// tell `apex vm run -- make -j4` from a flag of its own, and clap has already
/// stripped the separator the user typed.
fn run_argv(a: RunArgs) -> Vec<String> {
    let mut v = vec!["run".to_string(), "--image".to_string(), a.image];
    if let Some(n) = a.name {
        v.push("--name".to_string());
        v.push(n);
    }
    if let Some(m) = a.memory {
        v.push("--memory".to_string());
        v.push(m.to_string());
    }
    if let Some(c) = a.cpus {
        v.push("--cpus".to_string());
        v.push(c.to_string());
    }
    if let Some(t) = a.timeout {
        v.push("--timeout".to_string());
        v.push(t.to_string());
    }
    for p in a.copy_in {
        v.push("--copy-in".to_string());
        v.push(p);
    }
    for e in a.egress {
        v.push("--egress".to_string());
        v.push(e);
    }
    if let Some(d) = a.egress_to {
        v.push("--egress-to".to_string());
        v.push(d);
    }
    v.push("--".to_string());
    v.extend(a.command);
    v
}

fn snapshot_argv(cmd: SnapshotCmd) -> Vec<String> {
    match cmd {
        SnapshotCmd::Create {
            name,
            snapshot,
            description,
        } => {
            let mut v = vec!["snapshot".to_string(), "create".to_string(), name, snapshot];
            if let Some(d) = description {
                v.push("--description".to_string());
                v.push(d);
            }
            v
        }
        SnapshotCmd::List { name } => vec!["snapshot".to_string(), "list".to_string(), name],
        SnapshotCmd::Revert { name, snapshot } => {
            vec!["snapshot".to_string(), "revert".to_string(), name, snapshot]
        }
        SnapshotCmd::Delete { name, snapshot } => {
            vec!["snapshot".to_string(), "delete".to_string(), name, snapshot]
        }
    }
}

fn share_argv(cmd: ShareCmd) -> Vec<String> {
    match cmd {
        ShareCmd::Add {
            name,
            spec,
            readonly,
        } => {
            let mut v = vec!["share".to_string(), "add".to_string(), name, spec];
            if readonly {
                v.push("--readonly".to_string());
            }
            v
        }
        ShareCmd::Remove { name, tag } => {
            vec!["share".to_string(), "remove".to_string(), name, tag]
        }
        ShareCmd::List { name } => vec!["share".to_string(), "list".to_string(), name],
    }
}

fn usb_argv(cmd: UsbCmd) -> Vec<String> {
    match cmd {
        UsbCmd::Attach { name, id, live } => {
            let mut v = vec!["usb".to_string(), "attach".to_string(), name, id];
            if live {
                v.push("--live".to_string());
            }
            v
        }
        UsbCmd::Detach { name, id } => vec!["usb".to_string(), "detach".to_string(), name, id],
        UsbCmd::List { name } => vec!["usb".to_string(), "list".to_string(), name],
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::{CommandFactory, Parser};

    #[derive(Parser)]
    struct Harness {
        #[command(subcommand)]
        cmd: VmCmd,
    }

    fn build(args: &[&str]) -> Vec<String> {
        let mut full = vec!["vm"];
        full.extend_from_slice(args);
        argv(Harness::try_parse_from(full).expect("parses").cmd)
    }

    fn refused(args: &[&str]) -> bool {
        let mut full = vec!["vm"];
        full.extend_from_slice(args);
        Harness::try_parse_from(full).is_err()
    }

    #[test]
    fn a_plain_create_asks_for_no_weakening() {
        // THE default that matters. The engine creates an enforcing,
        // TPM-carrying VM when it is told nothing, and this asserts the CLI
        // never says otherwise on the user's behalf. A `--no-secure-boot` that
        // appeared here by accident would be a silently weaker machine that
        // still reported success.
        let a = build(&["create", "box"]);
        assert_eq!(a, vec!["create", "box"]);
        assert!(!a.iter().any(|x| x == "--no-secure-boot"));
        assert!(!a.iter().any(|x| x == "--no-tpm"));
    }

    #[test]
    fn every_weakening_is_explicit_in_the_argv() {
        // The two departures from the defaults, which must be visible in the
        // argv so `apex vm info` records them and so a script that weakens a
        // VM says so in its own source.
        let a = build(&["create", "box", "--no-secure-boot", "--no-tpm"]);
        assert!(a.contains(&"--no-secure-boot".to_string()));
        assert!(a.contains(&"--no-tpm".to_string()));
    }

    #[test]
    fn secure_boot_and_no_secure_boot_cannot_both_be_asked_for() {
        // Not a style point: an argv carrying both would have the engine's
        // last-flag-wins decide the security posture of the machine, which is
        // not a decision an argument order should make.
        assert!(refused(&["create", "box", "--secure-boot", "--no-secure-boot"]));
        assert!(refused(&["create", "box", "--tpm", "--no-tpm"]));
    }

    #[test]
    fn the_network_mode_reaches_the_engine_verbatim() {
        // Including the one the engine refuses. The CLI does not pre-judge
        // `bridge`: the refusal, and the sentence explaining what a bridge
        // would cost, live in one place.
        assert!(build(&["create", "box", "--network", "none"]).contains(&"none".to_string()));
        assert!(build(&["create", "box", "--network", "bridge"]).contains(&"bridge".to_string()));
    }

    #[test]
    fn read_only_shares_stay_read_only() {
        // --share and --share-ro are different flags all the way down. Folding
        // them into one with a boolean is how a read-only share becomes
        // writable in a refactor.
        let a = build(&[
            "create", "box", "--share", "/srv/data:data", "--share-ro", "/srv/ref:ref",
        ]);
        let i_rw = a.iter().position(|x| x == "--share").expect("--share");
        let i_ro = a.iter().position(|x| x == "--share-ro").expect("--share-ro");
        assert_eq!(a[i_rw + 1], "/srv/data:data");
        assert_eq!(a[i_ro + 1], "/srv/ref:ref");
    }

    #[test]
    fn repeatable_options_all_arrive() {
        // A `Vec` that reaches the engine as only its last element is a silent
        // loss: two shares asked for, one mounted, no error.
        let a = build(&[
            "create", "box", "--share", "/a:a", "--share", "/b:b", "--usb", "046d:c52b", "--usb",
            "1050:0407",
        ]);
        assert_eq!(a.iter().filter(|x| *x == "--share").count(), 2);
        assert!(a.contains(&"/a:a".to_string()) && a.contains(&"/b:b".to_string()));
        assert_eq!(a.iter().filter(|x| *x == "--usb").count(), 2);
        assert!(a.contains(&"046d:c52b".to_string()) && a.contains(&"1050:0407".to_string()));
    }

    #[test]
    fn the_uefi_variable_store_is_passed_through_for_the_engine_to_validate() {
        let a = build(&["create", "box", "--uefi-vars", "/tmp/vars.fd"]);
        let i = a.iter().position(|x| x == "--uefi-vars").expect("flag");
        assert_eq!(a[i + 1], "/tmp/vars.fd");
    }

    #[test]
    fn a_snapshot_names_its_vm_before_its_snapshot() {
        // Argument ORDER is the contract with the engine here, and getting it
        // backwards would snapshot a domain named after the snapshot.
        let a = build(&["snapshot", "create", "box", "before-update"]);
        assert_eq!(a, vec!["snapshot", "create", "box", "before-update"]);
        let r = build(&["snapshot", "revert", "box", "before-update"]);
        assert_eq!(r, vec!["snapshot", "revert", "box", "before-update"]);
    }

    #[test]
    fn stopping_hard_is_a_different_argv_from_stopping_politely() {
        assert_eq!(build(&["stop", "box"]), vec!["stop", "box"]);
        assert_eq!(build(&["stop", "box", "--force"]), vec!["stop", "box", "--force"]);
    }

    #[test]
    fn removing_a_vm_keeps_its_disk_only_when_asked() {
        // The default deletes. `--keep-disk` is the only thing standing
        // between "I removed the VM" and a directory of orphaned qcow2s.
        assert_eq!(build(&["rm", "box"]), vec!["rm", "box"]);
        assert!(build(&["rm", "box", "--keep-disk"]).contains(&"--keep-disk".to_string()));
    }

    #[test]
    fn usb_attach_is_persistent_unless_live_is_asked_for() {
        let a = build(&["usb", "attach", "box", "046d:c52b"]);
        assert_eq!(a, vec!["usb", "attach", "box", "046d:c52b"]);
        assert!(build(&["usb", "attach", "box", "046d:c52b", "--live"])
            .contains(&"--live".to_string()));
    }

    #[test]
    fn a_disposable_run_lets_nothing_out_unless_both_halves_are_typed() {
        // THE default that matters for P2-009. Two separate things have to be
        // said before a byte leaves the guest: WHICH files, and WHERE. A run
        // that named neither must produce an argv with neither.
        let a = build(&["run", "--image", "/g.qcow2", "--", "make"]);
        assert_eq!(a, vec!["run", "--image", "/g.qcow2", "--", "make"]);
        assert!(!a.iter().any(|x| x == "--egress"));
        assert!(!a.iter().any(|x| x == "--egress-to"));
    }

    #[test]
    fn every_nominated_file_reaches_the_engine_by_name() {
        // A `Vec` that arrives as only its last element would silently drop a
        // file the task was told to hand back, and the user would see an
        // empty result with no error.
        let a = build(&[
            "run", "--image", "/g.qcow2", "--egress", "report.json", "--egress", "log.txt",
            "--egress-to", "/home/u/out", "--", "make",
        ]);
        assert_eq!(a.iter().filter(|x| *x == "--egress").count(), 2);
        assert!(a.contains(&"report.json".to_string()));
        assert!(a.contains(&"log.txt".to_string()));
        let i = a.iter().position(|x| x == "--egress-to").expect("--egress-to");
        assert_eq!(a[i + 1], "/home/u/out");
    }

    #[test]
    fn the_task_command_keeps_its_separator() {
        // Without the `--`, the engine reads `-j4` as one of its own flags.
        let a = build(&["run", "--image", "/g.qcow2", "--", "make", "-j4"]);
        assert_eq!(&a[a.len() - 3..], &["--", "make", "-j4"]);
    }

    #[test]
    fn a_disposable_run_can_never_hand_the_engine_a_network_flag() {
        // `apex vm run` offers no way to give the guest an interface, and this
        // asserts the property that actually matters rather than the shape of
        // a parse error: whatever the user types, nothing before the `--`
        // separator is a --network flag, so the engine cannot receive one as
        // its own. (After the separator it is just words in the task's command
        // line, which is what a trailing var-arg is for.)
        //
        // Measured, not assumed: `--network user` typed before the separator
        // is absorbed into the trailing command by clap rather than rejected,
        // which is exactly why asserting "it is refused" would have been
        // asserting the wrong thing.
        let a = build(&["run", "--image", "/g.qcow2", "--network", "user", "--", "make"]);
        let sep = a.iter().position(|x| x == "--").expect("a separator");
        assert!(
            !a[..sep].iter().any(|x| x == "--network"),
            "a --network reached the engine as its own flag: {a:?}"
        );
        // And the option does not exist on the verb at all.
        let help = {
            #[derive(Parser)]
            struct H {
                #[command(subcommand)]
                _cmd: VmCmd,
            }
            H::command()
                .find_subcommand("run")
                .expect("run exists")
                .clone()
                .render_long_help()
                .to_string()
        };
        assert!(!help.contains("--network"), "run grew a --network option");
    }

    #[test]
    fn a_run_with_no_command_is_refused_rather_than_booting_an_idle_vm() {
        assert!(refused(&["run", "--image", "/g.qcow2"]));
    }

    #[test]
    fn there_is_no_viewer_verb_and_no_graphics_flag() {
        // The headless rule, asserted rather than trusted to review. A
        // `--graphics` or a `viewer` verb arriving in a later change would
        // open a window on somebody's desktop from a CLI whose whole contract
        // is that it does not.
        assert!(refused(&["viewer", "box"]));
        assert!(refused(&["create", "box", "--graphics", "spice"]));
        assert!(refused(&["console", "box", "--vnc"]));
    }
}
