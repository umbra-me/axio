//! `axio update` against a stub release server: the binary is replaced only
//! when its SHA-256 matches SHA256SUMS.

use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::Output;

const ASSETS: [&str; 5] = [
    "axio-aarch64-apple-darwin",
    "axio-x86_64-apple-darwin",
    "axio-x86_64-unknown-linux-musl",
    "axio-aarch64-unknown-linux-musl",
    "axio-x86_64-pc-windows-msvc.exe",
];

/// Serves `/latest`, `/sums` and `/bin` for a release `tag` whose binary is
/// `binary` and whose SHA256SUMS lists `sum` for every asset.
fn serve(tag: &str, binary: &'static [u8], sum: String) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://127.0.0.1:{}", listener.local_addr().unwrap().port());
    let assets: Vec<_> = ASSETS
        .iter()
        .map(|n| serde_json::json!({"name": n, "browser_download_url": format!("{base}/bin")}))
        .chain([serde_json::json!({"name": "SHA256SUMS", "browser_download_url": format!("{base}/sums")})])
        .collect();
    let latest = serde_json::json!({"tag_name": tag, "assets": assets}).to_string();
    let sums: String = ASSETS.iter().map(|n| format!("{sum}  {n}\n")).collect();
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut first = String::new();
            reader.read_line(&mut first).unwrap();
            loop {
                let mut line = String::new();
                if reader.read_line(&mut line).unwrap_or(0) == 0 || line == "\r\n" {
                    break;
                }
            }
            let path = first.split_whitespace().nth(1).unwrap_or("");
            let body: Vec<u8> = match path {
                "/latest" => latest.clone().into_bytes(),
                "/sums" => sums.clone().into_bytes(),
                "/bin" => binary.to_vec(),
                _ => Vec::new(),
            };
            let mut stream = stream;
            let _ = write!(
                stream,
                "HTTP/1.1 200 OK\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
                body.len()
            );
            let _ = stream.write_all(&body);
        }
    });
    format!("{base}/latest")
}

fn sha256_hex(data: &[u8]) -> String {
    // Computed by the system tool so the test does not trust axio's own hash.
    static N: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let n = N.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let dir = scratch(&format!("sum{n}"));
    let f = dir.join("data");
    std::fs::write(&f, data).unwrap();
    let out = if cfg!(windows) {
        std::process::Command::new("certutil")
            .args(["-hashfile", f.to_str().unwrap(), "SHA256"])
            .output()
            .unwrap()
    } else if cfg!(target_os = "macos") {
        std::process::Command::new("shasum")
            .args(["-a", "256", f.to_str().unwrap()])
            .output()
            .unwrap()
    } else {
        std::process::Command::new("sha256sum")
            .arg(&f)
            .output()
            .unwrap()
    };
    let text = String::from_utf8_lossy(&out.stdout).to_string();
    text.split_whitespace()
        .find(|w| w.len() == 64 && w.chars().all(|c| c.is_ascii_hexdigit()))
        .unwrap()
        .to_lowercase()
}

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("axio-update-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// A copy of axio in its own folder, so updating it leaves the build alone.
fn installed(name: &str) -> PathBuf {
    let dir = scratch(name);
    let exe = dir.join(Path::new(env!("CARGO_BIN_EXE_axio")).file_name().unwrap());
    std::fs::copy(env!("CARGO_BIN_EXE_axio"), &exe).unwrap();
    exe
}

fn update(exe: &Path, url: &str, args: &[&str]) -> Output {
    std::process::Command::new(exe)
        .arg("update")
        .args(args)
        .env("AXIO_UPDATE_URL", url)
        .env("AXIO_HOME", exe.parent().unwrap().join("home"))
        .output()
        .unwrap()
}

fn text(o: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&o.stdout),
        String::from_utf8_lossy(&o.stderr)
    )
}

#[test]
fn update_replaces_the_binary_when_the_hash_matches() {
    const NEW: &[u8] = b"the next axio";
    let url = serve("v99.0.0", NEW, sha256_hex(NEW));
    let exe = installed("ok");
    let out = update(&exe, &url, &[]);
    assert!(out.status.success(), "{}", text(&out));
    assert!(text(&out).contains("99.0.0"), "{}", text(&out));
    assert_eq!(std::fs::read(&exe).unwrap(), NEW);
}

#[test]
fn a_wrong_hash_leaves_the_binary_alone() {
    let url = serve("v99.0.0", b"tampered", sha256_hex(b"something else"));
    let exe = installed("bad");
    let before = std::fs::read(&exe).unwrap();
    let out = update(&exe, &url, &[]);
    assert!(!out.status.success(), "{}", text(&out));
    assert!(text(&out).contains("SHA-256"), "{}", text(&out));
    assert_eq!(std::fs::read(&exe).unwrap(), before);
    let left: Vec<_> = std::fs::read_dir(exe.parent().unwrap())
        .unwrap()
        .flatten()
        .map(|e| e.file_name())
        .collect();
    assert_eq!(left.len(), 1, "no leftover download: {left:?}");
}

#[test]
fn check_only_reports_a_newer_release() {
    let url = serve("v99.0.0", b"x", sha256_hex(b"x"));
    let exe = installed("check");
    let before = std::fs::read(&exe).unwrap();
    let out = update(&exe, &url, &["--check"]);
    assert!(out.status.success(), "{}", text(&out));
    assert!(text(&out).contains("99.0.0 is available"), "{}", text(&out));
    assert_eq!(std::fs::read(&exe).unwrap(), before);
}

#[test]
fn an_older_or_equal_release_changes_nothing() {
    let url = serve("v0.0.1", b"x", sha256_hex(b"x"));
    let exe = installed("old");
    let before = std::fs::read(&exe).unwrap();
    let out = update(&exe, &url, &[]);
    assert!(out.status.success(), "{}", text(&out));
    assert!(text(&out).contains("up to date"), "{}", text(&out));
    assert_eq!(std::fs::read(&exe).unwrap(), before);
}
