//! `apex build --on`, `apex send`, `apex open` — §20's handoff and remote
//! compute, and the I/O behind them.
//!
//! [`apexd_core::dispatch`] owns the decisions: which remote directory, whether
//! it is the same repository, whether a dirty worktree should stop the run, and
//! which build command a project implies. This file runs `git`, runs `ssh`, and
//! prints.
//!
//! ── Reaching another machine's *session* is the hard part ───────────────────
//!
//! `apex build --on` only needs a shell, which ssh gives it. `apex open` and
//! `apex send --clipboard` need something else entirely: the remote user's
//! **graphical session**. An ssh command has no `WAYLAND_DISPLAY` and no
//! `DBUS_SESSION_BUS_ADDRESS`, so `xdg-open` there fails or — worse — succeeds
//! in a way nobody sees.
//!
//! The fix is the standard one and it is worth stating: the per-user bus lives
//! at a predictable path, `/run/user/<uid>/bus`, so the remote command sets
//! `DBUS_SESSION_BUS_ADDRESS` to it explicitly.
//!
//! But the bus socket existing is **not** evidence of a graphical session — it
//! is there for any logged-in user, including the ssh login itself. So the
//! probe also requires a Wayland socket in the same directory. Without that
//! second check, `apex open katana https://…` on a machine sitting at its
//! greeter would report success and open a tab nobody can see.
//!
//! ── What this deliberately does not do ─────────────────────────────────────
//!
//! It does not sync a working tree. Every verb here either reads local files
//! and sends them somewhere explicit, or runs a command in a directory it has
//! verified. A dispatch tool that rsynced over another machine's checkout would
//! be one bad path away from destroying work, and §20 asks for handoff, not
//! replication.

use std::path::PathBuf;
use std::process::{Command, Stdio};

use anyhow::{anyhow, Context, Result};
use clap::Args;

use apexd_core::dispatch::{
    build_markers, check_clean, check_ident, detect_build, parse_remote_ident, plan_remote_dir,
    DispatchError, RemoteDir,
};
use apexd_core::host::{remote_sh, shell_quote, ssh_argv, Host, Hosts, Tty};

use crate::blueprint::EXIT_ERROR;
use crate::host::{hosts_path, CONNECT_TIMEOUT};

// ── shared plumbing ──────────────────────────────────────────────────────────

/// Read the registry the same way `apex host` does, through the same parser.
fn load_hosts() -> Result<Hosts> {
    let path = hosts_path();
    match std::fs::read_to_string(&path) {
        Ok(text) => Hosts::parse(&text).with_context(|| path.display().to_string()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Hosts::default()),
        Err(e) => Err(e).with_context(|| path.display().to_string()),
    }
}

/// Run `git` locally and return trimmed stdout, or `None` if it failed.
///
/// A failure is `None` rather than an error because every caller has a
/// meaningful answer for "this is not a repository" or "there is no origin",
/// and turning those into errors here would lose the distinction.
fn git(args: &[&str]) -> Option<String> {
    let out = Command::new("git").args(args).output().ok()?;
    if !out.status.success() {
        return None;
    }
    let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if s.is_empty() {
        None
    } else {
        Some(s)
    }
}

/// Run one command on a host, capturing stdout.
fn ssh_capture(host: &Host, name: &str, command: &str) -> Result<(bool, String)> {
    let argv = ssh_argv(
        host.destination(name),
        host.port,
        Tty::None,
        CONNECT_TIMEOUT,
        Some(command),
    );
    let out = Command::new(&argv[0])
        .args(&argv[1..])
        .output()
        .with_context(|| format!("running ssh for host {name:?}"))?;
    Ok((
        out.status.success(),
        String::from_utf8_lossy(&out.stdout).into_owned(),
    ))
}

/// Replace this process with `argv`. Returns only on failure.
///
/// Used for every verb whose output the user is watching: the remote exit
/// status becomes ours, signals reach it, and there is no local parent holding
/// a pipe. A build dispatched with `spawn`-and-wait would swallow Ctrl-C.
fn exec(argv: &[String]) -> Result<()> {
    use std::os::unix::process::CommandExt;
    let err = Command::new(&argv[0]).args(&argv[1..]).exec();
    Err(anyhow!("cannot run {}: {err}", argv[0]))
}

/// Ask ssh for a remote terminal only when there is a local one to forward.
///
/// `ssh -t` with a non-tty stdin prints "Pseudo-terminal will not be allocated
/// because stdin is not a terminal" and carries on — observed while dispatching
/// a real build to the katana from a non-interactive shell. Harmless, but it is
/// stderr noise on every scripted dispatch, and the honest behaviour is to
/// request a terminal only when one exists.
///
/// A watched build still gets its progress bars, colour and Ctrl-C reaching the
/// remote compiler; a piped one gets clean output.
fn tty_for_stdin() -> Tty {
    // SAFETY: isatty on a borrowed fd has no preconditions and cannot fail in a
    // way that matters — a non-tty and an error are the same answer here.
    if unsafe { libc::isatty(libc::STDIN_FILENO) } == 1 {
        Tty::Interactive
    } else {
        Tty::None
    }
}

/// The probe that identifies a remote directory.
///
/// Prints exactly one of `MISSING`, `NO_ORIGIN` or `ORIGIN <url>`. Three tokens
/// rather than an exit status, because "no such directory" and "not a
/// repository" need different messages and an exit code cannot carry the URL.
///
/// `exit 0` in every branch on purpose: a non-zero exit here would be
/// indistinguishable from ssh itself failing, and the caller must be able to
/// tell "the host answered, and the answer is MISSING" from "the host did not
/// answer".
fn ident_script(path: &str) -> String {
    let q = shell_quote(path);
    format!(
        r#"
if [ ! -d {q} ]; then echo MISSING; exit 0; fi
cd {q} 2>/dev/null || {{ echo MISSING; exit 0; }}
u=$(git remote get-url origin 2>/dev/null)
if [ -n "$u" ]; then echo "ORIGIN $u"; else echo NO_ORIGIN; fi
exit 0
"#
    )
}

/// Resolve, and where required verify, the remote directory for this project.
fn resolve_remote_dir(
    name: &str,
    host: &Host,
    explicit: Option<&str>,
    allow_dirty: bool,
) -> Result<RemoteDir> {
    // The local side first: cheap, and a refusal here costs no round trip.
    let local_root = match git(&["rev-parse", "--show-toplevel"]) {
        Some(r) => r,
        None => {
            if explicit.is_none() {
                let cwd = std::env::current_dir()?.display().to_string();
                return Err(DispatchError::NotARepo { local: cwd }.into());
            }
            String::new()
        }
    };
    let origin = git(&["remote", "get-url", "origin"]);

    // The dirty check is skipped when the user named the directory: they may be
    // dispatching into something unrelated to this checkout entirely.
    if explicit.is_none() {
        let changed = git(&["status", "--porcelain"])
            .map(|s| s.lines().filter(|l| !l.trim().is_empty()).count())
            .unwrap_or(0);
        check_clean(changed, allow_dirty)?;
    }

    let dir = plan_remote_dir(&local_root, origin.as_deref(), explicit)?;

    if let RemoteDir::Verify { path, .. } = &dir {
        let script = remote_sh(&["sh", "-c", &ident_script(path)]);
        let (ok, out) = ssh_capture(host, name, &script)?;
        if !ok && out.trim().is_empty() {
            return Err(anyhow!(
                "{name} did not answer. Check that `ssh {}` works — APEX runs ssh with \
                 BatchMode=yes, so a host needing a password fails here rather than prompting",
                host.destination(name)
            ));
        }
        let ident = parse_remote_ident(&out).ok_or_else(|| {
            anyhow!(
                "{name} answered the project probe with something unrecognised, so nothing \
                 was run. Expected MISSING, NO_ORIGIN or 'ORIGIN <url>', got: {}",
                out.lines().next().unwrap_or("(nothing)").trim()
            )
        })?;
        check_ident(name, &dir, &ident)?;
    }
    Ok(dir)
}

/// Discover a remote user's graphical session, for the verbs that need one.
///
/// Prints `BUS <path>` when there is a session to talk to, or one of
/// `NO_BUS` / `NO_SESSION` / `NO_TOOL <name>` so the refusal can say which
/// thing was missing. Requiring a Wayland socket and not merely the bus is the
/// whole point: the bus exists for any login, including the ssh connection
/// asking the question.
fn session_script(needs: &str) -> String {
    let q = shell_quote(needs);
    format!(
        r#"
uid=$(id -u); rt=/run/user/$uid
[ -S "$rt/bus" ] || {{ echo NO_BUS; exit 0; }}
wd=
for s in "$rt"/wayland-*; do
  [ -S "$s" ] || continue
  b=${{s##*/}}
  case "$b" in
    wayland-[0-9]|wayland-[0-9][0-9]) wd=$b; break ;;
  esac
done
[ -n "$wd" ] || {{ echo NO_SESSION; exit 0; }}
command -v {q} >/dev/null 2>&1 || {{ echo "NO_TOOL {q}"; exit 0; }}
echo "SESSION $rt/bus $wd"
exit 0
"#
    )
}

/// Turn the session probe's answer into the bus path and compositor socket, or
/// a message saying what is missing.
fn parse_session(out: &str, host: &str, tool: &str) -> Result<(String, String)> {
    let line = out
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or("");
    if let Some(rest) = line.strip_prefix("SESSION ") {
        let mut parts = rest.split_whitespace();
        if let (Some(bus), Some(wd)) = (parts.next(), parts.next()) {
            return Ok((bus.to_string(), wd.to_string()));
        }
    }
    Err(match line {
        "NO_BUS" => anyhow!(
            "{host} has no per-user bus, so nobody is logged in there. Nothing was sent."
        ),
        "NO_SESSION" => anyhow!(
            "{host} is logged in but has no compositor socket — it is probably sitting at \
             the greeter. Opening something there would succeed and be seen by nobody, so \
             nothing was sent."
        ),
        l if l.starts_with("NO_TOOL") => {
            let missing = l.strip_prefix("NO_TOOL ").unwrap_or(tool).trim_matches('\'');
            anyhow!("{host} has no {missing}. Nothing was sent.")
        }
        "" => anyhow!(
            "{host} did not answer the session probe. Check that `ssh {host}` works."
        ),
        other => anyhow!("{host} answered the session probe with {other:?}; nothing was sent."),
    })
}

/// Run one command inside the remote user's graphical session, and return what
/// it actually did.
///
/// Through `systemd-run --user`, not a bare ssh command with hand-set
/// variables. The reason is a bug this had on its first live run: `apex open`
/// reported "opened on katana" while nothing opened. `setsid --fork` returns 0
/// the moment it forks, so the exit status proved only that a fork happened,
/// and `WAYLAND_DISPLAY` was never set at all so the browser had no display to
/// connect to. Both faults were invisible because the output went to
/// /dev/null.
///
/// The user's own service manager already holds the session environment that
/// the compositor imported into it, so this stops reconstructing that
/// environment and asks the thing that has it. `--wait` makes the exit status
/// the *program's*, which is what turns "we forked something" into "it worked".
/// The remote launch script: set the session environment, start the program
/// in the background, then observe what happened to it.
fn launch_script(bus_addr: &str, wayland: &str, argv: &[String]) -> String {
    format!(
        r#"
export DBUS_SESSION_BUS_ADDRESS={bus} WAYLAND_DISPLAY={wd}
err=$(mktemp)
{cmd} >/dev/null 2>"$err" &
pid=$!
i=0
while [ $i -lt 15 ]; do
  kill -0 $pid 2>/dev/null || break
  sleep 0.1
  i=$((i+1))
done
if kill -0 $pid 2>/dev/null; then
  echo RUNNING
else
  wait $pid; echo "EXIT $?"
fi
cat "$err" >&2
rm -f "$err"
exit 0
"#,
        bus = shell_quote(bus_addr),
        wd = shell_quote(wayland),
        cmd = remote_sh(argv)
    )
}

fn session_run(
    host: &Host,
    name: &str,
    bus: &str,
    wayland: &str,
    argv: &[String],
) -> Result<(bool, String)> {
    // Launched in the background and then *observed*, rather than waited on.
    //
    // Two wrong versions came before this one, and both are worth recording.
    // The first used `setsid --fork`, which returns 0 the instant it forks — so
    // `apex open katana <url>` printed "opened" while nothing opened, because
    // WAYLAND_DISPLAY was never set and the browser had no display to reach.
    // The second used `systemd-run --user --wait`, which does propagate the
    // real exit status but blocks until the program *exits*: a browser becomes
    // the unit's main process, so the command hung for two minutes.
    //
    // What actually distinguishes success from failure here is short-lived:
    // xdg-open either fails quickly (no handler, no display) or hands off and
    // returns. So this waits up to 1.5s for it to exit, reports its status if
    // it did, and reports RUNNING if it is still going — which for a GUI launch
    // is the good case, not an unknown one.
    //
    // The third mistake, and the least obvious: the backgrounded child's
    // STDOUT must be redirected, not just its stderr. ssh does not close the
    // session while any descendant still holds the channel, so a browser
    // inheriting stdout kept `apex open` hanging for a full minute *after*
    // successfully launching — the launch worked and the command still looked
    // broken.
    let inner = launch_script(&format!("unix:path={bus}"), wayland, argv);
    let command = remote_sh(&["sh", "-c", &inner]);
    let ssh = ssh_argv(
        host.destination(name),
        host.port,
        Tty::None,
        CONNECT_TIMEOUT,
        Some(&command),
    );
    let out = Command::new(&ssh[0])
        .args(&ssh[1..])
        .output()
        .with_context(|| format!("running ssh for host {name:?}"))?;
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
    Ok((launch_succeeded(&stdout), stderr))
}

/// The launch script, for the assertions that check its shape.
///
/// Exists because the two faults that mattered — an unredirected stdout and a
/// blocking wait — are properties of the script text, and building it requires
/// no host.
#[cfg(test)]
fn launch_script_for_test() -> String {
    launch_script("unix:path=/run/user/1000/bus", "wayland-1", &["xdg-open".to_string()])
}

/// Whether the launch report says the program is doing its job.
///
/// `RUNNING` is success: a GUI application that is still alive after 1.5s
/// started. `EXIT 0` is success: it handed off and returned. Anything else,
/// including an answer this does not recognise, is a failure — the whole
/// purpose of this function is to stop reporting success without evidence.
fn launch_succeeded(stdout: &str) -> bool {
    for line in stdout.lines().map(str::trim) {
        if line == "RUNNING" {
            return true;
        }
        if let Some(code) = line.strip_prefix("EXIT ") {
            return code.trim() == "0";
        }
    }
    false
}

// ── apex build --on ──────────────────────────────────────────────────────────

#[derive(Args)]
pub struct BuildArgs {
    /// Build on this trusted device instead of here.
    ///
    /// Without it, `apex build` builds locally — the same command, so a
    /// dispatch and a local run cannot drift apart.
    #[arg(long = "on", value_name = "HOST")]
    pub on: Option<String>,
    /// The project directory on the remote, when it is not the same absolute
    /// path as here. Skips the same-repository check: you have said where.
    #[arg(long, value_name = "PATH")]
    pub remote_path: Option<String>,
    /// Build even though this worktree has uncommitted changes.
    ///
    /// They are NOT sent. The remote builds its own committed state.
    #[arg(long)]
    pub allow_dirty: bool,
    /// Print what would run, and where, without running it.
    #[arg(long)]
    pub dry_run: bool,
    /// The build command. Detected from the project when omitted.
    #[arg(last = true)]
    pub argv: Vec<String>,
}

/// Which build command to run, and why that one.
fn build_command(argv: &[String]) -> Result<(String, Vec<String>)> {
    if !argv.is_empty() {
        return Ok(("you asked for it".to_string(), argv.to_vec()));
    }
    // The marker files present in the project root, by exact name.
    let root = git(&["rev-parse", "--show-toplevel"])
        .map(PathBuf::from)
        .unwrap_or(std::env::current_dir()?);
    let mut present = Vec::new();
    if let Ok(rd) = std::fs::read_dir(&root) {
        for e in rd.flatten() {
            present.push(e.file_name().to_string_lossy().into_owned());
        }
    }
    match detect_build(&present) {
        // The marker is reported, not just the command: a detector silently
        // choosing between five possibilities is one nobody can correct.
        Some((marker, cmd)) => Ok((format!("{marker} is here"), cmd)),
        None => Err(anyhow!(
            "nothing in {} says how to build it. Looked for: {}.\n\
             Give the command explicitly: apex build{} -- <command>",
            root.display(),
            build_markers().join(", "),
            std::env::args()
                .skip(1)
                .find(|a| a == "--on")
                .map(|_| " --on <host>")
                .unwrap_or("")
        )),
    }
}

pub fn build(args: BuildArgs) -> i32 {
    match build_inner(args) {
        Ok(()) => 0,
        Err(e) => {
            eprintln!("apex build: {e:#}");
            EXIT_ERROR
        }
    }
}

fn build_inner(args: BuildArgs) -> Result<()> {
    let (why, cmd) = build_command(&args.argv)?;

    let Some(name) = args.on.clone() else {
        // Local. The same command, so the two paths cannot disagree about what
        // "the build" is.
        println!("building here ({why}): {}", cmd.join(" "));
        if args.dry_run {
            return Ok(());
        }
        return exec(&cmd);
    };

    let hosts = load_hosts()?;
    let host = hosts.get(&name)?.clone();
    let dir = resolve_remote_dir(&name, &host, args.remote_path.as_deref(), args.allow_dirty)?;

    // `cd <dir> && <cmd>` as one remote command, every component quoted.
    let inner = format!("cd {} && {}", shell_quote(dir.path()), remote_sh(&cmd));
    let command = remote_sh(&["sh", "-c", &inner]);
    let ssh = ssh_argv(
        host.destination(&name),
        host.port,
        // A watched build wants a terminal — progress bars, colour, and Ctrl-C
        // reaching the remote compiler rather than only the local ssh. A piped
        // one must not ask for one; see tty_for_stdin.
        tty_for_stdin(),
        CONNECT_TIMEOUT,
        Some(&command),
    );

    println!("building on {name} at {} ({why})", dir.path());
    println!("  {}", cmd.join(" "));
    if let RemoteDir::AsTold { .. } = dir {
        // Said out loud because the same-repository check was skipped.
        println!("  (--remote-path given, so the repository was not checked)");
    }
    if args.dry_run {
        println!("dry run: {}", ssh.join(" "));
        return Ok(());
    }
    exec(&ssh)
}

// ── apex send ────────────────────────────────────────────────────────────────

#[derive(Args)]
pub struct SendArgs {
    /// The device to send to.
    pub host: String,
    /// Send the clipboard instead of files.
    #[arg(long, conflicts_with = "paths")]
    pub clipboard: bool,
    /// Where to put the files. Defaults to ~/Downloads there, or ~ if that does
    /// not exist.
    #[arg(long, value_name = "DIR")]
    pub to: Option<String>,
    /// Overwrite files that already exist there. Off by default.
    #[arg(long)]
    pub force: bool,
    /// Files or directories to send.
    pub paths: Vec<PathBuf>,
}

pub fn send(args: SendArgs) -> i32 {
    match send_inner(args) {
        Ok(()) => 0,
        Err(e) => {
            eprintln!("apex send: {e:#}");
            EXIT_ERROR
        }
    }
}

fn send_inner(args: SendArgs) -> Result<()> {
    let hosts = load_hosts()?;
    let host = hosts.get(&args.host)?.clone();

    if args.clipboard {
        return send_clipboard(&args.host, &host);
    }
    if args.paths.is_empty() {
        return Err(anyhow!(
            "nothing to send. Give one or more paths, or --clipboard"
        ));
    }
    send_files(&args, &host)
}

fn send_clipboard(name: &str, host: &Host) -> Result<()> {
    // Local clipboard first: failing before touching the network means a
    // machine with no clipboard tool does not look like a network problem.
    let out = Command::new("wl-paste")
        .arg("--no-newline")
        .output()
        .map_err(|e| anyhow!("cannot read the clipboard: wl-paste is not available here ({e})"))?;
    if !out.status.success() {
        return Err(anyhow!("the clipboard is empty, or wl-paste could not read it"));
    }
    let bytes = out.stdout;
    if bytes.is_empty() {
        return Err(anyhow!("the clipboard is empty; nothing was sent"));
    }

    let (ok, sout) =
        ssh_capture(host, name, &remote_sh(&["sh", "-c", &session_script("wl-copy")]))?;
    if !ok && sout.trim().is_empty() {
        return Err(anyhow!("{name} did not answer"));
    }
    let (bus, wayland) = parse_session(&sout, name, "wl-copy")?;

    // Not through session_run: `wl-copy` forks and stays alive to serve the
    // selection, so `systemd-run --wait` would never return. It gets the same
    // two variables the service manager would have given it, set explicitly.
    //
    // The content goes over stdin, never in the argv: it can be megabytes and
    // can contain anything, and an argv has neither the room nor the safety.
    let inner = format!(
        "DBUS_SESSION_BUS_ADDRESS={} WAYLAND_DISPLAY={} wl-copy",
        shell_quote(&format!("unix:path={bus}")),
        shell_quote(&wayland)
    );
    let command = remote_sh(&["sh", "-c", &inner]);
    let argv = ssh_argv(
        host.destination(name),
        host.port,
        Tty::None,
        CONNECT_TIMEOUT,
        Some(&command),
    );
    let mut child = Command::new(&argv[0])
        .args(&argv[1..])
        .stdin(Stdio::piped())
        .spawn()
        .context("running ssh")?;
    {
        use std::io::Write;
        child
            .stdin
            .as_mut()
            .expect("stdin was piped")
            .write_all(&bytes)
            .context("sending the clipboard")?;
    }
    let status = child.wait().context("waiting for ssh")?;
    if !status.success() {
        return Err(anyhow!("{name} did not accept the clipboard"));
    }

    // Read it back. wl-copy exiting 0 means it forked, not that the selection
    // is being served — the same gap that made `apex open` claim success while
    // nothing opened. Comparing the round trip is the only honest confirmation.
    let (rok, rout) = ssh_capture(
        host,
        name,
        &remote_sh(&[
            "sh",
            "-c",
            &format!(
                "DBUS_SESSION_BUS_ADDRESS={} WAYLAND_DISPLAY={} wl-paste --no-newline | wc -c",
                shell_quote(&format!("unix:path={bus}")),
                shell_quote(&wayland)
            ),
        ]),
    )?;
    let there: Option<usize> = rout.trim().parse().ok();
    match (rok, there) {
        (true, Some(n)) if n == bytes.len() => {
            println!("sent {} bytes to {name}'s clipboard", bytes.len());
            Ok(())
        }
        (true, Some(n)) => Err(anyhow!(
            "{name}'s clipboard holds {n} bytes but {} were sent, so something else \
             took the selection. Try again.",
            bytes.len()
        )),
        _ => Err(anyhow!(
            "sent {} bytes to {name}, but reading its clipboard back did not confirm it",
            bytes.len()
        )),
    }
}

fn send_files(args: &SendArgs, host: &Host) -> Result<()> {
    let name = &args.host;
    for p in &args.paths {
        if !p.exists() {
            return Err(anyhow!("{} does not exist", p.display()));
        }
    }

    // The destination is resolved on the remote, not guessed here: whether
    // ~/Downloads exists is a fact about that machine.
    let dest_expr = match &args.to {
        Some(d) => {
            if !d.starts_with('/') && !d.starts_with('~') {
                return Err(anyhow!(
                    "--to {d:?} is relative; a path only means something there if it is \
                     absolute"
                ));
            }
            shell_quote(d)
        }
        None => "\"$(if [ -d \"$HOME/Downloads\" ]; then echo \"$HOME/Downloads\"; else echo \"$HOME\"; fi)\"".to_string(),
    };

    // --keep-old-files makes tar fail rather than overwrite. Refusing by
    // default is the right way round: a send that silently replaced a file on
    // another machine is not recoverable from this end.
    let keep = if args.force { "" } else { "--keep-old-files " };
    let inner = format!(
        "d={dest_expr}; mkdir -p \"$d\" && cd \"$d\" && tar -x {keep}-f - && printf 'INTO %s\\n' \"$d\""
    );
    let command = remote_sh(&["sh", "-c", &inner]);
    let argv = ssh_argv(
        host.destination(name),
        host.port,
        Tty::None,
        CONNECT_TIMEOUT,
        Some(&command),
    );

    // tar is given each path as `-C <parent> <basename>`, so what lands on the
    // far side is the file's own name rather than the sender's directory
    // layout. `apex send katana ~/a/b.txt` must not create `home/andre/a/` there.
    let mut tar = Command::new("tar");
    tar.arg("-c");
    for p in &args.paths {
        let abs = p.canonicalize().with_context(|| format!("{}", p.display()))?;
        let parent = abs
            .parent()
            .ok_or_else(|| anyhow!("{} has no parent directory", abs.display()))?;
        let base = abs
            .file_name()
            .ok_or_else(|| anyhow!("{} has no file name", abs.display()))?;
        tar.arg("-C").arg(parent).arg(base);
    }
    let tar_out = tar.stdout(Stdio::piped()).spawn().context("running tar")?;
    let child = Command::new(&argv[0])
        .args(&argv[1..])
        .stdin(Stdio::from(tar_out.stdout.expect("stdout was piped")))
        .stdout(Stdio::piped())
        // Captured rather than inherited. The remote tar prints its own
        // "Exiting with failure status due to previous errors" straight to the
        // terminal otherwise, above the message explaining what actually
        // happened — observed on the first live run. It is still shown, but as
        // detail under the explanation rather than instead of it.
        .stderr(Stdio::piped())
        .spawn()
        .context("running ssh")?;
    let out = child.wait_with_output().context("waiting for ssh")?;
    let text = String::from_utf8_lossy(&out.stdout);
    let landed = text
        .lines()
        .find_map(|l| l.strip_prefix("INTO "))
        .map(str::trim);

    if !out.status.success() || landed.is_none() {
        let detail = String::from_utf8_lossy(&out.stderr);
        let detail: Vec<&str> = detail.lines().map(str::trim).filter(|l| !l.is_empty()).collect();
        let suffix = if detail.is_empty() {
            String::new()
        } else {
            format!("\n  {}", detail.join("\n  "))
        };
        if !args.force {
            return Err(anyhow!(
                "{name} refused at least one file, most likely because it already exists \
                 there. Nothing was overwritten. Pass --force to replace.{suffix}"
            ));
        }
        return Err(anyhow!("{name} did not accept the files{suffix}"));
    }
    println!(
        "sent {} item(s) to {name}:{}",
        args.paths.len(),
        landed.unwrap_or("?")
    );
    Ok(())
}

// ── apex open ────────────────────────────────────────────────────────────────

#[derive(Args)]
pub struct OpenArgs {
    /// The device to open it on.
    pub host: String,
    /// A URL, or a path that exists on that device.
    pub target: String,
}

pub fn open(args: OpenArgs) -> i32 {
    match open_inner(args) {
        Ok(()) => 0,
        Err(e) => {
            eprintln!("apex open: {e:#}");
            EXIT_ERROR
        }
    }
}

fn open_inner(args: OpenArgs) -> Result<()> {
    let hosts = load_hosts()?;
    let host = hosts.get(&args.host)?.clone();
    let name = &args.host;

    // A leading '-' would be an option to xdg-open. Everything else is passed
    // through quoted, so no other character needs refusing.
    if args.target.starts_with('-') {
        return Err(anyhow!(
            "{:?} starts with '-', which xdg-open would read as an option",
            args.target
        ));
    }

    let (ok, sout) =
        ssh_capture(&host, name, &remote_sh(&["sh", "-c", &session_script("xdg-open")]))?;
    if !ok && sout.trim().is_empty() {
        return Err(anyhow!("{name} did not answer"));
    }
    let (bus, wayland) = parse_session(&sout, name, "xdg-open")?;

    let (ok, err) = session_run(
        &host,
        name,
        &bus,
        &wayland,
        &["xdg-open".to_string(), args.target.clone()],
    )?;
    if !ok {
        // The status is xdg-open's own, because systemd-run --wait propagates
        // it. This is the check that was missing when `apex open` reported
        // success while nothing opened.
        let detail: Vec<&str> =
            err.lines().map(str::trim).filter(|l| !l.is_empty()).collect();
        return Err(anyhow!(
            "{name} could not open it{}",
            if detail.is_empty() {
                String::new()
            } else {
                format!(":\n  {}", detail.join("\n  "))
            }
        ));
    }
    println!("opened on {name}: {}", args.target);
    Ok(())
}

// ── forwarding a verb to a remote apex ───────────────────────────────────────

/// Run `apex <argv>` on a trusted device and become that process.
///
/// For verbs that need no project directory — listing sessions, attaching to
/// one, asking a model something. There is nothing to resolve and nothing to
/// verify, so this is deliberately thinner than [`agent_run_remote`]: it
/// reaches the remote's own `apex`, which applies its own policy.
///
/// `require` names a capability the cached probe must not have *denied*. A host
/// that has never been probed is not refused — it may have been added a moment
/// ago — but one that was probed and lacks the thing is, because that is a real
/// answer and the alternative is an ssh that fails several seconds later with
/// "unrecognized subcommand".
pub fn forward_to_host(
    name: &str,
    argv: &[String],
    tty: Tty,
    require: Option<Capability>,
) -> Result<()> {
    let hosts = load_hosts()?;
    let host = hosts.get(name)?.clone();

    if let (Some(cap), Some(caps)) = (require, crate::host::cached_caps(name)) {
        if caps.is_apex() && !cap.present_in(&caps) {
            return Err(anyhow!(
                "{name} has no {}, so there is nothing there to run this on. \
                 `apex host probe {name}` if that has changed.",
                cap.describe()
            ));
        }
    }

    let mut full = vec!["apex".to_string()];
    full.extend(argv.iter().cloned());
    let command = remote_sh(&full);
    let ssh = ssh_argv(
        host.destination(name),
        host.port,
        tty,
        CONNECT_TIMEOUT,
        Some(&command),
    );
    exec(&ssh)
}

/// A capability a forwarded verb depends on.
///
/// One variant for now. `Ai` joins it in the commit that wires `apex ai --on`,
/// rather than sitting here unconstructed: a variant nothing builds is dead
/// code that looks like a wired feature.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Capability {
    /// The per-user agent runtime.
    Agentd,
}

impl Capability {
    fn present_in(&self, caps: &apexd_core::host::HostCaps) -> bool {
        match self {
            Self::Agentd => caps.agentd,
        }
    }

    fn describe(&self) -> &'static str {
        match self {
            Self::Agentd => "agent runtime",
        }
    }
}

// ── apex agent run --host ────────────────────────────────────────────────────

/// Re-run this `apex agent run` invocation on a remote device.
///
/// The whole command is forwarded rather than reimplemented: the remote's own
/// `apex agent run` applies its own sandbox policy, its own default agent and
/// its own checkpointing, which is what makes a remote session behave like a
/// local one. Reconstructing those decisions here would mean two
/// implementations of the same policy, and the remote's is the one that matters
/// because that is where the agent runs.
///
/// A tty is always allocated: an agent session is interactive, and the one case
/// that is not (`--detach`) still costs nothing to run under one.
pub fn agent_run_remote(
    name: &str,
    remote_path: Option<&str>,
    allow_dirty: bool,
    forward: &[String],
) -> Result<()> {
    let hosts = load_hosts()?;
    let host = hosts.get(name)?.clone();
    let dir = resolve_remote_dir(name, &host, remote_path, allow_dirty)?;

    let mut argv = vec!["apex".to_string(), "agent".to_string(), "run".to_string()];
    argv.extend(forward.iter().cloned());
    let inner = format!("cd {} && {}", shell_quote(dir.path()), remote_sh(&argv));
    let command = remote_sh(&["sh", "-c", &inner]);
    let ssh = ssh_argv(
        host.destination(name),
        host.port,
        tty_for_stdin(),
        CONNECT_TIMEOUT,
        Some(&command),
    );
    eprintln!("running the agent on {name} at {}", dir.path());
    exec(&ssh)
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── the identity probe script ────────────────────────────────────────────

    #[test]
    fn the_ident_script_quotes_the_path_it_was_given() {
        let s = ident_script("/home/a/my project");
        assert!(s.contains("'/home/a/my project'"), "unquoted path in {s}");
    }

    #[test]
    fn a_path_with_a_quote_in_it_cannot_break_the_ident_script() {
        // The realistic hostile case is not a hostile user, it is a directory
        // named with an apostrophe.
        let s = ident_script("/home/a/andre's stuff");
        assert!(s.contains(r"'/home/a/andre'\''s stuff'"), "got {s}");
        // And nothing that would end the quoting early.
        assert!(!s.contains("andre's stuff'"), "quoting escaped: {s}");
    }

    #[test]
    fn every_branch_of_the_ident_script_exits_zero() {
        // A non-zero exit would be indistinguishable from ssh failing, and the
        // caller must be able to tell "answered MISSING" from "did not answer".
        let s = ident_script("/p");
        assert_eq!(s.matches("exit 0").count(), 3, "branch count changed: {s}");
        assert!(!s.contains("exit 1"));
    }

    #[test]
    fn the_ident_script_prints_exactly_the_three_tokens_the_parser_knows() {
        let s = ident_script("/p");
        for token in ["MISSING", "NO_ORIGIN", "ORIGIN "] {
            assert!(s.contains(token), "{token} missing from the script");
        }
        // And the parser accepts each of them, so the two cannot drift.
        assert!(parse_remote_ident("MISSING").is_some());
        assert!(parse_remote_ident("NO_ORIGIN").is_some());
        assert!(parse_remote_ident("ORIGIN git@h:a/b").is_some());
    }

    // ── the session probe ────────────────────────────────────────────────────

    #[test]
    fn the_session_probe_requires_a_wayland_socket_not_just_the_bus() {
        // The bus exists for any login, including the ssh connection asking.
        // Without the second check, `apex open` on a machine at its greeter
        // reports success and opens a tab nobody sees.
        let s = session_script("xdg-open");
        assert!(s.contains("/bus"), "no bus check");
        assert!(s.contains("wayland-*"), "no wayland socket check");
        assert!(s.contains("NO_SESSION"), "no distinct greeter answer");
    }

    #[test]
    fn the_session_probe_quotes_the_tool_it_looks_for() {
        let s = session_script("wl-copy");
        assert!(s.contains("'wl-copy'"), "got {s}");
    }

    #[test]
    fn a_session_answer_yields_the_bus_and_the_compositor_socket() {
        let (bus, wd) =
            parse_session("SESSION /run/user/1000/bus wayland-1\n", "katana", "xdg-open").unwrap();
        assert_eq!(bus, "/run/user/1000/bus");
        // Not wayland-0. The katana's is wayland-1, and hardcoding 0 is how
        // this would silently fail on a real machine.
        assert_eq!(wd, "wayland-1");
    }

    #[test]
    fn a_session_answer_missing_the_socket_is_not_accepted() {
        // Half an answer must not yield an empty WAYLAND_DISPLAY, which would
        // fail later and further away.
        assert!(parse_session("SESSION /run/user/1000/bus\n", "k", "xdg-open").is_err());
    }

    #[test]
    fn the_probe_picks_the_compositor_socket_and_not_its_neighbours() {
        // Real directory contents from the katana:
        //   wayland-1                       <- the compositor
        //   wayland-1-awww-daemon.sock      <- another program's socket
        //   wayland-1.lock                  <- not a socket at all
        // A plain `wayland-*` glob with `head -1` happens to pick the right one
        // only because of sort order, which is not a reason.
        let s = session_script("xdg-open");
        assert!(
            s.contains("wayland-[0-9]|wayland-[0-9][0-9]"),
            "the socket is not matched by an exact pattern: {s}"
        );
        assert!(!s.contains("head -1"), "relying on sort order: {s}");
    }

    #[test]
    fn a_launch_report_is_believed_only_when_it_says_something_known() {
        // The point of the report is to stop claiming success without
        // evidence, so an unrecognised answer must be a failure.
        assert!(launch_succeeded("RUNNING\n"));
        assert!(launch_succeeded("EXIT 0\n"));
        assert!(!launch_succeeded("EXIT 1\n"));
        assert!(!launch_succeeded("EXIT 127\n"));
        assert!(!launch_succeeded(""));
        assert!(!launch_succeeded("probably fine\n"));
    }

    #[test]
    fn the_backgrounded_child_has_both_streams_redirected() {
        // ssh keeps the session open while any descendant holds the channel.
        // Redirecting only stderr left a launched browser holding stdout, and
        // `apex open` hung for a minute after it had already succeeded.
        let s = session_script("xdg-open");
        let _ = s;
        // The launch script is built by session_run, so assert on it directly.
        let inner = launch_script_for_test();
        assert!(inner.contains(">/dev/null"), "stdout not redirected: {inner}");
        assert!(inner.contains(r#"2>"$err""#), "stderr not captured: {inner}");
    }

    #[test]
    fn the_launch_is_observed_rather_than_waited_on() {
        // `systemd-run --wait` propagates a real status but blocks until a GUI
        // program exits — measured: two minutes and still going. `setsid
        // --fork` returns 0 immediately and proves nothing. Neither may come
        // back.
        let s = session_script("xdg-open");
        assert!(!s.contains("--wait"), "blocking launch: {s}");
        assert!(!s.contains("setsid"), "unverifiable launch: {s}");
    }

    #[test]
    fn a_missing_tool_is_named_in_the_refusal() {
        let e = parse_session("NO_TOOL systemd-run\n", "k", "xdg-open").unwrap_err();
        assert!(e.to_string().contains("systemd-run"), "got {e}");
    }

    #[test]
    fn the_greeter_case_says_it_would_be_seen_by_nobody() {
        // The message has to explain why success is not good enough here.
        let e = parse_session("NO_SESSION\n", "katana", "xdg-open").unwrap_err();
        assert!(e.to_string().contains("greeter"), "got {e}");
        assert!(e.to_string().contains("nobody"), "got {e}");
    }

    #[test]
    fn each_session_failure_says_which_thing_was_missing() {
        let msgs: Vec<String> = ["NO_BUS", "NO_SESSION", "NO_TOOL xdg-open", ""]
            .iter()
            .map(|a| parse_session(a, "k", "xdg-open").unwrap_err().to_string())
            .collect();
        for i in 0..msgs.len() {
            for j in (i + 1)..msgs.len() {
                assert_ne!(msgs[i], msgs[j], "answers {i} and {j} read identically");
            }
        }
        assert!(msgs[0].contains("nobody is logged in"), "got {}", msgs[0]);
        assert!(msgs[3].contains("did not answer"), "got {}", msgs[3]);
    }

    #[test]
    fn an_unrecognised_session_answer_is_refused_rather_than_assumed_fine() {
        let e = parse_session("hello\n", "k", "xdg-open").unwrap_err();
        assert!(e.to_string().contains("nothing was sent"), "got {e}");
    }

    #[test]
    fn a_terminal_is_requested_only_when_there_is_one_to_forward() {
        // Under `cargo test` stdin is not a terminal, so this asserts the
        // quiet branch — which is exactly the one that was printing
        // "Pseudo-terminal will not be allocated" on every scripted dispatch.
        assert_eq!(tty_for_stdin(), Tty::None);
    }

    // ── build command selection ──────────────────────────────────────────────

    #[test]
    fn an_explicit_command_is_used_verbatim_and_says_so() {
        let (why, cmd) = build_command(&["ninja".to_string(), "-C".to_string()]).unwrap();
        assert_eq!(cmd, vec!["ninja", "-C"]);
        assert!(why.contains("you asked"), "got {why}");
    }

    #[test]
    fn this_repository_detects_its_own_build_script() {
        // Run from the repo, so this asserts against the real tree: apex-os
        // ships build-local.sh, and it must win over Cargo.toml.
        //
        // Guarded rather than assumed, because a test that silently passes when
        // run from elsewhere is worse than one that says why it cannot run.
        if git(&["rev-parse", "--show-toplevel"]).is_none() {
            // Not a repository: this case cannot mean anything here.
            return;
        }
        let (why, cmd) = build_command(&[]).unwrap();
        assert!(
            why.contains("build-local.sh") || why.contains("Cargo.toml"),
            "unexpected reason: {why}"
        );
        assert!(!cmd.is_empty());
    }
}
