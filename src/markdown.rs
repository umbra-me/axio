//! Renders the model's markdown for the terminal as it streams, word by
//! word: headings, emphasis, inline code, fenced code blocks, bullets,
//! quotes, rules and links.

const BOLD: &str = "\x1b[1m";
const NO_BOLD: &str = "\x1b[22m";
const ITALIC: &str = "\x1b[3m";
const NO_ITALIC: &str = "\x1b[23m";
const CODE: &str = "\x1b[36m";
const NO_COLOR: &str = "\x1b[39m";
const DIM: &str = "\x1b[2m";

#[derive(Default)]
pub struct Markdown {
    /// Text of the current line not yet printed.
    pending: String,
    /// Set once the current line's prefix is printed: what closes it.
    open: Option<&'static str>,
    in_code: bool,
    /// Without colour: whether the last text printed left a line open.
    mid_line: bool,
    pub color: bool,
}

impl Markdown {
    pub fn new(color: bool) -> Markdown {
        Markdown {
            color,
            ..Markdown::default()
        }
    }

    /// Adds streamed text and returns what can be printed now: complete
    /// lines, plus whole words of the current line once its kind is known.
    pub fn push(&mut self, text: &str) -> String {
        if !self.color {
            if !text.is_empty() {
                self.mid_line = !text.ends_with('\n');
            }
            return text.to_string();
        }
        self.pending.push_str(text);
        let mut out = String::new();
        while let Some(i) = self.pending.find('\n') {
            let line: String = self.pending.drain(..=i).collect();
            out.push_str(&self.finish(line.trim_end_matches('\n')));
        }
        out.push_str(&self.partial());
        out
    }

    /// Renders whatever is left, at the end of a reply.
    pub fn flush(&mut self) -> String {
        if !self.color {
            return if std::mem::take(&mut self.mid_line) {
                "\n".into()
            } else {
                String::new()
            };
        }
        if self.pending.is_empty() && self.open.is_none() {
            return String::new();
        }
        let rest = std::mem::take(&mut self.pending);
        self.finish(&rest)
    }

    /// Forgets code-block state between replies.
    pub fn reset(&mut self) {
        self.pending.clear();
        self.open = None;
        self.in_code = false;
        self.mid_line = false;
    }

    fn finish(&mut self, rest: &str) -> String {
        let mut out = match self.open.take() {
            Some(close) if self.in_code => format!("{rest}{close}"),
            Some(close) => format!("{}{close}", inline_in(rest, base(close))),
            None => self.line(rest),
        };
        out.push('\n');
        out
    }

    /// Starts the current line once its kind is certain, then prints the
    /// words whose inline spans are closed.
    fn partial(&mut self) -> String {
        let mut out = String::new();
        if self.open.is_none() {
            let trimmed = self.pending.trim_start();
            let first_word_done = trimmed.contains(char::is_whitespace);
            if self.in_code {
                if trimmed.is_empty() || "```".starts_with(trimmed) || trimmed.starts_with("```") {
                    return out;
                }
                out = format!("{DIM}│{NO_BOLD} {CODE}");
                self.open = Some(NO_COLOR);
            } else {
                if !first_word_done || trimmed.starts_with(['`', '|']) {
                    return out;
                }
                let indent = self.pending.len() - trimmed.len();
                let (prefix, skip, close) = block(trimmed);
                out = format!("{}{prefix}", &self.pending[..indent]);
                self.pending.drain(..indent + skip);
                self.open = Some(close);
            }
        }
        if self.in_code {
            out.push_str(&std::mem::take(&mut self.pending));
            return out;
        }
        let safe = self
            .pending
            .char_indices()
            .rev()
            .filter(|(_, c)| c.is_whitespace())
            .map(|(i, c)| i + c.len_utf8())
            .find(|&i| balanced(&self.pending[..i]));
        if let Some(end) = safe {
            let done: String = self.pending.drain(..end).collect();
            out.push_str(&inline_in(&done, base(self.open.unwrap_or(""))));
        }
        out
    }

    fn line(&mut self, line: &str) -> String {
        let trimmed = line.trim_start();
        if trimmed.starts_with("```") {
            self.in_code = !self.in_code;
            let lang = trimmed.trim_start_matches('`').trim();
            return match (self.in_code, lang.is_empty()) {
                (true, false) => format!("{DIM}  {lang}{NO_BOLD}"),
                _ => String::new(),
            };
        }
        if self.in_code {
            return format!("{DIM}│{NO_BOLD} {CODE}{line}{NO_COLOR}");
        }
        let indent = &line[..line.len() - trimmed.len()];
        if matches!(trimmed, "---" | "***" | "___") {
            return format!("{DIM}{}{NO_BOLD}", "─".repeat(40));
        }
        if trimmed.starts_with('|') && trimmed.chars().all(|c| "|-: ".contains(c)) {
            return format!("{DIM}{line}{NO_BOLD}");
        }
        let (prefix, skip, close) = block(trimmed);
        format!(
            "{indent}{prefix}{}{close}",
            inline_in(&trimmed[skip..], base(close))
        )
    }
}

/// The prefix a line starts with, how many bytes of markup it replaces and
/// what ends it.
fn block(trimmed: &str) -> (String, usize, &'static str) {
    if let Some(rest) = heading(trimmed) {
        return (BOLD.into(), trimmed.len() - rest.len(), NO_BOLD);
    }
    if ["- ", "* ", "+ "].iter().any(|b| trimmed.starts_with(b)) {
        return ("• ".into(), 2, "");
    }
    if trimmed.starts_with("> ") || trimmed == ">" {
        return (
            format!("{DIM}│{NO_BOLD} {ITALIC}"),
            trimmed.len().min(2),
            NO_ITALIC,
        );
    }
    (String::new(), 0, "")
}

/// The style a line's prefix opened, to restore after an inner span ends.
fn base(close: &str) -> &'static str {
    match close {
        NO_BOLD => BOLD,
        NO_ITALIC => ITALIC,
        _ => "",
    }
}

/// Whether no inline span is left open at the end of `s`, so it can render
/// alone. Mirrors `inline`: a `*` only opens emphasis before a non-space
/// and not straight after a letter or digit, and parentheses matter only
/// in a link's URL, so prose such as "1) read" or "4* overall" streams.
fn balanced(s: &str) -> bool {
    let c: Vec<char> = s.chars().collect();
    let (mut code, mut bold, mut italic, mut link, mut url) = (false, false, false, false, false);
    let mut i = 0;
    while i < c.len() {
        let prev = i.checked_sub(1).map(|j| c[j]);
        let next = |k: usize| c.get(i + k).copied();
        match c[i] {
            '`' => code = !code,
            _ if code => {}
            '*' if next(1) == Some('*') => {
                if bold {
                    bold = false;
                } else if next(2).is_some_and(|n| !n.is_whitespace()) {
                    bold = true;
                }
                i += 2;
                continue;
            }
            '*' => {
                if italic && prev.is_some_and(|p| !p.is_whitespace()) {
                    italic = false;
                } else if next(1).is_some_and(|n| !n.is_whitespace())
                    && !prev.is_some_and(char::is_alphanumeric)
                {
                    italic = true;
                }
            }
            '[' if !url => link = true,
            ']' if link => {
                link = false;
                if next(1) == Some('(') {
                    url = true;
                    i += 2;
                    continue;
                }
            }
            ')' if url => url = false,
            _ => {}
        }
        i += 1;
    }
    !(code || bold || italic || link || url)
}

fn heading(line: &str) -> Option<&str> {
    let hashes = line.chars().take_while(|c| *c == '#').count();
    (1..=6)
        .contains(&hashes)
        .then(|| line[hashes..].strip_prefix(' '))
        .flatten()
}

/// `**bold**`, `*italic*`, `` `code` `` and `[text](url)` in one line.
#[cfg(test)]
fn inline(s: &str) -> String {
    inline_in(s, "")
}

/// `inline` inside text already styled with `base`, such as a bold
/// heading: closing an inner span turns `base` back on.
fn inline_in(s: &str, base: &str) -> String {
    let chars: Vec<char> = s.chars().collect();
    let mut out = String::new();
    let mut i = 0;
    let find = |from: usize, pat: &[char]| -> Option<usize> {
        (from..chars.len().saturating_sub(pat.len() - 1)).find(|&j| chars[j..j + pat.len()] == *pat)
    };
    while i < chars.len() {
        let c = chars[i];
        if c == '`'
            && let Some(end) = find(i + 1, &['`'])
        {
            let code: String = chars[i + 1..end].iter().collect();
            out.push_str(&format!("{CODE}{code}{NO_COLOR}"));
            i = end + 1;
            continue;
        }
        if c == '*'
            && chars.get(i + 1) == Some(&'*')
            && let Some(end) = find(i + 2, &['*', '*'])
            && end > i + 2
        {
            let inner: String = chars[i + 2..end].iter().collect();
            out.push_str(&format!(
                "{BOLD}{}{NO_BOLD}{base}",
                inline_in(&inner, &format!("{BOLD}{base}"))
            ));
            i = end + 2;
            continue;
        }
        if c == '*'
            && chars
                .get(i + 1)
                .is_some_and(|n| !n.is_whitespace() && *n != '*')
            && let Some(end) = find(i + 1, &['*'])
            && !chars[end - 1].is_whitespace()
        {
            let inner: String = chars[i + 1..end].iter().collect();
            out.push_str(&format!("{ITALIC}{inner}{NO_ITALIC}{base}"));
            i = end + 1;
            continue;
        }
        if c == '['
            && let Some(close) = find(i + 1, &[']', '('])
            && let Some(end) = find(close + 2, &[')'])
        {
            let text: String = chars[i + 1..close].iter().collect();
            let url: String = chars[close + 2..end].iter().collect();
            if url == text {
                out.push_str(&format!("{CODE}{url}{NO_COLOR}"));
            } else {
                out.push_str(&format!(
                    "{}{DIM} ({url}){NO_BOLD}{base}",
                    inline_in(&text, base)
                ));
            }
            i = end + 1;
            continue;
        }
        out.push(c);
        i += 1;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn streams_words_once_their_spans_close() {
        let mut md = Markdown::new(true);
        assert_eq!(
            md.push("## Plan\n- read the **co"),
            "\x1b[1mPlan\x1b[22m\n• read the "
        );
        assert_eq!(md.push("de** now"), "\x1b[1mcode\x1b[22m ");
        assert_eq!(md.flush(), "now\n");
        assert_eq!(md.flush(), "");
    }

    #[test]
    fn code_blocks_hide_fences_and_skip_inline_formatting() {
        let mut md = Markdown::new(true);
        let out = md.push("```rust\nlet x = **y**;\n```\nafter `z`\n");
        assert_eq!(
            out,
            "\x1b[2m  rust\x1b[22m\n\x1b[2m│\x1b[22m \x1b[36mlet x = **y**;\x1b[39m\n\nafter \x1b[36mz\x1b[39m\n"
        );
    }

    #[test]
    fn inline_forms() {
        assert_eq!(inline("a *b* c"), "a \x1b[3mb\x1b[23m c");
        assert_eq!(
            inline("2 * 3 * 4"),
            "2 * 3 * 4",
            "spaced asterisks are not emphasis"
        );
        assert_eq!(
            inline("see [docs](https://x.dev)"),
            "see docs\x1b[2m (https://x.dev)\x1b[22m"
        );
        assert_eq!(inline("snake_case_name"), "snake_case_name");
        assert_eq!(inline("unclosed `tick"), "unclosed `tick");
    }

    #[test]
    fn inner_spans_keep_the_lines_style() {
        let mut md = Markdown::new(true);
        assert_eq!(
            md.push("## See [docs](u) and **key** now\n"),
            format!(
                "{BOLD}See docs{DIM} (u){NO_BOLD}{BOLD} and {BOLD}key{NO_BOLD}{BOLD} now{NO_BOLD}\n"
            )
        );
    }

    #[test]
    fn prose_with_stray_marks_keeps_streaming() {
        let mut md = Markdown::new(true);
        assert_eq!(md.push("Steps: 1) read, "), "Steps: 1) read, ");
        assert_eq!(md.push("rated 4* overall "), "rated 4* overall ");
        assert_eq!(md.push("see [the docs"), "see ");
        assert_eq!(
            md.push("](https://x) ok "),
            "the docs\x1b[2m (https://x)\x1b[22m ok "
        );
    }

    #[test]
    fn holds_bold_until_it_closes() {
        let mut md = Markdown::new(true);
        assert_eq!(md.push("The two are **191 "), "The two are ");
        assert_eq!(md.push("and 353**: done"), "\x1b[1m191 and 353\x1b[22m: ");
    }

    #[test]
    fn waits_until_a_line_kind_is_known() {
        let mut md = Markdown::new(true);
        assert_eq!(md.push("-"), "");
        assert_eq!(md.push("--"), "");
        assert_eq!(md.push("\n"), format!("{DIM}{}{NO_BOLD}\n", "─".repeat(40)));
        assert_eq!(
            md.push("```sh\nls -"),
            format!("{DIM}  sh{NO_BOLD}\n{DIM}│{NO_BOLD} {CODE}ls -")
        );
        assert_eq!(md.push("la\n``"), format!("la{NO_COLOR}\n"));
        assert_eq!(md.push("`\ndone"), "\n");
        assert_eq!(md.flush(), "done\n");
    }

    #[test]
    fn plain_without_color() {
        let mut md = Markdown::new(false);
        assert_eq!(md.push("# Title\n```\nco"), "# Title\n```\nco");
        assert_eq!(md.flush(), "\n");
        assert_eq!(md.flush(), "");
    }
}
