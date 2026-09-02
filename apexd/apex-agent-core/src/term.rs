//! Terminal handling for the attaching client.
//!
//! Small, deliberate and RAII-based: the one thing that must never happen is
//! leaving the user's terminal in raw mode after `apex agent attach` exits.
//! [`RawMode`] restores the original `termios` in `Drop`, so a panic, an error
//! return and a normal detach all take the same path back.

use std::io;
use std::os::unix::io::{AsRawFd, RawFd};

/// A terminal size in character cells.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WinSize {
    pub cols: u16,
    pub rows: u16,
}

impl WinSize {
    /// A conservative size for when the real one cannot be read — a pipe, a
    /// systemd unit, a CI runner. 80x24 is the VT100 default and every TUI
    /// copes with it.
    pub const FALLBACK: WinSize = WinSize { cols: 80, rows: 24 };

    /// Reject a degenerate size. A zero dimension makes some TUIs divide by
    /// zero, and the kernel reports 0x0 for a terminal that is not attached
    /// yet.
    pub fn or_fallback(self) -> WinSize {
        if self.cols == 0 || self.rows == 0 {
            WinSize::FALLBACK
        } else {
            self
        }
    }
}

/// Read the window size of `fd`.
pub fn window_size(fd: RawFd) -> WinSize {
    let mut ws: libc::winsize = unsafe { std::mem::zeroed() };
    // Safe: TIOCGWINSZ writes a fixed-size struct we own. A non-tty fd fails
    // and leaves `ws` zeroed, which `or_fallback` then corrects.
    let rc = unsafe { libc::ioctl(fd, libc::TIOCGWINSZ, &mut ws) };
    if rc != 0 {
        return WinSize::FALLBACK;
    }
    WinSize {
        cols: ws.ws_col,
        rows: ws.ws_row,
    }
    .or_fallback()
}

/// The size of this process's controlling terminal, via stdout then stdin.
pub fn stdout_window_size() -> WinSize {
    let out = window_size(libc::STDOUT_FILENO);
    if out != WinSize::FALLBACK {
        return out;
    }
    window_size(libc::STDIN_FILENO)
}

/// Set the window size of a PTY master, which sends `SIGWINCH` to the
/// foreground process group on the other side.
pub fn set_window_size(fd: RawFd, size: WinSize) -> io::Result<()> {
    let ws = libc::winsize {
        ws_row: size.rows,
        ws_col: size.cols,
        ws_xpixel: 0,
        ws_ypixel: 0,
    };
    // Safe: TIOCSWINSZ reads a fixed-size struct we own.
    let rc = unsafe { libc::ioctl(fd, libc::TIOCSWINSZ, &ws) };
    if rc != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

/// Whether `fd` is a terminal.
pub fn is_tty(fd: RawFd) -> bool {
    // Safe: isatty only inspects the descriptor.
    unsafe { libc::isatty(fd) == 1 }
}

/// Puts a terminal into raw mode and puts it back on drop.
///
/// Raw mode is what makes an attached session feel like the agent's own
/// terminal: no line buffering, no echo, and control characters delivered to
/// the agent instead of being interpreted locally.
pub struct RawMode {
    fd: RawFd,
    original: libc::termios,
    restored: bool,
}

impl RawMode {
    /// Enter raw mode on `fd`. Returns `Ok(None)` when `fd` is not a terminal,
    /// so a piped `apex agent attach` still works — it just has nothing to
    /// switch.
    pub fn enter(fd: RawFd) -> io::Result<Option<RawMode>> {
        if !is_tty(fd) {
            return Ok(None);
        }
        let mut original: libc::termios = unsafe { std::mem::zeroed() };
        // Safe: tcgetattr fills a struct we own.
        if unsafe { libc::tcgetattr(fd, &mut original) } != 0 {
            return Err(io::Error::last_os_error());
        }

        let mut raw = original;
        // Safe: cfmakeraw only rewrites the struct.
        unsafe { libc::cfmakeraw(&mut raw) };
        // Block in read() until at least one byte arrives, with no timeout.
        // Polling with VMIN=0 would spin a core for the whole session.
        raw.c_cc[libc::VMIN] = 1;
        raw.c_cc[libc::VTIME] = 0;

        // TCSAFLUSH would discard input typed between the two calls. TCSADRAIN
        // waits for pending output and keeps it.
        if unsafe { libc::tcsetattr(fd, libc::TCSADRAIN, &raw) } != 0 {
            return Err(io::Error::last_os_error());
        }

        Ok(Some(RawMode {
            fd,
            original,
            restored: false,
        }))
    }

    /// Restore the saved settings now. Idempotent; `Drop` will not repeat it.
    pub fn restore(&mut self) {
        if self.restored {
            return;
        }
        self.restored = true;
        // Safe: writing back the struct we read at construction.
        unsafe {
            libc::tcsetattr(self.fd, libc::TCSADRAIN, &self.original);
        }
    }
}

impl Drop for RawMode {
    fn drop(&mut self) {
        self.restore();
    }
}

/// Parse a detach key written as `ctrl-]`, `^]`, or a bare character.
///
/// Returns the control byte the client watches for in the input stream.
pub fn parse_detach_key(spec: &str) -> Option<u8> {
    let spec = spec.trim();
    if spec.is_empty() {
        return None;
    }
    let rest = if let Some(r) = spec.strip_prefix("ctrl-") {
        r
    } else if let Some(r) = spec.strip_prefix("ctrl+") {
        r
    } else if let Some(r) = spec.strip_prefix('^') {
        r
    } else {
        // A bare single character is taken literally.
        let mut chars = spec.chars();
        let c = chars.next()?;
        if chars.next().is_some() || !c.is_ascii() {
            return None;
        }
        return Some(c as u8);
    };

    let mut chars = rest.chars();
    let c = chars.next()?;
    if chars.next().is_some() || !c.is_ascii() {
        return None;
    }
    // Ctrl masks off the top three bits: ctrl-a is 0x01, ctrl-] is 0x1d.
    let upper = c.to_ascii_uppercase() as u8;
    if !(0x3f..=0x5f).contains(&upper) {
        return None;
    }
    Some(upper & 0x1f)
}

/// The default detach key, `ctrl-]`.
///
/// The telnet escape: universally understood, and not bound by the TUI agents
/// this runtime launches. Overridable, because "not bound by anything" is never
/// quite true.
pub const DEFAULT_DETACH_KEY: &str = "ctrl-]";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_degenerate_size_falls_back() {
        assert_eq!(
            WinSize { cols: 0, rows: 24 }.or_fallback(),
            WinSize::FALLBACK
        );
        assert_eq!(
            WinSize { cols: 80, rows: 0 }.or_fallback(),
            WinSize::FALLBACK
        );
        assert_eq!(
            WinSize {
                cols: 100,
                rows: 30
            }
            .or_fallback(),
            WinSize {
                cols: 100,
                rows: 30
            }
        );
    }

    #[test]
    fn a_non_tty_reports_the_fallback_size() {
        // A pipe is never a terminal, so this exercises the ioctl failure path
        // without needing a tty in the test environment.
        let mut fds = [0i32; 2];
        assert_eq!(unsafe { libc::pipe(fds.as_mut_ptr()) }, 0);
        assert_eq!(window_size(fds[0]), WinSize::FALLBACK);
        assert!(!is_tty(fds[0]));
        unsafe {
            libc::close(fds[0]);
            libc::close(fds[1]);
        }
    }

    #[test]
    fn raw_mode_on_a_non_tty_is_a_no_op_not_an_error() {
        let mut fds = [0i32; 2];
        assert_eq!(unsafe { libc::pipe(fds.as_mut_ptr()) }, 0);
        let guard = RawMode::enter(fds[0]).expect("must not error on a pipe");
        assert!(guard.is_none());
        unsafe {
            libc::close(fds[0]);
            libc::close(fds[1]);
        }
    }

    #[test]
    fn detach_key_forms_all_parse_to_the_same_byte() {
        assert_eq!(parse_detach_key("ctrl-]"), Some(0x1d));
        assert_eq!(parse_detach_key("ctrl+]"), Some(0x1d));
        assert_eq!(parse_detach_key("^]"), Some(0x1d));
        assert_eq!(parse_detach_key(DEFAULT_DETACH_KEY), Some(0x1d));
    }

    #[test]
    fn detach_key_letters_map_to_their_control_codes() {
        assert_eq!(parse_detach_key("ctrl-a"), Some(0x01));
        assert_eq!(parse_detach_key("ctrl-A"), Some(0x01));
        assert_eq!(parse_detach_key("ctrl-d"), Some(0x04));
        assert_eq!(parse_detach_key("^_"), Some(0x1f));
    }

    #[test]
    fn a_bare_character_is_literal() {
        assert_eq!(parse_detach_key("q"), Some(b'q'));
    }

    #[test]
    fn nonsense_detach_keys_are_rejected() {
        assert_eq!(parse_detach_key(""), None);
        assert_eq!(parse_detach_key("   "), None);
        assert_eq!(parse_detach_key("ctrl-"), None);
        assert_eq!(parse_detach_key("ctrl-ab"), None);
        assert_eq!(parse_detach_key("ctrl-é"), None);
        assert_eq!(parse_detach_key("hello"), None);
        // Outside the control-code range.
        assert_eq!(parse_detach_key("ctrl-1"), None);
    }

    #[test]
    fn set_window_size_fails_on_a_non_tty_instead_of_pretending() {
        let mut fds = [0i32; 2];
        assert_eq!(unsafe { libc::pipe(fds.as_mut_ptr()) }, 0);
        assert!(set_window_size(fds[0], WinSize { cols: 80, rows: 24 }).is_err());
        unsafe {
            libc::close(fds[0]);
            libc::close(fds[1]);
        }
    }
}

/// Convenience for callers that hold a typed handle rather than a raw fd.
pub fn fd_of<T: AsRawFd>(t: &T) -> RawFd {
    t.as_raw_fd()
}
