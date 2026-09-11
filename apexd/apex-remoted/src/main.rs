//! `apex-remoted` — the desktop half of APEX Remote (roadmap §46, P1-050).
//!
//! Per-user, unprivileged, and deliberately a second daemon rather than a
//! feature of `apex-agentd`. Three reasons, in order of how much they matter:
//!
//! * **It is the only thing here that listens on a network.** `apex-agentd`
//!   binds a Unix socket in a 0700 directory and nothing else; giving it a TCP
//!   listener would put an attack surface on the process that owns every
//!   agent PTY on the machine. A separate process can be stopped, can fail,
//!   and can be compromised without taking the agent runtime with it.
//! * **It has a different lifetime.** `apex agent enable` starts the runtime
//!   for somebody who is going to run an agent locally, and most of them never
//!   pair a phone. `apex remote enable` is a separate decision.
//! * **It is a client of the runtime, not a part of it.** Everything it does
//!   goes through the same control socket `apex` uses, under an origin the
//!   daemon assigns. If this process is taken over, what the attacker gets is
//!   a `claude-remote-control` origin — no root approvals, no break-glass, no
//!   raw secrets — and not the runtime's own privileges.
//!
//! ## What it will not do
//!
//! Start unless this process is a systemd user service. A proxy started from
//! a login session is classified `local-terminal` by everything it connects
//! to, and §7 reserves approving a root operation for exactly that. The
//! per-connection origin latch in `apex-agentd` closes the hole properly;
//! this refuses to rely on one guard for it. `--allow-foreground` is there
//! for a developer running it by hand, and it says what it is giving up.

mod control;
mod discovery;
mod net;
mod peer;
mod proxy;
mod relay;
mod serve;
mod state;

use std::net::TcpListener;
use std::path::PathBuf;
use std::sync::Arc;

use apex_remote_core::identity::Identity;

use crate::state::State;

/// The port the LAN listener binds.
///
/// Registered in `files/system/firewall/services` as `apex-remote`, and
/// **closed by default** like everything else: the machine ships default-drop
/// and a user opens it with `apex firewall allow apex-remote`. That is a
/// deliberate cost. The alternative — opening a port because the feature
/// exists — is how a default-drop policy becomes a list of things somebody
/// once wanted.
///
/// The relay path needs no exception at all, because it is an outbound
/// connection. On a network where the owner has not opened the port, APEX
/// Remote still works through the relay; on their own LAN, opening it buys a
/// faster path that involves nobody else.
pub const DEFAULT_PORT: u16 = 7717;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|a| a == "--help" || a == "-h") {
        println!("{USAGE}");
        return;
    }
    if let Err(e) = run(&args) {
        eprintln!("apex-remoted: {e}");
        std::process::exit(1);
    }
}

const USAGE: &str = "\
apex-remoted — the desktop service for APEX Remote

  --port <n>          listen on this TCP port (default 7717)
  --handshake-timeout-ms <n>
                      how long an unauthenticated peer may take (default
                      30000, clamped to 1000..120000). Lower it in a test;
                      raising it past two minutes is refused, because the
                      deadline is what stops an idle connection holding a
                      thread on the only network listener in the stack.
  --relay <url>       the rendezvous to fall back to when no LAN path works
  --ping-interval-ms  how often an open connection is measured (default 15000)
  --no-announce       do not advertise this machine over mDNS on this network
  --allow-foreground  run outside a systemd user unit (see below)
  --help

This service is normally started by systemd:

  apex remote enable

It refuses to run from a login session, because `apex-agentd` would then
classify every request it forwards as a human at this machine. Pass
--allow-foreground to override that for development; the requests it forwards
are still recorded as claude-remote-control, because it declares that on every
connection, but the second guard is gone.
";

fn run(args: &[String]) -> Result<(), String> {
    let port = flag(args, "--port")
        .map(|v| v.parse::<u16>().map_err(|_| format!("--port {v} is not a port")))
        .transpose()?
        .unwrap_or(DEFAULT_PORT);
    let relay = flag(args, "--relay");
    let handshake = match flag(args, "--handshake-timeout-ms") {
        None => serve::HANDSHAKE_TIMEOUT,
        Some(v) => {
            let ms: u64 = v
                .parse()
                .map_err(|_| format!("--handshake-timeout-ms {v} is not a number"))?;
            // Clamped rather than trusted. This deadline is a denial-of-service
            // guard, and a flag that could switch it off would be a way to ask
            // for the bug it exists to prevent.
            std::time::Duration::from_millis(ms.clamp(1_000, 120_000))
        }
    };
    // How often an open connection is measured. A flag for the same reason
    // --handshake-timeout-ms is one: the shipped value is right for a phone
    // and wrong for a suite that has to watch a measurement happen. Clamped,
    // so it cannot be turned into a flood a device pays for.
    let ping_interval = match flag(args, "--ping-interval-ms") {
        None => serve::PING_INTERVAL,
        Some(v) => {
            let ms: u64 = v
                .parse()
                .map_err(|_| format!("--ping-interval-ms {v} is not a number"))?;
            std::time::Duration::from_millis(ms.clamp(50, 600_000))
        }
    };
    let allow_foreground = args.iter().any(|a| a == "--allow-foreground");

    guard_placement(allow_foreground)?;

    let state_home = apex_agent_core::paths::state_home();
    let identity = Identity::load_or_create(&Identity::path_in(&state_home))
        .map_err(|e| format!("this machine has no remote identity: {e}"))?;
    let store_path = apex_remote_core::device::DeviceStore::path_in(&state_home);
    let machine = machine_name();

    let listener = TcpListener::bind(("0.0.0.0", port))
        .map_err(|e| format!("cannot listen on port {port}: {e}"))?;
    let state = State::new(identity, machine, port, relay, store_path, ping_interval)
        .map_err(|e| format!("the paired-device store is unusable: {e}"))?;

    let control_path = crate::state::control_socket();
    let control = bind_control(&control_path)?;
    {
        let state = Arc::clone(&state);
        std::thread::spawn(move || control::serve(control, state));
    }

    eprintln!(
        "apex-remoted: listening on {port}, {} device(s) paired, identity {}",
        state
            .devices()
            .map(|s| s.list().iter().filter(|d| d.is_active()).count())
            .unwrap_or(0),
        // The PUBLIC key. It is what goes in a QR code and what a device
        // pins; printing it lets somebody check by eye that the code they
        // scanned belongs to this machine.
        state.identity.public_key()
    );

    let agentd = apex_agent_core::paths::control_socket();

    // The relay, when one is configured. Parsed here rather than at every
    // dial so a typo is one line in the journal at startup instead of a
    // failed connection every two seconds for the life of the machine — and
    // NOT a refusal to start, because a bad relay address must not cost the
    // owner the LAN path that does work.
    if let Some(url) = state.relay.clone() {
        match apex_remote_core::relay::Endpoint::parse(&url) {
            Ok(endpoint) => {
                let state = Arc::clone(&state);
                std::thread::spawn(move || relay::supervise(state, endpoint));
            }
            Err(e) => eprintln!(
                "apex-remoted: the configured relay is not usable, so this machine is \
                 reachable on this network only: {e}"
            ),
        }
    }

    // On the local network, for as long as this process runs. Deliberately
    // not a file in /etc/avahi/services, which would advertise the machine
    // whether or not the service was running; see discovery.rs.
    let _announced = (!args.iter().any(|a| a == "--no-announce")).then(|| discovery::announce(&state));

    for stream in listener.incoming() {
        let Ok(stream) = stream else { continue };
        // A dead connection must not hold a thread forever, and a live PTY
        // must not be killed by a read deadline. So the handshake gets a
        // deadline and `serve::session` clears it once the connection has
        // authenticated — an unauthenticated peer never reaches the clear.
        //
        // The first version of this set the deadline here and then cleared it
        // as the first line of the spawned thread, which is the opposite of
        // what its own comment said: a LAN peer that connected and sent
        // nothing held a thread forever, on the only network listener in the
        // stack.
        stream.set_read_timeout(Some(handshake)).ok();
        let state = Arc::clone(&state);
        let agentd = agentd.clone();
        std::thread::spawn(move || serve::connection(stream, state, agentd));
    }
    Ok(())
}

/// Refuse to run somewhere that would make every forwarded request look local.
fn guard_placement(allow_foreground: bool) -> Result<(), String> {
    match apex_agent_core::origin::is_a_user_service() {
        Ok(true) => Ok(()),
        Ok(false) if allow_foreground => {
            eprintln!(
                "apex-remoted: running outside a systemd user unit. Every connection this \
                 process opens to the agent runtime still declares itself \
                 claude-remote-control, but the placement check that would have caught a \
                 mistake there is off."
            );
            Ok(())
        }
        Ok(false) => Err(
            "this process is not a systemd user service, so the agent runtime would classify \
             every request it forwards as coming from a human at this machine. Start it with \
             `apex remote enable`, or pass --allow-foreground if you know why you want that"
                .to_string(),
        ),
        Err(why) if allow_foreground => {
            eprintln!("apex-remoted: cannot tell where this process is running ({why})");
            Ok(())
        }
        Err(why) => Err(format!(
            "cannot tell where this process is running ({why}), so it cannot be shown to be an \
             unattended service — and a proxy that might be classified as a human at this \
             machine must not start"
        )),
    }
}

/// Bind the local control socket, replacing a stale one.
fn bind_control(path: &PathBuf) -> Result<std::os::unix::net::UnixListener, String> {
    let dir = path.parent().ok_or("the control socket has no directory")?;
    apex_agent_core::paths::ensure_private_dir(dir)
        .map_err(|e| format!("{}: {e}", dir.display()))?;
    // A socket left behind by a killed process refuses `bind` with
    // AddrInUse, so it is removed first — but only after checking that
    // nothing is listening on it, or a second instance would take the first
    // one's socket away.
    if std::os::unix::net::UnixStream::connect(path).is_ok() {
        return Err(format!(
            "another apex-remoted is already listening on {}",
            path.display()
        ));
    }
    let _ = std::fs::remove_file(path);
    std::os::unix::net::UnixListener::bind(path).map_err(|e| format!("{}: {e}", path.display()))
}

/// What the QR code calls this machine.
///
/// The kernel hostname, which is what the owner already calls it everywhere
/// else. Falls back to a generic name rather than to an empty string, because
/// a device list with a blank entry in it is worse than one with `apex` in it.
fn machine_name() -> String {
    std::fs::read_to_string("/proc/sys/kernel/hostname")
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "apex".to_string())
}

fn flag(args: &[String], name: &str) -> Option<String> {
    let i = args.iter().position(|a| a == name)?;
    args.get(i + 1).cloned()
}

/// The service's own static secret, for the handshake.
///
/// A free function rather than a method so that `State` does not have to
/// expose it: nothing but the two handshake constructors ever needs it, and
/// both are in `serve`.
fn secret_of(state: &State) -> [u8; 32] {
    state.identity.secret_bytes_for_handshake()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_default_port_is_the_one_the_firewall_catalogue_names() {
        // The catalogue is the user-facing name for this port, and the two
        // drifting apart means `apex firewall allow apex-remote` opens a port
        // nothing is listening on.
        let catalogue = include_str!("../../../files/system/firewall/services");
        let line = catalogue
            .lines()
            .find(|l| l.starts_with("apex-remote "))
            .expect("apex-remote is not in the firewall catalogue");
        let port: u16 = line
            .split_whitespace()
            .nth(2)
            .and_then(|p| p.parse().ok())
            .unwrap_or_else(|| panic!("no port in {line:?}"));
        assert_eq!(port, DEFAULT_PORT);
        assert!(line.contains("tcp"), "{line}");
    }

    #[test]
    fn the_handshake_deadline_cannot_be_switched_off() {
        // It is a denial-of-service guard, so a flag that could raise it to an
        // hour or drop it to nothing would be a way to ask for the bug it
        // exists to prevent.
        let clamp = |ms: u64| std::time::Duration::from_millis(ms.clamp(1_000, 120_000));
        assert_eq!(clamp(0), std::time::Duration::from_secs(1));
        assert_eq!(clamp(u64::MAX), std::time::Duration::from_secs(120));
        assert_eq!(clamp(2_000), std::time::Duration::from_secs(2));
        assert!(serve::HANDSHAKE_TIMEOUT >= std::time::Duration::from_secs(1));
        assert!(serve::HANDSHAKE_TIMEOUT <= std::time::Duration::from_secs(120));
    }

    #[test]
    fn the_usage_says_what_allow_foreground_gives_up() {
        // A flag that turns off a security check has to say so where the
        // person turning it on will read it.
        assert!(USAGE.contains("--allow-foreground"));
        assert!(USAGE.contains("login session"));
        assert!(USAGE.contains("claude-remote-control"));
    }

    #[test]
    fn a_machine_name_is_never_empty() {
        let name = machine_name();
        assert!(!name.trim().is_empty());
        assert!(!name.contains('\n'), "{name:?}");
    }

    #[test]
    fn flags_are_read_by_name_and_not_by_position() {
        let args: Vec<String> = ["--relay", "https://r.invalid", "--port", "9000"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        assert_eq!(flag(&args, "--port").as_deref(), Some("9000"));
        assert_eq!(flag(&args, "--relay").as_deref(), Some("https://r.invalid"));
        assert_eq!(flag(&args, "--missing"), None);
        // A flag at the end with no value is absent rather than a panic.
        assert_eq!(flag(&["--port".to_string()], "--port"), None);
    }

    #[test]
    fn the_placement_guard_refuses_a_login_session_and_says_what_to_run() {
        // Runs against this process's real cgroup. The assertion is the rule
        // rather than a fixed answer: under `cargo test` in a terminal this
        // is a login session and must be refused, and in a container under a
        // user service it must be allowed.
        match (
            apex_agent_core::origin::is_a_user_service(),
            guard_placement(false),
        ) {
            (Ok(true), Ok(())) => {}
            (Ok(true), Err(e)) => panic!("a user service was refused: {e}"),
            (Ok(false), Err(e)) => {
                assert!(e.contains("apex remote enable"), "{e}");
                assert!(e.contains("a human at this machine"), "{e}");
            }
            (Ok(false), Ok(())) => panic!("a login session was allowed to start the proxy"),
            (Err(_), Err(e)) => assert!(e.contains("must not start"), "{e}"),
            (Err(why), Ok(())) => panic!("unclassifiable ({why}) and still started"),
        }
        // And the override is an override, wherever this runs.
        assert!(guard_placement(true).is_ok());
    }
}
