//! Changing two keys in a configuration file without rewriting it.
//!
//! Serialising a `Config` back out would be a few lines and would delete every
//! comment in the file, along with the ordering, the blank lines and any
//! section axio does not use. A configuration is something a person wrote; a
//! program that edits one owes it the rest of what is in there.
//!
//! So this is a line edit. It touches the two keys it is asked to touch and
//! leaves every other byte exactly where it was.

/// Set `provider` and `name` under `[model]`, preserving everything else.
///
/// Creates the section when the file has none, and appends a key the section
/// is missing rather than only replacing what is already written.
pub fn set_model(text: &str, provider: &str, name: &str) -> String {
    let mut out: Vec<String> = Vec::new();
    let mut in_model = false;
    let mut wrote_provider = false;
    let mut wrote_name = false;
    let mut model_seen = false;
    // Where the `[model]` section ended, so a missing key is appended inside it
    // rather than after whatever section happens to follow.
    let mut model_ends_at: Option<usize> = None;

    for line in text.lines() {
        let trimmed = line.trim();

        if trimmed.starts_with('[') && trimmed.ends_with(']') {
            if in_model {
                model_ends_at = Some(out.len());
            }
            // Exactly `[model]`. A `[model.something]` is a different table,
            // and a `provider` key inside it is not this one.
            in_model = trimmed == "[model]";
            if in_model {
                model_seen = true;
            }
            out.push(line.to_owned());
            continue;
        }

        if in_model && let Some(key) = key_of(trimmed) {
            if key == "provider" {
                out.push(format!("provider = {}", quote(provider)));
                wrote_provider = true;
                continue;
            }
            if key == "name" {
                out.push(format!("name = {}", quote(name)));
                wrote_name = true;
                continue;
            }
        }

        out.push(line.to_owned());
    }

    if in_model {
        model_ends_at = Some(out.len());
    }

    let mut missing: Vec<String> = Vec::new();
    if !wrote_provider {
        missing.push(format!("provider = {}", quote(provider)));
    }
    if !wrote_name {
        missing.push(format!("name = {}", quote(name)));
    }

    if !missing.is_empty() {
        match (model_seen, model_ends_at) {
            // The section exists: put what is missing at the end of it, before
            // whatever comes next.
            (true, Some(at)) => {
                let at = trim_back(&out, at);
                for (i, line) in missing.into_iter().enumerate() {
                    out.insert(at + i, line);
                }
            }
            _ => {
                if !out.is_empty() {
                    out.push(String::new());
                }
                out.push("[model]".to_owned());
                out.extend(missing);
            }
        }
    }

    let mut joined = out.join("\n");
    // A file that ended with a newline keeps ending with one; `lines()` does
    // not report the last one and rewriting without it changes every file.
    if text.is_empty() || text.ends_with('\n') {
        joined.push('\n');
    }
    joined
}

/// The key a line assigns to, if it assigns to one.
///
/// Comments and blanks are not assignments, and a key inside a quoted string is
/// not a key — but a value containing `=` is common, so only the first `=`
/// counts and only when what precedes it is a bare word.
fn key_of(trimmed: &str) -> Option<&str> {
    if trimmed.is_empty() || trimmed.starts_with('#') {
        return None;
    }
    let (key, _) = trimmed.split_once('=')?;
    let key = key.trim();
    if key.is_empty()
        || !key
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
    {
        return None;
    }
    Some(key)
}

/// Back up over trailing blank lines and comments so an appended key lands
/// against the section's own content rather than after the blank line that
/// separates it from the next one — and above a comment that belongs to what
/// follows.
fn trim_back(out: &[String], mut at: usize) -> usize {
    while at > 0 {
        let previous = out[at - 1].trim();
        if previous.is_empty() || previous.starts_with('#') {
            at -= 1;
        } else {
            break;
        }
    }
    at
}

/// A TOML basic string. Model and provider names are ASCII in practice, but a
/// name with a quote in it must not produce a file that no longer parses.
fn quote(value: &str) -> String {
    let escaped = value
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n");
    format!("\"{escaped}\"")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn both_keys_are_replaced_in_place() {
        let before = "[model]\nprovider = \"ollama\"\nname = \"kimi\"\n";
        let after = set_model(before, "openai-codex", "gpt-5.6-sol");
        assert_eq!(
            after,
            "[model]\nprovider = \"openai-codex\"\nname = \"gpt-5.6-sol\"\n"
        );
    }

    /// The whole reason this is a line edit. A round trip through the config
    /// type would delete all of this.
    #[test]
    fn comments_and_unrelated_sections_survive() {
        let before = "\
# axio configuration.

[model]
# Which dialect to speak.
provider = \"ollama\"
name = \"kimi\"
max_tokens = 32000

[permissions]
# Subjects are canonical strings.
allow = [\"bash:cargo\"]
";
        let after = set_model(before, "openai-codex", "gpt-5.6-sol");
        assert!(after.contains("# axio configuration."), "{after}");
        assert!(after.contains("# Which dialect to speak."), "{after}");
        assert!(after.contains("max_tokens = 32000"), "{after}");
        assert!(
            after.contains("# Subjects are canonical strings."),
            "{after}"
        );
        assert!(after.contains("allow = [\"bash:cargo\"]"), "{after}");
        assert!(after.contains("provider = \"openai-codex\""), "{after}");
        assert!(!after.contains("\"ollama\""), "{after}");
    }

    /// The trap: another section with a key of the same name. A previous
    /// generation of this file had `provider = "openai"` under `[voice]`.
    #[test]
    fn a_key_of_the_same_name_in_another_section_is_left_alone() {
        let before = "\
[voice]
provider = \"openai\"
name = \"whisper-1\"

[model]
provider = \"ollama\"
name = \"kimi\"
";
        let after = set_model(before, "anthropic", "claude-opus-5");
        assert!(after.contains("[voice]\nprovider = \"openai\""), "{after}");
        assert!(after.contains("name = \"whisper-1\""), "{after}");
        assert!(
            after.contains("[model]\nprovider = \"anthropic\""),
            "{after}"
        );
    }

    /// `[model.reasoning]` is a different table, and a key in it is not this
    /// one.
    #[test]
    fn a_subtable_is_not_the_model_section() {
        let before = "[model]\nname = \"kimi\"\n\n[model.extra]\nprovider = \"leave-me\"\n";
        let after = set_model(before, "anthropic", "claude-opus-5");
        assert!(after.contains("provider = \"leave-me\""), "{after}");
        assert!(after.contains("name = \"claude-opus-5\""), "{after}");
        // The missing key goes into `[model]`, not into the subtable.
        let model_at = after.find("[model]").unwrap();
        let sub_at = after.find("[model.extra]").unwrap();
        let added_at = after.find("provider = \"anthropic\"").unwrap();
        assert!(added_at > model_at && added_at < sub_at, "{after}");
    }

    #[test]
    fn a_missing_key_is_added_to_the_section_that_exists() {
        let before = "[model]\nname = \"kimi\"\nmax_tokens = 100\n\n[budget]\nmax_steps = 5\n";
        let after = set_model(before, "ollama", "kimi");
        let added_at = after.find("provider = \"ollama\"").unwrap();
        assert!(added_at < after.find("[budget]").unwrap(), "{after}");
        assert!(after.contains("max_steps = 5"), "{after}");
    }

    #[test]
    fn a_file_without_the_section_gains_one() {
        let after = set_model("[budget]\nmax_steps = 5\n", "ollama", "kimi");
        assert!(after.contains("[budget]"), "{after}");
        assert!(after.contains("[model]"), "{after}");
        assert!(after.contains("provider = \"ollama\""), "{after}");
        assert!(after.contains("name = \"kimi\""), "{after}");
    }

    #[test]
    fn an_empty_file_becomes_a_valid_one() {
        let after = set_model("", "ollama", "kimi");
        assert_eq!(after, "[model]\nprovider = \"ollama\"\nname = \"kimi\"\n");
    }

    /// Rewriting a file that had no trailing newline must not add one, and one
    /// that had it must keep it: every line of the diff would otherwise be
    /// this function's fault.
    #[test]
    fn the_trailing_newline_is_whatever_it_was() {
        let with = set_model("[model]\nname = \"a\"\nprovider = \"b\"\n", "b", "a");
        assert!(with.ends_with('\n'));
        let without = set_model("[model]\nname = \"a\"\nprovider = \"b\"", "b", "a");
        assert!(!without.ends_with('\n'));
    }

    /// A name that would break the file must not be written raw.
    #[test]
    fn a_value_needing_escapes_still_parses() {
        let after = set_model("", "ollama", "we\"ird\\name");
        assert!(after.contains(r#"name = "we\"ird\\name""#), "{after}");
        let parsed: toml::Value = toml::from_str(&after).expect("it must still parse");
        assert_eq!(parsed["model"]["name"].as_str(), Some("we\"ird\\name"));
    }

    /// Whatever it wrote has to load back as what was asked for, not merely
    /// look right.
    #[test]
    fn the_result_parses_to_the_values_that_were_set() {
        let before = "# a comment\n[model]\nprovider = \"ollama\"\nname = \"kimi\"\n";
        let after = set_model(before, "openai-codex", "gpt-5.6-sol");
        let parsed: toml::Value = toml::from_str(&after).expect("valid toml");
        assert_eq!(parsed["model"]["provider"].as_str(), Some("openai-codex"));
        assert_eq!(parsed["model"]["name"].as_str(), Some("gpt-5.6-sol"));
    }
}
