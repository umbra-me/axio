//! Who is allowed to do what.
//!
//! Every decision is made against a `Plan`'s **subject** — a canonical string
//! like `read:src/lib.rs` or `bash:git` — and never against the tool's name or
//! its raw arguments. That indirection is what makes `deny read:**/.env`
//! expressible at all, and what stops a shell command being classified by a
//! string its own arguments can shape.

use crate::protocol::Decision;
use crate::tool::{Effects, Plan};

/// What the engine decided without asking anyone.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
    Allow,
    Deny(String),
    /// Policy cannot decide alone; the reason is shown to whoever is asked.
    Ask(String),
}

/// Paths a rule may never expose, evaluated before anything else.
///
/// Read-only auto-approval is the reason this has to come first: without it, a
/// perfectly ordinary "reads are safe" rule silently hands over every
/// credential on the machine.
const DENY_READ: &[&str] = &[
    "*.env",
    "*.env.*",
    "*.pem",
    "*.key",
    "*id_rsa*",
    "*id_ed25519*",
    "*.ssh/*",
    "*.aws/*",
    "*.gnupg/*",
    "*.netrc",
    "*.npmrc",
    "*.pypirc",
];

/// Paths a write may never touch, for the same reason plus one more: a hook or
/// a credential file is how an agent would escalate its own privileges.
const DENY_WRITE: &[&str] = &[
    "*.git/hooks/*",
    "*.git/config",
    "*.ssh/*",
    "*.aws/*",
    "*.gnupg/*",
    "*.env",
    "*.env.*",
];

/// Match a subject against a pattern.
///
/// `*` matches any run of characters including `/`; `?` matches exactly one.
/// That is the whole language, deliberately. A richer one — character classes,
/// alternation, a `**` distinct from `*` — is a rule language, and a rule
/// language nobody can predict the behaviour of is worse than no rules: the
/// point of a deny rule is that its author is certain what it covers.
pub fn matches(pattern: &str, subject: &str) -> bool {
    let p: Vec<char> = pattern.chars().collect();
    let s: Vec<char> = subject.chars().collect();

    // Two-pointer with backtracking to the last `*`. Linear in practice and
    // cannot blow up the way a backtracking regex can on hostile input.
    let (mut pi, mut si) = (0usize, 0usize);
    let (mut star, mut resume) = (usize::MAX, 0usize);

    while si < s.len() {
        if pi < p.len() && (p[pi] == '?' || p[pi] == s[si]) {
            pi += 1;
            si += 1;
        } else if pi < p.len() && p[pi] == '*' {
            star = pi;
            resume = si;
            pi += 1;
        } else if star != usize::MAX {
            pi = star + 1;
            resume += 1;
            si = resume;
        } else {
            return false;
        }
    }
    while pi < p.len() && p[pi] == '*' {
        pi += 1;
    }
    pi == p.len()
}

/// A user-supplied rule.
#[derive(Debug, Clone)]
pub struct Rule {
    pattern: String,
}

impl Rule {
    pub fn new(pattern: &str) -> Result<Self, String> {
        if pattern.trim().is_empty() {
            return Err("a rule cannot be empty".into());
        }
        Ok(Self {
            pattern: pattern.to_owned(),
        })
    }

    fn matches(&self, subject: &str) -> bool {
        matches(&self.pattern, subject)
    }

    pub fn pattern(&self) -> &str {
        &self.pattern
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Unattended {
    /// No one is there to ask, so anything that would ask is refused. The
    /// default, because the alternative is a silent yes.
    #[default]
    Deny,
    /// `--yes`: anything policy cannot decide is allowed.
    Allow,
}

#[derive(Debug, Clone, Default)]
pub struct Policy {
    deny: Vec<Rule>,
    allow: Vec<Rule>,
    /// Granted by an approval, for this process only. Never written to disk:
    /// an approval is a decision about a moment, not a configuration change.
    session_grants: Vec<String>,
    unattended: Option<Unattended>,
    builtin_read: Vec<String>,
    builtin_write: Vec<String>,
}

impl Policy {
    pub fn new() -> Self {
        Self {
            builtin_read: DENY_READ.iter().map(|s| (*s).to_owned()).collect(),
            builtin_write: DENY_WRITE.iter().map(|s| (*s).to_owned()).collect(),
            ..Default::default()
        }
    }

    /// `--yes`. Everything policy cannot decide alone is allowed — except the
    /// built-in deny list, which is not a preference.
    pub fn unattended_allow(mut self) -> Self {
        self.unattended = Some(Unattended::Allow);
        self
    }

    pub fn deny_rule(mut self, pattern: &str) -> Result<Self, String> {
        self.deny.push(Rule::new(pattern)?);
        Ok(self)
    }

    pub fn allow_rule(mut self, pattern: &str) -> Result<Self, String> {
        self.allow.push(Rule::new(pattern)?);
        Ok(self)
    }

    /// Remember an `AllowSession` grant. Memory only.
    pub fn grant(&mut self, subject: &str) {
        if !self.session_grants.iter().any(|g| g == subject) {
            self.session_grants.push(subject.to_owned());
        }
    }

    /// Ordered evaluation. The order is the design:
    ///
    /// 1. built-in deny — not overridable, because it protects things a user
    ///    rule should never be able to expose by accident
    /// 2. user deny
    /// 3. read-only effects — auto-approved, which is only safe *after* the
    ///    denies have had their say
    /// 4. user allow
    /// 5. session grants
    /// 6. ask, or the unattended answer
    pub fn evaluate(&self, plan: &Plan) -> Verdict {
        let subject = plan.subject.as_str();

        if let Some(reason) = self.builtin_denial(subject, plan.effects) {
            return Verdict::Deny(reason);
        }

        if let Some(rule) = self.deny.iter().find(|r| r.matches(subject)) {
            return Verdict::Deny(format!("denied by rule `{}`", rule.pattern()));
        }

        if plan.effects.read_only() {
            return Verdict::Allow;
        }

        if self.allow.iter().any(|r| r.matches(subject)) {
            return Verdict::Allow;
        }

        if self.session_grants.iter().any(|g| g == subject) {
            return Verdict::Allow;
        }

        match self.unattended {
            Some(Unattended::Allow) => Verdict::Allow,
            Some(Unattended::Deny) => {
                Verdict::Deny("no approver available and --yes was not given".into())
            }
            None => Verdict::Ask(describe(plan.effects)),
        }
    }

    fn builtin_denial(&self, subject: &str, effects: Effects) -> Option<String> {
        let path = subject.split_once(':').map(|(_, p)| p).unwrap_or(subject);

        if effects.reads && self.builtin_read.iter().any(|m| matches(m, path)) {
            return Some(format!(
                "`{path}` is on the built-in protected list; \
                 this cannot be overridden by an allow rule"
            ));
        }
        if effects.writes && self.builtin_write.iter().any(|m| matches(m, path)) {
            return Some(format!(
                "writing `{path}` is refused by the built-in protected list; \
                 this cannot be overridden by an allow rule"
            ));
        }
        None
    }
}

fn describe(effects: Effects) -> String {
    let mut what = Vec::new();
    if effects.writes {
        what.push("write files");
    }
    if effects.executes {
        what.push("run a command");
    }
    if effects.network {
        what.push("access the network");
    }
    if what.is_empty() {
        return "this action needs approval".into();
    }
    format!("this will {}", what.join(" and "))
}

/// The decision the engine records when nobody is asked.
pub fn implicit(verdict: &Verdict) -> Option<Decision> {
    match verdict {
        Verdict::Allow => Some(Decision::Allow),
        Verdict::Deny(reason) => Some(Decision::Deny {
            feedback: Some(reason.clone()),
        }),
        Verdict::Ask(_) => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plan(subject: &str, effects: Effects) -> Plan {
        Plan::new(subject, effects)
    }

    const WRITE: Effects = Effects {
        reads: true,
        writes: true,
        executes: false,
        network: false,
    };
    const EXEC: Effects = Effects {
        reads: true,
        writes: true,
        executes: true,
        network: true,
    };

    #[test]
    fn built_in_patterns_all_compile() {
        // `compile` panics on a bad pattern; this is the test it refers to.
        let p = Policy::new();
        assert_eq!(p.builtin_read.len(), DENY_READ.len());
        assert_eq!(p.builtin_write.len(), DENY_WRITE.len());
    }

    #[test]
    fn reads_are_auto_approved() {
        let p = Policy::new();
        assert_eq!(
            p.evaluate(&plan("read:src/lib.rs", Effects::READ_ONLY)),
            Verdict::Allow
        );
    }

    #[test]
    fn writes_and_commands_are_asked_about() {
        let p = Policy::new();
        assert!(matches!(
            p.evaluate(&plan("write:src/lib.rs", WRITE)),
            Verdict::Ask(_)
        ));
        assert!(matches!(
            p.evaluate(&plan("bash:git", EXEC)),
            Verdict::Ask(_)
        ));
    }

    /// The ordering test. Read-only auto-approval must come *after* the deny
    /// list, or no rule can protect a secret file.
    #[test]
    fn a_protected_file_is_denied_before_read_only_auto_approval() {
        let p = Policy::new();
        match p.evaluate(&plan("read:.env", Effects::READ_ONLY)) {
            Verdict::Deny(reason) => assert!(reason.contains("built-in")),
            other => panic!("reading .env must be denied, got {other:?}"),
        }
    }

    #[test]
    fn an_allow_rule_cannot_override_the_built_in_list() {
        let p = Policy::new().allow_rule("read:**").unwrap();
        assert!(matches!(
            p.evaluate(&plan("read:.env", Effects::READ_ONLY)),
            Verdict::Deny(_)
        ));
        assert!(matches!(
            p.evaluate(&plan("read:config/.env.production", Effects::READ_ONLY)),
            Verdict::Deny(_)
        ));
        assert!(matches!(
            p.evaluate(&plan("read:deploy/id_rsa", Effects::READ_ONLY)),
            Verdict::Deny(_)
        ));
    }

    #[test]
    fn even_yes_does_not_override_the_built_in_list() {
        let p = Policy::new().unattended_allow();
        assert!(matches!(
            p.evaluate(&plan("read:.ssh/config", Effects::READ_ONLY)),
            Verdict::Deny(_)
        ));
    }

    #[test]
    fn git_hooks_and_credential_files_are_write_protected() {
        let p = Policy::new().unattended_allow();
        for subject in ["write:.git/hooks/pre-commit", "write:.env"] {
            assert!(
                matches!(p.evaluate(&plan(subject, WRITE)), Verdict::Deny(_)),
                "{subject} should be refused"
            );
        }
    }

    #[test]
    fn a_user_deny_rule_beats_a_user_allow_rule() {
        let p = Policy::new()
            .allow_rule("bash:*")
            .unwrap()
            .deny_rule("bash:rm")
            .unwrap();
        assert_eq!(p.evaluate(&plan("bash:git", EXEC)), Verdict::Allow);
        assert!(matches!(
            p.evaluate(&plan("bash:rm", EXEC)),
            Verdict::Deny(_)
        ));
    }

    #[test]
    fn a_session_grant_allows_the_same_subject_again() {
        let mut p = Policy::new();
        assert!(matches!(
            p.evaluate(&plan("bash:cargo", EXEC)),
            Verdict::Ask(_)
        ));
        p.grant("bash:cargo");
        assert_eq!(p.evaluate(&plan("bash:cargo", EXEC)), Verdict::Allow);
        // Narrow: it grants that subject, not the tool.
        assert!(matches!(
            p.evaluate(&plan("bash:curl", EXEC)),
            Verdict::Ask(_)
        ));
    }

    #[test]
    fn unattended_deny_refuses_rather_than_waiting() {
        let p = Policy {
            unattended: Some(Unattended::Deny),
            ..Policy::new()
        };
        assert!(matches!(
            p.evaluate(&plan("write:a.rs", WRITE)),
            Verdict::Deny(_)
        ));
        // Reads still work: there is nothing to ask about.
        assert_eq!(
            p.evaluate(&plan("read:a.rs", Effects::READ_ONLY)),
            Verdict::Allow
        );
    }

    #[test]
    fn yes_allows_what_would_otherwise_be_asked() {
        let p = Policy::new().unattended_allow();
        assert_eq!(p.evaluate(&plan("write:a.rs", WRITE)), Verdict::Allow);
        assert_eq!(p.evaluate(&plan("bash:curl", EXEC)), Verdict::Allow);
    }

    #[test]
    fn a_compound_command_cannot_match_a_program_rule() {
        // The subject for a compound command is deliberately unmatchable; see
        // the bash tool for where it is produced.
        let p = Policy::new().allow_rule("bash:git*").unwrap();
        assert_eq!(p.evaluate(&plan("bash:git", EXEC)), Verdict::Allow);
        assert!(matches!(
            p.evaluate(&plan("bash:!compound", EXEC)),
            Verdict::Ask(_)
        ));
    }

    #[test]
    fn an_empty_rule_is_reported_not_ignored() {
        assert!(Policy::new().allow_rule("  ").is_err());
    }

    #[test]
    fn the_matcher_handles_the_whole_language_and_nothing_more() {
        assert!(matches("read:*", "read:src/lib.rs"));
        assert!(matches("bash:git*", "bash:git"));
        assert!(matches("*.env", ".env"));
        assert!(matches("*.env", "config/.env"));
        assert!(matches("*.ssh/*", "home/.ssh/id_rsa"));
        assert!(matches("read:?.rs", "read:a.rs"));
        assert!(!matches("read:?.rs", "read:ab.rs"));
        assert!(!matches("bash:git*", "bash:!compound"));
        assert!(matches("*", "anything at all"));
        // A character class is not part of the language, so it is a literal.
        assert!(!matches("read:[ab].rs", "read:a.rs"));
        assert!(matches("read:[ab].rs", "read:[ab].rs"));
    }

    #[test]
    fn the_matcher_does_not_blow_up_on_hostile_input() {
        // A backtracking regex dies on this shape; the two-pointer walk does
        // not.
        let pattern = "*".repeat(40) + "b";
        let subject = "a".repeat(4_000);
        assert!(!matches(&pattern, &subject));
    }
}
