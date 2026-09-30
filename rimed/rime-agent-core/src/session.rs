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
//!   reverse. Clients report it through `rime agent event`.
//!
//! ## The one thing a published event changes about inference
//!
//! Silence is ambiguous: an agent waiting on a person and an agent running
//! `cargo test` both produce nothing. Output cannot tell them apart, so the
//! idle rule picks the more common one and is wrong for the whole of every
//! long tool call. A session that published a `PreToolUse` and has not yet
//! published its `PostToolUse` has resolved the ambiguity, and `next_state`
//! takes that answer — bounded, so a hook that stops firing hands the decision
//! back rather than freezing the session. [`crate::hook`] is where those
//! events come from for Claude; the parameter is a plain `Option<u64>` so any
//! agent can supply it through the same open event protocol.

use crate::protocol::AgentState;
use crate::term::WinSize;

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

/// How long a tool may be reported as running before the idle rule takes over
/// again.
///
/// The in-flight flag is set by a `PreToolUse` hook and cleared by the
/// `PostToolUse` that answers it. A hook that never fires — a crashed daemon,
/// a `--bare` session, an agent that rewrote its own settings — would
/// otherwise pin a session to `working` for as long as it existed. Fifteen
/// minutes: Claude's own Bash tool tops out at ten, so this is above every
/// real tool call and far below "forever". Past it the session is inferred
/// from output again, which is exactly the fallback §6.1 keeps.
pub const TOOL_IN_FLIGHT_MAX_SECS: u64 = 900;

/// Largest OSC payload retained while scanning. Past this the sequence is
/// abandoned and scanning returns to ground state — an OSC this long is
/// binary output that happened to contain `ESC ]`, not a real notification.
const MAX_OSC_PAYLOAD: usize = 4096;

/// Longest CSI parameter run remembered while scanning.
///
/// `?1049;2004` is ten bytes and no real private-mode list is close to this;
/// anything longer is binary output that happened to contain `ESC [`.
const MAX_CSI_PARAMS: usize = 64;

/// How long `input` with `submit` waits between the text and the carriage
/// return that sends it (`docs/remote-live-contract.md` §1.2).
///
/// A constant and not a knob, because the number is a measurement and not a
/// preference. Against Claude Code 2.1.283 in a real PTY (2026-09-30): a
/// ~250-character burst that ENDS in `\r` is taken as a paste, so the CR
/// becomes a newline inside the prompt and nothing is submitted; the same text
/// with the CR written 50 ms or 250 ms later submits. Short bursts happened to
/// submit, which is why a phone reply looked intermittently broken rather than
/// broken. 80 ms sits above the 50 ms that measured good and far below
/// anything a person would notice between typing and sending.
pub const SUBMIT_GAP: std::time::Duration = std::time::Duration::from_millis(80);

/// The most output one `peek` answers with (§1.6).
///
/// Bounded by the wire rather than by taste. A `peek` reply is one line of the
/// control protocol, and for a phone that line is one `Frame::Control` of at
/// most 65514 bytes. The bytes travel as base64, which is 4/3 of them: 8192
/// becomes at most 10924 characters, far inside the frame, where raw PTY bytes
/// JSON-escaped could have been six times their size.
pub const PEEK_MAX: usize = 8192;

/// The longest name a person may give a session, in characters (§1.3).
///
/// Characters and not bytes, because the limit is about what fits in a row of
/// the Agent Center and on a phone, and a name in Japanese is three bytes a
/// character without being three times as wide.
pub const MAX_NAME_CHARS: usize = 64;

/// The longest agent title kept, in characters (§1.3).
///
/// Longer than a name because it is not typed by anybody: Claude writes the
/// conversation's summary there, and cutting it at a person's limit would cut
/// most of them mid-word. Still bounded, because it is written by the agent and
/// ends up in a record the Shell and a phone render.
pub const MAX_TITLE_CHARS: usize = 80;

/// Check a name a person gave a session, and trim it.
///
/// `Ok(None)` is the answer for an empty or all-whitespace name, and it means
/// "clear the name" — so `rime agent rename 4 ""` and `--clear` are the same
/// request, which is what a person who deleted the text in a rename field
/// meant.
///
/// Refused rather than sanitised, for `unprintable_actor`'s reason in
/// `rime-agentd/src/privilege.rs`: the name is printed in `rime agent list`,
/// in the Agent Center and on a phone, and a control character in it can
/// rewrite the line around it. Silently stripping one would leave the person
/// believing they named the session something the record calls something
/// else. One function, used by the CLI before it sends and by the daemon when
/// it receives, so the two can never disagree about what a valid name is.
pub fn session_name(raw: &str) -> Result<Option<String>, String> {
    let name = raw.trim();
    if name.is_empty() {
        return Ok(None);
    }
    let chars = name.chars().count();
    if chars > MAX_NAME_CHARS {
        return Err(format!(
            "a session name may be at most {MAX_NAME_CHARS} characters; this one is {chars}"
        ));
    }
    if name.chars().any(char::is_control) {
        return Err(
            "a session name may not contain control characters: it is printed in terminals, in \
             the Agent Center and on a phone, and a control character there can rewrite the \
             line around it"
                .to_string(),
        );
    }
    Ok(Some(name.to_string()))
}

/// [`session_name`] for a field that may be absent, where absent also clears.
pub fn session_name_opt(raw: Option<&str>) -> Result<Option<String>, String> {
    match raw {
        None => Ok(None),
        Some(raw) => session_name(raw),
    }
}

/// Whether `c` is a spinner frame or a status mark an agent puts in front of
/// its terminal title.
///
/// Claude Code writes `✳ <summary>` when it is idle and cycles `· ✢ ✳ ✶ ✻ ✽`
/// while it works; braille-dot spinners (U+2800–U+28FF) are what most other
/// TUIs cycle. Stripping them is what makes the title a NAME: without it the
/// title changes ten times a second while the agent works, every change would
/// be a record written to disk, and a phone would render a different string
/// on every refresh.
///
/// Only the leading run is removed, never a glyph inside the title, so a
/// summary that mentions `*` or `●` keeps it.
fn is_status_glyph(c: char) -> bool {
    matches!(c,
        // Braille patterns: every dot spinner.
        '\u{2800}'..='\u{28FF}'
        // Dingbat asterisks, stars and florettes — ✢ ✣ ✤ ✥ ✦ ✧ ✳ ✴ ✵ ✶ ✻ ✼ ✽ …
        | '\u{2722}'..='\u{2742}'
        // Geometric shapes: ● ○ ◉ ◐ ◑ ◒ ◓ ■ □ ▪ ▫ ◆ ◇ …
        | '\u{25A0}'..='\u{25FF}'
        // The dots and marks that are not in either block.
        | '*' | '·' | '•' | '∙' | '⋅' | '⏺' | '★' | '☆' | '✓' | '✔' | '✗' | '✘'
    )
}

/// Turn the text of an OSC 0 / OSC 2 title into what a session record shows.
///
/// Control characters removed, the leading run of spinner glyphs and
/// whitespace stripped, trailing whitespace trimmed, cut at
/// [`MAX_TITLE_CHARS`]; `None` when nothing is left. An empty title is the
/// agent CLEARING its title, and `None` is how the record says so.
///
/// Sanitised rather than refused, which is the opposite of [`session_name`]
/// and on purpose: nobody is on the other end of a terminal title to be told
/// "no", and a title is display-only — no decision anywhere reads it. It is
/// agent-controlled text and is never trusted for anything.
pub fn clean_title(raw: &str) -> Option<String> {
    let printable: String = raw.chars().filter(|c| !c.is_control()).collect();
    let stripped = printable
        .trim_start_matches(|c: char| c.is_whitespace() || is_status_glyph(c))
        .trim_end();
    if stripped.is_empty() {
        return None;
    }
    let cut: String = stripped.chars().take(MAX_TITLE_CHARS).collect();
    Some(cut.trim_end().to_string())
}

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
    /// OSC 0 or OSC 2: the program set its terminal title, already put
    /// through [`clean_title`]. `None` is a title set to nothing — the program
    /// cleared it.
    ///
    /// Implies no state and carries no detail, and that is load-bearing:
    /// Claude sets its title while it works AND while it waits, so a title
    /// that moved the state would be the scanner guessing, and one that wrote
    /// `detail` would overwrite a notification's text with a summary.
    Title(Option<String>),
}

impl Signal {
    /// The state this signal implies, if any.
    pub fn implied_state(&self) -> Option<AgentState> {
        match self {
            Signal::Bell | Signal::Notification(_) | Signal::PromptReady => {
                Some(AgentState::WaitingForUser)
            }
            Signal::CommandStarted => Some(AgentState::Working),
            Signal::Title(_) => None,
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
    /// Inside `ESC [ ...`, collecting parameter bytes.
    Csi,
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
    /// Parameter bytes of the CSI being collected, bounded by
    /// [`MAX_CSI_PARAMS`].
    csi: Vec<u8>,
    /// Whether the application has `DECSET 2004` on right now.
    bracketed_paste: bool,
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
            csi: Vec::new(),
            bracketed_paste: false,
        }
    }

    /// Whether the program on this PTY has asked for bracketed paste.
    ///
    /// Read by the daemon before it types a path into a session
    /// ([`crate::inject`]): the markers are a service to an application that
    /// asked for them and line noise to one that did not, and the only way to
    /// know which this is, is to have watched it ask. The daemon owns the
    /// master from the moment the session is spawned, so it has seen every
    /// byte the application ever wrote and cannot have missed the request.
    pub fn bracketed_paste(&self) -> bool {
        self.bracketed_paste
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
                Scan::Esc => match b {
                    b']' => {
                        self.scan = Scan::Osc;
                        self.osc.clear();
                        self.overran = false;
                    }
                    b'[' => {
                        self.scan = Scan::Csi;
                        self.csi.clear();
                    }
                    // Not an OSC and not a CSI. The remaining escape forms
                    // cannot contain BEL, so plain ground scanning is safe; if
                    // this byte is itself an ESC we are starting over.
                    0x1b => self.scan = Scan::Esc,
                    _ => self.scan = Scan::Ground,
                },
                Scan::Csi => {
                    if (0x20..=0x3f).contains(&b) {
                        // A parameter or intermediate byte. Past the bound this
                        // is no longer a private mode set, so remembering stops
                        // while the search for the terminator continues.
                        if self.csi.len() < MAX_CSI_PARAMS {
                            self.csi.push(b);
                        } else if !self.csi.is_empty() {
                            self.csi.clear();
                        }
                    } else if (0x40..=0x7e).contains(&b) {
                        self.apply_csi(b);
                        self.scan = Scan::Ground;
                    } else {
                        // Not part of a CSI at all: an abandoned sequence. Fall
                        // back to ground and let this byte mean what it would
                        // have meant there, so a BEL inside a malformed escape
                        // is still a bell -- which is what the scanner did
                        // before it knew what a CSI was.
                        self.scan = Scan::Ground;
                        match b {
                            0x1b => self.scan = Scan::Esc,
                            0x07 => out.push(Signal::Bell),
                            _ => {}
                        }
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

    /// Apply a completed CSI, when it is one of the modes this cares about.
    ///
    /// Exactly one is: `ESC [ ? 2004 h` and its `l`. Everything else a program
    /// does to its terminal is the program's own business, and a scanner that
    /// modelled more of it would be a terminal emulator.
    fn apply_csi(&mut self, final_byte: u8) {
        if final_byte != b'h' && final_byte != b'l' {
            return;
        }
        // `?` marks a DEC private mode. The parameters after it are
        // semicolon-separated and 2004 may be any one of them, because
        // `ESC [ ? 1049 ; 2004 h` is a legal way to ask for both.
        let Some(params) = self.csi.strip_prefix(b"?") else {
            return;
        };
        let on = final_byte == b'h';
        for part in params.split(|b| *b == b';') {
            if part == b"2004" {
                self.bracketed_paste = on;
            }
        }
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
        // OSC 0 ; <title> sets the icon name and the window title, OSC 2 the
        // title alone; a terminal draws both as the title, and so does this.
        // OSC 1, the icon name by itself, is not a title anybody sees.
        "0" | "2" => Some(Signal::Title(clean_title(rest))),
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

/// Whether a published event says a tool is running, and for how long.
///
/// `None` is what every unintegrated agent has: no hook has ever said
/// anything, so the idle rule decides on its own exactly as it did before.
pub type ToolInFlight = Option<u64>;

/// Decide the state a live session should report.
///
/// `current` is what it reports now, `signals` is what the last read produced,
/// `had_output` is whether that read produced any bytes at all, `idle_secs` is
/// how long it has been since the last output or event, and `tool_in_flight`
/// is how long ago a published event said a tool started, when one did.
///
/// Terminal states are never left, and `permission_request` is never
/// overwritten by inference — only the process exiting or another published
/// event can move a session out of it. An agent that is genuinely blocked on a
/// permission decision produces no output, and letting the idle rule rewrite
/// that to `waiting_for_user` would discard the more specific truth.
///
/// `tool_in_flight` is the same argument applied to the other silent case.
/// `cargo test` prints nothing for two minutes; the idle rule reads that
/// silence as the user being asked a question and is wrong for a hundred and
/// ten seconds of it. A session that told us a tool started and has not told
/// us it finished is working, and silence is the evidence for that rather than
/// against it. Bounded by [`TOOL_IN_FLIGHT_MAX_SECS`] so a hook that stopped
/// firing hands the decision back to the idle rule instead of pinning the
/// session to `working` forever.
pub fn next_state(
    current: AgentState,
    signals: &[Signal],
    had_output: bool,
    idle_secs: u64,
    tool_in_flight: ToolInFlight,
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
        if matches!(tool_in_flight, Some(secs) if secs < TOOL_IN_FLIGHT_MAX_SECS) {
            return AgentState::Working;
        }
        return AgentState::WaitingForUser;
    }

    current
}

/// The state an exited session should report, from its wait status.
pub fn exit_state(code: Option<i32>, signal: Option<i32>) -> AgentState {
    match (code, signal) {
        (Some(0), None) => AgentState::Complete,
        (Some(_), None) => AgentState::Failed,
        // Killed by a signal. `rime agent kill` is the normal way a session
        // ends, so this is `exited`, not `failed` — a user stopping their own
        // agent has not suffered a failure.
        (_, Some(_)) => AgentState::Exited,
        (None, None) => AgentState::Exited,
    }
}

// ── §1.7: whose size the terminal is ────────────────────────────────────────
//
// A PTY has one size and a session can have several viewers. Until this, the
// rule was "whoever attached or resized last", and nothing ever put it back:
// a phone that opened a session's terminal left the desktop's terminal shrunk
// to 40 columns after the phone had gone. The rule now is that a phone gets
// its own size for as long as a phone is looking, and when the LAST phone
// stops looking the terminal returns to the size a local client last gave it.
//
// Pure, and a struct of its own rather than fields on the daemon's session, so
// the rule is tested here over every combination without a PTY, a socket or
// an origin that CI may not be able to classify. The daemon calls it and
// applies what it answers.

/// Which kind of client is looking at a session's terminal (§1.7).
///
/// Two classes, decided by the connection's §7 origin and never by which
/// socket a request arrived on: `rime-remoted` opens a fresh agentd
/// connection for every control frame, so a phone's `resize` does not arrive
/// on its attach connection, and only the origin ties the two together.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Viewer {
    /// Everything that is not a phone: the terminal `rime agent run` and
    /// `rime agent attach` are running in, a tmux pane, an ssh login.
    Local,
    /// A paired phone, through `rime-remoted`, which declares
    /// `claude-remote-control` on every connection it opens.
    Remote,
}

/// The sizes a session's terminal has been asked to be, by each kind of
/// viewer.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SizeClaims {
    /// The size a local client last gave it — its `run`, its attach, its
    /// `resize` — and so the size a departing phone gives back.
    pub local: Option<WinSize>,
    /// The size a phone last gave it.
    pub remote: Option<WinSize>,
}

impl SizeClaims {
    /// Record that `viewer` asked for `size`, without deciding anything.
    ///
    /// What a session's `run` does: the terminal it was started from is the
    /// first local size it has, so a session started on the desktop and
    /// opened on a phone before anybody attached still has a size to return
    /// to.
    pub fn claim(&mut self, viewer: Viewer, size: WinSize) {
        match viewer {
            Viewer::Local => self.local = Some(size),
            Viewer::Remote => self.remote = Some(size),
        }
    }

    /// A client attached at `size`, and the size the terminal takes.
    ///
    /// Always its own: whoever attaches is about to be sent a repaint, and a
    /// repaint drawn for another grid is garbage on this one. The phone gets
    /// the phone's size while it is looking; what changes is that the
    /// desktop's is remembered, and [`SizeClaims::settle`] gives it back.
    pub fn attached(&mut self, viewer: Viewer, size: WinSize) -> WinSize {
        self.claim(viewer, size);
        size
    }

    /// A client sent a `resize`, and the size the terminal should take, if
    /// any.
    ///
    /// A local resize always applies — somebody is dragging a window they are
    /// looking at. A remote one applies while a phone is attached, or when
    /// there is no local size to protect. The case it does NOT apply in is
    /// the one that would undo the whole point: a phone's resize frame that
    /// arrives after the phone's terminal has closed, on its own connection,
    /// and would shrink the desktop again with nothing left to restore it.
    pub fn resized(&mut self, viewer: Viewer, size: WinSize, remote_viewers: usize) -> Option<WinSize> {
        self.claim(viewer, size);
        match viewer {
            Viewer::Local => Some(size),
            Viewer::Remote if remote_viewers > 0 || self.local.is_none() => Some(size),
            Viewer::Remote => None,
        }
    }

    /// What the terminal should become now that its viewers changed, if
    /// anything.
    ///
    /// While any phone is attached, nothing: the phone keeps its size. Once
    /// none is, the local size, if there is one and the terminal is not
    /// already at it. Called whenever a viewer goes away, however it went —
    /// a detach, or a dead connection dropped mid-write.
    pub fn settle(&self, remote_viewers: usize, current: WinSize) -> Option<WinSize> {
        if remote_viewers > 0 {
            return None;
        }
        self.local.filter(|l| *l != current)
    }

    /// A viewer typed `bytes` into its own attach, and the size the terminal
    /// should take for it, if it is not already there.
    ///
    /// tmux's `window-size latest`: the person typing is the person looking.
    /// A desktop user who starts typing while a phone is attached gets the
    /// desktop's size back; the phone gets its own back when the phone types.
    ///
    /// Only for [`is_keystrokes`], and that restriction is what makes this
    /// safe to do at all. A terminal emulator writes on its own — replies to a
    /// program's queries (cursor position, device attributes, colours), focus
    /// and mouse reports — and EVERY attached terminal answers a query the
    /// program prints. Treating those as typing would flip the size to
    /// whichever terminal answered last, the resize would make the program
    /// repaint, the repaint could ask again, and the two terminals would take
    /// turns resizing it forever. Every one of those replies begins with ESC;
    /// a person's letters, Enter, Backspace and Ctrl-keys do not.
    pub fn typed(
        &self,
        viewer: Viewer,
        bytes: &[u8],
        remote_viewers: usize,
        current: WinSize,
    ) -> Option<WinSize> {
        if !is_keystrokes(bytes) {
            return None;
        }
        let want = match viewer {
            Viewer::Local => self.local?,
            Viewer::Remote if remote_viewers > 0 => self.remote?,
            Viewer::Remote => return None,
        };
        (want != current).then_some(want)
    }
}

/// Whether a chunk of a viewer's input can only be a person typing.
///
/// No ESC anywhere in it. See [`SizeClaims::typed`] for why that is the line:
/// it gives up arrow keys and pastes, which only delays the size change until
/// the next ordinary key, to rule out every byte a terminal sends unasked.
pub fn is_keystrokes(bytes: &[u8]) -> bool {
    !bytes.is_empty() && !bytes.contains(&0x1b)
}

/// Map a signal name accepted by `rime agent signal` to its number.
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

    /// `next_state` for a session no hook has ever spoken for, which is every
    /// agent but Claude and is what these cases are about.
    fn infer(
        current: AgentState,
        signals: &[Signal],
        had_output: bool,
        idle_secs: u64,
    ) -> AgentState {
        next_state(current, signals, had_output, idle_secs, None)
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
        assert!(scan(b"\x1b]8;;https://example.com\x07").is_empty());
        assert!(scan(b"\x1b]1;icon name\x07").is_empty());
    }

    #[test]
    fn a_window_title_is_a_title_and_never_a_notification() {
        // OSC 0 is the most common sequence in any TUI. It used to be ignored;
        // it is now the session's title (§1.3). What must stay true is that it
        // is never mistaken for a notification, and never moves the state.
        let signals = scan(b"\x1b]0;my terminal title\x07");
        assert_eq!(signals, vec![Signal::Title(Some("my terminal title".into()))]);
        assert!(!signals.iter().any(|s| matches!(s, Signal::Notification(_))));
        assert_eq!(signals[0].implied_state(), None);
        assert_eq!(signals[0].detail(), None);
        // Claude retitles itself while it waits, so a title that moved the
        // state would call a waiting agent working.
        assert_eq!(
            infer(AgentState::WaitingForUser, &signals, false, 0),
            AgentState::WaitingForUser
        );
        // OSC 2, with the string terminator instead of a bell.
        assert_eq!(
            scan(b"\x1b]2;other title\x1b\\"),
            vec![Signal::Title(Some("other title".into()))]
        );
        // And the BEL that closes it is not a bell.
        assert!(!scan(b"\x1b]0;t\x07").contains(&Signal::Bell));
    }

    #[test]
    fn claude_codes_title_loses_its_spinner_and_keeps_its_summary() {
        // The exact bytes measured from Claude Code: `ESC ] 0 ; ✳ <summary> BEL`.
        assert_eq!(
            scan("\x1b]0;✳ Rime showcase studio\x07".as_bytes()),
            vec![Signal::Title(Some("Rime showcase studio".into()))]
        );
        // Every frame of its working spinner, and a braille dot spinner, give
        // the SAME title — which is what keeps a working agent from writing
        // its record ten times a second.
        for frame in ["·", "✢", "✳", "✶", "✻", "✽", "⠋", "⠙", "⣾", "●", "*"] {
            let raw = format!("\x1b]0;{frame} Rime showcase studio\x07");
            assert_eq!(
                scan(raw.as_bytes()),
                vec![Signal::Title(Some("Rime showcase studio".into()))],
                "{frame}"
            );
        }
    }

    #[test]
    fn an_empty_title_clears_it() {
        assert_eq!(scan(b"\x1b]0;\x07"), vec![Signal::Title(None)]);
        assert_eq!(scan(b"\x1b]0\x07"), vec![Signal::Title(None)]);
        // A spinner with nothing after it is a title of nothing, too.
        assert_eq!(scan("\x1b]2;⠋  \x07".as_bytes()), vec![Signal::Title(None)]);
    }

    #[test]
    fn a_title_split_across_reads_is_still_one_title() {
        let mut s = OutputScanner::new();
        assert!(s.feed("\x1b]0;✳ Rime show".as_bytes()).is_empty());
        assert_eq!(
            s.feed(b"case studio\x07"),
            vec![Signal::Title(Some("Rime showcase studio".into()))]
        );
    }

    #[test]
    fn the_title_cleaner_drops_control_characters_and_caps_the_length() {
        // Control characters are removed, not refused: nobody is on the other
        // end of a terminal title to be told no.
        assert_eq!(clean_title("a\u{7f}b\u{1b}c\td").as_deref(), Some("abcd"));
        let long = "x".repeat(MAX_TITLE_CHARS + 40);
        let cut = clean_title(&long).unwrap();
        assert_eq!(cut.chars().count(), MAX_TITLE_CHARS);
        // Characters, not bytes: a cut through a multi-byte character would
        // be a panic in a slice, and a cut by bytes would give CJK titles a
        // third of the room.
        let wide = "漢".repeat(MAX_TITLE_CHARS + 5);
        assert_eq!(clean_title(&wide).unwrap().chars().count(), MAX_TITLE_CHARS);
        // Only the LEADING glyphs go; one inside the title is part of it.
        assert_eq!(clean_title("✳ fix * in globs").as_deref(), Some("fix * in globs"));
        assert_eq!(clean_title("  plain  ").as_deref(), Some("plain"));
        assert_eq!(clean_title(""), None);
    }

    #[test]
    fn a_session_name_is_trimmed_and_an_empty_one_clears() {
        assert_eq!(session_name("  auth refactor  "), Ok(Some("auth refactor".into())));
        assert_eq!(session_name(""), Ok(None));
        assert_eq!(session_name("   \t "), Ok(None));
        assert_eq!(session_name_opt(None), Ok(None));
        assert_eq!(session_name_opt(Some("x")), Ok(Some("x".into())));
    }

    #[test]
    fn a_session_name_is_refused_rather_than_sanitised() {
        // Exactly at the limit is fine, one past it is not, and the limit is
        // in characters.
        let at = "é".repeat(MAX_NAME_CHARS);
        assert_eq!(session_name(&at), Ok(Some(at.clone())));
        let past = "é".repeat(MAX_NAME_CHARS + 1);
        let why = session_name(&past).expect_err("one past the limit");
        assert!(why.contains("64"), "{why}");
        assert!(why.contains(&(MAX_NAME_CHARS + 1).to_string()), "{why}");
        // A control character anywhere inside is refused — `phone\r\nAPPROVED`
        // is a display attack, not a name — and trimming does not rescue one
        // in the middle. A trailing newline is only whitespace and is trimmed.
        for bad in ["a\rb", "a\u{1b}[2Jb", "a\u{7}b", "a\u{0}b", "a\u{85}b"] {
            let why = session_name(bad).expect_err(bad);
            assert!(why.contains("control characters"), "{why}");
        }
        assert_eq!(session_name("name\n"), Ok(Some("name".into())));
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
    fn bracketed_paste_is_off_until_the_application_asks() {
        let mut s = OutputScanner::new();
        assert!(!s.bracketed_paste());
        s.feed(b"hello world\n");
        assert!(!s.bracketed_paste(), "plain output must not turn it on");
        s.feed(b"\x1b[?2004h");
        assert!(s.bracketed_paste());
        s.feed(b"\x1b[?2004l");
        assert!(!s.bracketed_paste(), "the application asked for it to stop");
    }

    #[test]
    fn the_mode_is_recognised_when_it_arrives_beside_others() {
        // Every full-screen TUI sends the alternate screen and bracketed paste
        // together, and some send them in one sequence.
        let mut s = OutputScanner::new();
        s.feed(b"\x1b[?1049;2004h");
        assert!(s.bracketed_paste());
        s.feed(b"\x1b[?1049;2004l");
        assert!(!s.bracketed_paste());
    }

    #[test]
    fn a_sequence_split_across_reads_is_still_one_sequence() {
        // The reason the scanner carries state at all. A 4 KiB read boundary
        // lands wherever it lands.
        let mut s = OutputScanner::new();
        s.feed(b"\x1b[?20");
        assert!(!s.bracketed_paste(), "incomplete is not on");
        s.feed(b"04h");
        assert!(s.bracketed_paste());
    }

    #[test]
    fn a_number_that_merely_contains_2004_is_not_the_mode() {
        let mut s = OutputScanner::new();
        s.feed(b"\x1b[?12004h");
        assert!(!s.bracketed_paste(), "12004 is not 2004");
        s.feed(b"\x1b[?20041h");
        assert!(!s.bracketed_paste(), "20041 is not 2004");
        // And a public mode 2004 is a different mode from the private one.
        s.feed(b"\x1b[2004h");
        assert!(!s.bracketed_paste(), "no ? means no DEC private mode");
    }

    #[test]
    fn learning_about_csi_did_not_cost_the_scanner_a_bell() {
        // Before this scanner knew what a CSI was, `ESC [` fell straight back
        // to ground and a BEL after it was a bell. It still is: a malformed
        // escape must not swallow the one signal an agent uses to say it wants
        // attention.
        assert_eq!(scan(b"\x1b[31\x07"), vec![Signal::Bell]);
        assert_eq!(scan(b"\x1b[\x07"), vec![Signal::Bell]);
        // A well-formed CSI still ends at its final byte, and the bell after
        // it is seen.
        assert_eq!(scan(b"\x1b[31m\x07"), vec![Signal::Bell]);
        // And an OSC that follows a CSI still parses.
        assert_eq!(
            scan(b"\x1b[2J\x1b]9;done\x07"),
            vec![Signal::Notification("done".to_string())]
        );
    }

    #[test]
    fn an_oversized_csi_cannot_grow_without_bound_and_recovers() {
        let mut s = OutputScanner::new();
        let mut junk = Vec::from(&b"\x1b[?"[..]);
        junk.extend(std::iter::repeat(b'1').take(MAX_CSI_PARAMS + 64));
        junk.extend_from_slice(b"h");
        s.feed(&junk);
        assert!(!s.bracketed_paste());
        // Back in ground state: a real request is still recognised.
        s.feed(b"\x1b[?2004h");
        assert!(s.bracketed_paste());
    }

    #[test]
    fn output_alone_means_working() {
        assert_eq!(
            infer(AgentState::Starting, &[], true, 0),
            AgentState::Working
        );
    }

    #[test]
    fn silence_past_the_threshold_means_waiting() {
        assert_eq!(
            infer(AgentState::Working, &[], false, IDLE_TO_WAITING_SECS),
            AgentState::WaitingForUser
        );
        // Just under the threshold, nothing changes.
        assert_eq!(
            infer(AgentState::Working, &[], false, IDLE_TO_WAITING_SECS - 1),
            AgentState::Working
        );
    }

    #[test]
    fn a_signal_beats_the_idle_rule_and_raw_output() {
        assert_eq!(
            infer(AgentState::Working, &[Signal::Bell], true, 0),
            AgentState::WaitingForUser
        );
        assert_eq!(
            infer(
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
            infer(AgentState::Starting, &signals, true, 0),
            AgentState::Working
        );
    }

    #[test]
    fn permission_request_is_not_overwritten_by_inference() {
        // Neither output nor silence may downgrade a published permission
        // request; only another event or the process exiting.
        assert_eq!(
            infer(AgentState::PermissionRequest, &[], true, 0),
            AgentState::PermissionRequest
        );
        assert_eq!(
            infer(AgentState::PermissionRequest, &[], false, 3600),
            AgentState::PermissionRequest
        );
        // An explicit signal still moves it.
        assert_eq!(
            infer(
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
            assert_eq!(infer(s, &[Signal::Bell], true, 0), s);
            assert_eq!(infer(s, &[], false, 9999), s);
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

    #[test]
    fn a_tool_in_flight_holds_working_through_the_silence_the_idle_rule_misreads() {
        // The case §6.1 exists for. `cargo test` prints nothing for two
        // minutes; without the hook the idle rule calls that waiting_for_user
        // after ten seconds and is wrong for the rest of the run.
        let quiet = 120;
        assert_eq!(
            infer(AgentState::Working, &[], false, quiet),
            AgentState::WaitingForUser,
            "inference alone gets this wrong, which is the point"
        );
        assert_eq!(
            next_state(AgentState::Working, &[], false, quiet, Some(quiet)),
            AgentState::Working
        );
    }

    #[test]
    fn a_tool_that_never_reported_finishing_stops_pinning_the_session() {
        // A crashed daemon, a --bare session or an agent that rewrote its own
        // settings all end the event stream mid-call. The bound is what makes
        // that a delay rather than a session stuck on `working` forever.
        assert_eq!(
            next_state(
                AgentState::Working,
                &[],
                false,
                TOOL_IN_FLIGHT_MAX_SECS,
                Some(TOOL_IN_FLIGHT_MAX_SECS)
            ),
            AgentState::WaitingForUser
        );
        assert_eq!(
            next_state(
                AgentState::Working,
                &[],
                false,
                TOOL_IN_FLIGHT_MAX_SECS,
                Some(TOOL_IN_FLIGHT_MAX_SECS - 1)
            ),
            AgentState::Working
        );
    }

    #[test]
    fn a_tool_in_flight_changes_nothing_else_about_the_rule() {
        // It is one condition on one branch. Output still means working, a
        // signal still wins, a terminal state is still terminal, and a
        // permission request is still not overwritten — otherwise the flag
        // would be a second state machine racing the first.
        for tool in [None, Some(0), Some(5), Some(TOOL_IN_FLIGHT_MAX_SECS + 1)] {
            assert_eq!(
                next_state(AgentState::Starting, &[], true, 0, tool),
                AgentState::Working
            );
            assert_eq!(
                next_state(AgentState::Working, &[Signal::Bell], false, 0, tool),
                AgentState::WaitingForUser
            );
            assert_eq!(
                next_state(AgentState::PermissionRequest, &[], false, 3600, tool),
                AgentState::PermissionRequest
            );
            assert_eq!(
                next_state(AgentState::Complete, &[], false, 3600, tool),
                AgentState::Complete
            );
            // And below the idle threshold nothing moves either way.
            assert_eq!(
                next_state(AgentState::Working, &[], false, 1, tool),
                AgentState::Working
            );
        }
    }
    /// One turn, replayed a second at a time, counting the seconds the
    /// reported state disagrees with what the session was actually doing.
    ///
    /// The timeline is written out rather than derived, because it is the
    /// ground truth the two answers are scored against: the agent works from
    /// the prompt until `stop_at`, and a tool runs quietly in the middle of
    /// that. Output lands when the tool prints its result and when the agent
    /// prints its answer — the silence in between is the whole problem.
    ///
    /// Returns (seconds a running agent was called idle, seconds a finished
    /// turn was called working). The daemon's own order is reproduced: output
    /// is absorbed first and a published event overrides it, which is what
    /// happens when Claude prints its answer and then runs its `Stop` hook.
    fn misreported(hooks: bool, tool_at: u64, tool_secs: u64, stop_at: u64, turn: u64) -> (u64, u64) {
        let tool_end = tool_at + tool_secs;
        let mut state = AgentState::Working;
        let mut last_activity = 0u64;
        let mut tool_started: Option<u64> = None;
        let (mut called_idle, mut called_working) = (0, 0);

        for now in 1..=turn {
            let output = now == tool_end || now == stop_at;
            if output {
                last_activity = now;
            }
            let in_flight = tool_started.map(|at| now - at);
            state = next_state(state, &[], output, now - last_activity, in_flight);

            if hooks {
                let event = match now {
                    n if n == tool_at => Some(crate::hook::HookEvent::PreToolUse),
                    n if n == tool_end => Some(crate::hook::HookEvent::PostToolUse),
                    n if n == stop_at => Some(crate::hook::HookEvent::Stop),
                    _ => None,
                };
                if let Some(e) = event {
                    let o = crate::hook::observe(e, &crate::hook::Payload::default());
                    match o.tool {
                        crate::hook::ToolTransition::Started => tool_started = Some(now),
                        crate::hook::ToolTransition::Finished => tool_started = None,
                        crate::hook::ToolTransition::Unchanged => {}
                    }
                    if let Some(published) = o.state {
                        state = published;
                    }
                    last_activity = now;
                }
            }

            let working = now < stop_at;
            match (working, state == AgentState::Working) {
                (true, false) => called_idle += 1,
                (false, true) => called_working += 1,
                _ => {}
            }
        }
        (called_idle, called_working)
    }

    #[test]
    fn the_hook_bridge_is_measurably_more_accurate_than_the_idle_rule() {
        // Acceptance criterion 3 of P0-011 as a number rather than a claim. A
        // two-minute quiet tool call five seconds into a turn that ends at
        // 130s, watched for three minutes.
        let (tool_at, tool_secs, stop_at, turn) = (5, 120, 130, 180);
        let (idle_wrong, idle_late) = misreported(false, tool_at, tool_secs, stop_at, turn);
        let (hook_wrong, hook_late) = misreported(true, tool_at, tool_secs, stop_at, turn);

        // Inference: wrong from the tenth second of silence until the tool
        // printed — 115 of the 129 seconds the agent was working — and then
        // wrong the other way for the ten seconds after the turn ended, while
        // it waited for the silence to reach the threshold. 125 seconds of a
        // 180-second turn reported as the opposite of what was happening.
        assert_eq!(idle_wrong, tool_at + tool_secs - IDLE_TO_WAITING_SECS);
        assert_eq!(idle_wrong, 115);
        assert_eq!(idle_late, IDLE_TO_WAITING_SECS);
        assert_eq!(idle_wrong + idle_late, 125);

        // Hooks: right every second of the turn.
        assert_eq!((hook_wrong, hook_late), (0, 0));
    }

    #[test]
    fn the_idle_rule_cannot_reach_permission_request_at_all() {
        // The other half of criterion 3, and the larger half: no sequence of
        // output, silence or signals produces this state, because no pattern
        // match on arbitrary terminal output can recognise a permission prompt.
        // A hook publishes it directly. Accuracy for this state is therefore
        // not "better" — it is zero against one.
        let every_signal = [
            Signal::Bell,
            Signal::CommandStarted,
            Signal::PromptReady,
            Signal::Notification(String::new()),
            Signal::Title(Some("waiting for your permission".into())),
            Signal::Title(None),
        ];
        for current in [
            AgentState::Starting,
            AgentState::Working,
            AgentState::WaitingForUser,
        ] {
            for had_output in [true, false] {
                for idle in [0, 1, IDLE_TO_WAITING_SECS, 3600] {
                    for tool in [None, Some(0), Some(TOOL_IN_FLIGHT_MAX_SECS + 1)] {
                        assert_ne!(
                            next_state(current, &[], had_output, idle, tool),
                            AgentState::PermissionRequest
                        );
                        for sig in &every_signal {
                            assert_ne!(
                                next_state(
                                    current,
                                    std::slice::from_ref(sig),
                                    had_output,
                                    idle,
                                    tool
                                ),
                                AgentState::PermissionRequest,
                                "{sig:?}"
                            );
                        }
                    }
                }
            }
        }
        // And the hook is the one thing that does produce it.
        assert_eq!(
            crate::hook::observe(
                crate::hook::HookEvent::PermissionRequest,
                &crate::hook::Payload::default()
            )
            .state,
            Some(AgentState::PermissionRequest)
        );
    }


    // ── §1.7 ────────────────────────────────────────────────────────────────

    const DESK: WinSize = WinSize { cols: 180, rows: 50 };
    const DESK2: WinSize = WinSize { cols: 200, rows: 60 };
    const PHONE: WinSize = WinSize { cols: 46, rows: 30 };
    const PHONE_KB: WinSize = WinSize { cols: 46, rows: 18 };

    #[test]
    fn the_phone_gets_its_size_and_gives_the_desktops_back_when_it_goes() {
        // The bug, step by step: the desktop is attached, the phone opens the
        // same session, the phone closes it.
        let mut c = SizeClaims::default();
        let mut pty = c.attached(Viewer::Local, DESK);
        assert_eq!(pty, DESK);

        pty = c.attached(Viewer::Remote, PHONE);
        assert_eq!(pty, PHONE, "the phone must be drawn for the phone");
        // The phone's keyboard comes up: a resize, while it is attached.
        pty = c.resized(Viewer::Remote, PHONE_KB, 1).expect("applies while attached");
        assert_eq!(pty, PHONE_KB);
        // Still attached: nothing to settle.
        assert_eq!(c.settle(1, pty), None);

        // The phone detaches. Before §1.7 the answer here was None and the
        // desktop stayed 46 columns wide.
        assert_eq!(c.settle(0, pty), Some(DESK));
        // Once back, settling again is a no-op, not a resize storm.
        assert_eq!(c.settle(0, DESK), None);
    }

    #[test]
    fn a_late_phone_resize_cannot_shrink_the_desktop_again() {
        // rime-remoted sends every control frame on its own connection, so a
        // resize can land after the attach it belonged to has closed.
        let mut c = SizeClaims::default();
        c.attached(Viewer::Local, DESK);
        c.attached(Viewer::Remote, PHONE);
        assert_eq!(c.settle(0, PHONE), Some(DESK));
        assert_eq!(c.resized(Viewer::Remote, PHONE_KB, 0), None);
        // …unless there is no desktop size to protect: a session the phone
        // started, that nothing local has ever looked at.
        let mut phone_only = SizeClaims::default();
        phone_only.claim(Viewer::Remote, PHONE);
        assert_eq!(phone_only.resized(Viewer::Remote, PHONE_KB, 0), Some(PHONE_KB));
        assert_eq!(phone_only.settle(0, PHONE_KB), None, "nothing local to return to");
    }

    #[test]
    fn a_local_resize_always_applies_and_is_what_is_given_back() {
        let mut c = SizeClaims::default();
        c.attached(Viewer::Local, DESK);
        c.attached(Viewer::Remote, PHONE);
        // The desktop user resizes their window while the phone is looking.
        assert_eq!(c.resized(Viewer::Local, DESK2, 1), Some(DESK2));
        assert_eq!(c.local, Some(DESK2));
        // And the phone leaving gives back THAT size, not the older one.
        assert_eq!(c.settle(0, PHONE), Some(DESK2));
    }

    #[test]
    fn a_session_started_on_the_desktop_has_a_size_to_return_to_before_anyone_attaches() {
        // `rime agent run -d`, then the phone opens it.
        let mut c = SizeClaims::default();
        c.claim(Viewer::Local, DESK);
        c.attached(Viewer::Remote, PHONE);
        assert_eq!(c.settle(0, PHONE), Some(DESK));
    }

    #[test]
    fn two_phones_keep_the_phone_size_until_the_last_one_goes() {
        let mut c = SizeClaims::default();
        c.attached(Viewer::Local, DESK);
        c.attached(Viewer::Remote, PHONE);
        c.attached(Viewer::Remote, PHONE_KB);
        assert_eq!(c.settle(1, PHONE_KB), None);
        assert_eq!(c.settle(0, PHONE_KB), Some(DESK));
    }

    #[test]
    fn with_no_phone_ever_nothing_about_the_old_behaviour_changes() {
        // Two desktop terminals, one resize: the last one wins, as it always
        // did, and settling never moves anything.
        let mut c = SizeClaims::default();
        let a = c.attached(Viewer::Local, DESK);
        assert_eq!(c.settle(0, a), None);
        let b = c.attached(Viewer::Local, DESK2);
        assert_eq!(c.settle(0, b), None);
        assert_eq!(c.resized(Viewer::Local, DESK, 0), Some(DESK));
        assert_eq!(c.settle(0, DESK), None);
        assert_eq!(c.typed(Viewer::Local, b"ls\r", 0, DESK), None);
    }

    #[test]
    fn the_latest_typer_gets_its_size() {
        let mut c = SizeClaims::default();
        c.attached(Viewer::Local, DESK);
        let mut pty = c.attached(Viewer::Remote, PHONE);
        // The desktop user types while the phone is attached.
        pty = c.typed(Viewer::Local, b"y", 1, pty).expect("desktop typing takes it back");
        assert_eq!(pty, DESK);
        // Typing again changes nothing: it is already there.
        assert_eq!(c.typed(Viewer::Local, b"es", 1, pty), None);
        // The phone types: the phone's size.
        pty = c.typed(Viewer::Remote, b"\r", 1, pty).expect("the phone takes it back");
        assert_eq!(pty, PHONE);
    }

    #[test]
    fn a_terminal_answering_a_query_is_not_typing() {
        // The loop this guards: every attached terminal answers the program's
        // queries, so if an answer counted as typing each would take the size
        // in turn, and the repaint that follows a resize can ask again.
        let mut c = SizeClaims::default();
        c.attached(Viewer::Local, DESK);
        c.attached(Viewer::Remote, PHONE);
        for reply in [
            &b"\x1b[24;1R"[..],               // cursor position report
            b"\x1b[?62;22c",                  // device attributes
            b"\x1b]11;rgb:0000/0000/0000\x1b\\", // background colour
            b"\x1b[I",                        // focus in
            b"\x1b[<0;10;5M",                 // SGR mouse
            b"\x1b[A",                        // an arrow key, given up on purpose
            b"",
        ] {
            assert_eq!(c.typed(Viewer::Local, reply, 1, PHONE), None, "{reply:?}");
            assert_eq!(c.typed(Viewer::Remote, reply, 1, DESK), None, "{reply:?}");
        }
        for key in [&b"a"[..], b"\r", b"\x7f", b"\x03", b"\t", "é".as_bytes()] {
            assert!(is_keystrokes(key), "{key:?}");
        }
    }

    #[test]
    fn a_phone_that_is_not_attached_cannot_take_the_size_by_typing() {
        // Typing reaches this only through an attach, so a remote typist with
        // no remote viewer is a race — and the answer to a race is no.
        let mut c = SizeClaims::default();
        c.attached(Viewer::Local, DESK);
        c.claim(Viewer::Remote, PHONE);
        assert_eq!(c.typed(Viewer::Remote, b"x", 0, DESK), None);
    }
}
