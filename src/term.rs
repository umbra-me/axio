//! Terminal input without a terminal library: `stty` switches modes, a thread
//! reads raw bytes, and `parse` turns them into keys.

use std::collections::VecDeque;
use std::io::{Read, Write};
use std::process::{Command, Stdio};
use std::sync::OnceLock;
use std::sync::mpsc::{Receiver, channel};
use std::time::Duration;

/// How long a lone Escape waits for the rest of an escape sequence.
const ESC_WAIT: Duration = Duration::from_millis(30);

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Key {
    Char(char),
    Enter,
    AltEnter,
    Backspace,
    AltBackspace,
    Delete,
    Left,
    Right,
    WordLeft,
    WordRight,
    Up,
    Down,
    Home,
    End,
    Esc,
    Paste(String),
    Ctrl(char),
    /// A sequence axio has no use for, such as an F-key.
    Other,
}

/// Reads one key, pulling bytes from `next` (which waits up to the given time
/// and returns `None` when nothing arrived).
pub fn parse(next: &mut dyn FnMut(Option<Duration>) -> Option<u8>) -> Option<Key> {
    let b = next(None)?;
    Some(match b {
        0x1b => escape(next),
        b'\r' | b'\n' => Key::Enter,
        0x7f | 0x08 => Key::Backspace,
        0x02 => Key::Left,
        0x06 => Key::Right,
        0x10 => Key::Up,
        0x0e => Key::Down,
        0x01 => Key::Home,
        0x05 => Key::End,
        0x00..=0x1f => Key::Ctrl((b + b'a' - 1) as char),
        _ => Key::Char(utf8(b, next)),
    })
}

fn utf8(first: u8, next: &mut dyn FnMut(Option<Duration>) -> Option<u8>) -> char {
    let extra = match first {
        0xc0..=0xdf => 1,
        0xe0..=0xef => 2,
        0xf0..=0xf7 => 3,
        _ => 0,
    };
    let mut bytes = vec![first];
    for _ in 0..extra {
        match next(Some(ESC_WAIT)) {
            Some(b) => bytes.push(b),
            None => break,
        }
    }
    std::str::from_utf8(&bytes)
        .ok()
        .and_then(|s| s.chars().next())
        .unwrap_or('\u{fffd}')
}

fn escape(next: &mut dyn FnMut(Option<Duration>) -> Option<u8>) -> Key {
    let Some(b) = next(Some(ESC_WAIT)) else {
        return Key::Esc;
    };
    match b {
        b'[' => csi(next),
        b'O' => match next(Some(ESC_WAIT)) {
            Some(b'A') => Key::Up,
            Some(b'B') => Key::Down,
            Some(b'C') => Key::Right,
            Some(b'D') => Key::Left,
            Some(b'H') => Key::Home,
            Some(b'F') => Key::End,
            _ => Key::Other,
        },
        b'\r' | b'\n' => Key::AltEnter,
        b'b' => Key::WordLeft,
        b'f' => Key::WordRight,
        0x7f => Key::AltBackspace,
        // A second Esc is still Esc; anything else (Alt+key) is not.
        0x1b => Key::Esc,
        _ => Key::Other,
    }
}

fn csi(next: &mut dyn FnMut(Option<Duration>) -> Option<u8>) -> Key {
    let mut params = String::new();
    let fin = loop {
        match next(Some(ESC_WAIT)) {
            Some(b @ 0x40..=0x7e) => break b,
            Some(b) => params.push(b as char),
            None => return Key::Other,
        }
    };
    match (params.as_str(), fin) {
        (_, b'A') => Key::Up,
        (_, b'B') => Key::Down,
        ("1;3" | "1;5", b'C') => Key::WordRight,
        ("1;3" | "1;5", b'D') => Key::WordLeft,
        (_, b'C') => Key::Right,
        (_, b'D') => Key::Left,
        (_, b'H') | ("1" | "7", b'~') => Key::Home,
        (_, b'F') | ("4" | "8", b'~') => Key::End,
        ("3", b'~') => Key::Delete,
        ("200", b'~') => paste(next),
        // F-keys, Shift-Tab, focus events and the like: never a stop.
        _ => Key::Other,
    }
}

/// Bracketed paste: everything up to ESC [ 2 0 1 ~ is literal text.
fn paste(next: &mut dyn FnMut(Option<Duration>) -> Option<u8>) -> Key {
    const END: &[u8] = b"\x1b[201~";
    let mut bytes = Vec::new();
    while let Some(b) = next(Some(Duration::from_millis(500))) {
        bytes.push(b);
        if bytes.ends_with(END) {
            bytes.truncate(bytes.len() - END.len());
            break;
        }
    }
    Key::Paste(
        String::from_utf8_lossy(&bytes)
            .replace("\r\n", "\n")
            .replace('\r', "\n"),
    )
}

/// Keys from the terminal, read by a background thread so a running turn can
/// still notice Escape and Ctrl-C.
pub struct Keys {
    rx: Receiver<u8>,
    pending: VecDeque<u8>,
    /// Keys typed during a turn, replayed into the next prompt.
    typed_ahead: VecDeque<Key>,
    /// Lines sent with Enter during a turn, for the model mid-turn.
    steered: Vec<String>,
}

impl Keys {
    pub fn spawn() -> Keys {
        let (tx, rx) = channel();
        std::thread::spawn(move || {
            let mut stdin = std::io::stdin();
            let mut buf = [0u8; 256];
            while let Ok(n) = stdin.read(&mut buf) {
                if n == 0 || buf[..n].iter().any(|b| tx.send(*b).is_err()) {
                    break;
                }
            }
        });
        Keys {
            rx,
            pending: VecDeque::new(),
            typed_ahead: VecDeque::new(),
            steered: Vec::new(),
        }
    }

    fn byte(&mut self, wait: Option<Duration>) -> Option<u8> {
        if let Some(b) = self.pending.pop_front() {
            return Some(b);
        }
        match wait {
            None => self.rx.recv().ok(),
            Some(d) => self.rx.recv_timeout(d).ok(),
        }
    }

    /// Blocks for the next key, typed-ahead keys first; `None` when stdin closed.
    pub fn next(&mut self) -> Option<Key> {
        if let Some(k) = self.typed_ahead.pop_front() {
            return Some(k);
        }
        parse(&mut |w| self.byte(w))
    }

    /// During a turn: whether Esc or Ctrl-C was pressed. Other keys are kept
    /// for the next prompt.
    pub fn stop_requested(&mut self) -> bool {
        loop {
            if self.pending.is_empty() {
                match self.rx.try_recv() {
                    Ok(b) => self.pending.push_back(b),
                    Err(_) => return false,
                }
            }
            match parse(&mut |w| self.byte(w)) {
                Some(Key::Esc | Key::Ctrl('c')) => {
                    self.typed_ahead.clear();
                    return true;
                }
                Some(Key::Enter) => {
                    let line = typed_text(self.typed_ahead.drain(..));
                    if !line.trim().is_empty() {
                        self.steered.push(line.trim().to_string());
                    }
                }
                Some(k) => self.typed_ahead.push_back(k),
                None => return false,
            }
        }
    }
}

impl Keys {
    /// What the user has typed so far during this turn.
    pub fn typing(&self) -> String {
        typed_text(self.typed_ahead.iter().cloned())
    }

    /// Lines the user sent with Enter while a turn ran.
    pub fn take_steered(&mut self) -> Vec<String> {
        std::mem::take(&mut self.steered)
    }
}

/// The text a run of keys types, applying backspaces.
fn typed_text(keys: impl Iterator<Item = Key>) -> String {
    let mut s = String::new();
    for k in keys {
        match k {
            Key::Char(c) => s.push(c),
            Key::Paste(p) => s.push_str(&p),
            Key::AltEnter => s.push('\n'),
            Key::Backspace => {
                s.pop();
            }
            _ => {}
        }
    }
    s
}

static SAVED: OnceLock<String> = OnceLock::new();

fn stty(args: &[&str]) -> Option<String> {
    let out = Command::new("stty")
        .args(args)
        .stdin(Stdio::inherit())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// Character-at-a-time input with signals off, so Ctrl-C reaches the editor
/// as a key. Output processing stays on, so `\n` still returns the carriage.
pub struct RawMode;

impl RawMode {
    pub fn enter() -> Option<RawMode> {
        let saved = stty(&["-g"])?;
        stty(&[
            "-icanon", "-echo", "-isig", "-ixon", "-iexten", "min", "1", "time", "0",
        ])?;
        let _ = SAVED.set(saved);
        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            restore();
            previous(info);
        }));
        print!("\x1b[?2004h");
        let _ = std::io::stdout().flush();
        Some(RawMode)
    }
}

fn restore() {
    print!("\x1b[?2004l");
    let _ = std::io::stdout().flush();
    if let Some(saved) = SAVED.get() {
        let _ = stty(&[saved]);
    }
}

impl Drop for RawMode {
    fn drop(&mut self) {
        restore();
    }
}

/// Columns a character takes in a terminal: 2 for wide East Asian
/// characters and most emoji, 0 for combining marks, else 1.
pub fn char_width(c: char) -> usize {
    match c as u32 {
        0x0300..=0x036F | 0x200B..=0x200F | 0xFE00..=0xFE0F => 0,
        0x1100..=0x115F
        | 0x2E80..=0x303E
        | 0x3041..=0x33FF
        | 0x3400..=0x4DBF
        | 0x4E00..=0x9FFF
        | 0xA000..=0xA4CF
        | 0xAC00..=0xD7A3
        | 0xF900..=0xFAFF
        | 0xFE30..=0xFE4F
        | 0xFF00..=0xFF60
        | 0xFFE0..=0xFFE6
        | 0x1F300..=0x1F64F
        | 0x1F680..=0x1F6FF
        | 0x1F900..=0x1F9FF
        | 0x20000..=0x3FFFD => 2,
        _ => 1,
    }
}

/// Columns `s` takes in a terminal.
pub fn text_width(s: &str) -> usize {
    s.chars().map(char_width).sum()
}

pub fn width() -> usize {
    ioctl_width()
        .or_else(|| stty(&["size"]).and_then(|s| s.split_whitespace().nth(1)?.parse().ok()))
        .filter(|w: &usize| *w > 0)
        .or_else(|| std::env::var("COLUMNS").ok()?.parse().ok())
        .unwrap_or(80)
}

/// The terminal's columns straight from the kernel: cheap enough to ask on
/// every spinner frame, so a resized window is noticed at once.
#[cfg(any(target_os = "macos", target_os = "linux"))]
fn ioctl_width() -> Option<usize> {
    #[repr(C)]
    #[derive(Default)]
    struct WinSize {
        rows: u16,
        cols: u16,
        x_pixels: u16,
        y_pixels: u16,
    }
    unsafe extern "C" {
        fn ioctl(fd: i32, request: std::ffi::c_ulong, ...) -> i32;
    }
    #[cfg(target_os = "macos")]
    const TIOCGWINSZ: std::ffi::c_ulong = 0x4008_7468;
    #[cfg(target_os = "linux")]
    const TIOCGWINSZ: std::ffi::c_ulong = 0x5413;
    for fd in [2, 1, 0] {
        let mut size = WinSize::default();
        // SAFETY: TIOCGWINSZ only writes one `struct winsize`, which `size` is.
        if unsafe { ioctl(fd, TIOCGWINSZ, &mut size) } == 0 && size.cols > 0 {
            return Some(size.cols.into());
        }
    }
    None
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn ioctl_width() -> Option<usize> {
    None
}

/// Whether stdin has input (or its end) within `wait`. A pipe nobody writes
/// to, such as an open socket from a task runner, would otherwise block a
/// read forever.
#[cfg(not(unix))]
pub fn input_within(_wait: std::time::Duration) -> bool {
    true
}

/// Whether stdin has input (or its end) within `wait`. A pipe nobody writes
/// to, such as an open socket from a task runner, would otherwise block a
/// read forever.
#[cfg(unix)]
pub fn input_within(wait: std::time::Duration) -> bool {
    #[repr(C)]
    struct PollFd {
        fd: i32,
        events: i16,
        revents: i16,
    }
    #[cfg(target_os = "linux")]
    type Nfds = std::ffi::c_ulong;
    #[cfg(not(target_os = "linux"))]
    type Nfds = std::ffi::c_uint;
    unsafe extern "C" {
        fn poll(fds: *mut PollFd, nfds: Nfds, timeout: i32) -> i32;
    }
    const POLLIN: i16 = 1;
    let mut fd = PollFd {
        fd: 0,
        events: POLLIN,
        revents: 0,
    };
    let ms = i32::try_from(wait.as_millis()).unwrap_or(i32::MAX);
    // SAFETY: one valid pollfd for the length of the call.
    unsafe { poll(&mut fd, 1, ms) > 0 }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn keys(input: &[u8]) -> Vec<Key> {
        let mut bytes: VecDeque<u8> = input.iter().copied().collect();
        let mut out = Vec::new();
        while let Some(k) = parse(&mut |_| bytes.pop_front()) {
            out.push(k);
        }
        out
    }

    #[test]
    fn parses_text_controls_and_sequences() {
        assert_eq!(
            keys("hé\r\x7f\x03\x1b[A\x1b[D\x1bOC\x1b[3~\x1b\r\x1b[1;5D".as_bytes()),
            [
                Key::Char('h'),
                Key::Char('é'),
                Key::Enter,
                Key::Backspace,
                Key::Ctrl('c'),
                Key::Up,
                Key::Left,
                Key::Right,
                Key::Delete,
                Key::AltEnter,
                Key::WordLeft,
            ]
        );
    }

    #[test]
    fn lone_escape_is_a_key() {
        assert_eq!(keys(b"\x1b"), [Key::Esc]);
        assert_eq!(
            keys(b"\x1b[Z\x1b[15~\x1bd\x1b[I\x1bOP"),
            [Key::Other, Key::Other, Key::Other, Key::Other, Key::Other],
            "Shift-Tab, F5, Alt-d, focus-in and F1 are not Esc"
        );
    }

    #[test]
    fn bracketed_paste_keeps_newlines_literal() {
        assert_eq!(
            keys(b"\x1b[200~a\r\nb\x1b[201~x"),
            [Key::Paste("a\nb".into()), Key::Char('x')]
        );
    }
}
