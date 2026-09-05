//! Explicit process handoff; profile ownership stays with the local launcher.
use std::ffi::OsString;

pub(crate) fn run(profile: &str, args: &[OsString]) -> u8 {
    let mut command = std::process::Command::new("axio-local");
    command.arg("--profile").arg(profile).arg("axio").args(args);
    if let Ok(exe) = std::env::current_exe()
        && let Some(parent) = exe.parent()
    {
        let mut paths = vec![parent.to_path_buf()];
        paths.extend(std::env::split_paths(
            &std::env::var_os("PATH").unwrap_or_default(),
        ));
        if let Ok(path) = std::env::join_paths(paths) {
            command.env("PATH", path);
        }
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        let error = command.exec();
        eprintln!(
            "could not start Axio Local: {error}. Install axio-local and configure the named profile."
        );
        2
    }
    #[cfg(not(unix))]
    {
        let _ = command;
        eprintln!("Axio Local requires macOS, Linux or WSL.");
        2
    }
}
