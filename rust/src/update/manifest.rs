// SPDX-License-Identifier: MPL-2.0
use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    path::{Component, Path, PathBuf},
};

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum PackageKind {
    Installed,
    Portable,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Plan {
    pub schema: u32,
    pub package: PathBuf,
    pub kind: PackageKind,
    pub app_dir: PathBuf,
    pub config_dir: PathBuf,
    pub parent_pid: u32,
    pub version: String,
    pub sha256: String,
    pub arguments: Vec<String>,
    #[serde(default)]
    pub show_window: bool,
}
#[derive(Deserialize)]
pub struct Payload {
    pub version: String,
    pub commit: String,
    pub files: BTreeMap<String, String>,
}
pub fn validate_hash(hash: &str) -> Result<()> {
    ensure!(
        hash.len() == 64
            && hash
                .bytes()
                .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c)),
        "Invalid SHA-256"
    );
    Ok(())
}
pub fn safe_relative(name: &str) -> Result<&Path> {
    let path = Path::new(name);
    ensure!(
        !name.is_empty()
            && !name.contains(['\\', ':', '\0'])
            && path.components().all(|p| matches!(p, Component::Normal(_)))
            && !name.split('/').any(|c| c.is_empty()
                || c.ends_with(['.', ' '])
                || matches!(
                    c.split('.')
                        .next()
                        .unwrap_or("")
                        .to_ascii_uppercase()
                        .as_str(),
                    "CON"
                        | "PRN"
                        | "AUX"
                        | "NUL"
                        | "COM1"
                        | "COM2"
                        | "COM3"
                        | "COM4"
                        | "COM5"
                        | "COM6"
                        | "COM7"
                        | "COM8"
                        | "COM9"
                        | "LPT1"
                        | "LPT2"
                        | "LPT3"
                        | "LPT4"
                        | "LPT5"
                        | "LPT6"
                        | "LPT7"
                        | "LPT8"
                        | "LPT9"
                )),
        "Unsafe package path: {name}"
    );
    Ok(path)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn update_paths_cannot_escape_or_alias_windows_devices() {
        for path in [
            "../x", "/x", "x/../y", "x\\y", "C:/x", "NUL.txt", "a/COM1", "x.", "x ", "a//b",
        ] {
            assert!(safe_relative(path).is_err(), "{path}");
        }
        assert!(safe_relative("sources/airdock-source.zip").is_ok());
    }
}
