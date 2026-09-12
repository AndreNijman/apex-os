//! Announcing this machine on the local network, for as long as it is running.
//!
//! ## Why not a service file
//!
//! The obvious way to advertise a service over mDNS is to drop an XML file in
//! `/etc/avahi/services/`. It is also wrong here, and the design note for
//! P1-050 already said so: a static file advertises the machine whether or not
//! `apex-remoted` is running, whether or not the user has ever enabled APEX
//! Remote, and whether or not the port is open. A machine that has never been
//! paired with anything would be telling every network it joins that it has a
//! remote-control service on port 7717.
//!
//! So the record is published by the running service and goes away with it.
//! That is not a convention this module maintains by remembering to clean up;
//! it is what the mechanism does.
//!
//! ## Why a child process and not D-Bus
//!
//! `avahi-publish-service` holds its registration for exactly as long as it
//! runs, which is the property this needs, and it costs no dependency.
//! Speaking to `org.freedesktop.Avahi` directly would mean `zbus` and a
//! runtime inside a daemon that has neither — `apex-remoted`'s whole
//! dependency list is `apex-remote-core, apex-agent-core, anyhow, serde,
//! serde_json, libc` — to build the same `EntryGroup` the tool builds. It is
//! the same trade `net::local_addresses` took when it called `getifaddrs`
//! rather than adding three crates for a list of IP addresses, and the same
//! one `apex-secretd` took when it shelled out to `curl` rather than linking
//! TLS.
//!
//! The child is killed on the way out **and** asks the kernel to kill it if
//! this process dies without getting the chance. Without the second half, a
//! `SIGKILL`ed daemon would leave a process advertising a service that is no
//! longer there until the next reboot.
//!
//! ## What the record says, and what it deliberately does not
//!
//! The service name is this machine's name, which avahi is already publishing
//! as `<hostname>.local` on every network it joins, so the record adds no
//! identifier that was not already on the wire. The TXT record carries the
//! protocol version and **nothing else** — in particular not the rendezvous
//! id, which is stable across networks and would let anybody on a café
//! network recognise this laptop again next week.
//!
//! A device therefore learns an address and a port and nothing about which
//! machine it is. That costs a device with several APEX machines on one
//! network a few failed handshakes, and it costs nothing else: `Noise_IK` is
//! what decides whether the far end is the desktop that was paired, and a
//! wrong desktop cannot read the attempt.

use std::process::{Child, Command};

use crate::state::State;

/// The DNS-SD service type.
///
/// Registered nowhere and not meant to be: it is an underscore-prefixed
/// private type, which is what DNS-SD says to use for a service that is not
/// in IANA's list.
pub const SERVICE: &str = "_apex-remote._tcp";

/// The tool that holds the registration.
pub const PUBLISH: &str = "avahi-publish-service";

/// Where to find it. Overridable so a test can point at a stub instead of
/// putting a real record on whatever network the machine is on.
///
/// Deliberately an environment variable and not a flag: it is a testing seam,
/// not a thing to configure, and the same shape the shell suites use for
/// `APEX_DISPLAY_ENGINE`.
pub const PUBLISH_ENV: &str = "APEX_REMOTE_AVAHI_PUBLISH";

/// The command line, as a value, so it can be asserted about without running
/// anything.
pub fn argv(machine: &str, port: u16, version: u32) -> Vec<String> {
    vec![
        machine.to_string(),
        SERVICE.to_string(),
        port.to_string(),
        // One TXT record. See the module note for what is NOT in it.
        format!("v={version}"),
    ]
}

/// A live registration. Dropping it takes the record off the network.
pub struct Announcement {
    child: Child,
}

impl Drop for Announcement {
    fn drop(&mut self) {
        // `kill` then `wait`: without the wait this leaves a zombie for the
        // life of the daemon, which is a process table entry per restart of
        // something that is not supposed to restart.
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Put this machine on the local network.
///
/// `None` when the tool is not installed, which is a degraded machine and not
/// a failed one: LAN addresses still travel in the QR code, which is how a
/// device found this machine before any of this existed.
pub fn announce(state: &State) -> Option<Announcement> {
    let program = std::env::var(PUBLISH_ENV).unwrap_or_else(|_| PUBLISH.to_string());
    let args = argv(
        &state.machine,
        state.port,
        apex_remote_core::REMOTE_PROTOCOL_VERSION,
    );
    let mut command = Command::new(&program);
    command
        .args(&args)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());

    // If this process dies without running `Drop` — `SIGKILL`, a panic in a
    // thread that takes the process down, an OOM kill — the kernel sends the
    // child a `SIGTERM` instead. Without it, a machine that was killed rather
    // than stopped goes on advertising a service that is not there.
    //
    // Safe: `pre_exec` runs between fork and exec in the child, and `prctl`
    // with `PR_SET_PDEATHSIG` is async-signal-safe. Nothing here allocates.
    unsafe {
        use std::os::unix::process::CommandExt;
        command.pre_exec(|| {
            libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGTERM);
            Ok(())
        });
    }

    match command.spawn() {
        Ok(child) => {
            eprintln!(
                "apex-remoted: announcing {SERVICE} on this network as {:?} port {}",
                state.machine, state.port
            );
            Some(Announcement { child })
        }
        Err(e) => {
            eprintln!(
                "apex-remoted: not announcing on this network ({program}: {e}); a device can \
                 still reach this machine at the addresses in its pairing code"
            );
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_record_advertises_the_port_this_daemon_is_actually_on() {
        // The firewall catalogue's 7717 is the DEFAULT, not the port. A
        // record that hardcoded it would send every device on the network to
        // a port nothing is listening on the moment somebody passes --port.
        let args = argv("l16", 9312, 1);
        assert_eq!(args[2], "9312");
        assert_ne!(args[2], crate::DEFAULT_PORT.to_string());
    }

    #[test]
    fn the_service_type_is_the_one_the_design_note_names() {
        assert_eq!(SERVICE, "_apex-remote._tcp");
        // Underscore-prefixed and _tcp, which is what DNS-SD requires of a
        // private service type. A type without them is not browsable.
        assert!(SERVICE.starts_with('_'));
        assert!(SERVICE.ends_with("._tcp"));
        assert_eq!(argv("l16", 1, 1)[1], SERVICE);
    }

    #[test]
    fn nothing_stable_about_this_machine_goes_on_the_wire() {
        // The leak this avoids: the rendezvous id is stable across networks,
        // so a record carrying it would let anybody on a café network
        // recognise this laptop again next week. A device does not need it —
        // Noise_IK is what identifies the machine — so it is not published.
        let key = [0x5au8; 32];
        let rendezvous = apex_remote_core::rendezvous::rendezvous_id(&key);
        let identity = apex_remote_core::b64_encode(&key);
        let args = argv("l16", 7717, 1);
        let whole = args.join(" ");
        assert!(!whole.contains(&rendezvous), "the rendezvous id is on the wire: {whole}");
        assert!(!whole.contains(&identity), "the public key is on the wire: {whole}");
        // And exactly one TXT record, so a later addition is a deliberate act
        // rather than something that slid in.
        let txt: Vec<&String> = args.iter().skip(3).collect();
        assert_eq!(txt.len(), 1, "{txt:?}");
        assert_eq!(txt[0], "v=1");
    }

    #[test]
    fn the_version_published_is_the_one_this_build_speaks() {
        // A hardcoded 1 here would keep saying 1 after the protocol moved,
        // and a device would connect to a desktop it cannot talk to.
        let args = argv("l16", 7717, apex_remote_core::REMOTE_PROTOCOL_VERSION);
        assert_eq!(
            args[3],
            format!("v={}", apex_remote_core::REMOTE_PROTOCOL_VERSION)
        );
    }
}
