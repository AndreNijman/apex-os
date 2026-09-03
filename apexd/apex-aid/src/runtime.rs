//! Finding, confining, starting and stopping the inference runtime.
//!
//! The runtime is **not in the image** — see `apexd_core::ai`'s module docs —
//! so the first job here is to look for one and, when there is none, produce
//! the exact command that installs it. Nothing in this file downloads, builds
//! or unpacks anything: `AGENTS.md` forbids a second package mechanism and P1
//! already shipped both of the ones the hint names.
//!
//! ── Why the backend is confined, and what that buys ────────────────────────
//!
//! The backend is a large C++ program that maps untrusted weights, parses HTTP
//! from anything that can open the API socket, and holds the user's prompts. It
//! needs: its own binary and libraries, one read-only model file, one socket
//! directory, and a GPU device. It does **not** need the network, the user's
//! home, or a writable filesystem.
//!
//! So it is started under `bubblewrap` with `--unshare-net`, and that flag is
//! the load-bearing one. A path-named `AF_UNIX` socket is unaffected by network
//! namespaces — only the abstract namespace is per-netns — so the daemon still
//! reaches the backend through the filesystem while the backend has no
//! reachable network stack at all. That makes "no local account can find a port
//! to connect to" true by construction rather than by the planner having got
//! `--host` right, and it is the second line behind
//! `apexd_core::ai::plan_launch`'s refusal to emit a TCP address.
//!
//! `bubblewrap` is the same security dependency the agent runtime uses, present
//! because flatpak pulls it in and asserted in `Containerfile.base`. When it is
//! genuinely absent the backend runs unconfined and the daemon **says so** in
//! `apex ai status` — a silent downgrade is what `AGENTS.md` forbids, and
//! refusing to serve at all would be worse than a stated weaker posture for a
//! process that is still a per-user, unprivileged child.

use std::io;
use std::os::unix::net::UnixStream;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};
use apexd_core::ai::{Backend, LaunchPlan};

/// How long to wait for the backend to bind its socket.
///
/// Three minutes. A 4 GiB model already in the page cache is ready in a second
/// or two; the same file cold from a spinning disk, or one being paged into
/// VRAM over PCIe, is minutes. The bound exists so a runtime that will never
/// come up is reported rather than waited on forever — and the wait aborts the
/// moment the child exits, which is the case that actually happens.
const READY_TIMEOUT: Duration = Duration::from_secs(180);

/// How often to try connecting while waiting.
const READY_POLL: Duration = Duration::from_millis(100);

/// How long a stopped backend gets to exit before it is killed.
const STOP_GRACE: Duration = Duration::from_secs(5);

/// `bubblewrap`. The same path the agent runtime asserts in `Containerfile.base`.
const BWRAP: &str = "/usr/bin/bwrap";

/// Whether the backend will be confined.
pub fn confinement_available() -> bool {
    Path::new(BWRAP).is_file()
}

/// Paths the confined backend must be able to see.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Exposed<'a> {
    /// The model blob. Bound read-only — the store is root-owned and `0444`
    /// anyway, so this is the second of two independent reasons the inference
    /// process cannot alter a model.
    pub model: &'a Path,
    /// The socket directory, bound writable so the backend can create its
    /// listener there.
    pub socket_dir: &'a Path,
    /// Device nodes to pass through, already filtered to ones that exist.
    pub devices: Vec<PathBuf>,
}

/// Which device nodes a backend needs, given its compute backend.
///
/// Only paths that exist are returned: `--dev-bind` of a missing node makes
/// `bwrap` fail, so a CUDA machine's node list must not be asked for on an AMD
/// one. Verified labels on the katana, an SELinux-enforcing APEX host:
/// `/dev/nvidia*` are `xserver_misc_device_t` and `/dev/dri/renderD*` are
/// `dri_device_t`, both of which an unconfined user process may open — which is
/// why this needs no policy module of its own.
pub fn device_nodes(backend: Backend) -> Vec<PathBuf> {
    let candidates: &[&str] = match backend {
        Backend::Cuda => &[
            "/dev/nvidiactl",
            "/dev/nvidia0",
            "/dev/nvidia1",
            "/dev/nvidia-uvm",
            "/dev/nvidia-uvm-tools",
            "/dev/nvidia-modeset",
        ],
        // ROCm needs the compute device and the render nodes.
        Backend::Rocm => &["/dev/kfd", "/dev/dri"],
        // Vulkan is render nodes only.
        Backend::Vulkan => &["/dev/dri"],
        Backend::Cpu => &[],
    };
    candidates
        .iter()
        .map(PathBuf::from)
        .filter(|p| p.exists())
        .collect()
}

/// Build the `bwrap` argv that runs `plan` confined.
///
/// Pure, so every flag can be asserted without a sandbox. The order matters and
/// is not arbitrary: `--dev /dev` replaces the device tree, so the `--dev-bind`
/// entries have to come after it or they are thrown away — a mistake whose
/// symptom is "CUDA cannot find a device" three layers from its cause.
pub fn confine_argv(plan: &LaunchPlan, exposed: &Exposed<'_>) -> Vec<String> {
    let mut a: Vec<String> = vec![BWRAP.to_string()];

    a.extend(
        [
            // No network. THE flag: see the module docs.
            "--unshare-net",
            "--unshare-ipc",
            "--unshare-uts",
            "--unshare-pid",
            // The backend must not outlive the daemon that is supposed to
            // unload it.
            "--die-with-parent",
            // A new session, so the backend cannot reach a controlling
            // terminal.
            "--new-session",
            // A read-only system. /usr carries the runtime and its libraries —
            // the system-extension merge is already applied, so a
            // sysext-installed llama-server and its dependencies are both
            // inside it. /etc carries ld.so.cache, without which nothing
            // dynamically linked starts.
            "--ro-bind",
            "/usr",
            "/usr",
            "--ro-bind",
            "/etc",
            "/etc",
        ]
        .iter()
        .map(|s| s.to_string()),
    );
    for link in ["lib", "lib64", "bin", "sbin"] {
        a.push("--symlink".into());
        a.push(format!("usr/{link}"));
        a.push(format!("/{link}"));
    }
    a.extend(
        ["--proc", "/proc", "--dev", "/dev", "--tmpfs", "/tmp"]
            .iter()
            .map(|s| s.to_string()),
    );

    // The two paths that are this backend's job.
    a.push("--ro-bind".into());
    a.push(exposed.model.display().to_string());
    a.push(exposed.model.display().to_string());
    a.push("--bind".into());
    a.push(exposed.socket_dir.display().to_string());
    a.push(exposed.socket_dir.display().to_string());

    // Devices, AFTER --dev.
    for d in &exposed.devices {
        a.push("--dev-bind".into());
        a.push(d.display().to_string());
        a.push(d.display().to_string());
    }

    a.push("--".into());
    a.push(plan.program.clone());
    a.extend(plan.argv.iter().cloned());
    a
}

/// A running backend.
pub struct Running {
    /// The model it holds.
    pub model: String,
    /// The socket it listens on.
    pub socket: PathBuf,
    /// Whether it is confined.
    pub confined: bool,
    /// The plan that started it, for `apex ai status`.
    pub plan: LaunchPlan,
    child: Child,
    /// Process group, so stopping reaches `bwrap` and whatever it started.
    pgid: libc::pid_t,
    started: Instant,
}

impl Running {
    /// How long it has been up.
    pub fn uptime(&self) -> Duration {
        self.started.elapsed()
    }

    /// Whether the child is still alive. Reaps it if not, so a backend that
    /// crashed does not linger as a zombie claiming to be loaded.
    pub fn alive(&mut self) -> bool {
        match self.child.try_wait() {
            Ok(None) => true,
            Ok(Some(status)) => {
                eprintln!("apex-aid: backend exited: {status}");
                false
            }
            Err(e) => {
                eprintln!("apex-aid: cannot check the backend: {e}");
                false
            }
        }
    }
}

/// Start a backend and wait for it to be reachable.
///
/// The socket file is removed first: a path left by a previous backend cannot
/// be bound over, and the failure reads as "the runtime will not start".
pub fn start(
    plan: &LaunchPlan,
    model: &str,
    socket: &Path,
    exposed: &Exposed<'_>,
    confine: bool,
) -> Result<Running> {
    let _ = std::fs::remove_file(socket);

    let argv: Vec<String> = if confine {
        confine_argv(plan, exposed)
    } else {
        let mut a = vec![plan.program.clone()];
        a.extend(plan.argv.iter().cloned());
        a
    };
    let (program, args) = argv
        .split_first()
        .context("a launch plan with no program")?;

    let mut cmd = Command::new(program);
    cmd.args(args);

    // The environment is CLEARED and rebuilt from a declared list, the rule the
    // agent runtime's sandbox follows. Inheriting the user's environment would
    // let `LLAMA_ARG_*` — which llama-server reads for most of its flags —
    // silently override the plan, and would hand the backend every token and
    // API key in the shell.
    cmd.env_clear();
    cmd.env("PATH", "/usr/bin:/usr/sbin");
    // A locale-independent child, so log parsing and number formatting cannot
    // depend on the user's settings.
    cmd.env("LC_ALL", "C");
    for (k, v) in &plan.env {
        cmd.env(k, v);
    }

    // No terminal, and no way to read one. stdout and stderr go to this
    // process's, which under `systemd --user` is the journal.
    cmd.stdin(Stdio::null());
    cmd.stdout(Stdio::inherit());
    cmd.stderr(Stdio::inherit());

    // Its own process group, so stopping it signals `bwrap` and everything
    // beneath rather than only the direct child.
    // Safe: setpgid(0, 0) in the forked child is async-signal-safe and touches
    // nothing this process owns.
    unsafe {
        cmd.pre_exec(|| {
            if libc::setpgid(0, 0) != 0 {
                return Err(io::Error::last_os_error());
            }
            Ok(())
        });
    }

    let child = cmd
        .spawn()
        .with_context(|| format!("starting {}", plan.program))?;
    let pgid = child.id() as libc::pid_t;

    let mut running = Running {
        model: model.to_string(),
        socket: socket.to_path_buf(),
        confined: confine,
        plan: plan.clone(),
        child,
        pgid,
        started: Instant::now(),
    };

    match wait_ready(&mut running) {
        Ok(()) => Ok(running),
        Err(e) => {
            stop(&mut running);
            Err(e)
        }
    }
}

/// Poll until the backend's socket accepts a connection.
///
/// Checks whether the child is still alive on every pass. Without that, a
/// runtime that exits immediately — a missing library, an unreadable model, a
/// GPU that will not initialise — would be waited on for three minutes and then
/// reported as a timeout, hiding the real error that is already in the journal.
fn wait_ready(running: &mut Running) -> Result<()> {
    let deadline = Instant::now() + READY_TIMEOUT;
    loop {
        if !running.alive() {
            bail!(
                "{} exited before it was ready; its output is in `journalctl --user -u apex-aid`",
                running.plan.program
            );
        }
        if UnixStream::connect(&running.socket).is_ok() {
            return Ok(());
        }
        if Instant::now() >= deadline {
            bail!(
                "{} did not bind {} within {}s",
                running.plan.program,
                running.socket.display(),
                READY_TIMEOUT.as_secs()
            );
        }
        std::thread::sleep(READY_POLL);
    }
}

/// Stop a backend, releasing its VRAM.
///
/// `SIGTERM` to the process group, then `SIGKILL` after [`STOP_GRACE`]. The
/// group rather than the pid because with confinement the direct child is
/// `bwrap`; `--die-with-parent` and the pid namespace would eventually clean up
/// after it, but "eventually" is not good enough for a function whose whole
/// purpose is that the VRAM is free when it returns.
pub fn stop(running: &mut Running) {
    signal_group(running.pgid, libc::SIGTERM);
    let deadline = Instant::now() + STOP_GRACE;
    while Instant::now() < deadline {
        match running.child.try_wait() {
            Ok(Some(_)) => {
                let _ = std::fs::remove_file(&running.socket);
                return;
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(50)),
            Err(_) => break,
        }
    }
    eprintln!(
        "apex-aid: backend did not stop within {}s; killing it",
        STOP_GRACE.as_secs()
    );
    signal_group(running.pgid, libc::SIGKILL);
    let _ = running.child.wait();
    let _ = std::fs::remove_file(&running.socket);
}

/// Signal a whole process group.
fn signal_group(pgid: libc::pid_t, sig: libc::c_int) {
    if pgid <= 1 {
        // Never `kill(-1, …)`, which means every process this user can signal.
        eprintln!("apex-aid: refusing to signal process group {pgid}");
        return;
    }
    // Safe: kill() with a negative pid signals the group and cannot corrupt
    // memory. The guard above is what keeps it from meaning "everything".
    unsafe {
        libc::kill(-pgid, sig);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use apexd_core::ai::{
        plan_fit, plan_launch, select_backend, Accel, Device, LaunchRequest, Listen, Runtime,
    };

    fn a_plan() -> LaunchPlan {
        let fit = plan_fit(4096, 32, 32, 8192, 7628);
        let choice = select_backend(
            &Accel { cuda: true, rocm: false, vulkan: true },
            &[Device { index: 0, name: "gpu".into(), total_mib: 8192, used_mib: 52 }],
            None,
        )
        .unwrap();
        plan_launch(&LaunchRequest {
            runtime: Runtime::LlamaCpp,
            program: "/usr/bin/llama-server",
            model_path: Path::new("/var/lib/apex/ai/models/blobs/sha256-aa"),
            listen: Listen::Unix(PathBuf::from("/run/user/1000/apex-ai/backend.sock")),
            fit: &fit,
            choice: &choice,
            threads: 8,
        })
        .unwrap()
    }

    fn exposed<'a>(model: &'a Path, dir: &'a Path) -> Exposed<'a> {
        Exposed { model, socket_dir: dir, devices: vec![PathBuf::from("/dev/nvidiactl")] }
    }

    #[test]
    fn the_confined_backend_has_no_network() {
        // THE assertion. Without --unshare-net the "no port another account can
        // open" claim rests entirely on the planner having got --host right.
        let plan = a_plan();
        let argv = confine_argv(
            &plan,
            &exposed(Path::new("/var/lib/apex/ai/models/blobs/sha256-aa"), Path::new("/run/x")),
        );
        assert!(argv.contains(&"--unshare-net".to_string()), "{argv:?}");
    }

    #[test]
    fn device_binds_come_after_dev_or_they_are_discarded() {
        // `--dev /dev` replaces the device tree. A --dev-bind before it is
        // silently undone, and the symptom is "CUDA found no device".
        let plan = a_plan();
        let argv = confine_argv(&plan, &exposed(Path::new("/m.gguf"), Path::new("/run/x")));
        let dev = argv.iter().position(|a| a == "--dev").expect("--dev is present");
        let bind = argv
            .iter()
            .position(|a| a == "--dev-bind")
            .expect("--dev-bind is present");
        assert!(bind > dev, "--dev-bind at {bind} precedes --dev at {dev}: {argv:?}");
    }

    #[test]
    fn the_model_is_bound_read_only_and_the_socket_directory_is_not() {
        let plan = a_plan();
        let argv = confine_argv(&plan, &exposed(Path::new("/m.gguf"), Path::new("/run/x")));
        // Exact triples, not "contains": a --bind where --ro-bind belongs would
        // make the store writable by the inference process, which is the thing
        // the whole store design refuses.
        assert!(
            argv.windows(3).any(|w| w == ["--ro-bind", "/m.gguf", "/m.gguf"]),
            "{argv:?}"
        );
        assert!(
            argv.windows(3).any(|w| w == ["--bind", "/run/x", "/run/x"]),
            "{argv:?}"
        );
        assert!(
            !argv.windows(3).any(|w| w == ["--bind", "/m.gguf", "/m.gguf"]),
            "the model must never be bound writable: {argv:?}"
        );
    }

    #[test]
    fn the_system_is_read_only_and_home_is_never_bound() {
        let plan = a_plan();
        let argv = confine_argv(&plan, &exposed(Path::new("/m.gguf"), Path::new("/run/x")));
        assert!(argv.windows(3).any(|w| w == ["--ro-bind", "/usr", "/usr"]), "{argv:?}");
        assert!(argv.windows(3).any(|w| w == ["--ro-bind", "/etc", "/etc"]), "{argv:?}");
        // The backend has no business in the user's home. It gets one model
        // file and one socket directory.
        for a in &argv {
            assert!(!a.contains("/home/"), "home is bound into the sandbox: {a}");
            assert!(!a.contains("/var/home/"), "home is bound into the sandbox: {a}");
        }
        // And there is no writable bind other than the socket directory.
        let writable: Vec<&String> = argv
            .iter()
            .enumerate()
            .filter(|(i, a)| *a == "--bind" && argv.get(i + 1).is_some())
            .map(|(i, _)| &argv[i + 1])
            .collect();
        assert_eq!(writable, vec!["/run/x"], "unexpected writable bind: {argv:?}");
    }

    #[test]
    fn the_program_and_its_arguments_come_after_a_double_dash() {
        let plan = a_plan();
        let argv = confine_argv(&plan, &exposed(Path::new("/m.gguf"), Path::new("/run/x")));
        let dd = argv.iter().position(|a| a == "--").expect("--");
        assert_eq!(argv[dd + 1], plan.program);
        assert_eq!(&argv[dd + 2..], &plan.argv[..]);
    }

    #[test]
    fn the_confinement_argv_is_pure() {
        let plan = a_plan();
        let e = exposed(Path::new("/m.gguf"), Path::new("/run/x"));
        assert_eq!(confine_argv(&plan, &e), confine_argv(&plan, &e));
    }





    #[test]
    fn signalling_group_zero_or_one_is_refused() {
        // kill(-1, …) signals every process this user owns. The guard is the
        // difference between "stop the backend" and "log out".
        // Nothing is signalled here: pgid 0 and 1 both take the refusal path.
        signal_group(0, libc::SIGTERM);
        signal_group(1, libc::SIGTERM);
        signal_group(-5, libc::SIGTERM);
        // Reaching this line without having killed the test runner is the
        // assertion.
    }
}
