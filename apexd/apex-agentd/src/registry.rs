//! The live session table.
//!
//! One [`Session`] per PTY, each with its own reader thread. A `Mutex` per
//! session rather than one lock over the table, so a busy agent producing
//! megabytes of output never blocks `apex agent list`.
//!
//! Locking rule, and the reason this stays deadlock-free: the registry lock is
//! only ever held long enough to clone an `Arc` out of the map. Session locks
//! are taken *after* the registry lock is released, never the other way round,
//! and no code path holds two session locks at once.

use std::collections::HashMap;
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::os::unix::io::RawFd;
use std::os::unix::net::UnixStream;
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use apex_agent_core::paths;
use apex_agent_core::protocol::{AgentState, SessionInfo};
use apex_agent_core::session::{OutputScanner, Scrollback, SCROLLBACK_BYTES};

use crate::pty;

/// Cap on a session's on-disk transcript.
///
/// An agent that loops printing output must not fill the user's home. When the
/// cap is passed the log stops growing and the fact is recorded once; the
/// in-memory scrollback keeps working, so `attach` is unaffected.
pub const LOG_LIMIT_BYTES: u64 = 32 * 1024 * 1024;

/// How long a write to an attached client may block before that client is
/// dropped.
///
/// Without this the session stalls indefinitely behind a terminal that has
/// stopped reading — a suspended client, a stalled SSH connection, a terminal
/// paused with ctrl-S. A Unix socket buffers about 176 KiB (measured on this
/// kernel) and then blocks, so a busy agent reaches that in well under a
/// second and the *agent* stops running, not just the display.
///
/// Two seconds is far longer than any healthy client needs and short enough
/// that a wedged one cannot hold up the work. The session keeps running; only
/// the unresponsive viewer is disconnected, and it can reattach.
const ATTACH_WRITE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(2);

/// One live session.
#[derive(Debug)]
pub struct Session {
    pub info: SessionInfo,
    /// Master side of the PTY.
    pub master: RawFd,
    pub pid: libc::pid_t,
    pub pgid: libc::pid_t,
    pub scrollback: Scrollback,
    pub scanner: OutputScanner,
    /// Connections currently mirroring this session's output.
    pub attachers: Vec<UnixStream>,
    log: Option<File>,
    log_bytes: u64,
    log_capped: bool,
    /// True once the reader thread has been asked to stop.
    pub closing: bool,
}

impl Session {
    /// Append output: scrollback, transcript, and every attached client.
    pub fn absorb(&mut self, data: &[u8]) {
        if data.is_empty() {
            return;
        }
        self.scrollback.push(data);
        self.write_log(data);
        self.broadcast(data);
        self.info.last_activity = now_secs();
    }

    fn write_log(&mut self, data: &[u8]) {
        if self.log_capped {
            return;
        }
        let Some(log) = self.log.as_mut() else {
            return;
        };
        if self.log_bytes + data.len() as u64 > LOG_LIMIT_BYTES {
            let _ = log.write_all(
                b"\r\n[apex-agentd: transcript truncated, session log limit reached]\r\n",
            );
            let _ = log.flush();
            self.log_capped = true;
            return;
        }
        if log.write_all(data).is_ok() {
            self.log_bytes += data.len() as u64;
        }
    }

    /// Mirror bytes to attached clients, dropping any that have gone away.
    fn broadcast(&mut self, data: &[u8]) {
        if self.attachers.is_empty() {
            return;
        }
        self.attachers.retain_mut(|s| s.write_all(data).is_ok());
        self.info.attached = self.attachers.len() as u32;
    }

    /// Register a client and hand it the scrollback to repaint with.
    pub fn attach(&mut self, mut stream: UnixStream, replay: usize) -> Result<()> {
        // A client that stops reading must not be able to stall the session.
        // See ATTACH_WRITE_TIMEOUT.
        stream.set_write_timeout(Some(ATTACH_WRITE_TIMEOUT)).ok();
        if replay > 0 && !self.scrollback.is_empty() {
            let tail = self.scrollback.tail(replay);
            stream
                .write_all(&tail)
                .context("sending scrollback to the attaching client")?;
        }
        self.attachers.push(stream);
        self.info.attached = self.attachers.len() as u32;
        Ok(())
    }

    /// Record a state transition, refusing to leave a terminal state.
    pub fn set_state(&mut self, state: AgentState, detail: Option<String>) {
        if self.info.state.is_terminal() && !state.is_terminal() {
            return;
        }
        self.info.state = state;
        if detail.is_some() {
            self.info.detail = detail;
        }
        self.info.last_activity = now_secs();
    }

    /// Record that the process ended.
    pub fn set_exited(&mut self, code: Option<i32>, signal: Option<i32>) {
        self.info.exit_code = code;
        self.info.exit_signal = signal;
        self.info.state = apex_agent_core::session::exit_state(code, signal);
        self.info.last_activity = now_secs();
        self.attachers.clear();
        self.info.attached = 0;
        if let Some(log) = self.log.as_mut() {
            let _ = log.flush();
        }
    }

    /// Seconds since the last output or published event.
    pub fn idle_secs(&self) -> u64 {
        now_secs().saturating_sub(self.info.last_activity)
    }
}

pub fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// A session handle shared between the control threads and its reader thread.
pub type Handle = Arc<Mutex<Session>>;

/// The table of live sessions.
#[derive(Debug, Default)]
pub struct Registry {
    next_id: u32,
    sessions: HashMap<u32, Handle>,
}

impl Registry {
    pub fn new() -> Registry {
        Registry {
            next_id: 1,
            sessions: HashMap::new(),
        }
    }

    /// Allocate the next session id.
    pub fn allocate(&mut self) -> u32 {
        let id = self.next_id;
        self.next_id += 1;
        id
    }

    /// Add a session, opening its transcript.
    pub fn insert(
        &mut self,
        info: SessionInfo,
        master: RawFd,
        pid: libc::pid_t,
        pgid: libc::pid_t,
    ) -> Handle {
        let id = info.id;
        let log = open_log(id);
        let handle = Arc::new(Mutex::new(Session {
            info,
            master,
            pid,
            pgid,
            scrollback: Scrollback::new(SCROLLBACK_BYTES),
            scanner: OutputScanner::new(),
            attachers: Vec::new(),
            log,
            log_bytes: 0,
            log_capped: false,
            closing: false,
        }));
        self.sessions.insert(id, Arc::clone(&handle));
        handle
    }

    pub fn get(&self, id: u32) -> Option<Handle> {
        self.sessions.get(&id).map(Arc::clone)
    }

    /// Every session, ordered by id.
    pub fn list(&self) -> Vec<Handle> {
        let mut ids: Vec<&u32> = self.sessions.keys().collect();
        ids.sort();
        ids.into_iter()
            .filter_map(|id| self.sessions.get(id).map(Arc::clone))
            .collect()
    }

    /// Drop a session from the table.
    pub fn remove(&mut self, id: u32) -> Option<Handle> {
        self.sessions.remove(&id)
    }

    pub fn len(&self) -> usize {
        self.sessions.len()
    }

    pub fn is_empty(&self) -> bool {
        self.sessions.is_empty()
    }
}

fn open_log(id: u32) -> Option<File> {
    let path = paths::session_log(id);
    let dir = path.parent()?;
    paths::ensure_private_dir(dir).ok()?;
    let file = OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(&path)
        .ok()?;
    // A transcript is a record of the user's work; nobody else on the machine
    // needs to read it.
    use std::os::unix::fs::PermissionsExt;
    let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
    Some(file)
}

/// Persist a session record so `apex agent list` can describe it after the
/// daemon has restarted.
pub fn write_record(info: &SessionInfo) {
    let path = paths::session_record(info.id);
    let Some(dir) = path.parent() else { return };
    if paths::ensure_private_dir(dir).is_err() {
        return;
    }
    let Ok(text) = serde_json::to_string_pretty(info) else {
        return;
    };
    let tmp = path.with_extension("json.tmp");
    if std::fs::write(&tmp, text).is_ok() {
        let _ = std::fs::rename(&tmp, &path);
    }
}

/// Delete a session's record and transcript.
pub fn forget_record(id: u32) {
    let _ = std::fs::remove_file(paths::session_record(id));
    let _ = std::fs::remove_file(paths::session_log(id));
}

/// Read the tail of a session's transcript from disk.
pub fn read_log(id: u32, bytes: usize) -> Result<String> {
    use std::io::{Read, Seek, SeekFrom};

    let path = paths::session_log(id);
    let mut file = File::open(&path).with_context(|| format!("no transcript for session {id}"))?;
    let len = file.metadata()?.len();
    let want = bytes as u64;
    if len > want {
        file.seek(SeekFrom::Start(len - want))?;
    }
    let mut buf = Vec::with_capacity(want.min(len) as usize);
    file.read_to_end(&mut buf)?;
    Ok(String::from_utf8_lossy(&buf).into_owned())
}

/// Mark records left behind by a previous daemon as exited.
///
/// Sessions do not survive the daemon — every PTY it owned closed with it — so
/// a record still claiming `working` at startup is stale, and leaving it would
/// show the user a session they can neither attach to nor kill.
pub fn reconcile_stale_records() {
    let dir = paths::state_dir().join("sessions");
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        let Ok(mut info) = serde_json::from_str::<SessionInfo>(&text) else {
            // Unparseable record: remove it rather than leave something no
            // version of the CLI can read.
            let _ = std::fs::remove_file(&path);
            continue;
        };
        if info.state.is_terminal() {
            continue;
        }
        info.state = AgentState::Exited;
        info.attached = 0;
        if info.exit_code.is_none() && info.exit_signal.is_none() {
            info.exit_code = Some(-1);
        }
        write_record(&info);
    }
}

/// Every persisted record, for listing sessions the current daemon does not own.
pub fn historical_records() -> Vec<SessionInfo> {
    let dir = paths::state_dir().join("sessions");
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        if let Ok(text) = std::fs::read_to_string(&path) {
            if let Ok(info) = serde_json::from_str::<SessionInfo>(&text) {
                out.push(info);
            }
        }
    }
    out.sort_by_key(|i| i.id);
    out
}

/// Stop a session's process group and close its terminal.
pub fn terminate(session: &mut Session) {
    if session.info.is_live() {
        let _ = pty::signal_group(session.pgid, libc::SIGHUP);
        let _ = pty::signal_group(session.pgid, libc::SIGTERM);
    }
    session.closing = true;
}

#[cfg(test)]
mod tests {
    use super::*;
    use apex_agent_core::protocol::SandboxPolicy;

    fn info(id: u32) -> SessionInfo {
        SessionInfo {
            id,
            agent: "generic".into(),
            program: "sh".into(),
            args: vec![],
            cwd: "/tmp".into(),
            project: None,
            project_name: None,
            worktree: None,
            state: AgentState::Starting,
            detail: None,
            sandbox: SandboxPolicy::Project,
            pid: 0,
            started: 0,
            last_activity: 0,
            exit_code: None,
            exit_signal: None,
            attached: 0,
            checkpoint: None,
            cols: 80,
            rows: 24,
        }
    }

    fn session(id: u32) -> Session {
        Session {
            info: info(id),
            master: -1,
            pid: 0,
            pgid: 0,
            scrollback: Scrollback::new(1024),
            scanner: OutputScanner::new(),
            attachers: Vec::new(),
            log: None,
            log_bytes: 0,
            log_capped: false,
            closing: false,
        }
    }

    #[test]
    fn ids_are_allocated_in_order_and_never_reused() {
        let mut r = Registry::new();
        let a = r.allocate();
        let b = r.allocate();
        assert_eq!((a, b), (1, 2));
        r.insert(info(a), -1, 0, 0);
        r.remove(a);
        // Removing a session must not let the next one take its id: a stale
        // `apex agent attach 1` would otherwise reach a different session.
        assert_eq!(r.allocate(), 3);
    }

    #[test]
    fn sessions_are_listed_in_id_order() {
        let mut r = Registry::new();
        for id in [3u32, 1, 2] {
            r.insert(info(id), -1, 0, 0);
        }
        let ids: Vec<u32> = r
            .list()
            .into_iter()
            .map(|h| h.lock().unwrap().info.id)
            .collect();
        assert_eq!(ids, vec![1, 2, 3]);
    }

    #[test]
    fn absorbing_output_fills_the_scrollback() {
        let mut s = session(1);
        s.absorb(b"hello ");
        s.absorb(b"world");
        assert_eq!(s.scrollback.tail(64), b"hello world".to_vec());
        assert!(s.info.last_activity > 0);
    }

    #[test]
    fn a_terminal_state_is_not_reopened_by_a_late_event() {
        let mut s = session(1);
        s.set_exited(Some(0), None);
        assert_eq!(s.info.state, AgentState::Complete);
        s.set_state(AgentState::Working, None);
        assert_eq!(
            s.info.state,
            AgentState::Complete,
            "a late event reopened a finished session"
        );
    }

    #[test]
    fn a_terminal_state_may_be_replaced_by_another_terminal_state() {
        let mut s = session(1);
        s.set_state(AgentState::Complete, None);
        s.set_state(AgentState::Failed, None);
        assert_eq!(s.info.state, AgentState::Failed);
    }

    #[test]
    fn exiting_clears_attachments() {
        let mut s = session(1);
        let (a, _b) = UnixStream::pair().unwrap();
        s.attach(a, 0).unwrap();
        assert_eq!(s.info.attached, 1);
        s.set_exited(Some(0), None);
        assert_eq!(s.info.attached, 0);
        assert!(s.attachers.is_empty());
    }

    #[test]
    fn attaching_replays_the_scrollback() {
        use std::io::Read;
        let mut s = session(1);
        s.absorb(b"earlier output");
        let (client, mut peer) = UnixStream::pair().unwrap();
        s.attach(client, 1024).unwrap();

        let mut buf = [0u8; 64];
        let n = peer.read(&mut buf).unwrap();
        assert_eq!(&buf[..n], b"earlier output");
    }

    #[test]
    fn attaching_with_zero_replay_sends_nothing_first() {
        use std::io::Read;
        let mut s = session(1);
        s.absorb(b"earlier output");
        let (client, mut peer) = UnixStream::pair().unwrap();
        s.attach(client, 0).unwrap();

        peer.set_read_timeout(Some(std::time::Duration::from_millis(50)))
            .unwrap();
        let mut buf = [0u8; 64];
        assert!(peer.read(&mut buf).is_err(), "replay was sent anyway");

        // ...but live output still arrives.
        s.absorb(b"live");
        let n = peer.read(&mut buf).unwrap();
        assert_eq!(&buf[..n], b"live");
    }

    #[test]
    fn output_reaches_every_attached_client() {
        use std::io::Read;
        let mut s = session(1);
        let (c1, mut p1) = UnixStream::pair().unwrap();
        let (c2, mut p2) = UnixStream::pair().unwrap();
        s.attach(c1, 0).unwrap();
        s.attach(c2, 0).unwrap();
        assert_eq!(s.info.attached, 2);

        s.absorb(b"shared");
        let mut buf = [0u8; 32];
        let n1 = p1.read(&mut buf).unwrap();
        assert_eq!(&buf[..n1], b"shared");
        let n2 = p2.read(&mut buf).unwrap();
        assert_eq!(&buf[..n2], b"shared");
    }

    #[test]
    fn a_disconnected_client_is_dropped_without_killing_the_session() {
        let mut s = session(1);
        let (c1, p1) = UnixStream::pair().unwrap();
        let (c2, mut p2) = UnixStream::pair().unwrap();
        s.attach(c1, 0).unwrap();
        s.attach(c2, 0).unwrap();
        drop(p1);

        // Keep the live client drained, so the only thing under test is the
        // dead one. Writing without a reader would fill the socket buffer and
        // stall regardless of which client was at fault.
        let drain = std::thread::spawn(move || {
            use std::io::Read;
            let mut sink = [0u8; 8192];
            let mut total = 0usize;
            while let Ok(n) = p2.read(&mut sink) {
                if n == 0 {
                    break;
                }
                total += n;
            }
            total
        });

        for _ in 0..64 {
            s.absorb(&[b'x'; 4096]);
        }
        assert_eq!(s.info.attached, 1, "the dead client was not dropped");

        s.attachers.clear();
        assert!(drain.join().unwrap() > 0, "the live client received nothing");
    }

    #[test]
    fn a_client_that_stops_reading_is_dropped_instead_of_stalling_the_session() {
        // The failure this guards against is not cosmetic: without a write
        // timeout the session's reader thread blocks inside write_all once the
        // socket buffer fills (~176 KiB on this kernel), which stops the agent
        // itself, not just its display.
        let mut s = session(1);
        let (client, _peer) = UnixStream::pair().unwrap();
        s.attach(client, 0).unwrap();

        let started = std::time::Instant::now();
        // Comfortably past the socket buffer, with the peer never reading.
        for _ in 0..128 {
            s.absorb(&[b'x'; 8192]);
        }
        let elapsed = started.elapsed();

        assert_eq!(s.info.attached, 0, "the stalled client was not dropped");
        assert!(
            elapsed < std::time::Duration::from_secs(20),
            "the session was blocked for {elapsed:?} by an unresponsive client"
        );
    }

    #[test]
    fn idle_seconds_grow_from_the_last_activity() {
        let mut s = session(1);
        s.info.last_activity = now_secs() - 30;
        assert!(s.idle_secs() >= 30);
        s.absorb(b"x");
        assert!(s.idle_secs() < 2);
    }
}
