// SPDX-License-Identifier: MPL-2.0
use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub schema_version: u32,
    pub audio_device: String,
    pub language: crate::i18n::Language,
    pub gpu_preference: String,
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
    pub window_width: u32,
    pub window_height: u32,
    pub hls_enabled: bool,
    pub automatic_updates: bool,
    pub update_mirrors_enabled: bool,
    pub update_mirrors: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WindowPreferences {
    pub width: u32,
    pub height: u32,
    pub fullscreen: bool,
    pub hide_ui: bool,
}
impl WindowPreferences {
    pub fn from_settings(s: &Settings) -> Self {
        Self {
            width: s.window_width,
            height: s.window_height,
            fullscreen: s.fullscreen,
            hide_ui: s.hide_ui,
        }
    }
    pub fn apply(&self, s: &mut Settings) {
        s.window_width = self.width;
        s.window_height = self.height;
        s.fullscreen = self.fullscreen;
        s.hide_ui = self.hide_ui;
    }
}

/// Clamp stored geometry against the logical desktop work area before opening.
pub fn restored_window_size(width: u32, height: u32, area: Option<(u32, u32)>) -> (u32, u32) {
    let (w, h) = area.unwrap_or((8192, 8192));
    (
        width.clamp(760.min(w), w.max(1)),
        height.clamp(520.min(h), h.max(1)),
    )
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            schema_version: 1,
            audio_device: "default".into(),
            language: crate::i18n::Language::Auto,
            gpu_preference: "balanced".into(),
            name: crate::brand::NAME.into(),
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
            window_width: 1120,
            window_height: 760,
            hls_enabled: false,
            automatic_updates: true,
            update_mirrors_enabled: true,
            update_mirrors: vec![crate::update::DEFAULT_MIRROR.into()],
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
        if !["balanced", "low-power", "high-performance"].contains(&self.gpu_preference.as_str()) {
            bail!("Invalid GPU preference")
        }
        if self.schema_version != 1 {
            bail!("Unsupported settings schema version")
        }
        anyhow::ensure!(
            self.update_mirrors.len() <= 4,
            "At most four update mirrors are supported"
        );
        for mirror in &self.update_mirrors {
            anyhow::ensure!(mirror.len() <= 2048, "Update mirror URL too long");
            crate::update::validate_mirror(mirror)?;
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
    directories::ProjectDirs::from("", "", "AirDock")
        .map(|p| p.config_dir().to_owned())
        .unwrap_or_else(|| PathBuf::from(".airdock"))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn fullscreen_geometry_is_clamped_to_logical_work_area() {
        assert_eq!(
            restored_window_size(1920, 1080, Some((1504, 800))),
            (1504, 800)
        );
        assert_eq!(
            restored_window_size(1120, 760, Some((1504, 800))),
            (1120, 760)
        );
    }
    #[test]
    fn settings_round_trip_and_read_preserve_the_file() {
        let path =
            std::env::temp_dir().join(format!("airdock-settings-{}.json", std::process::id()));
        let stored =
            br#"{"schema_version":1,"name":"My AirDock","language":"zh-CN","fullscreen":true}"#;
        std::fs::write(&path, stored).unwrap();
        let settings = Settings::load(&path).unwrap();
        assert_eq!(settings.schema_version, 1);
        assert_eq!(settings.name, "My AirDock");
        assert_eq!(settings.language, crate::i18n::Language::Chinese);
        assert!(settings.fullscreen);
        assert_eq!(std::fs::read(&path).unwrap(), stored);
        settings.save(&path).unwrap();
        assert_eq!(Settings::load(&path).unwrap(), settings);
        std::fs::remove_file(path).unwrap();
    }
    #[test]
    fn future_schema_is_not_silently_downgraded() {
        let settings: Settings = serde_json::from_str(r#"{"schema_version":99}"#).unwrap();
        assert!(settings.validate().is_err());
    }
}
