//! `Request::Inject`: hand a file to a running session (P1-035).
//!
//! The pure half — what a copied file is called, where it lands and which
//! bytes reach the PTY — is [`apex_agent_core::inject`], and its module
//! documentation is where the reasoning about the injection surface lives.
//! This file is the I/O and the refusals.
//!
//! ## Why the refusal at the top is the important line in this file
//!
//! The daemon runs outside every sandbox and reads the source file with the
//! daemon's own access. For the human at the keyboard that is exactly the
//! point: `~/Pictures/Screenshots` is masked by the tmpfs over `$HOME`, the
//! session cannot see it, and the daemon can carry the file across.
//!
//! For a *session* it would be a complete escape. `apex agent send 4
//! ~/.ssh/id_ed25519`, issued from inside session 3, would have the daemon
//! read a file the sandbox exists to hide and drop it somewhere a session can
//! read. So the caller is resolved through `SO_PEERCRED` and `/proc` ancestry
//! the way [`crate::privilege::decide`] resolves it, and anything that lands on
//! a managed session is refused — including a session naming *itself*, because
//! the file it wants is one it cannot reach and the refusal is about the read,
//! not about the destination.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use apex_agent_core::inject;
use apex_agent_core::journal;
use apex_agent_core::paths;
use apex_agent_core::protocol::{ErrorKind, Response};

use crate::peer::Peer;
use crate::privilege;
use crate::pty;
use crate::Daemon;

/// Largest file that may be handed to a session.
///
/// The destination is the session scratch under `/tmp`, which on this image is
/// a tmpfs sized from RAM, and the read happens in one buffer. 32 MiB is far
/// above any screenshot, a crash dump or a log tail, and far below a number
/// that would let one command cost the machine its memory. A refusal names the
/// limit, so the answer to a bigger file is `--allow` on the session and a
/// path, not a bigger constant here.
pub const MAX_BYTES: u64 = 32 * 1024 * 1024;

/// Handle one `Inject`.
pub fn handle(daemon: &Arc<Daemon>, creds: Option<Peer>, id: u32, source: &str) -> Response {
    // 1. Who is asking. See the module note: this is the boundary.
    let who = privilege::origin(daemon, creds);
    if let Some(session) = who.session {
        return Response::error(
            ErrorKind::PermissionDenied,
            format!(
                "session {session} may not hand a file to a session. This verb reads the source \
                 with the daemon's own access, which is outside every sandbox — a session that \
                 could ask for it could ask for anything the sandbox hides"
            ),
        );
    }

    // 2. What is being handed over. Absolute, because the daemon's working
    //    directory is not the caller's and resolving a relative path here
    //    would silently name a different file.
    let src = Path::new(source);
    if !src.is_absolute() {
        return Response::error(
            ErrorKind::BadRequest,
            format!("{source} is not an absolute path, and the daemon's working directory is not yours"),
        );
    }
    let safe = match src
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or(inject::NameError::NoFileName)
        .and_then(inject::safe_name)
    {
        Ok(name) => name,
        Err(e) => return Response::error(ErrorKind::BadRequest, e.to_string()),
    };

    let bytes = match read_regular_file(src) {
        Ok(b) => b,
        Err(e) => return Response::error(ErrorKind::BadRequest, e),
    };

    // 3. The session. Everything from here is under its lock: the sequence
    //    number, the copy and the write to the master have to be one step, or
    //    two `send`s at once can pick the same name and the second overwrites a
    //    file whose path has already been typed.
    let Some(handle) = daemon.registry.lock().expect("registry lock").get(id) else {
        return Response::error(ErrorKind::NoSuchSession, format!("no session {id}"));
    };
    let mut s = handle.lock().expect("session lock");
    if !s.info.is_live() {
        return Response::error(
            ErrorKind::SessionExited,
            format!("session {id} has already exited, so there is nothing reading its terminal"),
        );
    }

    let scratch = scratch_for(&s, id);
    let seq = s.info.injected.saturating_add(1);
    let dest = inject::destination(&scratch, seq, &safe);

    // 4. The text, checked at the point of the write rather than trusted from
    //    the construction above.
    let Some(text) = inject::payload(&dest) else {
        return Response::error(
            ErrorKind::Internal,
            format!(
                "refusing to type {} into a terminal: it is not a plain path",
                dest.display()
            ),
        );
    };

    if let Some(parent) = dest.parent() {
        if let Err(e) = paths::ensure_private_dir(parent) {
            return Response::error(
                ErrorKind::Internal,
                format!("preparing {}: {e}", parent.display()),
            );
        }
    }
    if let Err(e) = std::fs::write(&dest, &bytes) {
        return Response::error(
            ErrorKind::Internal,
            format!("writing {}: {e}", dest.display()),
        );
    }

    // 5. The keystrokes. Bracketed only when this program asked for the mode.
    let bracketed = s.scanner.bracketed_paste();
    let keys = inject::keystrokes(&text, bracketed);
    if let Err(e) = pty::write_all(s.master, &keys) {
        // The copy is announced by being typed. A copy nobody was told about
        // is litter in a directory the agent can read, so it goes with the
        // failure rather than staying behind it.
        let _ = std::fs::remove_file(&dest);
        return Response::error(
            ErrorKind::Internal,
            format!("writing to session {id}'s terminal: {e}"),
        );
    }
    s.info.injected = seq;
    drop(s);

    // 6. The record the session cannot edit. Best-effort by construction —
    //    see `journal` — and after the write, so it records what happened
    //    rather than what was about to.
    journal::send(
        &format!("handed {source} to agent session {id} as {text}"),
        &[
            ("APEX_INJECT_SESSION", id.to_string()),
            ("APEX_INJECT_SOURCE", source.to_string()),
            ("APEX_INJECT_DEST", text.clone()),
            ("APEX_INJECT_BYTES", bytes.len().to_string()),
        ],
    );

    Response::Injected {
        id,
        path: text,
        bracketed,
    }
}

/// The directory this session's sandbox actually bound read-write.
///
/// Taken from the confinement the session was started with, not recomputed
/// from the id: the daemon canonicalises the scratch path before binding it,
/// so on a machine where `/tmp` is a symlink the bound path and the derived
/// one are different strings for the same directory — and the one the agent
/// can open is the bound one. Falls back to the derived path for a session with
/// no recorded confinement, which is an unconfined one, where the two are the
/// same directory and nothing is masked anyway.
fn scratch_for(s: &crate::registry::Session, id: u32) -> PathBuf {
    s.confinement
        .as_ref()
        .map(|c| c.spec.scratch.clone())
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| paths::scratch_dir(id))
}

/// Read a regular file, refusing anything that is not one.
///
/// The type is checked TWICE and the open is non-blocking, and both of those
/// are the same defect: `File::open` on a FIFO **blocks until a writer
/// arrives**, so a daemon that opened first and asked what it had opened
/// second would park a thread for as long as nobody wrote — a path to a FIFO
/// is a denial of service on the agent runtime, from any caller, with no
/// privilege. Found by the test below, which hung.
///
/// So the path is stated first and a non-regular file never reaches an
/// `open` at all; the descriptor is then checked again, because a path can
/// become something else between the two calls, and `O_NONBLOCK` is what makes
/// that second check reachable rather than a comment about a call that already
/// blocked. `O_NONBLOCK` costs a regular file nothing: on Linux a read from
/// one never returns `EAGAIN`.
///
/// The read is bounded independently of the reported length, because a file
/// may grow between the stat and the read.
fn read_regular_file(src: &Path) -> Result<Vec<u8>, String> {
    use std::os::unix::fs::OpenOptionsExt;

    let stated = std::fs::metadata(src).map_err(|e| format!("{}: {e}", src.display()))?;
    refuse_unless_regular(src, &stated)?;

    let file = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NONBLOCK)
        .open(src)
        .map_err(|e| format!("{}: {e}", src.display()))?;
    let meta = file
        .metadata()
        .map_err(|e| format!("{}: {e}", src.display()))?;
    refuse_unless_regular(src, &meta)?;

    if meta.len() > MAX_BYTES {
        return Err(format!(
            "{} is {} bytes; the limit for handing a file to a session is {MAX_BYTES}",
            src.display(),
            meta.len()
        ));
    }
    let mut bytes = Vec::with_capacity(meta.len() as usize);
    file.take(MAX_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| format!("{}: {e}", src.display()))?;
    if bytes.len() as u64 > MAX_BYTES {
        return Err(format!(
            "{} grew past the {MAX_BYTES}-byte limit while it was being read",
            src.display()
        ));
    }
    Ok(bytes)
}

/// The one rule, applied to a stat from either side of the open.
fn refuse_unless_regular(src: &Path, meta: &std::fs::Metadata) -> Result<(), String> {
    if meta.is_dir() {
        return Err(format!(
            "{} is a directory; hand over a file, or an archive of it",
            src.display()
        ));
    }
    if !meta.is_file() {
        return Err(format!(
            "{} is not a regular file, and reading it could block this daemon indefinitely",
            src.display()
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A private directory of this test's own, named for the process so
    /// parallel runs cannot collide. The repository's idiom; no dev-dependency
    /// is added for it.
    fn tmpdir(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!(
            "apex-inject-{tag}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).expect("tempdir");
        d
    }

    #[test]
    fn a_directory_is_refused_by_name_rather_than_read() {
        let dir = std::env::temp_dir();
        let err = read_regular_file(&dir).expect_err("a directory is not a file");
        assert!(err.contains("is a directory"), "{err}");
    }

    #[test]
    fn a_fifo_is_refused_before_it_can_block_the_daemon() {
        // The reason the type is checked before the open. There is deliberately
        // NO writer opened here: a FIFO with no writer is the case that hangs,
        // and a test that arranged a writer would assert the safe case only.
        // If this ever regresses, the symptom is this test never finishing.
        let dir = tmpdir("fifo");
        let fifo = dir.join("pipe");
        let c = std::ffi::CString::new(fifo.to_string_lossy().as_bytes()).unwrap();
        // Safe: a null-terminated path this test owns.
        let rc = unsafe { libc::mkfifo(c.as_ptr(), 0o600) };
        assert_eq!(rc, 0, "mkfifo failed: {}", std::io::Error::last_os_error());

        let err = read_regular_file(&fifo).expect_err("a fifo is not a regular file");
        assert!(err.contains("not a regular file"), "{err}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_file_past_the_limit_is_refused_and_the_limit_is_named() {
        let dir = tmpdir("big");
        let big = dir.join("big.bin");
        let f = std::fs::File::create(&big).expect("create");
        f.set_len(MAX_BYTES + 1).expect("grow");
        drop(f);
        let err = read_regular_file(&big).expect_err("past the limit");
        assert!(err.contains(&MAX_BYTES.to_string()), "{err}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_ordinary_file_comes_back_byte_for_byte() {
        let dir = tmpdir("plain");
        let path = dir.join("shot.png");
        let content = b"\x89PNG\r\n\x1a\nnot really a png".to_vec();
        std::fs::write(&path, &content).expect("write");
        assert_eq!(read_regular_file(&path).expect("read"), content);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
