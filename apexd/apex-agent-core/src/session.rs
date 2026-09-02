//! Pure session logic: scrollback, terminal-escape scanning and the state
//! policy that turns raw PTY bytes into an [`AgentState`].
//!
//! None of this touches the operating system, which is deliberate — it is the
//! part that is easy to get subtly wrong (escape sequences split across reads,
//! a bell that is really an OSC terminator) and the part that has to be tested
//! directly rather than through a live PTY.
//!
//! ## What is actually detected, and what is not
//!
//! The roadmap lists process tree, PTY activity, terminal bell, OSC sequences,
//! shell hooks and exit status as fallback signals, and says to prefer official
//! events where an agent publishes them. That is exactly the split here:
//!
//! * `working` / `waiting_for_user` are inferred from output — a bare BEL, an
//!   OSC 9 / OSC 777 desktop notification, OSC 133 prompt markers, or silence
//!   past [`IDLE_TO_WAITING_SECS`];
//! * `complete` / `failed` come from the process exit status;
//! * `permission_request` is **only** ever set by a published event, never
//!   guessed. There is no reliable way to recognise a permission prompt in
//!   arbitrary terminal output, and a wrong guess here is worse than no guess:
//!   it would tell the user an agent is blocked when it is working, or the
//!   reverse. Clients report it through `apex agent event`.

use crate::protocol::AgentState;

/// Bytes of PTY output kept in memory per session for replay on attach.
///
/// 256 KiB is roughly a 1000-line scrollback of dense TUI output. The full
/// transcript still goes to disk; this is only what a reattaching terminal
/// gets repainted with.
pub const SCROLLBACK_BYTES: usize = 256 * 1024;

/// How long a live session may produce no output before it is reported as
/// waiting on the user.
///
/// Ten seconds rather than two or three: agents that stream a spinner go quiet
/// only when they genuinely stop, but agents that think silently before
/// printing anything are common, and calling those "waiting for user" after
/// three seconds would make the Agent Center flicker between states for the
/// entire run.
pub const IDLE_TO_WAITING_SECS: u64 = 10;

/// Largest OSC payload retained while scanning. Past this the sequence is
/// abandoned and scanning returns to ground state — an OSC this long is
/// binary output that happened to contain `ESC ]`, not a real notification.
const MAX_OSC_PAYLOAD: usize = 4096;

/// A fixed-capacity byte ring holding the tail of a session's output.
///
/// Deliberately byte-oriented and not line-oriented: this is replayed straight
/// back into a terminal, so it has to preserve escape sequences and partial
/// lines exactly as they were written.
#[derive(Debug)]
pub struct Scrollback {
    buf: Vec<u8>,
    /// Write cursor; only meaningful once `full` is true.
    head: usize,
    full: bool,
    capacity: usize,
}

impl Scrollback {
    pub fn new(capacity: usize) -> Scrollback {
        let capacity = capacity.max(1);
        Scrollback {
            buf: Vec::with_capacity(capacity.min(64 * 1024)),
            head: 0,
            full: false,
            capacity,
        }
    }

    /// Bytes currently retained.
    pub fn len(&self) -> usize {
        if self.full {
            self.capacity
        } else {
            self.buf.len()
        }
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Append output, discarding the oldest bytes once capacity is reached.
    pub fn push(&mut self, data: &[u8]) {
        if data.is_empty() {
            return;
        }

        // A single write larger than the ring: keep only its tail.
        if data.len() >= self.capacity {
            let tail = &data[data.len() - self.capacity..];
            self.buf.clear();
            self.buf.extend_from_slice(tail);
            self.head = 0;
            self.full = true;
            return;
        }

        if !self.full {
            self.buf.extend_from_slice(data);
            if self.buf.len() >= self.capacity {
                // Grew past capacity: drop the front and switch to ring mode.
                let excess = self.buf.len() - self.capacity;
                self.buf.drain(..excess);
                self.head = 0;
                self.full = true;
            }
            return;
        }

        for &b in data {
            self.buf[self.head] = b;
            self.head = (self.head + 1) % self.capacity;
        }
    }

    /// The most recent `max` bytes, oldest first.
    pub fn tail(&self, max: usize) -> Vec<u8> {
        let mut out = Vec::with_capacity(self.len().min(max));
        if !self.full {
            let start = self.buf.len().saturating_sub(max);
            out.extend_from_slice(&self.buf[start..]);
            return out;
        }
        // Ring order: head..end, then 0..head.
        out.extend_from_slice(&self.buf[self.head..]);
        out.extend_from_slice(&self.buf[..self.head]);
        if out.len() > max {
            let start = out.len() - max;
            out.drain(..start);
        }
        out
    }
}

/// Something noticed in the output stream.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Signal {
    /// A bare `BEL` that was not an OSC terminator.
    Bell,
    /// A desktop notification (OSC 9, or OSC 777 `notify`), with its text.
    Notification(String),
    /// OSC 133 `A` or `D`: the program is back at a prompt.
    PromptReady,
    /// OSC 133 `C`: a command started.
    CommandStarted,
}

impl Signal {
    /// The state this signal implies, if any.
    pub fn implied_state(&self) -> Option<AgentState> {
        match self {
            Signal::Bell | Signal::Notification(_) | Signal::PromptReady => {
                Some(AgentState::WaitingForUser)
            }
            Signal::CommandStarted => Some(AgentState::Working),
        }
    }

    /// Text to surface alongside the state, when the signal carries any.
    pub fn detail(&self) -> Option<&str> {
        match self {
            Signal::Notification(text) if !text.is_empty() => Some(text),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Scan {
    Ground,
    /// Saw `ESC`.
    Esc,
    /// Inside `ESC ] … `.
    Osc,
    /// Inside an OSC and saw `ESC`, which may begin the `ESC \` terminator.
    OscEsc,
}

/// Incremental scanner for the terminal signals listed above.
///
/// Carries its state across `feed` calls because a read boundary can land in
/// the middle of an escape sequence. Getting this wrong is the difference
/// between "the agent rang the bell" and "the agent's notification text
/// contained a 0x07 terminator".
#[derive(Debug)]
pub struct OutputScanner {
    scan: Scan,
    osc: Vec<u8>,
    /// Set when an OSC payload overran; suppresses the completion event.
    overran: bool,
}

impl Default for OutputScanner {
    fn default() -> Self {
        Self::new()
    }
}

impl OutputScanner {
    pub fn new() -> OutputScanner {
        OutputScanner {
            scan: Scan::Ground,
            osc: Vec::new(),
            overran: false,
        }
    }

    /// Feed a chunk of PTY output, returning every signal it completed.
    pub fn feed(&mut self, data: &[u8]) -> Vec<Signal> {
        let mut out = Vec::new();
        for &b in data {
            match self.scan {
                Scan::Ground => match b {
                    0x1b => self.scan = Scan::Esc,
                    0x07 => out.push(Signal::Bell),
                    _ => {}
                },
                Scan::Esc => {
                    if b == b']' {
                        self.scan = Scan::Osc;
                        self.osc.clear();
                        self.overran = false;
                    } else {
                        // Not an OSC. CSI and the other escape forms cannot
                        // contain BEL, so plain ground scanning is safe; if
                        // this byte is itself an ESC we are starting over.
                        self.scan = if b == 0x1b { Scan::Esc } else { Scan::Ground };
                    }
                }
                Scan::Osc => match b {
                    0x07 => {
                        if let Some(sig) = self.finish_osc() {
                            out.push(sig);
                        }
                    }
                    0x1b => self.scan = Scan::OscEsc,
                    _ => self.push_osc(b),
                },
                Scan::OscEsc => {
                    if b == b'\\' {
                        if let Some(sig) = self.finish_osc() {
                            out.push(sig);
                        }
                    } else {
                        // A stray ESC inside the payload. Keep both bytes and
                        // stay in the OSC.
                        self.push_osc(0x1b);
                        self.push_osc(b);
                        self.scan = Scan::Osc;
                    }
                }
            }
        }
        out
    }

    fn push_osc(&mut self, b: u8) {
        if self.osc.len() >= MAX_OSC_PAYLOAD {
            self.overran = true;
            return;
        }
        self.osc.push(b);
    }

    fn finish_osc(&mut self) -> Option<Signal> {
        let payload = std::mem::take(&mut self.osc);
        let overran = self.overran;
        self.scan = Scan::Ground;
        self.overran = false;
        if overran {
            return None;
        }
        parse_osc(&payload)
    }
}

/// Interpret an OSC payload (everything between `ESC ]` and its terminator).
fn parse_osc(payload: &[u8]) -> Option<Signal> {
    let text = String::from_utf8_lossy(payload);
    let (code, rest) = match text.split_once(';') {
        Some((code, rest)) => (code, rest),
        // OSC 133 markers sometimes arrive without a payload separator.
        None => (text.as_ref(), ""),
    };

    match code {
        // OSC 9 ; <text> — the widely implemented "growl" notification.
        "9" => Some(Signal::Notification(rest.trim().to_string())),
        // OSC 777 ; notify ; <title> ; <body>
        "777" => {
            let mut parts = rest.splitn(3, ';');
            match parts.next() {
                Some("notify") => {
                    let title = parts.next().unwrap_or("").trim();
                    let body = parts.next().unwrap_or("").trim();
                    let text = match (title.is_empty(), body.is_empty()) {
                        (true, true) => String::new(),
                        (false, true) => title.to_string(),
                        (true, false) => body.to_string(),
                        (false, false) => format!("{title}: {body}"),
                    };
                    Some(Signal::Notification(text))
                }
                _ => None,
            }
        }
        // OSC 133 shell integration: A = prompt start, C = command start,
        // D = command finished.
        "133" => match rest.chars().next() {
            Some('A') | Some('D') => Some(Signal::PromptReady),
            Some('C') => Some(Signal::CommandStarted),
            _ => None,
        },
        _ => None,
    }
}

/// Decide the state a live session should report.
///
/// `current` is what it reports now, `signals` is what the last read produced,
/// `had_output` is whether that read produced any bytes at all, and
/// `idle_secs` is how long it has been since the last output or event.
///
/// Terminal states are never left, and `permission_request` is never
/// overwritten by inference — only the process exiting or another published
/// event can move a session out of it. An agent that is genuinely blocked on a
/// permission decision produces no output, and letting the idle rule rewrite
/// that to `waiting_for_user` would discard the more specific truth.
pub fn next_state(
    current: AgentState,
    signals: &[Signal],
    had_output: bool,
    idle_secs: u64,
) -> AgentState {
    if current.is_terminal() {
        return current;
    }

    // An explicit signal wins over both the idle rule and raw output. Later
    // signals in the same read win over earlier ones.
    if let Some(state) = signals.iter().rev().find_map(|s| s.implied_state()) {
        return state;
    }

    if current == AgentState::PermissionRequest {
        // Output alone does not clear a permission request; a client that
        // resolved one publishes the next event.
        return current;
    }

    if had_output {
        return AgentState::Working;
    }

    if idle_secs >= IDLE_TO_WAITING_SECS {
        return AgentState::WaitingForUser;
    }

    current
}

/// The state an exited session should report, from its wait status.
pub fn exit_state(code: Option<i32>, signal: Option<i32>) -> AgentState {
    match (code, signal) {
        (Some(0), None) => AgentState::Complete,
        (Some(_), None) => AgentState::Failed,
        // Killed by a signal. `apex agent kill` is the normal way a session
        // ends, so this is `exited`, not `failed` — a user stopping their own
        // agent has not suffered a failure.
        (_, Some(_)) => AgentState::Exited,
        (None, None) => AgentState::Exited,
    }
}

/// Map a signal name accepted by `apex agent signal` to its number.
pub fn signal_number(name: &str) -> Option<i32> {
    match name.to_ascii_lowercase().as_str() {
        "int" | "sigint" | "interrupt" => Some(libc::SIGINT),
        "term" | "sigterm" | "terminate" => Some(libc::SIGTERM),
        "kill" | "sigkill" => Some(libc::SIGKILL),
        "stop" | "sigstop" | "pause" => Some(libc::SIGSTOP),
        "cont" | "sigcont" | "continue" | "resume" => Some(libc::SIGCONT),
        "hup" | "sighup" => Some(libc::SIGHUP),
        "quit" | "sigquit" => Some(libc::SIGQUIT),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scan(data: &[u8]) -> Vec<Signal> {
        OutputScanner::new().feed(data)
    }

    #[test]
    fn scrollback_keeps_the_tail_and_drops_the_head() {
        let mut sb = Scrollback::new(8);
        sb.push(b"abc");
        assert_eq!(sb.tail(64), b"abc".to_vec());
        sb.push(b"defgh");
        assert_eq!(sb.tail(64), b"abcdefgh".to_vec());
        sb.push(b"ij");
        assert_eq!(sb.tail(64), b"cdefghij".to_vec());
        assert_eq!(sb.len(), 8);
    }

    #[test]
    fn scrollback_handles_a_write_larger_than_capacity() {
        let mut sb = Scrollback::new(4);
        sb.push(b"0123456789");
        assert_eq!(sb.tail(64), b"6789".to_vec());
        sb.push(b"ab");
        assert_eq!(sb.tail(64), b"89ab".to_vec());
    }

    #[test]
    fn scrollback_tail_limit_is_respected() {
        let mut sb = Scrollback::new(16);
        sb.push(b"abcdefghij");
        assert_eq!(sb.tail(4), b"ghij".to_vec());
        // And after wrapping.
        sb.push(b"klmnopqrstuvwxyz");
        assert_eq!(sb.tail(4), b"wxyz".to_vec());
    }

    #[test]
    fn scrollback_is_byte_exact_for_escape_sequences() {
        let mut sb = Scrollback::new(64);
        let payload = b"\x1b[31mred\x1b[0m\x07";
        sb.push(payload);
        assert_eq!(sb.tail(64), payload.to_vec());
    }

    #[test]
    fn bare_bell_is_a_bell() {
        assert_eq!(scan(b"done\x07"), vec![Signal::Bell]);
    }

    #[test]
    fn osc_terminating_bell_is_not_a_bell() {
        // The whole point of the scanner: this BEL closes OSC 9, it is not the
        // program ringing the terminal.
        let signals = scan(b"\x1b]9;build finished\x07");
        assert_eq!(
            signals,
            vec![Signal::Notification("build finished".to_string())]
        );
        assert!(!signals.contains(&Signal::Bell));
    }

    #[test]
    fn osc_string_terminator_is_accepted() {
        assert_eq!(
            scan(b"\x1b]9;hello\x1b\\"),
            vec![Signal::Notification("hello".to_string())]
        );
    }

    #[test]
    fn escape_sequence_split_across_reads_is_still_recognised() {
        let mut s = OutputScanner::new();
        assert!(s.feed(b"\x1b").is_empty());
        assert!(s.feed(b"]9;split ").is_empty());
        assert!(s.feed(b"notification").is_empty());
        assert_eq!(
            s.feed(b"\x07"),
            vec![Signal::Notification("split notification".to_string())]
        );
    }

    #[test]
    fn bell_split_from_its_osc_across_reads_is_not_a_bell() {
        let mut s = OutputScanner::new();
        assert!(s.feed(b"\x1b]9;x").is_empty());
        let signals = s.feed(b"\x07");
        assert_eq!(signals, vec![Signal::Notification("x".to_string())]);
        assert!(!signals.contains(&Signal::Bell));
    }

    #[test]
    fn osc_777_notify_joins_title_and_body() {
        assert_eq!(
            scan(b"\x1b]777;notify;Claude Code;needs your input\x07"),
            vec![Signal::Notification(
                "Claude Code: needs your input".to_string()
            )]
        );
    }

    #[test]
    fn osc_777_without_notify_is_ignored() {
        assert!(scan(b"\x1b]777;precmd\x07").is_empty());
    }

    #[test]
    fn osc_133_markers_map_to_prompt_and_command() {
        assert_eq!(scan(b"\x1b]133;A\x07"), vec![Signal::PromptReady]);
        assert_eq!(scan(b"\x1b]133;D;0\x07"), vec![Signal::PromptReady]);
        assert_eq!(scan(b"\x1b]133;C\x07"), vec![Signal::CommandStarted]);
    }

    #[test]
    fn csi_sequences_are_ignored_and_do_not_swallow_a_later_bell() {
        assert_eq!(scan(b"\x1b[2J\x1b[H\x07"), vec![Signal::Bell]);
    }

    #[test]
    fn unknown_osc_codes_are_ignored() {
        // OSC 0 (window title) is the most common sequence in any TUI; it must
        // never be mistaken for a notification.
        assert!(scan(b"\x1b]0;my terminal title\x07").is_empty());
        assert!(scan(b"\x1b]8;;https://example.com\x07").is_empty());
    }

    #[test]
    fn oversized_osc_is_abandoned_and_scanning_recovers() {
        let mut s = OutputScanner::new();
        let mut junk = Vec::from(&b"\x1b]9;"[..]);
        junk.extend(std::iter::repeat(b'x').take(MAX_OSC_PAYLOAD + 64));
        junk.push(0x07);
        assert!(s.feed(&junk).is_empty(), "overrun must not emit a signal");
        // The scanner is back in ground state and still sees a real bell.
        assert_eq!(s.feed(b"\x07"), vec![Signal::Bell]);
    }

    #[test]
    fn output_alone_means_working() {
        assert_eq!(
            next_state(AgentState::Starting, &[], true, 0),
            AgentState::Working
        );
    }

    #[test]
    fn silence_past_the_threshold_means_waiting() {
        assert_eq!(
            next_state(AgentState::Working, &[], false, IDLE_TO_WAITING_SECS),
            AgentState::WaitingForUser
        );
        // Just under the threshold, nothing changes.
        assert_eq!(
            next_state(AgentState::Working, &[], false, IDLE_TO_WAITING_SECS - 1),
            AgentState::Working
        );
    }

    #[test]
    fn a_signal_beats_the_idle_rule_and_raw_output() {
        assert_eq!(
            next_state(AgentState::Working, &[Signal::Bell], true, 0),
            AgentState::WaitingForUser
        );
        assert_eq!(
            next_state(
                AgentState::WaitingForUser,
                &[Signal::CommandStarted],
                false,
                600
            ),
            AgentState::Working
        );
    }

    #[test]
    fn the_last_signal_in_a_read_wins() {
        let signals = vec![Signal::Bell, Signal::CommandStarted];
        assert_eq!(
            next_state(AgentState::Starting, &signals, true, 0),
            AgentState::Working
        );
    }

    #[test]
    fn permission_request_is_not_overwritten_by_inference() {
        // Neither output nor silence may downgrade a published permission
        // request; only another event or the process exiting.
        assert_eq!(
            next_state(AgentState::PermissionRequest, &[], true, 0),
            AgentState::PermissionRequest
        );
        assert_eq!(
            next_state(AgentState::PermissionRequest, &[], false, 3600),
            AgentState::PermissionRequest
        );
        // An explicit signal still moves it.
        assert_eq!(
            next_state(
                AgentState::PermissionRequest,
                &[Signal::CommandStarted],
                false,
                0
            ),
            AgentState::Working
        );
    }

    #[test]
    fn terminal_states_are_never_left() {
        for s in [AgentState::Complete, AgentState::Failed, AgentState::Exited] {
            assert_eq!(next_state(s, &[Signal::Bell], true, 0), s);
            assert_eq!(next_state(s, &[], false, 9999), s);
        }
    }

    #[test]
    fn exit_status_maps_to_complete_failed_or_exited() {
        assert_eq!(exit_state(Some(0), None), AgentState::Complete);
        assert_eq!(exit_state(Some(1), None), AgentState::Failed);
        assert_eq!(exit_state(Some(127), None), AgentState::Failed);
        // A user stopping their own agent is not a failure.
        assert_eq!(exit_state(None, Some(libc::SIGTERM)), AgentState::Exited);
        assert_eq!(exit_state(None, Some(libc::SIGKILL)), AgentState::Exited);
    }

    #[test]
    fn signal_names_resolve_and_unknown_ones_do_not() {
        assert_eq!(signal_number("int"), Some(libc::SIGINT));
        assert_eq!(signal_number("SIGTERM"), Some(libc::SIGTERM));
        assert_eq!(signal_number("pause"), Some(libc::SIGSTOP));
        assert_eq!(signal_number("resume"), Some(libc::SIGCONT));
        assert_eq!(signal_number("nope"), None);
        assert_eq!(signal_number(""), None);
    }
}
