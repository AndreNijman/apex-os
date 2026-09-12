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
//!
//! ## And the third caller, which arrived after this was written
//!
//! A *proxy* is neither of the two above. `apex-remoted` terminates a paired
//! phone's channel and forwards what it carries, and its `control()` is a
//! denylist of exactly one verb (`attach`) — so every other verb, this one
//! included, reached the daemon from a device that is not at this machine. The
//! session check does not fire, because a proxy is not a managed session.
//!
//! That made the escape above available to a phone rather than to an agent:
//! `{"cmd":"inject","id":N,"source":"/home/u/.ssh/id_ed25519"}` would have had
//! the daemon read the key and drop it where the session can read it. So the
//! origin is checked too, with `decide`'s own predicate — local, or refused —
//! and an origin that could not be established is refused rather than
//! defaulted.
//!
//! What that costs: nothing that works today. The only caller of this verb is
//! `apex agent send`, which is a human at a terminal. What it would cost a
//! phone handoff is nothing either, because a phone cannot name a useful host
//! path in the first place — the file it wants to hand over is on the phone.
//!
//! ## The verb for that file, which is the other half of this module
//!
//! [`handle_receive`] is `Request::Receive`: the same hand-over for bytes that
//! arrive on the connection instead of being read off this machine. It shares
//! [`deliver`] — the session lock, the sequence number, the copy and the
//! keystrokes — and deliberately does NOT share the refusals above, because
//! the refusals above are about reading a host path and that verb reads none.
//! Its gate is [`crate::privilege::refuse_input`], which is the rule about the
//! *destination*: this ends by typing into somebody's terminal.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use apex_agent_core::inject;
use apex_agent_core::journal;
use apex_agent_core::paths;
use apex_agent_core::protocol::{ErrorKind, Response};

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

/// Longest one read of an upload may block with nothing arriving.
///
/// A connection that has gone away without closing looks exactly like a slow
/// one, and without this the daemon thread waits for the first of those
/// forever. Thirty seconds is far longer than any gap a working link produces
/// and short enough that a dead one costs a thread for half a minute.
pub const STALL_SECS: u64 = 30;

/// Longest one upload may take from the takeover reply to the last byte.
///
/// The timeout above is per read, and on its own it is not a bound: a client
/// sending one byte every twenty-nine seconds satisfies it indefinitely and
/// would hold a daemon thread for days. Five minutes for 32 MiB is about
/// 110 KB/s, which is below what a poor mobile link manages, so this refuses
/// stalling rather than slowness.
pub const MAX_UPLOAD_SECS: u64 = 300;

/// Handle one `Inject`.
pub fn handle(daemon: &Arc<Daemon>, caller: &privilege::Caller, id: u32, source: &str) -> Response {
    // 1. Who is asking. See the module note: this is the boundary.
    let who = privilege::origin(daemon, caller);
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

    // 1b. Where the connection came from. The check above was written when the
    //     only two callers imaginable were a human running `apex agent send`
    //     and a session trying to escape, and it is exactly right about both.
    //     It is blind to the third, which arrived later: `apex-remoted`
    //     terminates a paired phone's channel and forwards what it carries,
    //     and its `control()` is a DENYLIST of one verb (`attach`). A proxy is
    //     not a managed session, so nothing above stops it — and `source` is
    //     an absolute path on the HOST, read with the daemon's own access,
    //     outside every sandbox.
    //
    //     So without this, a paired device could send
    //     `{"cmd":"inject","id":N,"source":"/home/u/.ssh/id_ed25519"}` and
    //     have the daemon carry that key into a session's inbox and type the
    //     path. That is the module note's own threat model — "a session that
    //     could ask for it could ask for anything the sandbox hides" — with
    //     the phone standing where the session stood.
    //
    //     The predicate is `decide`'s, deliberately: an origin that could not
    //     be established is refused rather than defaulted, and the message
    //     names what could not be read.
    let Some(asker) = who.request_origin else {
        return Response::error(
            ErrorKind::PermissionDenied,
            format!(
                "{}, so this connection cannot be shown to be at this machine — and handing over \
                 a file read with the daemon's own access is reserved for one",
                who.origin_unreadable
                    .unwrap_or_else(|| "the origin could not be established".to_string())
            ),
        );
    };
    if !asker.is_local() {
        return Response::error(
            ErrorKind::PermissionDenied,
            format!(
                "a {} connection may not hand a file to a session. `source` names a path on this \
                 machine and the daemon reads it with the daemon's own access, outside every \
                 sandbox, so a remote caller that could ask for it could ask for any file this \
                 user owns. Hand the file over from a terminal here with `apex agent send`",
                asker.origin
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

    deliver(daemon, id, &safe, bytes, source)
}

/// Copy bytes into a session's inbox and type the path, whatever produced them.
///
/// Steps 3 to 6 of [`handle`], factored out because [`handle_receive`] needs
/// exactly them and nothing above. The split is where it is because it is where
/// the difference stops: everything before it answers "may this caller hand
/// over *these* bytes", and the two verbs answer that differently — one reads a
/// host path with the daemon's own access, the other reads a connection. From
/// here down the two are the same act, and a second copy of it would be a
/// second place to forget the session lock, the sequence number, the
/// `payload` check or the removal of a file nobody was told about.
///
/// `source` is what the journal records the bytes came from. A host path for
/// [`handle`]; `upload:<name>` for [`handle_receive`], where the name is the
/// reduced one, so the field is always in [`inject::SAFE`]'s alphabet whoever
/// wrote it.
fn deliver(
    daemon: &Arc<Daemon>,
    id: u32,
    safe: &str,
    bytes: Vec<u8>,
    source: &str,
) -> Response {
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
    let dest = inject::destination(&scratch, seq, safe);

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

/// Handle one `Receive`: the same hand-over, for bytes that are not here.
///
/// ## The shape, and why it is a takeover rather than a field
///
/// This is the second verb that does not return to the request loop. The
/// daemon answers [`Response::Receiving`], reads exactly `len` bytes from the
/// same connection, and answers again with the [`Response::Injected`] that
/// [`handle`] would have produced. `apex-remoted` opens a channel for it the
/// way it opens one for a terminal, so the bytes arrive as `Frame::Data` —
/// which the transport already splits at `MAX_PAYLOAD` and reassembles as an
/// ordinary byte stream. Nothing in this file knows that, and that is the
/// point: the chunking was never missing, only a sink to chunk into.
///
/// ## The refusal is `Input`'s, not `Inject`'s, and the difference is the verb
///
/// [`handle`] refuses a managed session AND a non-local origin, because its
/// `source` is a host path read outside every sandbox. Neither applies here:
/// there is no host path, and refusing a remote origin would refuse the only
/// caller this verb exists for.
///
/// What survives is the half about the *destination*, which is identical for
/// both — this ends by typing a path into a session's terminal, exactly as
/// [`Request::Input`] does. So the gate is [`privilege::refuse_input`] itself
/// rather than a paraphrase of it: a caller that resolves to a managed session
/// is refused, and so is one this daemon cannot classify, because an
/// unclassifiable connection cannot be shown not to be a session driving
/// another session.
///
/// ## Everything refusable is refused BEFORE the takeover reply
///
/// The caller, the name, the length and the session are all checked first. A
/// phone that is told `receiving` may start sending, and a phone told anything
/// else has sent nothing — which matters most for the length: discovering a
/// file is too big after 32 MiB has crossed a mobile connection is a bill
/// somebody pays for a refusal that was decidable at the start.
///
/// ## Why there is no staging file and nothing to clean up
///
/// The bytes are buffered, bounded by [`MAX_BYTES`], and the file is written
/// only once all of them have arrived. [`handle`] already reads a whole file
/// into one `Vec` under the same bound, so this costs the daemon nothing it
/// did not already spend — and an upload that dies half-way leaves no partial
/// file in a session's inbox, no staging directory, and nothing for a reaper
/// to find. The alternative, streaming to disk, would buy a larger cap this
/// verb does not want and would owe a cleanup path for every way a connection
/// can end.
///
/// ## The two clocks
///
/// [`STALL_SECS`] bounds one read: nothing arriving for that long is a
/// connection that has gone away without saying so, and without it the thread
/// parks forever. [`MAX_UPLOAD_SECS`] bounds the whole upload, and it is the
/// one that is not obvious — a per-read timeout alone lets a client send one
/// byte just inside every window and hold a daemon thread for days. Both are
/// named in the refusal they cause, because "the upload failed" is not
/// something a user can act on and "no bytes arrived for 30s" is.
pub fn handle_receive(
    daemon: &Arc<Daemon>,
    caller: &privilege::Caller,
    mut writer: std::os::unix::net::UnixStream,
    mut reader: std::io::BufReader<std::os::unix::net::UnixStream>,
    id: u32,
    name: &str,
    len: u64,
) -> anyhow::Result<()> {
    let safe = match prepare(daemon, caller, id, name, len) {
        Ok(safe) => safe,
        Err(refusal) => return crate::session::write_response(&mut writer, &refusal),
    };

    crate::session::write_response(&mut writer, &Response::Receiving { id, len })?;

    let response = match read_body(
        &mut writer,
        &mut reader,
        len,
        Duration::from_secs(STALL_SECS),
        Duration::from_secs(MAX_UPLOAD_SECS),
    ) {
        Ok(bytes) => deliver(daemon, id, &safe, bytes, &format!("upload:{safe}")),
        Err(why) => Response::error(ErrorKind::BadRequest, why),
    };
    crate::session::write_response(&mut writer, &response)
}

/// Everything decidable before a byte of the upload has been sent.
///
/// Returns the name the copy will be given, which is never the caller's
/// string: [`inject::safe_name`] has no `/` in its alphabet, so a `name` of
/// `../../.ssh/authorized_keys` becomes `.._.._.ssh_authorized_keys` and lands
/// in the inbox like anything else. The caller chooses no part of the path.
fn prepare(
    daemon: &Arc<Daemon>,
    caller: &privilege::Caller,
    id: u32,
    name: &str,
    len: u64,
) -> Result<String, Box<Response>> {
    // Boxed for `clippy::result_large_err`: a `Response` is 160 bytes, and an
    // unboxed one would be paid for on the success path of every upload.
    let who = privilege::origin(daemon, caller);
    if let Some(refusal) = privilege::refuse_input(&who, id) {
        return Err(Box::new(refusal));
    }

    if len == 0 {
        // Not merely useless: a zero-length upload would take the connection
        // over, read nothing, and hand the session an empty file whose path is
        // typed into its terminal as though something had been sent.
        return Err(Box::new(Response::error(
            ErrorKind::BadRequest,
            "an upload of no bytes is not a file; nothing was taken over",
        )));
    }
    if len > MAX_BYTES {
        return Err(Box::new(Response::error(
            ErrorKind::BadRequest,
            format!(
                "{len} bytes is past the {MAX_BYTES}-byte limit for handing a file to a session, \
                 so nothing was read; this is refused before the upload starts rather than after"
            ),
        )));
    }

    let safe = inject::safe_name(name)
        .map_err(|e| Box::new(Response::error(ErrorKind::BadRequest, e.to_string())))?;

    // The session is checked here as well as in `deliver`, and both checks are
    // load-bearing. This one saves the upload: there is no point carrying two
    // megabytes across a mobile link to a session that exited an hour ago. The
    // one in `deliver` is the one that is correct, because it holds the lock —
    // a session can exit while the bytes are in flight, and this check cannot
    // see that.
    let Some(handle) = daemon.registry.lock().expect("registry lock").get(id) else {
        return Err(Box::new(Response::error(
            ErrorKind::NoSuchSession,
            format!("no session {id}"),
        )));
    };
    let live = handle.lock().expect("session lock").info.is_live();
    if !live {
        return Err(Box::new(Response::error(
            ErrorKind::SessionExited,
            format!("session {id} has already exited, so there is nothing reading its terminal"),
        )));
    }
    Ok(safe)
}

/// Read exactly `len` bytes, or say which way it went wrong.
///
/// Read through the `BufReader` rather than the socket, because it may already
/// hold the first of these bytes: a client is free to write the request line
/// and the body in one `write`, and `read_line` takes whatever the kernel had.
/// Reading the socket directly would silently drop that prefix and then time
/// out waiting for bytes it had already been given.
///
/// Never reads past `len`. A client that sends more leaves the surplus unread
/// and the connection is closed under it, so a declared length is a ceiling as
/// well as a promise.
///
/// `stall` and `budget` are parameters rather than the constants they are in
/// production, and that is deliberate: a test of a thirty-second timeout that
/// waits thirty seconds is a test nobody runs. [`handle_receive`] passes
/// [`STALL_SECS`] and [`MAX_UPLOAD_SECS`]; the tests pass milliseconds.
fn read_body(
    socket: &mut std::os::unix::net::UnixStream,
    reader: &mut std::io::BufReader<std::os::unix::net::UnixStream>,
    len: u64,
    stall: Duration,
    budget: Duration,
) -> Result<Vec<u8>, String> {
    socket
        .set_read_timeout(Some(stall))
        .map_err(|e| format!("the upload could not be given a read timeout: {e}"))?;
    let deadline = Instant::now() + budget;

    // `len` is at most `MAX_BYTES`; `prepare` refused anything larger before
    // this function could be reached, so the allocation is bounded by the cap
    // and not by the caller.
    let mut bytes: Vec<u8> = Vec::with_capacity(len as usize);
    let mut chunk = [0u8; 64 * 1024];
    while (bytes.len() as u64) < len {
        if Instant::now() >= deadline {
            return Err(format!(
                "the upload was still arriving after {:?} ({} of {len} bytes); nothing was \
                 written",
                budget,
                bytes.len()
            ));
        }
        let want = std::cmp::min(chunk.len() as u64, len - bytes.len() as u64) as usize;
        match reader.read(&mut chunk[..want]) {
            Ok(0) => {
                return Err(format!(
                    "the connection ended after {} of {len} bytes; nothing was written",
                    bytes.len()
                ))
            }
            Ok(n) => bytes.extend_from_slice(&chunk[..n]),
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(e)
                if e.kind() == std::io::ErrorKind::WouldBlock
                    || e.kind() == std::io::ErrorKind::TimedOut =>
            {
                return Err(format!(
                    "no bytes arrived for {:?} ({} of {len} received); nothing was written",
                    stall,
                    bytes.len()
                ))
            }
            Err(e) => return Err(format!("reading the upload: {e}")),
        }
    }
    Ok(bytes)
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

    // ── Request::Receive's body reader ──────────────────────────────────────
    //
    // A socketpair, because that is what the verb reads from and a `Cursor`
    // would answer every one of these questions the easy way: it never blocks,
    // so no timeout is reachable; it has no peer, so no half-close is
    // reachable; and it has no BufReader above it holding bytes the request
    // line already pulled in, which is the defect the first test is about.

    use std::io::{BufRead, BufReader, Write};
    use std::os::unix::net::UnixStream;

    /// The two halves a `Receive` sees: the daemon's socket, and the wrapper
    /// `serve` read the request line through.
    fn pair() -> (UnixStream, BufReader<UnixStream>, UnixStream) {
        let (daemon, client) = UnixStream::pair().expect("socketpair");
        let reader = BufReader::new(daemon.try_clone().expect("clone"));
        (daemon, reader, client)
    }

    const FOREVER: Duration = Duration::from_secs(30);

    #[test]
    fn body_bytes_the_request_line_already_pulled_in_are_not_lost() {
        // THE reason the body is read through the `BufReader` and not the
        // socket. A client is free to write `{"cmd":"receive",…}\nPNG…` in one
        // `write`, and `read_line` takes whatever the kernel had — which is all
        // of it. A `read_body` that went to the socket would find those bytes
        // already consumed, read the tail only, and then wait for a remainder
        // that was sitting in a buffer six feet away.
        let (mut daemon, mut reader, mut client) = pair();
        let body = b"\x89PNG\r\n\x1a\nnot really a png";
        let mut one_write = br#"{"cmd":"receive","id":4,"name":"shot.png","len":24}"#.to_vec();
        one_write.push(b'\n');
        one_write.extend_from_slice(body);
        client.write_all(&one_write).expect("write");

        let mut line = String::new();
        reader.read_line(&mut line).expect("request line");
        assert!(line.starts_with("{\"cmd\":\"receive\""), "{line}");

        let got = read_body(&mut daemon, &mut reader, body.len() as u64, FOREVER, FOREVER)
            .expect("the body was already buffered and must still be readable");
        assert_eq!(got, body);
    }

    #[test]
    fn an_upload_that_stops_short_is_refused_and_says_how_far_it_got() {
        // A phone that loses signal mid-upload. The daemon must not hand the
        // session half a screenshot, and the refusal has to carry the counts:
        // "the upload failed" is not something a user can act on.
        let (mut daemon, mut reader, mut client) = pair();
        client.write_all(b"half").expect("write");
        drop(client);

        let err = read_body(&mut daemon, &mut reader, 16, FOREVER, FOREVER)
            .expect_err("a short upload must not be delivered");
        assert!(err.contains("4 of 16 bytes"), "{err}");
        assert!(err.contains("nothing was written"), "{err}");
    }

    #[test]
    fn nothing_past_the_declared_length_is_read() {
        // The declared length is a ceiling as well as a promise. A client that
        // sends more must not have the surplus counted, and — the half that
        // matters — must not have it read at all: a `Receive` that swallowed
        // trailing bytes on a connection that is about to be reused would be
        // consuming somebody's next request.
        let (mut daemon, mut reader, mut client) = pair();
        client.write_all(b"ABCDEFGHsurplus").expect("write");

        let got = read_body(&mut daemon, &mut reader, 8, FOREVER, FOREVER).expect("eight bytes");
        assert_eq!(got, b"ABCDEFGH");

        let mut left = [0u8; 7];
        reader.read_exact(&mut left).expect("the surplus is still there");
        assert_eq!(&left, b"surplus", "the surplus was consumed by the upload");
    }

    #[test]
    fn a_connection_that_goes_quiet_is_refused_rather_than_parking_the_thread() {
        // A peer that stops sending without closing — a phone in a tunnel, a
        // NAT that dropped the mapping. Nothing arrives and nothing ever will,
        // and without the read timeout this thread is gone for good. The client
        // half is held open on purpose: dropping it would produce the EOF case
        // above and assert the wrong thing.
        let (mut daemon, mut reader, client) = pair();
        let err = read_body(
            &mut daemon,
            &mut reader,
            16,
            Duration::from_millis(120),
            FOREVER,
        )
        .expect_err("a silent connection must be refused");
        assert!(err.contains("no bytes arrived"), "{err}");
        assert!(err.contains("0 of 16"), "{err}");
        drop(client);
    }

    #[test]
    fn an_upload_that_trickles_past_the_budget_is_refused_though_no_single_read_stalled() {
        // The case the per-read timeout does not cover, and the reason there
        // are two clocks. Every read here arrives well inside `stall`, so a
        // reader with only that timeout would keep going — and a client sending
        // one byte just inside every window holds a daemon thread for days.
        let (mut daemon, mut reader, mut client) = pair();
        let writer = std::thread::spawn(move || {
            for _ in 0..20 {
                if client.write_all(b"x").is_err() {
                    return;
                }
                std::thread::sleep(Duration::from_millis(20));
            }
        });

        let err = read_body(
            &mut daemon,
            &mut reader,
            20,
            Duration::from_secs(5),
            Duration::from_millis(60),
        )
        .expect_err("a trickle past the budget must be refused");
        assert!(err.contains("still arriving after"), "{err}");
        assert!(err.contains("of 20 bytes"), "{err}");
        let _ = writer.join();
    }

    #[test]
    fn an_upload_delivered_in_pieces_is_reassembled_in_order() {
        // What `Frame::Data` actually looks like arriving: `apex-remoted`
        // writes each frame's bytes to this socket as it decodes them, so the
        // body reaches the daemon in however many pieces the transport split it
        // into. The read loop must not assume one read per upload.
        let (mut daemon, mut reader, mut client) = pair();
        let writer = std::thread::spawn(move || {
            for piece in [&b"one"[..], b"two", b"three"] {
                client.write_all(piece).expect("write");
                std::thread::sleep(Duration::from_millis(5));
            }
        });
        let got = read_body(&mut daemon, &mut reader, 11, FOREVER, FOREVER).expect("reassembled");
        assert_eq!(got, b"onetwothree");
        let _ = writer.join();
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
