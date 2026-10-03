use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub name: String,
    pub mirror_width: u32,
    pub mirror_height: u32,
    pub max_fps: u32,
    pub refresh_rate: u32,
    pub hevc_enabled: bool,
    pub hardware_decode: bool,
    pub vsync: bool,
    pub fullscreen: bool,
    pub hide_ui: bool,
    pub close_to_tray: bool,
    pub minimize_to_tray: bool,
    pub start_hidden: bool,
    pub autostart: bool,
    pub recording_directory: PathBuf,
    pub recording_codec: String,
    pub recording_encoder: String,
    pub recording_mbps: u32,
    pub window_width: u32,
    pub window_height: u32,
    pub hls_enabled: bool,
}

impl Default for Settings {
    fn default() -> Self {
        let videos = directories::UserDirs::new()
            .and_then(|d| d.video_dir().map(Path::to_owned))
            .unwrap_or_else(|| PathBuf::from("recordings"));
        Self {
            name: "AirPlay-Windows".into(),
            mirror_width: 2560,
            mirror_height: 1440,
            max_fps: 60,
            refresh_rate: 60,
            hevc_enabled: true,
            hardware_decode: false,
            vsync: true,
            fullscreen: false,
            hide_ui: false,
            close_to_tray: true,
            minimize_to_tray: false,
            start_hidden: false,
            autostart: false,
            recording_directory: videos.join("AirPlay-Windows"),
            recording_codec: "h264".into(),
            recording_encoder: "auto".into(),
            recording_mbps: 12,
            window_width: 1120,
            window_height: 760,
            hls_enabled: false,
        }
    }
}

impl Settings {
    pub fn validate(&self) -> Result<()> {
        if self.name.trim().is_empty() || self.name.len() > 128 {
            bail!("Receiver name must be 1–128 bytes")
        }
        if !(320..=8192).contains(&self.mirror_width) || !(320..=8192).contains(&self.mirror_height)
        {
            bail!("Resolution must be between 320 and 8192 pixels")
        }
        if !(15..=240).contains(&self.max_fps) || !(15..=240).contains(&self.refresh_rate) {
            bail!("Frame rate must be 15–240")
        }
        if !(2..=40).contains(&self.recording_mbps) {
            bail!("Recording bitrate must be 2–40 Mbps")
        }
        if !["h264", "hevc"].contains(&self.recording_codec.as_str()) {
            bail!("Unsupported recording codec")
        }
        if !["auto", "gpu", "cpu"].contains(&self.recording_encoder.as_str()) {
            bail!("Unsupported encoder selection")
        }
        Ok(())
    }
    pub fn load(path: &Path) -> Result<Self> {
        if !path.exists() {
            return Ok(Self::default());
        }
        let settings: Self = serde_json::from_slice(&std::fs::read(path)?)
            .with_context(|| format!("Invalid settings: {}", path.display()))?;
        settings.validate()?;
        Ok(settings)
    }
    pub fn save(&self, path: &Path) -> Result<()> {
        self.validate()?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let temp = path.with_extension("json.tmp");
        {
            use std::io::Write;
            let mut file = std::fs::File::create(&temp)?;
            file.write_all(&serde_json::to_vec_pretty(self)?)?;
            file.sync_all()?;
        }
        // Windows rename cannot replace an existing file. MoveFileExW is atomic.
        #[cfg(windows)]
        {
            use std::os::windows::ffi::OsStrExt;
            use windows::Win32::Storage::FileSystem::{
                MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH, MoveFileExW,
            };
            use windows::core::PCWSTR;
            let from: Vec<u16> = temp.as_os_str().encode_wide().chain(Some(0)).collect();
            let to: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
            unsafe {
                MoveFileExW(
                    PCWSTR(from.as_ptr()),
                    PCWSTR(to.as_ptr()),
                    MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
                )?;
            }
        }
        #[cfg(not(windows))]
        std::fs::rename(temp, path)?;
        Ok(())
    }
}

pub fn data_dir() -> PathBuf {
    directories::ProjectDirs::from("", "", "AirPlay-Windows")
        .map(|p| p.config_dir().to_owned())
        .unwrap_or_else(|| PathBuf::from(".airplay-windows"))
}
