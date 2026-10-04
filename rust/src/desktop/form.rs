//! Settings drafts never mutate live receiver or independently saved window preferences.
use crate::config::{Settings, WindowPreferences};

#[derive(Default)]
pub struct Errors {
    pub name: Option<String>,
    pub width: Option<String>,
    pub height: Option<String>,
    pub fps: Option<String>,
}
pub struct Form {
    pub draft: Settings,
    pub saved: Settings,
    pub width: String,
    pub height: String,
    pub fps: String,
    pub errors: Errors,
    pub feedback: String,
    pub audio_pending: bool,
    revision: u64,
    pub pending: Option<u64>,
}
impl Form {
    pub fn new(settings: Settings) -> Self {
        Self {
            width: settings.mirror_width.to_string(),
            height: settings.mirror_height.to_string(),
            fps: settings.max_fps.to_string(),
            draft: settings.clone(),
            saved: settings,
            errors: Errors::default(),
            feedback: String::new(),
            audio_pending: false,
            revision: 0,
            pending: None,
        }
    }
    pub fn changed(&mut self) {
        self.revision += 1;
        self.feedback.clear();
        self.errors = Errors::default();
    }
    pub fn dirty(&self) -> bool {
        let mut draft = self.draft.clone();
        WindowPreferences::from_settings(&self.saved).apply(&mut draft);
        draft.audio_device.clone_from(&self.saved.audio_device);
        draft.mirror_width = self.saved.mirror_width;
        draft.mirror_height = self.saved.mirror_height;
        draft.max_fps = self.saved.max_fps;
        draft.refresh_rate = self.saved.refresh_rate;
        draft != self.saved
            || self.width != self.saved.mirror_width.to_string()
            || self.height != self.saved.mirror_height.to_string()
            || self.fps != self.saved.max_fps.to_string()
    }
    pub fn validate(&mut self) -> Result<Settings, String> {
        let mut candidate = self.draft.clone();
        self.errors = Errors::default();
        if candidate.name.trim().is_empty() || candidate.name.len() > 128 {
            self.errors.name = Some("Enter a name of 1–128 bytes".into());
        }
        for (value, error, target, label, range) in [
            (
                &self.width,
                &mut self.errors.width,
                &mut candidate.mirror_width,
                "Width",
                320..=8192,
            ),
            (
                &self.height,
                &mut self.errors.height,
                &mut candidate.mirror_height,
                "Height",
                320..=8192,
            ),
            (
                &self.fps,
                &mut self.errors.fps,
                &mut candidate.max_fps,
                "Frame rate",
                15..=240,
            ),
        ] {
            match value.parse::<u32>() {
                Ok(n) if range.contains(&n) => *target = n,
                _ => {
                    *error = Some(format!(
                        "{label}: enter a whole number from {} to {}",
                        range.start(),
                        range.end()
                    ))
                }
            }
        }
        if self.errors.name.is_some()
            || self.errors.width.is_some()
            || self.errors.height.is_some()
            || self.errors.fps.is_some()
        {
            return Err("Check the highlighted fields".into());
        }
        candidate.refresh_rate = candidate.max_fps;
        candidate.validate().map_err(|e| e.to_string())?;
        self.pending = Some(self.revision);
        Ok(candidate)
    }
    pub fn committed(&mut self, result: Result<Settings, String>) {
        let revision = self.pending.take();
        match result {
            Ok(settings) => {
                self.saved = settings;
                if revision == Some(self.revision) {
                    self.draft = self.saved.clone();
                    self.width = self.saved.mirror_width.to_string();
                    self.height = self.saved.mirror_height.to_string();
                    self.fps = self.saved.max_fps.to_string();
                    self.feedback = "Settings saved".into();
                } else {
                    self.feedback = "Saved; newer edits are still unsaved".into();
                }
            }
            Err(error) => self.feedback = format!("Could not save: {error}"),
        }
    }
    pub fn reset(&mut self) {
        self.draft = Settings::default();
        WindowPreferences::from_settings(&self.saved).apply(&mut self.draft);
        self.draft.audio_device.clone_from(&self.saved.audio_device);
        self.width = self.draft.mirror_width.to_string();
        self.height = self.draft.mirror_height.to_string();
        self.fps = self.draft.max_fps.to_string();
        self.changed();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn invalid_form_does_not_partially_apply_and_pending_save_keeps_newer_edits() {
        let mut f = Form::new(Settings::default());
        f.width = "1920".into();
        f.height = "oops".into();
        f.changed();
        assert!(f.validate().is_err());
        assert!(f.errors.height.is_some());
        assert_eq!(f.saved.mirror_width, 2560);
        f.height = "1080".into();
        f.changed();
        let committed = f.validate().unwrap();
        f.draft.name = "New edit during save".into();
        f.changed();
        f.committed(Ok(committed));
        assert_eq!(f.saved.mirror_width, 1920);
        assert_eq!(f.draft.name, "New edit during save");
        assert!(f.dirty());
    }
    #[test]
    fn reset_is_a_draft_and_preserves_live_audio_and_window_preferences() {
        let initial = Settings {
            name: "Custom".into(),
            audio_device: "Speakers".into(),
            window_width: 960,
            ..Settings::default()
        };
        let mut f = Form::new(initial.clone());
        f.reset();
        assert_eq!(f.saved, initial);
        assert_eq!(f.draft.window_width, 960);
        assert_eq!(f.draft.audio_device, "Speakers");
        assert!(f.dirty());
    }
}
