//! The system prompt, and the project's own instructions inside it.
//!
//! A pure function of the working directory: it reads two files and formats a
//! string, and it reaches nothing the surfaces own. Split from `mod` on that
//! boundary rather than on width — the parent takes one name from here and
//! nothing here needs anything back.

/// The project's own instructions to whoever works on it, if it has any.
///
/// Three models were asked to add one line to this repository and all three
/// wrote the same wrong thing, because the fact they needed — where session
/// files live on disk — is written down here and was never shown to any of
/// them. The strongest of the three made no other mistake at all: its tool
/// calls were correct, its formatting was clean, and it still had to guess,
/// because guessing was the only thing available.
///
/// Capped, because this is prepended to every request in the session and a
/// repository that keeps a novel in `AGENTS.md` should pay for the part that
/// fits rather than for all of it.
const MAX_INSTRUCTIONS_BYTES: usize = 32 * 1024;

fn project_instructions(cwd: &std::path::Path) -> Option<String> {
    // `AGENTS.md` first: `CLAUDE.md` in this repository is one line importing
    // it, and a convention of pointing at the other is common enough that
    // reading both would usually mean reading the same file twice.
    let text = ["AGENTS.md", "CLAUDE.md"]
        .iter()
        .find_map(|name| std::fs::read_to_string(cwd.join(name)).ok())?;
    let text = text.trim();
    if text.is_empty() {
        return None;
    }
    let end = text
        .char_indices()
        .take_while(|(i, _)| *i < MAX_INSTRUCTIONS_BYTES)
        .last()
        .map(|(i, c)| i + c.len_utf8())
        .unwrap_or(0);
    Some(text[..end].to_owned())
}

pub(super) fn system_prompt(cwd: &std::path::Path) -> String {
    let instructions = match project_instructions(cwd) {
        Some(text) => format!(
            "\n\nThe project has left instructions for whoever works on it. They describe this \
             codebase specifically and outrank your general habits; where they say nothing, use \
             your judgement.\n\n<project-instructions>\n{text}\n</project-instructions>"
        ),
        None => String::new(),
    };
    format!(
        "You are axio, a coding agent running in a terminal.\n\
         Working directory: {}\n\
         Platform: {}\n\n\
         You have tools: read, write, edit, glob, grep and bash. Prefer the project's own \
         commands over reimplementing what they do.\n\n\
         Match the conventions of the code you are changing. When a project has a formatter, \
         a linter or a test command, run them on what you changed before you finish — an edit \
         that does not survive the project's own checks is not done, and finding that out is \
         a command you can run rather than something to leave for whoever reads it.\n\n\
         Keep responses focused, brief, and concise. Lead with the outcome, then the detail.\n\
         Deliver what was asked at the scope intended: make routine judgement calls yourself, \
         and check in only when different readings would lead to materially different work.\n\
         You are operating in a single turn; the user cannot answer questions mid-task.\n\n\
         Some actions require approval. If one is refused, that decision is final for this \
         run: do not retry it and do not invent an argument to bypass it. Never state that \
         work was done when the call that would have done it was refused or failed — say \
         plainly what you could not do and why.{}",
        cwd.display(),
        std::env::consts::OS,
        instructions,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_project_with_instructions_has_them_carried_into_the_prompt() {
        let dir = tempfile::tempdir().expect("a temp dir");
        std::fs::write(
            dir.path().join("AGENTS.md"),
            "# Notes\n\nSessions live in day directories.\n",
        )
        .expect("the instructions");

        let prompt = system_prompt(dir.path());
        assert!(
            prompt.contains("Sessions live in day directories."),
            "{prompt}"
        );
        assert!(prompt.contains("<project-instructions>"), "{prompt}");
    }

    #[test]
    fn a_project_without_them_says_nothing_about_them() {
        let dir = tempfile::tempdir().expect("a temp dir");
        let prompt = system_prompt(dir.path());
        assert!(!prompt.contains("project-instructions"), "{prompt}");
    }

    #[test]
    fn claude_md_is_read_when_there_is_no_agents_md() {
        let dir = tempfile::tempdir().expect("a temp dir");
        std::fs::write(dir.path().join("CLAUDE.md"), "read me").expect("the instructions");
        assert!(system_prompt(dir.path()).contains("read me"));
    }

    #[test]
    fn an_empty_file_is_not_instructions() {
        let dir = tempfile::tempdir().expect("a temp dir");
        std::fs::write(dir.path().join("AGENTS.md"), "   \n\n").expect("an empty file");
        assert!(!system_prompt(dir.path()).contains("project-instructions"));
    }

    #[test]
    fn a_very_long_file_is_capped_on_a_character_boundary() {
        // Multi-byte, so a naive byte slice would panic rather than truncate.
        let dir = tempfile::tempdir().expect("a temp dir");
        std::fs::write(dir.path().join("AGENTS.md"), "é".repeat(40 * 1024)).expect("a long file");
        let prompt = system_prompt(dir.path());
        assert!(prompt.contains("project-instructions"));
        assert!(prompt.len() < 40 * 1024 + 4096, "the cap did not apply");
    }
}
