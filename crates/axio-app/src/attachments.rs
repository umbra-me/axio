//! Import a versioned capture without trusting its path or metadata.
use crate::model::AppError;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::{
    io::{Read, Write},
    path::{Path, PathBuf},
};
const LIMIT: u64 = 32 * 1024 * 1024;
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Manifest {
    schema_version: u32,
    media_type: String,
    file: String,
    sha256: String,
    width: u32,
    height: u32,
}
fn error(e: impl std::fmt::Display) -> AppError {
    AppError::Unavailable(e.to_string())
}
fn read(path: &Path, cap: u64) -> Result<Vec<u8>, AppError> {
    let mut bytes = Vec::new();
    std::fs::File::open(path)
        .map_err(error)?
        .take(cap + 1)
        .read_to_end(&mut bytes)
        .map_err(error)?;
    if bytes.len() as u64 > cap {
        return Err(error("attachment exceeds size limit"));
    }
    Ok(bytes)
}
pub fn import(manifest: &Path, destination: &Path) -> Result<PathBuf, AppError> {
    let m: Manifest = serde_json::from_slice(&read(manifest, 8192)?).map_err(error)?;
    if m.schema_version != 1
        || m.media_type != "image/png"
        || m.file.is_empty()
        || m.file.contains(['/', '\\', ':'])
        || m.file == "."
        || m.file == ".."
    {
        return Err(error("unsupported capture format or unsafe image filename"));
    }
    let parent = manifest
        .parent()
        .ok_or_else(|| error("missing capture directory"))?
        .canonicalize()
        .map_err(error)?;
    let source = parent.join(&m.file).canonicalize().map_err(error)?;
    if source.parent() != Some(parent.as_path()) {
        return Err(error("capture image escapes its directory"));
    }
    let png = read(&source, LIMIT)?;
    let digest = format!("{:x}", Sha256::digest(&png));
    if digest != m.sha256
        || png.len() < 24
        || !png.starts_with(b"\x89PNG\r\n\x1a\n")
        || &png[12..16] != b"IHDR"
        || u32::from_be_bytes(png[16..20].try_into().unwrap()) != m.width
        || u32::from_be_bytes(png[20..24].try_into().unwrap()) != m.height
        || m.width == 0
        || m.height == 0
        || u64::from(m.width) * u64::from(m.height) > 80_000_000
    {
        return Err(error("capture image hash or dimensions do not match"));
    }
    std::fs::create_dir_all(destination).map_err(error)?;
    let target = destination.join(format!("{digest}.png"));
    match std::fs::File::options()
        .write(true)
        .create_new(true)
        .open(&target)
    {
        Ok(mut output) => {
            output.write_all(&png).map_err(error)?;
            output.sync_all().map_err(error)?;
        }
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
            if read(&target, LIMIT)? != png {
                return Err(error("cached attachment differs"));
            }
        }
        Err(e) => return Err(error(e)),
    }
    Ok(target)
}

/// The argument parser accepts quoting but never runs a shell.
pub fn image_args(path: &Path) -> String {
    let value = path
        .to_string_lossy()
        .replace('\\', "\\\\")
        .replace('"', "\\\"");
    format!("--image \"{value}\"")
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn path_quoting_survives_the_actual_launch_parser() {
        let path = Path::new("/tmp/capture space/quote\"and\\slash.png");
        assert_eq!(
            axio_pty::split_args(&image_args(path)).unwrap(),
            vec!["--image", path.to_str().unwrap()]
        );
    }
    #[test]
    fn import_checks_hash_and_refuses_traversal() {
        let d = tempfile::tempdir().unwrap();
        // Header suffices here: image decoding belongs to the consuming agent.
        let mut png = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR".to_vec();
        png.extend(1u32.to_be_bytes());
        png.extend(1u32.to_be_bytes());
        std::fs::write(d.path().join("image.png"), &png).unwrap();
        let mut value = serde_json::json!({"schema_version":1,"media_type":"image/png","file":"image.png","sha256":format!("{:x}",Sha256::digest(&png)),"width":1,"height":1});
        let file = d.path().join("capture.json");
        let target = d.path().join("copies");
        std::fs::write(&file, value.to_string()).unwrap();
        let copied = import(&file, &target).unwrap();
        assert_eq!(std::fs::read(copied).unwrap(), png);
        value["sha256"] = "bad".into();
        std::fs::write(&file, value.to_string()).unwrap();
        assert!(import(&file, &target).is_err());
        value["file"] = "../image.png".into();
        std::fs::write(&file, value.to_string()).unwrap();
        assert!(import(&file, &target).is_err());
    }
}
