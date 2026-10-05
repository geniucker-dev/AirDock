// SPDX-License-Identifier: MPL-2.0
//! Release discovery and cancellable downloads run outside presentation/media callbacks.
pub mod install;
pub mod manifest;

use anyhow::{Context, Result, bail, ensure};
use manifest::{PackageKind, Plan};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    io::Write,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

pub const REPOSITORY: &str = "geniucker-dev/AirDock";
const RELEASE_API: &str = "https://api.github.com/repos/geniucker-dev/AirDock/releases/latest";
pub const DEFAULT_MIRROR: &str = "https://gh-proxy.com";
pub const INSTALL_SUPPORTED: bool = cfg!(all(windows, target_arch = "x86_64"));
const MAX_ASSET: u64 = 1024 * 1024 * 1024;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Asset {
    pub name: String,
    pub url: String,
    pub size: u64,
    pub sha256: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Release {
    pub version: String,
    pub page: String,
    pub installer: Asset,
    pub portable: Asset,
}
#[derive(Deserialize)]
struct GithubRelease {
    tag_name: String,
    draft: bool,
    prerelease: bool,
    assets: Vec<GithubAsset>,
}
#[derive(Deserialize)]
struct GithubAsset {
    name: String,
    size: u64,
    browser_download_url: String,
    digest: Option<String>,
}
#[derive(Clone, Debug)]
pub struct Downloaded {
    pub release: Release,
    pub path: PathBuf,
    pub kind: PackageKind,
}
#[derive(Default, Clone, Debug)]
pub enum Status {
    #[default]
    Idle,
    Checking,
    Current,
    Available(Release),
    Downloading(Release),
    Ready(Downloaded),
    Preparing,
    Failed(String),
}
#[derive(Serialize, Deserialize)]
struct Cache {
    checked_at: u64,
    release: Option<Release>,
}
pub struct State {
    pub status: Status,
    pub progress: Arc<AtomicU64>,
    pub total_size: u64,
    pub confirm: bool,
    pub operation: u64,
    pub abort: Option<futures::future::AbortHandle>,
    attempted: Option<Instant>,
    checked_at: u64,
}
impl State {
    pub fn new(directory: &Path) -> Self {
        let cache = std::fs::read(directory.join("updates/check.json"))
            .ok()
            .and_then(|b| serde_json::from_slice::<Cache>(&b).ok());
        let mut status = cache
            .as_ref()
            .and_then(|c| c.release.clone())
            .filter(|r| newer(&r.version, env!("CARGO_PKG_VERSION")).unwrap_or(false))
            .map(Status::Available)
            .unwrap_or_else(|| {
                if cache.is_some() {
                    Status::Current
                } else {
                    Status::Idle
                }
            });
        let report_path = directory.join("updates/result.json");
        if let Ok(bytes) = std::fs::read(&report_path) {
            if let Ok(report) = serde_json::from_slice::<serde_json::Value>(&bytes)
                && report["success"] == false
            {
                status = Status::Failed(report["error"].as_str().unwrap_or("Update failed").into());
            }
            let _ = std::fs::remove_file(report_path);
        }
        let attempted = matches!(status, Status::Failed(_)).then(Instant::now);
        Self {
            status,
            progress: Arc::default(),
            total_size: 0,
            confirm: false,
            operation: 0,
            abort: None,
            attempted,
            checked_at: cache.map_or(0, |c| c.checked_at),
        }
    }
    pub fn due(&self) -> bool {
        !matches!(
            self.status,
            Status::Checking | Status::Downloading(_) | Status::Ready(_) | Status::Preparing
        ) && self
            .attempted
            .is_none_or(|t| t.elapsed() >= Duration::from_secs(3600))
            && (self.checked_at > now() || now().saturating_sub(self.checked_at) >= 86400)
    }
    pub fn begin(&mut self) -> (u64, futures::future::AbortRegistration) {
        self.cancel();
        self.attempted = Some(Instant::now());
        self.progress.store(0, Ordering::Relaxed);
        let (handle, registration) = futures::future::AbortHandle::new_pair();
        self.abort = Some(handle);
        (self.operation, registration)
    }
    pub fn cancel(&mut self) {
        if let Some(handle) = self.abort.take() {
            handle.abort();
        }
        self.operation = self.operation.wrapping_add(1);
    }
    pub fn checked(&mut self) {
        self.checked_at = now();
        self.abort = None;
    }
}
impl Drop for State {
    fn drop(&mut self) {
        self.cancel();
    }
}
fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}
fn client() -> Result<Client> {
    Ok(Client::builder()
        .https_only(true)
        .user_agent(concat!("AirDock/", env!("CARGO_PKG_VERSION")))
        .connect_timeout(Duration::from_secs(4))
        .read_timeout(Duration::from_secs(5))
        .build()?)
}
pub fn newer(release: &str, current: &str) -> Result<bool> {
    let target = semver::Version::parse(release.trim_start_matches('v'))?;
    let current = semver::Version::parse(current)?;
    Ok(target.pre.is_empty() && target > current)
}
fn parse_release(bytes: &[u8], current: &str) -> Result<Option<Release>> {
    let release: GithubRelease = serde_json::from_slice(bytes)?;
    if release.draft || release.prerelease || !newer(&release.tag_name, current)? {
        return Ok(None);
    }
    let version = semver::Version::parse(release.tag_name.trim_start_matches('v'))?.to_string();
    ensure!(
        release.tag_name == format!("v{version}"),
        "Unexpected release tag format"
    );
    let prefix = format!("airdock-{version}-");
    let find = |suffix: &str| -> Result<Asset> {
        let assets: Vec<_> = release
            .assets
            .iter()
            .filter(|a| a.name.starts_with(&prefix) && a.name.ends_with(suffix))
            .collect();
        ensure!(
            assets.len() == 1,
            "Release must contain exactly one matching Windows x64 package"
        );
        let asset = assets[0];
        let url = format!(
            "https://github.com/{REPOSITORY}/releases/download/{}/{}",
            release.tag_name, asset.name
        );
        ensure!(
            asset.browser_download_url == url,
            "Unexpected release download URL"
        );
        ensure!(
            asset.size > 0 && asset.size <= MAX_ASSET,
            "Invalid update download size"
        );
        manifest::safe_relative(&asset.name)?;
        let digest = asset
            .digest
            .as_deref()
            .and_then(|d| d.strip_prefix("sha256:"))
            .context("GitHub did not provide the package SHA-256")?;
        manifest::validate_hash(digest)?;
        Ok(Asset {
            name: asset.name.clone(),
            url,
            size: asset.size,
            sha256: digest.into(),
        })
    };
    Ok(Some(Release {
        version,
        page: format!(
            "https://github.com/{REPOSITORY}/releases/tag/{}",
            release.tag_name
        ),
        installer: find("-windows-x64-setup.exe")?,
        portable: find("-windows-x64-portable.zip")?,
    }))
}
pub async fn check(directory: PathBuf) -> Result<Option<Release>> {
    let response = client()?
        .get(RELEASE_API)
        .header("Accept", "application/vnd.github+json")
        .header("X-GitHub-Api-Version", "2022-11-28")
        .timeout(Duration::from_secs(15))
        .send()
        .await?;
    ensure!(
        response.url().host_str() == Some("api.github.com"),
        "Release metadata left the official GitHub API"
    );
    let result = if response.status() == reqwest::StatusCode::NOT_FOUND {
        None
    } else {
        let mut response = response.error_for_status()?;
        let mut body = Vec::new();
        while let Some(chunk) = response.chunk().await? {
            ensure!(
                body.len() + chunk.len() <= 1024 * 1024,
                "Release metadata too large"
            );
            body.extend_from_slice(&chunk);
        }
        parse_release(&body, env!("CARGO_PKG_VERSION"))?
    };
    std::fs::create_dir_all(directory.join("updates"))?;
    let cache = serde_json::to_vec(&Cache {
        checked_at: now(),
        release: result.clone(),
    })?;
    // This cache carries no receiver configuration and never overwrites settings.
    std::fs::write(directory.join("updates/check.json"), cache)?;
    Ok(result)
}
fn asset(release: &Release, kind: PackageKind) -> &Asset {
    match kind {
        PackageKind::Installed => &release.installer,
        PackageKind::Portable => &release.portable,
    }
}
pub fn package_kind() -> Result<PackageKind> {
    let app = std::env::current_exe()?
        .parent()
        .context("Application directory unavailable")?
        .to_path_buf();
    Ok(if app.join("unins000.exe").is_file() {
        PackageKind::Installed
    } else {
        PackageKind::Portable
    })
}
struct Partial(PathBuf);
impl Drop for Partial {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}
async fn download_one(
    client: &Client,
    url: &str,
    asset: &Asset,
    partial: &Path,
    progress: &AtomicU64,
) -> Result<()> {
    let mut response = client
        .get(url)
        .timeout(Duration::from_secs(600))
        .send()
        .await?
        .error_for_status()?;
    let mut file = std::fs::File::create(partial)?;
    let mut hash = Sha256::new();
    let mut received = 0u64;
    while let Some(chunk) = response.chunk().await? {
        received += chunk.len() as u64;
        ensure!(
            received <= asset.size,
            "Update exceeded GitHub's declared size"
        );
        file.write_all(&chunk)?;
        hash.update(&chunk);
        progress.store(received, Ordering::Relaxed);
    }
    ensure!(received == asset.size, "Incomplete update download");
    ensure!(
        hex::encode(hash.finalize()) == asset.sha256,
        "Update SHA-256 does not match GitHub"
    );
    file.sync_all()?;
    Ok(())
}
async fn probe(client: &Client, url: String) -> Result<(String, f64)> {
    let start = Instant::now();
    let mut response = client
        .get(&url)
        .header("Range", "bytes=0-65535")
        .timeout(Duration::from_secs(4))
        .send()
        .await?
        .error_for_status()?;
    let mut bytes = 0;
    while let Some(chunk) = response.chunk().await? {
        bytes += chunk.len().min(65536 - bytes);
        if bytes >= 65536 {
            break;
        }
    }
    ensure!(bytes > 0, "Mirror returned no data");
    Ok((url, bytes as f64 / start.elapsed().as_secs_f64().max(0.001)))
}
pub async fn download(
    release: Release,
    kind: PackageKind,
    directory: PathBuf,
    mirrors: Vec<String>,
    progress: Arc<AtomicU64>,
) -> Result<Downloaded> {
    let asset = asset(&release, kind);
    let expected = format!(
        "https://github.com/{REPOSITORY}/releases/download/v{}/{}",
        release.version, asset.name
    );
    ensure!(asset.url == expected, "Unexpected update source");
    manifest::safe_relative(&asset.name)?;
    manifest::validate_hash(&asset.sha256)?;
    ensure!(
        asset.size > 0 && asset.size <= MAX_ASSET,
        "Invalid update size"
    );
    let destination = directory.join("updates").join(&release.version);
    std::fs::create_dir_all(&destination)?;
    let path = destination.join(&asset.name);
    // A cancelled future may be dropped after its replacement has started.
    // Give each transfer its own scratch path so an old guard cannot remove
    // or truncate the replacement download on a rapid cancel/retry.
    let nonce = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos();
    let partial = Partial(path.with_extension(format!("part-{}-{nonce}", std::process::id())));
    let client = client()?;
    let direct = download_one(&client, &asset.url, asset, &partial.0, &progress).await;
    let mut errors = Vec::new();
    if let Err(error) = direct {
        errors.push(format!("GitHub: {error:#}"));
        let probes = mirrors.into_iter().take(4).map(|base| {
            let client = &client;
            let source = &asset.url;
            async move {
                validate_mirror(&base)?;
                probe(client, format!("{}/{source}", base.trim_end_matches('/'))).await
            }
        });
        let mut routes = Vec::new();
        for result in futures::future::join_all(probes).await {
            match result {
                Ok(route) => routes.push(route),
                Err(e) => errors.push(format!("Mirror: {e:#}")),
            }
        }
        routes.sort_by(|a, b| b.1.total_cmp(&a.1));
        let mut completed = false;
        for (url, _) in routes {
            progress.store(0, Ordering::Relaxed);
            match download_one(&client, &url, asset, &partial.0, &progress).await {
                Ok(()) => {
                    completed = true;
                    break;
                }
                Err(e) => errors.push(format!("Mirror download: {e:#}")),
            }
        }
        if !completed {
            bail!("{}", errors.join("\n"));
        }
    }
    // Windows rename cannot overwrite an already cached version.
    if path.exists() {
        std::fs::remove_file(&path)?;
    }
    std::fs::rename(&partial.0, &path)?;
    Ok(Downloaded {
        release,
        path,
        kind,
    })
}
pub fn validate_mirror(base: &str) -> Result<()> {
    let url = url::Url::parse(base)?;
    ensure!(
        url.scheme() == "https"
            && url.host_str().is_some()
            && url.username().is_empty()
            && url.password().is_none()
            && url.query().is_none()
            && url.fragment().is_none(),
        "Mirror must be an HTTPS URL without credentials, query or fragment"
    );
    Ok(())
}
pub fn prepare_install(
    download: &Downloaded,
    directory: &Path,
    arguments: Vec<String>,
    show_window: bool,
) -> Result<()> {
    ensure!(
        INSTALL_SUPPORTED,
        "In-app installation currently supports Windows x64"
    );
    let app = std::env::current_exe()?
        .parent()
        .context("Application directory unavailable")?
        .canonicalize()?;
    let stage = directory
        .join("updates")
        .join(format!("helper-{}-{}", std::process::id(), now()));
    std::fs::create_dir_all(&stage)?;
    let runtime: Vec<String> =
        serde_json::from_slice(&std::fs::read(app.join("UPDATER_RUNTIME.json"))?)?;
    ensure!(
        runtime.len() <= 32 && runtime.iter().any(|s| s == "airdock-updater.exe"),
        "Invalid updater runtime manifest"
    );
    for name in runtime {
        let relative = manifest::safe_relative(&name)?;
        ensure!(
            relative.components().count() == 1
                && (name.ends_with(".dll") || name == "airdock-updater.exe"),
            "Invalid updater runtime file"
        );
        std::fs::copy(app.join(relative), stage.join(relative))?;
    }
    let plan = Plan {
        schema: 1,
        package: download.path.canonicalize()?,
        kind: download.kind,
        app_dir: app,
        config_dir: directory.canonicalize()?,
        parent_pid: std::process::id(),
        version: download.release.version.clone(),
        sha256: asset(&download.release, download.kind).sha256.clone(),
        arguments,
        show_window,
    };
    let path = stage.join("plan.json");
    std::fs::write(&path, serde_json::to_vec(&plan)?)?;
    std::process::Command::new(stage.join("airdock-updater.exe"))
        .arg("--plan")
        .arg(path)
        .spawn()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn install_failure_is_not_immediately_hidden_by_a_background_check() {
        let root =
            std::env::temp_dir().join(format!("airdock-update-receipt-{}", std::process::id()));
        std::fs::create_dir_all(root.join("updates")).unwrap();
        std::fs::write(
            root.join("updates/check.json"),
            br#"{"checked_at":0,"release":null}"#,
        )
        .unwrap();
        std::fs::write(
            root.join("updates/result.json"),
            br#"{"success":false,"error":"rollback fixture"}"#,
        )
        .unwrap();
        let state = State::new(&root);
        assert!(matches!(&state.status, Status::Failed(error) if error == "rollback fixture"));
        assert!(!state.due());
        assert!(!root.join("updates/result.json").exists());
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn semantic_versions_reject_downgrades_and_prereleases() {
        assert!(newer("v0.10.0", "0.9.0").unwrap());
        assert!(!newer("v0.1.0", "0.1.0").unwrap());
        assert!(!newer("0.1.0", "0.2.0").unwrap());
        assert!(!newer("v1.0.0-beta.1", "0.1.0").unwrap());
        assert!(newer("../../bad", "0.1.0").is_err());
    }
    #[test]
    fn official_metadata_requires_matching_packages_and_hashes() {
        let assets = ["setup.exe", "portable.zip"].map(|suffix| {
            let name = format!("airdock-0.2.0-abcdef0-windows-x64-{suffix}");
            serde_json::json!({"name":name,"size":123,"browser_download_url":format!("https://github.com/{REPOSITORY}/releases/download/v0.2.0/{name}"),"digest":format!("sha256:{}","a".repeat(64))})
        });
        let mut value = serde_json::json!({"tag_name":"v0.2.0","draft":false,"prerelease":false,"assets":assets});
        assert!(
            parse_release(&serde_json::to_vec(&value).unwrap(), "0.1.0")
                .unwrap()
                .is_some()
        );
        value["assets"][0]["digest"] = serde_json::Value::Null;
        assert!(parse_release(&serde_json::to_vec(&value).unwrap(), "0.1.0").is_err());
        value["prerelease"] = true.into();
        assert!(
            parse_release(&serde_json::to_vec(&value).unwrap(), "0.1.0")
                .unwrap()
                .is_none()
        );
    }
    fn http_fixture(bytes: Vec<u8>, delay: Duration) -> (String, std::thread::JoinHandle<()>) {
        use std::io::Read;
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/file", listener.local_addr().unwrap());
        let worker = std::thread::spawn(move || {
            let (mut socket, _) = listener.accept().unwrap();
            let mut request = [0u8; 4096];
            let _ = socket.read(&mut request).unwrap();
            std::thread::sleep(delay);
            let header = format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                bytes.len()
            );
            socket.write_all(header.as_bytes()).unwrap();
            let _ = socket.write_all(&bytes);
        });
        (url, worker)
    }
    #[test]
    fn downloads_verify_actual_bytes_and_probe_measures_available_routes() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        runtime.block_on(async {
            let client = Client::builder()
                .timeout(Duration::from_secs(2))
                .build()
                .unwrap();
            let bytes = vec![42u8; 65536];
            let (url, server) = http_fixture(bytes.clone(), Duration::ZERO);
            let partial = Partial(
                std::env::temp_dir().join(format!("airdock-download-{}.part", std::process::id())),
            );
            let mut asset = Asset {
                name: "package.zip".into(),
                url: url.clone(),
                size: bytes.len() as u64,
                sha256: hex::encode(Sha256::digest(&bytes)),
            };
            let progress = AtomicU64::new(0);
            download_one(&client, &url, &asset, &partial.0, &progress)
                .await
                .unwrap();
            server.join().unwrap();
            assert_eq!(std::fs::read(&partial.0).unwrap(), bytes);
            assert_eq!(progress.load(Ordering::Relaxed), asset.size);
            let (url, server) = http_fixture(bytes.clone(), Duration::ZERO);
            asset.sha256 = "0".repeat(64);
            assert!(
                download_one(&client, &url, &asset, &partial.0, &progress)
                    .await
                    .is_err()
            );
            server.join().unwrap();
            let (fast, a) = http_fixture(bytes.clone(), Duration::ZERO);
            let (slow, b) = http_fixture(bytes, Duration::from_millis(150));
            let routes =
                futures::future::join_all([probe(&client, fast), probe(&client, slow)]).await;
            assert!(routes[0].as_ref().unwrap().1 > routes[1].as_ref().unwrap().1);
            a.join().unwrap();
            b.join().unwrap();
            let path = partial.0.clone();
            drop(partial);
            assert!(!path.exists());
        });
    }
}
