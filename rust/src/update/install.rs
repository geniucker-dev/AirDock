// SPDX-License-Identifier: MPL-2.0
//! Runs in a separate, FFmpeg-independent executable after receiver shutdown.
use super::manifest::{PackageKind, Payload, Plan, safe_relative, validate_hash};
use anyhow::{Context, Result, bail, ensure};
use sha2::{Digest, Sha256};
#[cfg(not(windows))]
use std::time::{Duration, Instant};
use std::{
    collections::BTreeSet,
    fs,
    io::Read,
    path::{Path, PathBuf},
};

fn hash_file(path: &Path) -> Result<String> {
    let mut file = fs::File::open(path)?;
    let mut hash = Sha256::new();
    let mut buffer = [0u8; 65536];
    loop {
        let n = file.read(&mut buffer)?;
        if n == 0 {
            break;
        }
        hash.update(&buffer[..n]);
    }
    Ok(hex::encode(hash.finalize()))
}
fn verify_payload(directory: &Path, version: &str) -> Result<Payload> {
    let manifest: Payload =
        serde_json::from_slice(&fs::read(directory.join("PACKAGE_SHA256.json"))?)?;
    ensure!(
        manifest.version == version
            && manifest.commit.len() == 40
            && manifest.commit.bytes().all(|b| b.is_ascii_hexdigit()),
        "Update payload version or commit is invalid"
    );
    ensure!(
        manifest.files.contains_key("airdock.exe")
            && manifest.files.contains_key("airdock-updater.exe"),
        "Update does not include both application executables"
    );
    for (name, expected) in &manifest.files {
        validate_hash(expected)?;
        let path = directory.join(safe_relative(name)?);
        ensure!(
            path.canonicalize()?.starts_with(directory.canonicalize()?),
            "Payload escaped its directory"
        );
        ensure!(
            hash_file(&path)? == *expected,
            "Payload hash mismatch: {name}"
        );
    }
    Ok(manifest)
}
struct Scratch(PathBuf);
impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn extract(package: &Path, destination: &Path) -> Result<()> {
    let mut zip = zip::ZipArchive::new(fs::File::open(package)?)?;
    let mut total = 0u64;
    let mut names = BTreeSet::new();
    ensure!(zip.len() <= 10000, "Too many update files");
    for index in 0..zip.len() {
        let mut entry = zip.by_index(index)?;
        let name = entry.name().trim_end_matches('/').to_owned();
        let relative = safe_relative(&name)?;
        ensure!(
            entry
                .unix_mode()
                .is_none_or(|mode| mode & 0o170000 != 0o120000),
            "Update contains a symlink"
        );
        ensure!(names.insert(name.clone()), "Duplicate update archive path");
        total = total
            .checked_add(entry.size())
            .context("Archive size overflow")?;
        ensure!(
            total <= 2 * 1024 * 1024 * 1024 && entry.size() <= 512 * 1024 * 1024,
            "Update archive is too large"
        );
        let target = destination.join(relative);
        if entry.is_dir() {
            fs::create_dir_all(target)?;
            continue;
        }
        fs::create_dir_all(target.parent().context("Invalid archive target")?)?;
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&target)?;
        let size = entry.size();
        let copied = std::io::copy(&mut entry.by_ref().take(size + 1), &mut file)?;
        ensure!(copied == size, "Incomplete archive entry");
        file.sync_all()?;
    }
    Ok(())
}
fn owned_files(directory: &Path) -> Result<BTreeSet<String>> {
    let manifest: Payload =
        serde_json::from_slice(&fs::read(directory.join("PACKAGE_SHA256.json"))?)?;
    let mut names = manifest.files.into_keys().collect::<BTreeSet<_>>();
    names.insert("PACKAGE_SHA256.json".into());
    for name in &names {
        safe_relative(name)?;
    }
    Ok(names)
}
pub fn apply_portable(plan: &Plan) -> Result<()> {
    let app = plan.app_dir.canonicalize()?;
    let nonce = format!(
        "{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_nanos()
    );
    let stage = Scratch(app.join(format!(".airdock-update-{nonce}")));
    let backup = app.join(format!(".airdock-backup-{nonce}"));
    fs::create_dir(&stage.0)?;
    extract(&plan.package, &stage.0)?;
    let payload = verify_payload(&stage.0, &plan.version)?;
    let mut incoming: BTreeSet<_> = payload.files.into_keys().collect();
    incoming.insert("PACKAGE_SHA256.json".into());
    // Reject hidden extra files even if they were not selected for installation.
    fn list(root: &Path, base: &Path, files: &mut BTreeSet<String>) -> Result<()> {
        for entry in fs::read_dir(root)? {
            let path = entry?.path();
            if path.is_dir() {
                list(&path, base, files)?;
            } else {
                files.insert(
                    path.strip_prefix(base)?
                        .to_str()
                        .context("Invalid Unicode package path")?
                        .replace('\\', "/"),
                );
            }
        }
        Ok(())
    }
    let mut extracted = BTreeSet::new();
    list(&stage.0, &stage.0, &mut extracted)?;
    ensure!(
        incoming == extracted,
        "Archive contains files absent from its manifest"
    );
    let previous = owned_files(&app)?;
    let affected: BTreeSet<_> = previous.union(&incoming).cloned().collect();
    for name in &affected {
        let target = app.join(safe_relative(name)?);
        if target.exists() {
            ensure!(
                previous.contains(name),
                "Update conflicts with a user file: {name}"
            );
            ensure!(
                target.is_file() && target.canonicalize()?.starts_with(&app),
                "Unsafe existing payload path: {name}"
            );
        }
        // Resolve the nearest existing ancestor to reject junctions/symlinks
        // which would redirect a new payload file outside the application.
        let mut parent = target.parent().context("Invalid payload parent")?;
        while !parent.exists() {
            parent = parent.parent().context("Invalid payload ancestor")?;
        }
        ensure!(
            parent.canonicalize()?.starts_with(&app),
            "Payload directory escaped the application"
        );
    }
    fs::create_dir(&backup)?;
    let mut saved = Vec::new();
    let mut installed = Vec::new();
    let result: Result<()> = (|| {
        // Hide the launchable application first. Publish its replacement last,
        // after the entire DLL/source closure is installed.
        let mut backup_order: Vec<_> = affected.iter().collect();
        backup_order.sort_by_key(|name| (*name != "airdock.exe", *name));
        for name in backup_order {
            let relative = safe_relative(name)?;
            let target = app.join(relative);
            if target.exists() {
                let old = backup.join(relative);
                fs::create_dir_all(old.parent().context("Invalid backup path")?)?;
                fs::rename(&target, &old)?;
                saved.push(name.clone());
            }
        }
        let mut install_order: Vec<_> = incoming.iter().collect();
        install_order.sort_by_key(|name| (*name == "airdock.exe", *name));
        for name in install_order {
            let relative = safe_relative(name)?;
            let target = app.join(relative);
            fs::create_dir_all(target.parent().context("Invalid destination path")?)?;
            fs::rename(stage.0.join(relative), &target)?;
            installed.push(name.clone());
        }
        verify_payload(&app, &plan.version)?;
        Ok(())
    })();
    if let Err(error) = result {
        let mut recovery = Vec::new();
        for name in installed.iter().rev() {
            if let Err(e) = fs::remove_file(app.join(name)) {
                recovery.push(e.to_string());
            }
        }
        for name in saved.iter().rev() {
            if let Err(e) = fs::rename(backup.join(name), app.join(name)) {
                recovery.push(e.to_string());
            }
        }
        if recovery.is_empty() {
            let _ = fs::remove_dir_all(&backup);
            return Err(error);
        }
        bail!(
            "Update failed: {error:#}. Backup retained at {}. Recovery errors: {}",
            backup.display(),
            recovery.join("; ")
        );
    }
    let _ = fs::remove_dir_all(backup);
    Ok(())
}
fn wait_for_parent(plan: &Plan) -> Result<fs::File> {
    ensure!(
        plan.parent_pid != 0 && plan.parent_pid != std::process::id(),
        "Invalid parent process"
    );
    #[cfg(windows)]
    {
        use windows::Win32::{
            Foundation::{CloseHandle, WAIT_OBJECT_0},
            System::Threading::{OpenProcess, PROCESS_ACCESS_RIGHTS, WaitForSingleObject},
        };
        // Synchronize-only access retains this process identity even if its PID
        // is subsequently reused. The receiver is never forcibly terminated.
        if let Ok(handle) =
            unsafe { OpenProcess(PROCESS_ACCESS_RIGHTS(0x00100000), false, plan.parent_pid) }
        {
            let outcome = unsafe { WaitForSingleObject(handle, 60000) };
            unsafe {
                CloseHandle(handle)?;
            }
            ensure!(
                outcome == WAIT_OBJECT_0,
                "Receiver did not shut down in time"
            );
        }
    }
    #[cfg(not(windows))]
    {
        let deadline = Instant::now() + Duration::from_secs(60);
        while unsafe { libc::kill(plan.parent_pid as i32, 0) } == 0 {
            ensure!(
                Instant::now() < deadline,
                "Receiver did not shut down in time"
            );
            std::thread::sleep(Duration::from_millis(50));
        }
    }
    // A second receiver using this configuration must not run during updates.
    let lock = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(plan.config_dir.join("receiver.lock"))?;
    fs2::FileExt::try_lock_exclusive(&lock).context("Receiver configuration is still in use")?;
    Ok(lock)
}
fn execute(plan: &Plan) -> Result<()> {
    ensure!(
        plan.schema == 1
            && plan.app_dir.is_absolute()
            && plan.package.is_absolute()
            && plan.config_dir.is_absolute(),
        "Invalid update plan"
    );
    validate_hash(&plan.sha256)?;
    ensure!(
        semver::Version::parse(&plan.version)?.pre.is_empty(),
        "Update is not a stable release"
    );
    ensure!(
        hash_file(&plan.package)? == plan.sha256,
        "Downloaded package changed before installation"
    );
    let current: Payload =
        serde_json::from_slice(&fs::read(plan.app_dir.join("PACKAGE_SHA256.json"))?)?;
    ensure!(
        semver::Version::parse(&plan.version)? > semver::Version::parse(&current.version)?,
        "Update would not advance the installed version"
    );
    let _lock = wait_for_parent(plan)?;
    match plan.kind {
        PackageKind::Portable => apply_portable(plan),
        PackageKind::Installed => {
            ensure!(cfg!(windows), "Installer updates require Windows");
            let status = std::process::Command::new(&plan.package)
                .args([
                    "/VERYSILENT",
                    "/SUPPRESSMSGBOXES",
                    "/NORESTART",
                    "/CLOSEAPPLICATIONS",
                ])
                .arg(format!("/DIR={}", installer_directory(&plan.app_dir)))
                .status()?;
            ensure!(status.success(), "Installer failed: {status}");
            verify_payload(&plan.app_dir, &plan.version)?;
            Ok(())
        }
    }
}
fn installer_directory(path: &Path) -> String {
    // Canonical Win32 paths use a verbatim prefix; Inno's directory argument
    // expects the ordinary drive/UNC spelling, retaining all Unicode text.
    let path = path.to_string_lossy();
    if let Some(unc) = path.strip_prefix("\\\\?\\UNC\\") {
        format!("\\\\{unc}")
    } else {
        path.strip_prefix("\\\\?\\").unwrap_or(&path).to_owned()
    }
}
pub fn run() -> Result<()> {
    let mut arguments = std::env::args_os().skip(1);
    ensure!(
        arguments.next().as_deref() == Some(std::ffi::OsStr::new("--plan")),
        "Expected --plan PATH"
    );
    let path = arguments.next().context("Missing update plan")?;
    ensure!(arguments.next().is_none(), "Unexpected updater arguments");
    let plan: Plan = serde_json::from_slice(&fs::read(path)?)?;
    let result = execute(&plan);
    let report = serde_json::json!({"version":plan.version,"success":result.is_ok(),"error":result.as_ref().err().map(|e|format!("{e:#}"))});
    let _ = fs::write(
        plan.config_dir.join("updates/result.json"),
        serde_json::to_vec(&report)?,
    );
    if result.is_ok()
        && let (Ok(package), Ok(cache)) = (
            plan.package.canonicalize(),
            plan.config_dir.join("updates").canonicalize(),
        )
        && package.starts_with(cache)
    {
        let _ = fs::remove_file(&plan.package);
    }
    // On failure restart the intact old application, which shows the report.
    if plan.app_dir.join("airdock.exe").is_file() {
        if plan.show_window {
            fs::write(plan.config_dir.join("restore-window"), b"show")?;
        }
        std::process::Command::new(plan.app_dir.join("airdock.exe"))
            .args(&plan.arguments)
            .current_dir(&plan.app_dir)
            .spawn()?;
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    fn fixture(root: &Path, version: &str, extra: bool) -> PathBuf {
        let package = root.join(format!("{version}.zip"));
        let mut files = std::collections::BTreeMap::new();
        let contents = [
            ("airdock.exe", version.as_bytes()),
            ("airdock-updater.exe", b"helper".as_slice()),
        ];
        let mut archive = zip::ZipWriter::new(fs::File::create(&package).unwrap());
        let options = zip::write::SimpleFileOptions::default();
        for (name, bytes) in contents {
            files.insert(name, hex::encode(Sha256::digest(bytes)));
            archive.start_file(name, options).unwrap();
            archive.write_all(bytes).unwrap();
        }
        archive.start_file("PACKAGE_SHA256.json", options).unwrap();
        archive
            .write_all(
                &serde_json::to_vec(
                    &serde_json::json!({"version":version,"commit":"a".repeat(40),"files":files}),
                )
                .unwrap(),
            )
            .unwrap();
        if extra {
            archive.start_file("unexpected.exe", options).unwrap();
            archive.write_all(b"extra").unwrap();
        }
        archive.finish().unwrap();
        package
    }
    #[test]
    fn portable_update_preserves_user_files_and_rejects_unlisted_payload() {
        let root = std::env::temp_dir().join(format!(
            "airdock-update-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir(&root).unwrap();
        let app = root.join("app");
        fs::create_dir(&app).unwrap();
        let first = fixture(&root, "0.1.0", false);
        extract(&first, &app).unwrap();
        fs::write(app.join("settings.json"), b"user settings").unwrap();
        fs::write(app.join("personal.txt"), b"user file").unwrap();
        let mut plan = Plan {
            schema: 1,
            package: fixture(&root, "0.2.0", false),
            kind: PackageKind::Portable,
            app_dir: app.clone(),
            config_dir: root.clone(),
            parent_pid: 1,
            version: "0.2.0".into(),
            sha256: String::new(),
            arguments: vec![],
            show_window: false,
        };
        apply_portable(&plan).unwrap();
        assert_eq!(fs::read(app.join("airdock.exe")).unwrap(), b"0.2.0");
        assert_eq!(
            fs::read(app.join("settings.json")).unwrap(),
            b"user settings"
        );
        assert_eq!(fs::read(app.join("personal.txt")).unwrap(), b"user file");
        plan.version = "0.3.0".into();
        plan.package = fixture(&root, "0.3.0", true);
        assert!(apply_portable(&plan).is_err());
        assert_eq!(fs::read(app.join("airdock.exe")).unwrap(), b"0.2.0");
        fs::remove_dir_all(root).unwrap();
    }
}
