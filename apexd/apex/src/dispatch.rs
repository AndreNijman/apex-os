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
have=
for s in "$rt"/wayland-*; do
  [ -S "$s" ] && have=1 && break
done
[ -n "$have" ] || {{ echo NO_SESSION; exit 0; }}
command -v {q} >/dev/null 2>&1 || {{ echo "NO_TOOL {q}"; exit 0; }}
echo "BUS $rt/bus"
exit 0
"#
    )
}

/// Turn the session probe's answer into a bus path, or a message saying what is
/// missing.
fn parse_session(out: &str, host: &str, tool: &str) -> Result<String> {
    let line = out
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or("");
    if let Some(bus) = line.strip_prefix("BUS ") {
        return Ok(bus.trim().to_string());
    }
    Err(match line {
        "NO_BUS" => anyhow!(
            "{host} has no per-user bus, so nobody is logged in there. \
             Nothing was sent."
        ),
        "NO_SESSION" => anyhow!(
            "{host} is logged in but has no graphical session — it is probably sitting at \
             the greeter. Opening something there would succeed and be seen by nobody, so \
             nothing was sent."
        ),
        l if l.starts_with("NO_TOOL") => anyhow!(
            "{host} has no {tool}. Nothing was sent."
        ),
        "" => anyhow!(
            "{host} did not answer the session probe. Check that `ssh {host}` works."
        ),
        other => anyhow!("{host} answered the session probe with {other:?}; nothing was sent."),
    })
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
        .map_err(|e| {
            anyhow!("cannot read the clipboard: wl-paste is not available here ({e})")
        })?;
    if !out.status.success() {
        return Err(anyhow!(
            "the clipboard is empty, or wl-paste could not read it"
        ));
    }
    let bytes = out.stdout;
    if bytes.is_empty() {
        return Err(anyhow!("the clipboard is empty; nothing was sent"));
    }

    let (ok, sout) = ssh_capture(host, name, &remote_sh(&["sh", "-c", &session_script("wl-copy")]))?;
    if !ok && sout.trim().is_empty() {
        return Err(anyhow!("{name} did not answer"));
    }
    let bus = parse_session(&sout, name, "wl-copy")?;

    // The clipboard content goes over stdin, never in the argv: it can be
    // megabytes, it can contain anything, and an argv has neither the room nor
    // the safety for it.
    let inner = format!(
        "DBUS_SESSION_BUS_ADDRESS={} WAYLAND_DISPLAY=$(basename $(ls -1 /run/user/$(id -u)/wayland-* | head -1)) wl-copy",
        shell_quote(&format!("unix:path={bus}"))
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
    println!("sent {} bytes to {name}'s clipboard", bytes.len());
    Ok(())
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
        .spawn()
        .context("running ssh")?;
    let out = child.wait_with_output().context("waiting for ssh")?;
    let text = String::from_utf8_lossy(&out.stdout);
    let landed = text
        .lines()
        .find_map(|l| l.strip_prefix("INTO "))
        .map(str::trim);

    if !out.status.success() || landed.is_none() {
        if !args.force {
            return Err(anyhow!(
                "{name} refused at least one file, most likely because it already exists \
                 there. Nothing was overwritten. Pass --force to replace."
            ));
        }
        return Err(anyhow!("{name} did not accept the files"));
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
    let bus = parse_session(&sout, name, "xdg-open")?;

    // setsid, so the opened application is not killed when ssh disconnects —
    // which is what happens without it, and it looks like "nothing opened".
    let inner = format!(
        "DBUS_SESSION_BUS_ADDRESS={} setsid --fork xdg-open {} >/dev/null 2>&1",
        shell_quote(&format!("unix:path={bus}")),
        shell_quote(&args.target)
    );
    let command = remote_sh(&["sh", "-c", &inner]);
    let (ok, _) = ssh_capture(&host, name, &command)?;
    if !ok {
        return Err(anyhow!("{name} could not open it"));
    }
    println!("opened on {name}: {}", args.target);
    Ok(())
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
    fn a_session_answer_yields_the_bus_path() {
        let bus = parse_session("BUS /run/user/1000/bus\n", "katana", "xdg-open").unwrap();
        assert_eq!(bus, "/run/user/1000/bus");
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
