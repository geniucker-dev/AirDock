// SPDX-License-Identifier: MPL-2.0
use super::{App, Message, Task, window};
use crate::update::{self, Status};
use futures::future::Abortable;
use std::sync::atomic::Ordering;

impl App {
    pub(super) fn check_updates(&mut self) -> Task<Message> {
        if matches!(
            self.updates.status,
            Status::Downloading(_) | Status::Ready(_) | Status::Preparing
        ) {
            return Task::none();
        }
        let (operation, cancellation) = self.updates.begin();
        self.updates.status = Status::Checking;
        let directory = self.directory.clone();
        Task::perform(
            async move {
                Abortable::new(update::check(directory), cancellation)
                    .await
                    .ok()
                    .map(|r| r.map_err(|e| format!("{e:#}")))
            },
            move |result| Message::UpdateChecked(operation, result),
        )
    }
    pub(super) fn show_updates(&mut self) -> Task<Message> {
        self.connection_reveal.keep_open();
        self.page = 1;
        let mut tasks = Vec::new();
        if self.fullscreen {
            self.fullscreen = false;
            self.window_mode_revision += 1;
            self.preferences.fullscreen = false;
            self.preferences_dirty = Some(std::time::Instant::now());
            tasks.push(self.apply_window_mode());
            tasks.push(self.save_preferences());
        }
        self.client
            .shared
            .media
            .display
            .visible
            .store(false, Ordering::Release);
        tasks.push(self.open_window());
        if let Some(id) = self.window {
            tasks.push(window::gain_focus(id));
        }
        if matches!(self.updates.status, Status::Idle | Status::Current) {
            tasks.push(self.check_updates());
        }
        Task::batch(tasks).chain(iced::widget::operation::snap_to_end("settings-form"))
    }
    pub(super) fn download_update(&mut self) -> Task<Message> {
        self.updates.confirm = false;
        if let Status::Ready(_) = &self.updates.status {
            return self.install_update();
        }
        let Status::Available(release) = self.updates.status.clone() else {
            return Task::none();
        };
        let kind = match update::package_kind() {
            Ok(kind) => kind,
            Err(error) => {
                self.updates.status = Status::Failed(format!("{error:#}"));
                return Task::none();
            }
        };
        let (operation, cancellation) = self.updates.begin();
        self.updates.status = Status::Downloading(release.clone());
        self.updates.total_size = match kind {
            update::manifest::PackageKind::Installed => release.installer.size,
            update::manifest::PackageKind::Portable => release.portable.size,
        };
        let mirrors = if self.form.saved.update_mirrors_enabled {
            self.form.saved.update_mirrors.clone()
        } else {
            vec![]
        };
        let directory = self.directory.clone();
        let progress = self.updates.progress.clone();
        Task::perform(
            async move {
                Abortable::new(
                    update::download(release, kind, directory, mirrors, progress),
                    cancellation,
                )
                .await
                .ok()
                .map(|r| r.map_err(|e| format!("{e:#}")))
            },
            move |result| Message::UpdateDownloaded(operation, result),
        )
    }
    pub(super) fn install_update(&mut self) -> Task<Message> {
        let Status::Ready(download) = &self.updates.status else {
            return Task::none();
        };
        let download = download.clone();
        let arguments = std::env::args().skip(1).collect();
        let directory = self.directory.clone();
        let operation = self.updates.operation;
        let show_window = self.window.is_some() && !self.minimized;
        self.updates.status = Status::Preparing;
        Task::perform(
            async move {
                tokio::task::spawn_blocking(move || {
                    update::prepare_install(&download, &directory, arguments, show_window)
                })
                .await
                .map_err(|e| e.to_string())
                .and_then(|r| r.map_err(|e| format!("{e:#}")))
            },
            move |result| Message::UpdatePrepared(operation, result),
        )
    }
}
