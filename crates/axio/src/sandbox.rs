//! Filesystem confinement for everything axio runs, on Linux.
//!
//! `SECURITY.md` says there is no sandbox, and for the default configuration
//! that stays true. This is the opt-in exception: Landlock, the kernel's own
//! unprivileged sandbox, restricting the axio process — and therefore every
//! command it spawns, because a Landlock ruleset is inherited and cannot be
//! relaxed by a child.
//!
//! **What it is.** An allow-list over paths. The workspace is writable, the
//! system is readable and executable, and everything else — `~/.ssh`, `~/.aws`,
//! a sibling checkout, axio's own credential file — is not there at all. A
//! shell command that goes looking gets `ENOENT` or `EACCES` from the kernel,
//! not a refusal from a policy engine it might talk its way around.
//!
//! **What it is not.** It is filesystem only: a sandboxed command can still
//! open a socket and send the workspace anywhere. It is Linux only. It is
//! applied to axio itself, so it cannot distinguish axio's own reads from a
//! command's — the built-in deny list is still what stops the `read` tool
//! opening `.env`, and this is a second wall behind it, not a replacement.
//!
//! **Applied before the async runtime exists.** A Landlock domain belongs to
//! the calling *thread* and is inherited by threads it goes on to create — so
//! restricting a tokio worker restricts that worker and nothing else, and the
//! command runs on a different one. The symptom is a sandbox that reports
//! itself applied and confines nothing, which is the worst of both.
//!
//! axio's own configuration directory is granted read-only, because the
//! credential is read after this point. That is a deliberate hole and a small
//! one: `Policy::protect` already refuses `auth.json` to the file tools and to
//! a shell command's arguments. The user's home directory is not granted, so
//! `~/.ssh` and `~/.aws` are not reachable at all.

use std::path::{Path, PathBuf};

/// Paths a toolchain needs to read before it will run at all.
///
/// Not a guess at what a user wants — a list of things that make `git`, `cargo`
/// and friends work at all, and whose contents are not secrets. Anything else
/// goes in `[sandbox] read`, where the user has said it out loud.
const SYSTEM_READ: &[&str] = &[
    "/usr", "/bin", "/sbin", "/lib", "/lib64", "/opt", "/etc", "/nix", "/snap",
    // Not optional, and not obvious: spawning anything at all needs these.
    // `/proc` is how the standard library closes inherited descriptors —
    // without it the shell cannot start and every command fails with
    // "permission denied" before it has run.
    "/proc", "/sys",
];

/// Devices, which a command writes to as well as reads from — `/dev/null`,
/// where `Stdio::null()` points, being the one nothing works without. Landlock
/// only ever removes access, so granting the directory cannot exceed what the
/// user could already do.
const DEVICE_WRITE: &[&str] = &["/dev"];

/// Home-relative entries a build tool reads and a credential does not live in.
const HOME_READ: &[&str] = &[".gitconfig", ".config/git", ".cargo", ".rustup"];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Plan {
    /// Read, write, execute.
    pub write: Vec<PathBuf>,
    /// Read and execute only.
    pub read: Vec<PathBuf>,
}

impl Plan {
    /// The default confinement for a run.
    ///
    /// The system temp directory is deliberately **not** granted. Tools need
    /// somewhere to scribble, so they get one inside the state directory and
    /// `TMPDIR` points at it — granting all of `/tmp` would hand over every
    /// other process's scratch files, which is most of what is interesting on a
    /// shared machine.
    pub fn new(workspace: &Path, state: &Path, axio_home: &Path, user_home: Option<&Path>) -> Self {
        let mut write = vec![workspace.to_path_buf(), state.to_path_buf()];
        write.extend(DEVICE_WRITE.iter().map(PathBuf::from));

        let mut read: Vec<PathBuf> = SYSTEM_READ.iter().map(PathBuf::from).collect();
        // Read-only, and only this directory: the credential is read after the
        // sandbox is in place. The user's home is not granted, so `~/.ssh` and
        // `~/.aws` are not reachable through it.
        read.push(axio_home.to_path_buf());
        if let Some(home) = user_home {
            for entry in HOME_READ {
                read.push(home.join(entry));
            }
        }
        Self { write, read }
    }

    pub fn allow_read(mut self, paths: impl IntoIterator<Item = PathBuf>) -> Self {
        self.read.extend(paths);
        self
    }

    pub fn allow_write(mut self, paths: impl IntoIterator<Item = PathBuf>) -> Self {
        self.write.extend(paths);
        self
    }
}

/// Where a sandboxed command may write temporary files.
///
/// Inside the state directory, which is already granted, so a tool that honours
/// `TMPDIR` keeps working without opening up the shared one.
pub fn scratch_dir(state: &Path) -> PathBuf {
    state.join("tmp")
}

/// What happened, so the caller can say it rather than guess.
///
/// Only `Unsupported` is reachable off Linux, and the other two are still part
/// of what this type means — CI compiles with `-D warnings`, so the exemption
/// has to be written down rather than discovered.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
#[derive(Debug, PartialEq, Eq)]
pub enum Outcome {
    /// Every requested access is now the only access there is.
    Enforced,
    /// The kernel has Landlock but not every restriction asked for. Reported
    /// rather than swallowed: a partial sandbox is a different promise.
    Partial(String),
    /// No Landlock here. Not an error — the caller decides whether that is
    /// fatal — but never silently treated as success.
    Unsupported(String),
}

#[cfg(target_os = "linux")]
pub fn apply(plan: &Plan) -> Outcome {
    use landlock::{
        ABI, Access, AccessFs, CompatLevel, Compatible, PathBeneath, PathFd, RulesetAttr,
        RulesetCreatedAttr, RulesetStatus,
    };

    // The newest ABI we know how to ask for. `set_compatibility(BestEffort)`
    // degrades on an older kernel rather than failing, and the status tells us
    // which happened — so "enforced" never gets reported for a kernel that
    // quietly dropped half the request.
    let abi = ABI::V5;
    let read_only = AccessFs::from_read(abi);
    let read_write = AccessFs::from_all(abi);

    let mut ruleset = match landlock::Ruleset::default()
        .set_compatibility(CompatLevel::BestEffort)
        .handle_access(read_write)
    {
        Ok(r) => match r.create() {
            Ok(created) => created,
            Err(e) => return Outcome::Unsupported(format!("could not create a ruleset: {e}")),
        },
        Err(e) => return Outcome::Unsupported(format!("this kernel has no Landlock: {e}")),
    };

    // A path that does not exist is skipped, not fatal: `/nix` on a machine
    // without it is an absence, not a misconfiguration.
    // Directory rights on a regular file are not a smaller request but an
    // impossible one: `ReadDir` on `~/.gitconfig` cannot be honoured, so the
    // kernel enforces a reduced ruleset and the whole sandbox reports itself as
    // only partly applied. The rights have to match what the path is.
    //
    // Narrowed by intersection, never replaced. `AccessFs::from_file` is every
    // right a file can carry — including `WriteFile` and `Truncate` — so
    // substituting it for a *read* grant made `~/.gitconfig` writable, and a
    // writable git config is `core.hooksPath` pointing anywhere it likes.
    let file_rights = AccessFs::from_file(abi);
    for (paths, dir_access) in [(&plan.read, read_only), (&plan.write, read_write)] {
        for path in paths {
            let Ok(fd) = PathFd::new(path) else { continue };
            let is_dir = std::fs::metadata(path).map(|m| m.is_dir()).unwrap_or(false);
            let access = if is_dir {
                dir_access
            } else {
                dir_access & file_rights
            };
            if access.is_empty() {
                continue;
            }
            match ruleset.add_rule(PathBeneath::new(fd, access)) {
                Ok(next) => ruleset = next,
                Err(e) => return Outcome::Partial(format!("{}: {e}", path.display())),
            }
        }
    }

    match ruleset.restrict_self() {
        Ok(status) => match status.ruleset {
            RulesetStatus::FullyEnforced => Outcome::Enforced,
            RulesetStatus::PartiallyEnforced => {
                Outcome::Partial("this kernel supports only part of the ruleset".into())
            }
            RulesetStatus::NotEnforced => {
                Outcome::Unsupported("this kernel did not enforce the ruleset".into())
            }
        },
        Err(e) => Outcome::Unsupported(format!("could not restrict this process: {e}")),
    }
}

#[cfg(not(target_os = "linux"))]
pub fn apply(_plan: &Plan) -> Outcome {
    Outcome::Unsupported("the sandbox is Linux-only".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plan() -> Plan {
        Plan::new(
            Path::new("/work"),
            Path::new("/state"),
            Path::new("/home/u/.config/axio"),
            Some(Path::new("/home/u")),
        )
    }

    #[test]
    fn the_users_home_is_never_granted_wholesale() {
        // Granting `$HOME` to make `git` work would hand over `~/.ssh` and
        // `~/.aws` with it, which is most of what this exists to stop.
        let plan = plan();
        assert!(!plan.read.iter().any(|p| p == Path::new("/home/u")));
        assert!(!plan.write.iter().any(|p| p == Path::new("/home/u")));
        assert!(!plan.read.iter().any(|p| p.ends_with(".ssh")));
    }

    #[test]
    fn axio_config_is_readable_but_never_writable() {
        // The credential is read after the sandbox is applied, so this has to
        // be reachable; nothing needs to write it mid-run.
        let plan = plan();
        let home = PathBuf::from("/home/u/.config/axio");
        assert!(plan.read.contains(&home));
        assert!(!plan.write.contains(&home));
    }

    #[test]
    fn the_workspace_and_the_state_directory_are_writable() {
        let plan = plan();
        assert!(plan.write.contains(&PathBuf::from("/work")));
        assert!(plan.write.contains(&PathBuf::from("/state")));
        // The spill file lives under the state directory and the model is told
        // to read it back, so writable state is not optional.
    }

    #[test]
    fn the_shared_temp_directory_is_not_granted() {
        // Every other process on the machine scribbles there, and most of what
        // is interesting on a shared box is in someone else's scratch file.
        let plan = plan();
        let temp = std::env::temp_dir();
        assert!(!plan.write.contains(&temp), "{:?}", plan.write);
        assert!(!plan.read.contains(&temp));
    }

    #[test]
    fn scratch_lives_under_the_state_directory() {
        assert!(scratch_dir(Path::new("/state")).starts_with("/state"));
    }

    #[test]
    fn the_system_is_readable_so_a_toolchain_still_runs() {
        let plan = plan();
        for expected in ["/usr", "/bin", "/etc"] {
            assert!(
                plan.read.contains(&PathBuf::from(expected)),
                "{expected} must be readable or nothing runs"
            );
        }
        // Dropping either of these is a change nothing else catches: the
        // end-to-end test looks for a canary that is equally absent when the
        // shell never started at all.
        assert!(
            plan.read.contains(&PathBuf::from("/proc")),
            "the standard library closes inherited descriptors through /proc"
        );
        assert!(
            plan.write.contains(&PathBuf::from("/dev")),
            "Stdio::null() opens /dev/null"
        );
    }
}
