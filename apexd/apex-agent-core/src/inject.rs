//! Handing a file to a running TUI agent.
//!
//! A screenshot, a log, a crash dump: something the user has in front of them
//! and wants the agent that is already running to look at. The session is
//! confined, so the file is very often somewhere the session cannot see —
//! `~/Pictures/Screenshots` is masked by the tmpfs over `$HOME` — and the only
//! process that can see both sides is `apex-agentd`, which is outside the
//! sandbox and owns the PTY.
//!
//! So the daemon copies the file into the session's own scratch directory,
//! which is already bound read-write and is deleted when the session ends, and
//! then types the path.
//!
//! ## The channel this writes on is the user's keyboard
//!
//! Bytes written to a PTY master arrive at the program as keystrokes. There is
//! no field in a terminal for "this came from somewhere else": a language model
//! reading its own input stream cannot tell an injected byte from a typed one,
//! and no amount of care here changes that. What the design does instead is
//! make the difference not matter, in four ways.
//!
//! 1. **The contents never travel on this channel.** Only a path does. The file
//!    reaches the model the way every other file does — through its own read
//!    tool, where its harness already treats the result as data rather than as
//!    instruction. This feature therefore makes a file exactly as trusted as
//!    `cat` would, and no more.
//! 2. **The daemon composes the text, not the caller.** The caller names a
//!    source; the destination is built here out of the session's scratch path,
//!    a counter and a name reduced to [`SAFE`]. A source called
//!    `x\r\n/quit\r\n.png` cannot put a newline on the PTY because the bytes on
//!    the PTY were never the caller's to begin with. [`payload`] re-checks that
//!    at the write site rather than trusting the construction.
//! 3. **Nothing is submitted.** No newline, no carriage return. The path is
//!    staged in the agent's input line and a human presses Enter. An injection
//!    that could submit would be a way to make an agent act while nobody is
//!    looking.
//! 4. **Every injection is recorded where the session cannot reach it.** The
//!    daemon mirrors it to `systemd-journald`, the same trail
//!    [`crate::journal`] uses for grants, so a person can ask what was handed
//!    to a session even though the machine's own agent runs as the user and
//!    could rewrite any file the user owns.
//!
//! ### What it costs, said plainly
//!
//! The bytes land wherever the PTY's foreground process is reading. If the
//! agent has spawned an editor or a pager, the path is typed into that instead
//! — harmless, and confusing. And a person who has been shown a path is a
//! person who may be persuaded to press Enter; staging is a speed bump in front
//! of a human, not a boundary.
//!
//! ### The one in-band signal that does exist
//!
//! `DECSET 2004` — bracketed paste. A terminal application that has asked for
//! it receives pasted text wrapped in `ESC [ 200 ~` … `ESC [ 201 ~`, which is
//! how a TUI tells a paste from typing. The daemon is this session's terminal,
//! so it knows whether the application asked: [`crate::session::OutputScanner`]
//! watches the output stream for the mode being set and cleared, and the
//! markers are sent only when it is on. That gives the *application* a way to
//! know; it still gives the model none, because whether the distinction
//! survives into the prompt is the application's choice and not this daemon's.

use std::path::{Path, PathBuf};

/// Directory inside a session's scratch that handed-in files land in.
///
/// A subdirectory rather than the scratch root: the scratch also holds the
/// hook settings, the redacted Claude configuration and the git shim, and a
/// user file dropped beside them could shadow one by name.
pub const INBOX: &str = "inbox";

/// The characters a copied file's name may contain.
///
/// Everything else is replaced. This is deliberately narrower than what a
/// filesystem accepts: the name ends up as bytes on a PTY, where a space
/// splits an argument, a quote opens a string and `$` starts an expansion.
/// None of those has to be *escaped* if none of them can be *present*.
pub const SAFE: &str = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789._-";

/// Longest name kept from a source file. Longer names are truncated from the
/// front, so the extension — the part that tells the agent what it is looking
/// at — survives.
pub const MAX_NAME: usize = 48;

/// Why a source file cannot be handed to a session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NameError {
    /// The path has no final component, or it is `.` or `..`.
    NoFileName,
    /// The name carries a byte a terminal would act on rather than display.
    ///
    /// Rejected rather than replaced. Reducing it silently would hand back a
    /// different file from the one the user named, and a user who typed a name
    /// with a newline in it has either made a mistake worth seeing or is being
    /// used by something that made it for them.
    ControlByte(u8),
}

impl std::fmt::Display for NameError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            NameError::NoFileName => write!(f, "that path names no file"),
            NameError::ControlByte(b) => write!(
                f,
                "that file's name contains a control character (0x{b:02x}), \
                 which a terminal would act on rather than show"
            ),
        }
    }
}

impl std::error::Error for NameError {}

/// Reduce a source file's name to something safe to type.
///
/// Returns the name a copy will be given, which is never the caller's string
/// verbatim.
pub fn safe_name(raw: &str) -> Result<String, NameError> {
    if raw.is_empty() || raw == "." || raw == ".." {
        return Err(NameError::NoFileName);
    }
    if let Some(b) = raw.bytes().find(|b| *b < 0x20 || *b == 0x7f) {
        return Err(NameError::ControlByte(b));
    }

    let mut out = String::with_capacity(raw.len());
    for ch in raw.chars() {
        if ch.is_ascii() && SAFE.contains(ch) {
            out.push(ch);
        } else {
            out.push('_');
        }
    }
    // A name that reduced to nothing but dots would make the destination
    // `001-..`, which is a directory traversal spelled as a file name. A name
    // that reduced to nothing at all would make it `001-`, which is merely
    // ugly. Both get the same fix, because both come from the same case: a
    // file whose name is entirely outside the alphabet, which on a machine
    // that is not in English is ordinary rather than hostile.
    if out.chars().all(|c| c == '.') {
        out = format!("file{out}");
    }
    if out.len() > MAX_NAME {
        // From the front: the tail carries the extension.
        out = out.split_off(out.len() - MAX_NAME);
    }
    Ok(out)
}

/// The path a handed-in file gets, given a session's scratch directory.
///
/// `seq` makes two files of the same name distinct without asking the
/// filesystem, and makes the order they were handed over readable.
pub fn destination(scratch: &Path, seq: u32, safe: &str) -> PathBuf {
    scratch.join(INBOX).join(format!("{seq:03}-{safe}"))
}

/// The text the daemon types, or `None` when it would not be plain.
///
/// The path is built from a scratch directory the daemon chose and a name
/// [`safe_name`] reduced, so this can only fail if one of those assumptions
/// stops holding. It is checked anyway, at the point of the write, because the
/// consequence of being wrong is an escape sequence delivered to a terminal as
/// though the user had typed it — and a check that runs where the bytes leave
/// is the one that cannot be bypassed by a new caller.
pub fn payload(path: &Path) -> Option<String> {
    let text = path.to_str()?;
    if text.is_empty() {
        return None;
    }
    if !text.bytes().all(is_plain) {
        return None;
    }
    Some(text.to_string())
}

/// Whether a byte may appear in the text typed into a session.
///
/// The safe name's alphabet plus `/`, and nothing else — no space, no quote,
/// no `$`, no ESC, no CR and no LF.
pub fn is_plain(b: u8) -> bool {
    b == b'/' || SAFE.as_bytes().contains(&b)
}

/// `ESC [ 200 ~`, the start of a bracketed paste.
pub const PASTE_START: &[u8] = b"\x1b[200~";
/// `ESC [ 201 ~`, the end of one.
pub const PASTE_END: &[u8] = b"\x1b[201~";

/// The exact bytes written to a session's PTY master.
///
/// A single trailing space and never a newline: the space separates this path
/// from the next one or from what the user types after it, and the absence of
/// the newline is what leaves submitting the line to a human. The space is
/// outside the paste markers because it is not part of the pasted text.
pub fn keystrokes(text: &str, bracketed: bool) -> Vec<u8> {
    let mut out = Vec::with_capacity(text.len() + PASTE_START.len() + PASTE_END.len() + 1);
    if bracketed {
        out.extend_from_slice(PASTE_START);
    }
    out.extend_from_slice(text.as_bytes());
    if bracketed {
        out.extend_from_slice(PASTE_END);
    }
    out.push(b' ');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_name_that_could_end_a_line_is_refused_rather_than_repaired() {
        // The whole reason this module composes the text itself. A file called
        // this is not a file the user meant to hand over.
        for raw in [
            "shot\n/quit.png",
            "shot\r.png",
            "shot\x1b[201~.png",
            "a\u{7f}b",
        ] {
            assert!(
                matches!(safe_name(raw), Err(NameError::ControlByte(_))),
                "accepted {raw:?}"
            );
        }
    }

    #[test]
    fn everything_a_shell_or_a_terminal_would_act_on_is_gone() {
        let out = safe_name("we ird;$(id)`x`'q'\"q\"|&<>*?[]{}~!#.png").expect("no control bytes");
        assert!(
            out.bytes().all(|b| SAFE.as_bytes().contains(&b)),
            "survived: {out:?}"
        );
        assert!(out.ends_with(".png"));
    }

    #[test]
    fn a_name_with_no_latin_letters_still_produces_something_typeable() {
        // Ordinary on a machine that is not in English, and it must not become
        // a name a terminal or a path resolver would read as anything but a
        // file.
        let out = safe_name("スクリーンショット.png").expect("no control bytes");
        assert!(out.ends_with(".png"), "{out}");
        assert!(out.bytes().all(|b| SAFE.as_bytes().contains(&b)), "{out}");

        // The degenerate case is a name that survives as nothing but dots.
        // `001-..` would be a directory traversal spelled as a file name.
        for raw in ["...", "....", "…", "。。"] {
            let out = safe_name(raw).expect("no control bytes");
            assert!(
                out.chars().any(|c| c != '.'),
                "{raw:?} reduced to {out:?}, which names a directory"
            );
        }
    }

    #[test]
    fn a_long_name_keeps_its_extension() {
        let raw = format!("{}.png", "a".repeat(400));
        let out = safe_name(&raw).unwrap();
        assert!(out.len() <= MAX_NAME);
        assert!(out.ends_with(".png"), "{out}");
    }

    #[test]
    fn no_file_name_at_all_is_an_error_and_not_an_empty_string() {
        for raw in ["", ".", ".."] {
            assert_eq!(safe_name(raw), Err(NameError::NoFileName), "{raw:?}");
        }
    }

    #[test]
    fn the_payload_refuses_anything_a_terminal_would_read_as_a_command() {
        assert!(payload(Path::new("/tmp/apex-agent/3/inbox/001-shot.png")).is_some());
        for bad in [
            "/tmp/a b",
            "/tmp/a\nb",
            "/tmp/a\rb",
            "/tmp/a\x1b[201~b",
            "/tmp/a'b",
            "/tmp/a$b",
            "",
        ] {
            assert!(payload(Path::new(bad)).is_none(), "accepted {bad:?}");
        }
    }

    #[test]
    fn every_destination_a_safe_name_can_produce_is_typeable() {
        // The two halves have to agree: if `safe_name` can produce a name that
        // `payload` then refuses, a legitimate file becomes an internal error
        // at the moment the user tries to hand it over.
        let scratch = Path::new("/tmp/apex-agent/7");
        for raw in [
            "shot.png",
            "we ird;$(id).log",
            "スクリーンショット.png",
            "..hidden..",
            &"z".repeat(300),
        ] {
            let safe = safe_name(raw).expect("no control bytes");
            let dest = destination(scratch, 1, &safe);
            assert!(
                payload(&dest).is_some(),
                "safe_name({raw:?}) -> {safe:?} -> {} was refused",
                dest.display()
            );
        }
    }

    #[test]
    fn nothing_written_can_submit_the_line() {
        // The property that keeps a human in the loop. Asserted over both
        // forms, because the bracketed one adds bytes.
        let text = "/tmp/apex-agent/3/inbox/001-shot.png";
        for bracketed in [false, true] {
            let bytes = keystrokes(text, bracketed);
            assert!(!bytes.contains(&b'\n'), "newline in {bytes:?}");
            assert!(!bytes.contains(&b'\r'), "carriage return in {bytes:?}");
            assert!(bytes.ends_with(b" "));
        }
    }

    #[test]
    fn the_paste_markers_are_sent_only_when_the_application_asked_for_them() {
        let text = "/tmp/x";
        let plain = keystrokes(text, false);
        assert_eq!(plain, b"/tmp/x ".to_vec());
        assert!(!plain.contains(&0x1b), "an escape reached a plain terminal");

        let wrapped = keystrokes(text, true);
        assert!(wrapped.starts_with(PASTE_START));
        assert!(wrapped.ends_with(b"\x1b[201~ "));
        // The path itself is between the markers, not around them.
        assert_eq!(
            &wrapped[PASTE_START.len()..PASTE_START.len() + text.len()],
            text.as_bytes()
        );
    }
}
