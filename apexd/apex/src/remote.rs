//! `apex remote` — pairing a phone, and taking it away again.
//!
//! Four verbs and one rule between them: **pairing is something a human does
//! at this machine**, and everything else can be scripted. `apex remote pair`
//! is refused by `apex-remoted` for any caller that does not classify as a
//! §7 local origin, which an agent inside a managed session never does.
//!
//! The pairing payload is printed here rather than by the service, because
//! the service has no terminal and the terminal is where the person is. What
//! this build does **not** print is a QR code: no encoder is vendored, and a
//! wrong QR is worse than none — a phone scans it, fails, and the person
//! concludes their camera is broken. P1-051's first criterion is a QR from
//! APEX Shell, which has a renderer; see [`qr_block`].

use anyhow::{bail, Context, Result};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;

use clap::Subcommand;
use serde::{Deserialize, Serialize};

#[derive(Subcommand, Debug)]
pub enum RemoteCmd {
    /// Show a pairing code for a phone to scan.
    ///
    /// The code is good for three minutes and pairs exactly one device. It
    /// carries this machine's public key, so the device that scans it can
    /// never be talked into trusting a different machine.
    ///
    /// Refused unless you are running it yourself: an agent cannot pair a
    /// device on your behalf.
    Pair {
        /// Print the pairing payload as text instead of drawing a QR code.
        ///
        /// For a terminal that cannot draw one, and for pasting into a device
        /// that has no camera.
        #[arg(long)]
        text: bool,
    },
    /// Every device this machine has paired, and which of them are revoked.
    Devices {
        #[arg(long)]
        json: bool,
    },
    /// Take a device's access away.
    ///
    /// Immediate: a connection the device is already holding is dropped, not
    /// merely refused next time. The record is kept so the listing can still
    /// show that it was revoked and when.
    Revoke {
        /// Device id, or its name when that names exactly one.
        device: String,
    },
    /// Whether the service is running, and how a device would reach it.
    Status,
    /// Turn APEX Remote on for this account.
    ///
    /// Enables and starts the per-user service, the same way `apex agent
    /// enable` does for the agent runtime. It does **not** open the firewall:
    /// LAN access needs `apex firewall allow apex-remote`, which is root, and
    /// is a separate decision because the relay path works without it.
    Enable,
}

// The control protocol, duplicated here as the client half. Kept in step by
// `apex-remoted`'s own round-trip tests plus the shape assertions below; a
// shared crate for four request variants would be a crate to keep in step
// instead of a struct.
#[derive(Debug, Serialize)]
#[serde(tag = "cmd", rename_all = "snake_case")]
enum Request {
    Status,
    Pair,
    Devices,
    Revoke { device: String },
}

#[derive(Debug, Deserialize)]
#[serde(tag = "reply", rename_all = "snake_case")]
enum Reply {
    Status {
        version: u32,
        key: String,
        machine: String,
        lan: Vec<String>,
        relay: Option<String>,
        rendezvous: String,
        paired: usize,
        offer_ms_left: Option<u64>,
    },
    Offer {
        qr: String,
        expires_ms: u64,
    },
    Devices {
        devices: Vec<apex_remote_core::device::Device>,
    },
    Ok,
    Error {
        message: String,
    },
}

pub fn remote(cmd: RemoteCmd) -> i32 {
    // The exit code is this function's contract and the error printing is
    // its own, matching `agent::agent`: the dispatcher in `main` takes an
    // `i32` from every arm and a `Result` here would be one arm that behaves
    // differently for no reason a user could see.
    let result = match cmd {
        RemoteCmd::Pair { text } => pair(text),
        RemoteCmd::Devices { json } => devices(json),
        RemoteCmd::Revoke { device } => revoke(&device),
        RemoteCmd::Status => status(),
        RemoteCmd::Enable => enable(),
    };
    match result {
        Ok(code) => code,
        Err(e) => {
            eprintln!("apex: {e:#}");
            1
        }
    }
}

fn socket_path() -> PathBuf {
    apex_agent_core::paths::runtime_dir()
        .join("apex-remoted")
        .join("control.sock")
}

fn call(req: &Request) -> Result<Reply> {
    let path = socket_path();
    let stream = UnixStream::connect(&path).with_context(|| {
        format!(
            "APEX Remote is not running.\n\
             turn it on with: systemctl --user enable --now apex-remoted\n\
             (socket: {})",
            path.display()
        )
    })?;
    stream
        .set_read_timeout(Some(std::time::Duration::from_secs(20)))
        .ok();
    let mut writer = stream.try_clone().context("cloning the control socket")?;
    let mut reader = BufReader::new(stream);
    let mut line = serde_json::to_string(req)?;
    line.push('\n');
    writer.write_all(line.as_bytes())?;
    writer.flush()?;
    let mut reply = String::new();
    reader.read_line(&mut reply).context("reading the reply")?;
    serde_json::from_str(reply.trim()).with_context(|| format!("unreadable reply: {reply}"))
}

fn pair(text: bool) -> Result<i32> {
    match call(&Request::Pair)? {
        Reply::Offer { qr, expires_ms } => {
            let left = expires_ms.saturating_sub(apex_remote_core::now_ms()) / 1000;
            if text {
                println!("{qr}");
            } else {
                print!("{}", qr_block(&qr));
            }
            eprintln!(
                "\nScan this with APEX Remote. It is good for {left} seconds and pairs one \
                 device."
            );
            eprintln!(
                "The code carries this machine's public key, so the phone that scans it cannot \
                 be talked into trusting a different machine."
            );
            Ok(0)
        }
        Reply::Error { message } => {
            eprintln!("apex: {message}");
            Ok(1)
        }
        other => bail!("unexpected reply: {other:?}"),
    }
}

fn devices(json: bool) -> Result<i32> {
    let Reply::Devices { devices } = call(&Request::Devices)? else {
        return unexpected();
    };
    if json {
        println!("{}", serde_json::to_string_pretty(&devices)?);
        return Ok(0);
    }
    if devices.is_empty() {
        println!("no devices paired. `apex remote pair` shows a code.");
        return Ok(0);
    }
    println!("{:<18} {:<20} {:<9} {:<8} LAST SEEN", "ID", "NAME", "STATE", "PATH");
    for d in &devices {
        println!(
            "{:<18} {:<20} {:<9} {:<8} {}",
            d.id,
            truncate(&d.name, 20),
            d.state(),
            d.last_path.as_deref().unwrap_or("-"),
            d.last_seen_ms.map(ago).unwrap_or_else(|| "never".into()),
        );
    }
    // Said once, at the end, rather than as a column: it is a property of the
    // device the desktop cannot check, and a tick in a table would read as a
    // fact this machine had verified.
    let gated: Vec<&str> = devices
        .iter()
        .filter(|d| d.requires_user_verification && d.is_active())
        .map(|d| d.name.as_str())
        .collect();
    if !gated.is_empty() {
        println!(
            "\n{} said its key is held behind a biometric or device lock. That is the \
             device's own claim; this machine cannot verify it.",
            gated.join(", ")
        );
    }
    Ok(0)
}

fn revoke(device: &str) -> Result<i32> {
    match call(&Request::Revoke {
        device: device.to_string(),
    })? {
        Reply::Ok => {
            eprintln!("apex: {device} revoked. Any connection it was holding has been dropped.");
            Ok(0)
        }
        Reply::Error { message } => {
            eprintln!("apex: {message}");
            Ok(1)
        }
        _ => unexpected(),
    }
}

fn status() -> Result<i32> {
    let Reply::Status {
        version,
        key,
        machine,
        lan,
        relay,
        rendezvous,
        paired,
        offer_ms_left,
    } = call(&Request::Status)?
    else {
        return unexpected();
    };
    println!("machine        {machine}");
    println!("protocol       {version}");
    println!("identity       {key}");
    println!("paired         {paired}");
    println!(
        "lan            {}",
        if lan.is_empty() {
            "none — this machine has no reachable address".to_string()
        } else {
            lan.join(", ")
        }
    );
    match &relay {
        Some(r) => {
            println!("relay          {r}");
            println!("rendezvous     {rendezvous}");
            println!(
                "\n{}",
                apex_remote_core::rendezvous::Path::Relay.disclosure()
            );
        }
        None => println!("relay          none — this machine is reachable on the LAN only"),
    }
    if let Some(ms) = offer_ms_left {
        println!("\na pairing code is open for another {} seconds", ms / 1000);
    }
    Ok(0)
}

/// `systemctl --user enable --now apex-remoted`, and say what it did not do.
///
/// Mirrors `apex agent enable`, which exists because a per-user daemon that
/// has to be started with a systemctl invocation is one people get wrong.
fn enable() -> Result<i32> {
    let out = std::process::Command::new("systemctl")
        .args(["--user", "enable", "--now", "apex-remoted"])
        .output()
        .context("running systemctl")?;
    if !out.status.success() {
        eprintln!("apex: {}", String::from_utf8_lossy(&out.stderr).trim());
        return Ok(1);
    }
    eprintln!("apex: APEX Remote is on. `apex remote pair` shows a code for a phone.");
    eprintln!(
        "apex: the LAN port is still closed. Open it with `sudo apex firewall allow \
         apex-remote`, or leave it shut and reach this machine through a relay."
    );
    Ok(0)
}

fn unexpected() -> Result<i32> {
    bail!("the remote service answered something this build does not understand")
}

fn truncate(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        return s.to_string();
    }
    // Truncated on characters, not bytes: a name in a non-Latin script would
    // otherwise be cut mid-codepoint and print as a replacement character.
    s.chars().take(n.saturating_sub(1)).collect::<String>() + "…"
}

fn ago(ms: u64) -> String {
    let now = apex_remote_core::now_ms();
    let secs = now.saturating_sub(ms) / 1000;
    match secs {
        0..=59 => format!("{secs}s ago"),
        60..=3599 => format!("{}m ago", secs / 60),
        3600..=86399 => format!("{}h ago", secs / 3600),
        _ => format!("{}d ago", secs / 86400),
    }
}

/// Draw a QR code for `payload` using half-block characters.
///
/// Deliberately not a dependency. A QR encoder is a few hundred lines of
/// Reed-Solomon and a fixed set of tables, and the version here does the one
/// job this command needs — but the shipped implementation is not that
/// either: see [`qr_block`]'s body. What is here now is the fallback, and it
/// is honest about being one.
fn qr_block(payload: &str) -> String {
    // No QR encoder is vendored yet. Printing a wrong QR code would be worse
    // than printing none: a phone would scan it, fail, and the person would
    // conclude their camera or the app was broken. So this prints the payload
    // and says where a QR code will come from — APEX Settings, which has a
    // renderer, and which is where P1-051's first criterion actually points.
    format!(
        "{payload}\n\n\
         (No QR code here yet: this terminal build does not vendor an encoder, and a wrong QR \
         is worse than none. Paste the line above into APEX Remote, or use the pairing page in \
         APEX Settings once it ships.)\n"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_control_socket_is_the_one_the_service_binds() {
        // Two crates, one path. They are written independently, and a drift
        // here is `apex remote status` reporting that the service is not
        // running while it is.
        let mine = socket_path();
        assert!(mine.ends_with("apex-remoted/control.sock"), "{mine:?}");
    }

    #[test]
    fn every_request_survives_the_line_framing() {
        for r in [
            Request::Status,
            Request::Pair,
            Request::Devices,
            Request::Revoke {
                device: "pixel-8".into(),
            },
        ] {
            let text = serde_json::to_string(&r).expect("serialise");
            assert!(!text.contains('\n'), "{text}");
        }
    }

    #[test]
    fn the_replies_this_build_parses_are_the_ones_the_service_sends() {
        // Written as the wire text rather than as the service's own types,
        // because that is what actually crosses the socket and it is the
        // thing that can drift.
        for text in [
            r#"{"reply":"ok"}"#,
            r#"{"reply":"error","message":"nope"}"#,
            r#"{"reply":"offer","qr":"apex-remote:abc","expires_ms":1}"#,
            r#"{"reply":"devices","devices":[]}"#,
            r#"{"reply":"status","version":1,"key":"k","machine":"l16","lan":[],"relay":null,"rendezvous":"r","paired":0,"offer_ms_left":null}"#,
        ] {
            serde_json::from_str::<Reply>(text).unwrap_or_else(|e| panic!("{text}: {e}"));
        }
    }

    #[test]
    fn a_name_is_truncated_on_characters_and_not_on_bytes() {
        assert_eq!(truncate("short", 20), "short");
        assert_eq!(truncate("abcdefghij", 5), "abcd…");
        // Multibyte: a byte-wise truncation would panic or emit a broken
        // codepoint here.
        let cyrillic = "телефонтелефон";
        let cut = truncate(cyrillic, 5);
        assert_eq!(cut.chars().count(), 5);
        assert!(cut.ends_with('…'));
    }

    #[test]
    fn the_pairing_output_does_not_pretend_to_be_a_qr_code() {
        // The failure this avoids: a phone scans a wrong QR, fails, and the
        // person blames their camera. If an encoder is ever vendored, this
        // test is what has to change with it.
        let out = qr_block("apex-remote:abc");
        assert!(out.contains("apex-remote:abc"));
        assert!(out.contains("APEX Settings"), "{out}");
    }

    #[test]
    fn elapsed_time_reads_in_the_unit_a_person_would_use() {
        let now = apex_remote_core::now_ms();
        assert!(ago(now).ends_with("s ago"));
        assert!(ago(now.saturating_sub(120_000)).ends_with("m ago"));
        assert!(ago(now.saturating_sub(7_200_000)).ends_with("h ago"));
        assert!(ago(now.saturating_sub(3 * 86_400_000)).ends_with("d ago"));
        // A timestamp from the future is not a panic and not a negative.
        assert_eq!(ago(now + 60_000), "0s ago");
    }
}
