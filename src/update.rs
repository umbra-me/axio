//! `axio update`: replaces this binary with the latest GitHub release, after
//! checking its SHA-256 against the release's SHA256SUMS.

use std::io::Read;
use std::path::Path;
use std::process::ExitCode;

const LATEST: &str = "https://api.github.com/repos/umbra-me/axio/releases/latest";
/// Larger than any build; stops a bad server from filling memory.
const MAX_BYTES: u64 = 256 << 20;

/// This platform's asset name in a release; see .github/workflows/release.yml.
pub fn asset() -> Option<&'static str> {
    Some(match (std::env::consts::ARCH, std::env::consts::OS) {
        ("aarch64", "macos") => "axio-aarch64-apple-darwin",
        ("x86_64", "macos") => "axio-x86_64-apple-darwin",
        ("x86_64", "linux") => "axio-x86_64-unknown-linux-musl",
        ("aarch64", "linux") => "axio-aarch64-unknown-linux-musl",
        ("x86_64", "windows") => "axio-x86_64-pc-windows-msvc.exe",
        _ => return None,
    })
}

/// `1.2.3` or `v1.2.3-beta.1` as numbers plus whether it is a prerelease.
fn version(s: &str) -> Option<((u64, u64, u64), bool)> {
    let s = s.strip_prefix('v').unwrap_or(s);
    let (core, pre) = match s.split_once('-') {
        Some((c, _)) => (c, true),
        None => (s, false),
    };
    let mut n = core.split('.').map(|p| p.parse::<u64>().ok());
    let v = (n.next()??, n.next()??, n.next()??);
    n.next().is_none().then_some((v, pre))
}

/// Whether `latest` is a later release than `current`.
fn newer(latest: &str, current: &str) -> bool {
    match (version(latest), version(current)) {
        (Some((l, lp)), Some((c, cp))) => l > c || (l == c && cp && !lp),
        _ => false,
    }
}

fn get(url: &str) -> Result<Vec<u8>, String> {
    // GitHub serves release files through a redirect to another host, so
    // this client follows redirects; it sends no credentials.
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .http_status_as_error(false)
        .https_only(crate::provider::env("AXIO_UPDATE_URL").is_none())
        .build()
        .into();
    let resp = agent
        .get(url)
        .header("user-agent", concat!("axio/", env!("CARGO_PKG_VERSION")))
        .header("accept", "application/vnd.github+json")
        .call()
        .map_err(|e| format!("{url}: {e}"))?;
    let status = resp.status().as_u16();
    if status == 404 {
        return Err(format!(
            "{url}: no published release found (none yet, or the repository is private)"
        ));
    }
    if !(200..300).contains(&status) {
        return Err(format!("{url}: HTTP {status}"));
    }
    let mut body = Vec::new();
    resp.into_body()
        .into_reader()
        .take(MAX_BYTES)
        .read_to_end(&mut body)
        .map_err(|e| format!("{url}: {e}"))?;
    Ok(body)
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

pub fn main(args: &[String]) -> Result<ExitCode, String> {
    let mut check = false;
    for a in args {
        match a.as_str() {
            "--check" => check = true,
            _ => return Err(format!("unknown option {a} (usage: axio update [--check])")),
        }
    }
    let current = env!("CARGO_PKG_VERSION");
    let exe = std::env::current_exe()
        .and_then(|p| p.canonicalize())
        .map_err(|e| format!("cannot find this binary: {e}"))?;
    // Left behind by the last update on Windows, where a running binary can
    // be renamed but not deleted.
    let _ = std::fs::remove_file(old_path(&exe));
    let custom = crate::provider::env("AXIO_UPDATE_URL");
    let release: serde_json::Value =
        serde_json::from_slice(&get(custom.as_deref().unwrap_or(LATEST))?)
            .map_err(|e| format!("release: {e}"))?;
    let tag = release["tag_name"].as_str().ok_or("release: no tag_name")?;
    let latest = tag.strip_prefix('v').unwrap_or(tag);
    if !newer(latest, current) {
        println!("axio {current} is up to date (latest release {latest})");
        return Ok(ExitCode::SUCCESS);
    }
    if check {
        println!("axio {latest} is available (this is {current}); `axio update` installs it");
        return Ok(ExitCode::SUCCESS);
    }
    let name = asset().ok_or("no release binary is built for this platform")?;
    let url_of = |n: &str| -> Result<String, String> {
        let url = release["assets"]
            .as_array()
            .into_iter()
            .flatten()
            .find(|a| a["name"] == n)
            .and_then(|a| a["browser_download_url"].as_str())
            .ok_or(format!("release {tag} has no {n}"))?;
        if custom.is_none() && !url.starts_with("https://") {
            return Err(format!("refusing a download that is not https: {url}"));
        }
        Ok(url.to_string())
    };
    let sums = String::from_utf8_lossy(&get(&url_of("SHA256SUMS")?)?).to_string();
    let want = sums
        .lines()
        .filter_map(|l| l.split_once(char::is_whitespace))
        .find(|(_, file)| file.trim().trim_start_matches('*') == name)
        .map(|(sum, _)| sum.to_lowercase())
        .ok_or(format!("SHA256SUMS for {tag} does not list {name}"))?;
    let binary = get(&url_of(name)?)?;
    let got = hex(&crate::crypto::sha256(&binary));
    if got != want {
        return Err(format!(
            "the downloaded {name} does not match its SHA-256 in SHA256SUMS; nothing was changed"
        ));
    }
    replace(&exe, &binary)?;
    println!("updated axio {current} → {latest} ({})", exe.display());
    Ok(ExitCode::SUCCESS)
}

fn old_path(exe: &Path) -> std::path::PathBuf {
    exe.with_extension("old")
}

/// Writes the new binary beside the old one and renames it into place, so
/// the path is never half-written.
fn replace(exe: &Path, binary: &[u8]) -> Result<(), String> {
    let dir = exe.parent().ok_or("this binary has no folder")?;
    let tmp = dir.join(format!(".axio-update-{}", std::process::id()));
    let result = (|| -> Result<(), String> {
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o755);
        let mut f = options
            .open(&tmp)
            .map_err(|e| format!("cannot write in {}: {e}", dir.display()))?;
        std::io::Write::write_all(&mut f, binary).map_err(|e| e.to_string())?;
        f.sync_all().map_err(|e| e.to_string())?;
        drop(f);
        if cfg!(windows) {
            let old = old_path(exe);
            std::fs::rename(exe, &old).map_err(|e| format!("{}: {e}", exe.display()))?;
            if let Err(e) = std::fs::rename(&tmp, exe) {
                let _ = std::fs::rename(&old, exe);
                return Err(format!("{}: {e}", exe.display()));
            }
            Ok(())
        } else {
            std::fs::rename(&tmp, exe).map_err(|e| format!("{}: {e}", exe.display()))
        }
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions_compare_by_number_and_prerelease() {
        assert!(newer("0.1.1", "0.1.0"));
        assert!(newer("v0.10.0", "0.9.9"));
        assert!(newer("0.2.0", "0.2.0-beta.1"));
        assert!(!newer("0.2.0-beta.1", "0.2.0"));
        assert!(!newer("0.1.0", "0.1.0"));
        assert!(!newer("0.0.9", "0.1.0"));
        assert!(!newer("nonsense", "0.1.0"));
        assert!(!newer("1.2", "0.1.0"));
    }

    #[test]
    fn this_platform_has_a_release_binary() {
        assert!(asset().is_some());
    }
}
