// SPDX-License-Identifier: MPL-2.0
//! Desktop view composition. Media frames are selected by the video primitive.
use super::appearance as look;
use super::*;
use crate::i18n::Language;
use iced::widget::{column, text as raw_text};
use std::borrow::Cow;

#[derive(Default)]
pub(super) struct Stats {
    pub fps: f64,
    pub processing_ms: Option<f64>,
    pub mbps: f64,
    pub dropped: u64,
    pub video: String,
    pub timing: String,
    pub audio: String,
    pub network: String,
}

impl App {
    pub(super) fn view(&self, id: window::Id) -> Element<'_, Message, Theme, Renderer> {
        let content = self.view_content(id);
        if !self.updates.confirm {
            return content;
        }
        let lang = self.language();
        let panel = container(column![
            text(lang, "Install update?").size(22).font(look::strong(lang)),
            text(lang, "After downloading, AirDock will stop receiving, install the update and restart. Your configuration will be preserved.").size(14),
            row![
                button(text(lang, "Download and install").size(14)).padding([10,16]).style(look::primary).on_press(Message::UpdateConfirm),
                button(text(lang, "Cancel").size(14)).padding([10,16]).style(look::secondary).on_press(Message::UpdateCancel)
            ].spacing(12).align_y(iced::Alignment::Center)
        ].spacing(20)).padding(28).max_width(540).style(look::panel);
        widget::stack![
            content,
            widget::opaque(
                container(panel)
                    .center_x(Length::Fill)
                    .center_y(Length::Fill)
                    .style(|_| container::Style {
                        background: Some(iced::Color::from_rgba(0., 0., 0., 0.45).into()),
                        ..Default::default()
                    })
            )
        ]
        .into()
    }
    fn view_content(&self, _: window::Id) -> Element<'_, Message, Theme, Renderer> {
        let lang = self.language();
        if self.fullscreen || (self.focus && self.page == 0 && self.status.error.is_empty()) {
            let player = widget::mouse_area(
                container(self.video_view())
                    .width(Length::Fill)
                    .height(Length::Fill)
                    .style(|_| container::Style {
                        background: Some(iced::Color::BLACK.into()),
                        ..Default::default()
                    }),
            )
            .on_double_click(Message::Fullscreen);
            let mut layers = widget::stack![player];
            if self.controls_visible {
                let controls = widget::mouse_area(
                    container(self.player_controls(true))
                        .width(Length::Fill)
                        .max_width(960.)
                        .height(if self.viewport.width < 740. {
                            112.
                        } else {
                            64.
                        })
                        .padding(14)
                        .style(look::overlay),
                )
                .on_enter(Message::ControlsHovered(true))
                .on_exit(Message::ControlsHovered(false));
                layers = layers.push(
                    container(column![
                        widget::space().height(Length::Fill),
                        container(controls).center_x(Length::Fill)
                    ])
                    .padding(16)
                    .height(Length::Fill)
                    .width(Length::Fill),
                );
            }
            if !self.status.error.is_empty() {
                layers = layers.push(self.error_banner());
            }
            return widget::mouse_area(layers)
                .on_move(|_| Message::Pointer)
                .interaction(if self.controls_visible {
                    iced::mouse::Interaction::Idle
                } else {
                    iced::mouse::Interaction::Hidden
                })
                .into();
        }
        let content: Element<'_, Message, Theme, Renderer> = if self.page == 1 {
            self.settings_view()
        } else {
            // Cache UI composition, not media. prepare() reads the newest frame.
            container(widget::lazy(
                (
                    self.ui_revision,
                    lang,
                    self.client.shared.generation(),
                    self.frame.is_some(),
                    self.sampled_at,
                    self.viewport.width.to_bits(),
                    self.viewport.height.to_bits(),
                    self.crop,
                    self.diagnostics_expanded,
                    self.status.volume_db.to_bits(),
                    &self.form.saved.name,
                ),
                |_| self.receive_view(),
            ))
            .width(Length::Fill)
            .height(Length::Fill)
            .into()
        };
        let mut body = column![self.topbar(), widget::rule::horizontal(1), content]
            .height(Length::Fill)
            .width(Length::Fill);
        if !self.status.error.is_empty() {
            body = body.push(self.error_banner());
        }
        if matches!(
            self.updates.status,
            crate::update::Status::Available(_) | crate::update::Status::Failed(_)
        ) {
            body = body.push(
                container(
                    row![
                        text(
                            lang,
                            if matches!(self.updates.status, crate::update::Status::Failed(_)) {
                                "Update failed"
                            } else {
                                "An AirDock update is available."
                            }
                        )
                        .size(12),
                        widget::space().width(Length::Fill),
                        button(text(lang, "View update").size(12))
                            .style(look::secondary)
                            .on_press(Message::UpdateOpen)
                    ]
                    .align_y(iced::Alignment::Center),
                )
                .padding([8, 24]),
            );
        }
        body.into()
    }
    fn topbar(&self) -> Element<'_, Message, Theme, Renderer> {
        let lang = self.language();
        let selected = self.page;
        let brand = row![
            widget::image(crate::brand::image_handle())
                .width(28)
                .height(28),
            text(lang, crate::brand::NAME)
                .font(look::strong(lang))
                .size(18)
        ]
        .spacing(12)
        .align_y(iced::Alignment::Center)
        .width(210);
        let navigation = row![
            button(
                text(lang, "Receive")
                    .size(14)
                    .font(look::strong(lang))
                    .width(Length::Fill)
                    .height(Length::Fill)
                    .align_x(iced::alignment::Horizontal::Center)
                    .align_y(iced::alignment::Vertical::Center)
            )
            .width(96)
            .height(36)
            .padding([9, 12])
            .style(move |_, status| look::nav(selected == 0, status))
            .on_press(Message::Page(0)),
            button(
                text(lang, "Settings")
                    .size(14)
                    .font(look::strong(lang))
                    .width(Length::Fill)
                    .height(Length::Fill)
                    .align_x(iced::alignment::Horizontal::Center)
                    .align_y(iced::alignment::Vertical::Center)
            )
            .width(96)
            .height(36)
            .padding([9, 12])
            .style(move |_, status| look::nav(selected == 1, status))
            .on_press(Message::Page(1))
        ]
        .spacing(8);
        let indicator = container(widget::space().width(7).height(7)).style(|_| container::Style {
            background: Some(iced::Color::from_rgb8(52, 126, 112).into()),
            border: iced::Border {
                radius: 4.into(),
                ..Default::default()
            },
            ..Default::default()
        });
        let connection = if self.status.peer.is_empty() {
            "Ready"
        } else if self.status.kind == "Audio" {
            "Audio"
        } else {
            "Connected"
        };
        let service = row![
            indicator,
            text(lang, connection).size(12).color(look::MUTED)
        ]
        .spacing(7)
        .align_y(iced::Alignment::Center);
        container(
            row![
                brand,
                navigation,
                widget::space().width(Length::Fill),
                service,
                button(
                    text(lang, "Hide")
                        .size(13)
                        .width(Length::Fill)
                        .align_x(iced::alignment::Horizontal::Center)
                )
                .width(58)
                .padding([9, 10])
                .style(look::secondary)
                .on_press_maybe(self.tray_available().then_some(Message::Hide)),
                button(
                    text(lang, "Quit")
                        .size(13)
                        .width(Length::Fill)
                        .align_x(iced::alignment::Horizontal::Center)
                )
                .width(50)
                .padding([9, 10])
                .style(look::secondary)
                .on_press(Message::Quit)
            ]
            .spacing(16)
            .align_y(iced::Alignment::Center),
        )
        .padding([8, 16])
        .height(56)
        .width(Length::Fill)
        .style(look::topbar)
        .into()
    }
    fn error_banner(&self) -> Element<'_, Message, Theme, Renderer> {
        let lang = self.language();
        container(
            row![
                text(lang, &self.status.error).size(13),
                widget::space().width(Length::Fill),
                button(text(lang, "Dismiss").size(13))
                    .on_press(Message::ClearError)
                    .style(look::secondary)
            ]
            .spacing(16)
            .align_y(iced::Alignment::Center),
        )
        .padding([12, 24])
        .width(Length::Fill)
        .style(|_| container::Style {
            background: Some(iced::Color::from_rgb8(255, 238, 232).into()),
            text_color: Some(iced::Color::from_rgb8(143, 59, 45)),
            ..Default::default()
        })
        .into()
    }
    fn receive_view(&self) -> Element<'static, Message, Theme, Renderer> {
        let lang = self.language();
        self.client
            .shared
            .metrics
            .receiver_view_builds
            .fetch_add(1, Ordering::Relaxed);
        let video = widget::mouse_area(
            container(self.video_view())
                .width(Length::Fill)
                .height(Length::Fill)
                .style(|_| container::Style {
                    background: Some(iced::Color::BLACK.into()),
                    text_color: Some(iced::Color::WHITE),
                    border: iced::Border {
                        radius: 8.into(),
                        ..Default::default()
                    },
                    ..Default::default()
                }),
        )
        .on_double_click(Message::Fullscreen);
        let mut stage = widget::stack![video]
            .width(Length::Fill)
            .height(Length::Fill);
        if self.diagnostics_expanded {
            let details = column![
                row![text(lang, "Playback details").size(14).font(look::strong(lang)),
                    widget::space().width(Length::Fill),
                    button(text(lang, "Close").size(12)).padding([5,8]).style(look::secondary)
                        .on_press(Message::Diagnostics)].align_y(iced::Alignment::Center),
                scrollable(column![
                    detail_group(lang, "Video", self.stats.video.clone()),
                    detail_group(lang, "Timing", self.stats.timing.clone()),
                    detail_group(lang, "Audio", self.stats.audio.clone()),
                    text(lang, self.stats.network.clone()).size(12).color(look::MUTED),
                    text(lang, "FPS counts new video submissions. Processing excludes sender, network transit and physical screen latency. AV offset is video minus estimated audible audio.")
                        .size(12).color(look::MUTED)
                ].spacing(16)).height(Length::Fill)
            ].spacing(12);
            let panel = container(details)
                .padding(16)
                .width(370)
                .height(Length::Fill)
                .max_height(500)
                .style(look::panel);
            stage = stage.push(
                container(row![widget::space().width(Length::Fill), panel])
                    .padding(12)
                    .width(Length::Fill)
                    .height(Length::Fill),
            );
        }
        let summary = text(
            lang,
            format!(
                "{:.1} fps     Processing {}     {:.2} Mbps     Skipped {}",
                self.stats.fps,
                self.stats
                    .processing_ms
                    .map(|v| format!("{v:.1} ms"))
                    .unwrap_or_else(|| "—".into()),
                self.stats.mbps,
                self.stats.dropped
            ),
        )
        .size(12)
        .color(look::MUTED);
        let rail = row![
            summary,
            widget::space().width(Length::Fill),
            button(
                text(
                    lang,
                    if self.diagnostics_expanded {
                        "Hide details"
                    } else {
                        "Details"
                    }
                )
                .size(12)
            )
            .padding([5, 8])
            .style(look::secondary)
            .on_press(Message::Diagnostics),
            button(text(lang, "Disconnect").size(12))
                .padding([5, 8])
                .style(look::secondary)
                .on_press_maybe((!self.status.peer.is_empty()).then_some(Message::Disconnect))
        ]
        .spacing(10)
        .align_y(iced::Alignment::Center)
        .height(26);
        column![stage, self.player_controls(false), rail]
            .spacing(8)
            .padding(12)
            .width(Length::Fill)
            .height(Length::Fill)
            .into()
    }
    fn settings_view(&self) -> Element<'_, Message, Theme, Renderer> {
        let lang = self.language();
        fn field<'a>(
            lang: Language,
            label: &'static str,
            value: &'a str,
            error: Option<&'a str>,
            on_input: fn(String) -> Message,
        ) -> Element<'a, Message, Theme, Renderer> {
            let mut content = column![
                text(lang, label).size(12).color(look::MUTED),
                text_input("", value)
                    .font(look::font(lang))
                    .size(14)
                    .on_input(on_input)
                    .padding([10, 12])
                    .style(look::input)
            ]
            .spacing(7)
            .width(Length::Fill);
            if let Some(error) = error {
                content = content.push(
                    text(lang, error)
                        .size(12)
                        .color(iced::Color::from_rgb8(167, 69, 64)),
                );
            }
            content.into()
        }
        let f = &self.form;
        let receiver = column![
            section_title(
                lang,
                "Receiver",
                "Set how this receiver appears and what your device sends."
            ),
            field(
                lang,
                "Receiver name",
                &f.draft.name,
                f.errors.name.as_deref(),
                Message::Name
            ),
            row![
                field(
                    lang,
                    "Width (px)",
                    &f.width,
                    f.errors.width.as_deref(),
                    Message::Width
                ),
                field(
                    lang,
                    "Height (px)",
                    &f.height,
                    f.errors.height.as_deref(),
                    Message::Height
                ),
                field(
                    lang,
                    "Frame rate (fps)",
                    &f.fps,
                    f.errors.fps.as_deref(),
                    Message::Fps
                )
            ]
            .spacing(16),
            text(lang, "Receiver changes apply on the next connection.")
                .size(12)
                .color(look::MUTED),
            widget::rule::horizontal(1),
            section_title(
                lang,
                "Playback",
                "Keep decoding fast and the picture in sync with your display."
            ),
            checkbox(f.draft.hardware_decode)
                .label(lang.translate("Use hardware decoding").into_owned())
                .font(look::font(lang))
                .size(16)
                .text_size(14)
                .on_toggle(Message::Hardware),
            checkbox(f.draft.hevc_enabled)
                .label(lang.translate("Allow HEVC video").into_owned())
                .font(look::font(lang))
                .size(16)
                .text_size(14)
                .on_toggle(Message::Hevc),
            checkbox(f.draft.hls_enabled)
                .label(lang.translate("Allow HLS playback").into_owned())
                .font(look::font(lang))
                .size(16)
                .text_size(14)
                .on_toggle(Message::Hls),
            checkbox(f.draft.vsync)
                .label(
                    lang.translate("Synchronize with display (VSync)")
                        .into_owned()
                )
                .font(look::font(lang))
                .size(16)
                .text_size(14)
                .on_toggle(Message::Vsync),
            text(lang, "Rendering GPU").size(12).color(look::MUTED),
            pick_list(
                vec![
                    "balanced".to_owned(),
                    "low-power".to_owned(),
                    "high-performance".to_owned()
                ]
                .into_iter()
                .map(|value| UiOption::new(lang, value))
                .collect::<Vec<_>>(),
                Some(UiOption::new(lang, f.draft.gpu_preference.clone())),
                |value| Message::Gpu(value.value)
            )
            .font(look::font(lang))
            .style(look::picker)
            .text_size(14)
            .padding([10, 12])
            .width(Length::Fill),
            text(
                lang,
                "Balanced follows the display's GPU. GPU and VSync changes require a restart."
            )
            .size(12)
            .color(look::MUTED)
        ]
        .spacing(14);
        let audio = column![
            section_title(
                lang,
                "Audio output",
                "Choose where you want to hear the stream."
            ),
            pick_list(
                self.devices
                    .iter()
                    .cloned()
                    .map(|value| UiOption::device(lang, value))
                    .collect::<Vec<_>>(),
                self.devices
                    .iter()
                    .find(|d| d.id == f.draft.audio_device)
                    .cloned()
                    .map(|value| UiOption::device(lang, value)),
                |value| Message::Audio(value.value)
            )
            .font(look::font(lang))
            .style(look::picker)
            .placeholder(lang.translate("Saved output unavailable").into_owned())
            .text_size(14)
            .padding([10, 12])
            .width(Length::Fill),
            text(
                lang,
                "Device changes apply and save immediately, without saving other edits."
            )
            .size(12)
            .color(look::MUTED),
            text(
                lang,
                if self.status.audio_status.is_empty() {
                    "Ready when audio starts"
                } else {
                    &self.status.audio_status
                }
            )
            .size(12)
            .color(look::MUTED)
        ]
        .spacing(14);
        let desktop = column![
            section_title(
                lang,
                "Window and startup",
                "Keep receiving while the player is out of the way."
            ),
            text(lang, "Language").size(12).color(look::MUTED),
            pick_list(
                vec![Language::Auto, Language::English, Language::Chinese]
                    .into_iter()
                    .map(|value| UiOption::new(lang, value))
                    .collect::<Vec<_>>(),
                Some(UiOption::new(lang, f.saved.language)),
                |value| Message::Language(value.value)
            )
            .font(look::font(lang))
            .text_size(14)
            .padding([10, 12])
            .style(look::picker)
            .width(Length::Fill),
            text(lang, "Language changes apply and save immediately.")
                .size(12)
                .color(look::MUTED),
            checkbox(f.draft.close_to_tray)
                .label(lang.translate("Close window to tray").into_owned())
                .font(look::font(lang))
                .size(16)
                .text_size(14)
                .on_toggle(Message::CloseTray),
            text(
                lang,
                "Otherwise, closing minimizes to the taskbar. Use Quit to stop receiving."
            )
            .size(12)
            .color(look::MUTED),
            checkbox(f.draft.minimize_to_tray)
                .label(lang.translate("Minimize to tray").into_owned())
                .font(look::font(lang))
                .size(16)
                .text_size(14)
                .on_toggle(Message::MinimizeTray),
            checkbox(f.draft.start_hidden)
                .label(lang.translate("Start in tray").into_owned())
                .font(look::font(lang))
                .size(16)
                .text_size(14)
                .on_toggle(Message::StartHidden),
            checkbox(f.draft.autostart)
                .label(lang.translate("Start with Windows").into_owned())
                .font(look::font(lang))
                .size(16)
                .text_size(14)
                .on_toggle(Message::Autostart),
            text(
                lang,
                "Window size and fullscreen preference are remembered automatically."
            )
            .size(12)
            .color(look::MUTED)
        ]
        .spacing(14);
        let right = column![
            audio,
            widget::rule::horizontal(1),
            desktop,
            widget::rule::horizontal(1),
            self.updates_view()
        ]
        .spacing(24);
        let fields: Element<'_, Message, Theme, Renderer> = if self.viewport.width >= 1050. {
            row![
                container(receiver).width(Length::FillPortion(3)),
                container(right).width(Length::FillPortion(2))
            ]
            .spacing(36)
            .into()
        } else {
            column![receiver, widget::rule::horizontal(1), right]
                .spacing(24)
                .into()
        };
        let form = container(fields)
            .padding(24)
            .width(Length::Fill)
            .style(look::panel);
        let busy = f.pending.is_some() || f.audio_pending;
        let state = if busy {
            "Saving…"
        } else if f.dirty() {
            "Unsaved changes"
        } else {
            "All changes saved"
        };
        let footer = row![
            button(
                text(lang, if busy { "Saving…" } else { "Save settings" })
                    .size(14)
                    .font(look::strong(lang))
            )
            .padding([10, 18])
            .style(look::primary)
            .on_press_maybe((!busy && f.dirty()).then_some(Message::Save)),
            button(text(lang, "Reset form").size(14))
                .padding([10, 14])
                .style(look::secondary)
                .on_press_maybe((!busy).then_some(Message::Reset)),
            widget::space().width(Length::Fill),
            text(lang, state).size(12).color(look::MUTED)
        ]
        .spacing(12)
        .align_y(iced::Alignment::Center);
        column![
            row![
                text(lang, "Receiver settings")
                    .size(22)
                    .font(look::strong(lang))
            ],
            scrollable(form).id("settings-form").height(Length::Fill),
            text(
                lang,
                if f.feedback.is_empty() {
                    " "
                } else {
                    &f.feedback
                }
            )
            .size(12),
            footer
        ]
        .spacing(14)
        .padding(24)
        .height(Length::Fill)
        .width(Length::Fill)
        .into()
    }
    fn updates_view(&self) -> Element<'_, Message, Theme, Renderer> {
        use crate::update::Status;
        let lang = self.language();
        let settings = &self.form.draft;
        let label = match &self.updates.status {
            Status::Idle => lang
                .translate("Updates have not been checked yet.")
                .into_owned(),
            Status::Checking => lang.translate("Checking for updates…").into_owned(),
            Status::Current => lang.translate("AirDock is up to date.").into_owned(),
            Status::Available(release) => format!(
                "{}: v{}",
                lang.translate("Update available"),
                release.version
            ),
            Status::Downloading(release) => {
                let _ = release;
                let total = self.updates.total_size;
                let done = self
                    .updates
                    .progress
                    .load(std::sync::atomic::Ordering::Relaxed);
                format!(
                    "{}: {:.0}%",
                    lang.translate("Downloading update"),
                    done as f64 * 100. / total.max(1) as f64
                )
            }
            Status::Ready(_) => lang
                .translate("Update downloaded and verified.")
                .into_owned(),
            Status::Preparing => lang.translate("Preparing update…").into_owned(),
            Status::Failed(error) => format!("{}: {error}", lang.translate("Update failed")),
        };
        let busy = matches!(
            self.updates.status,
            Status::Checking | Status::Downloading(_) | Status::Preparing
        );
        let mut buttons = row![
            button(text(lang, "Check for updates").size(13))
                .padding([9, 12])
                .style(look::secondary)
                .on_press_maybe((!busy).then_some(Message::UpdateCheck))
        ]
        .spacing(10)
        .align_y(iced::Alignment::Center);
        if matches!(self.updates.status, Status::Available(_) | Status::Ready(_)) {
            buttons = buttons.push(
                button(
                    text(
                        lang,
                        if crate::update::INSTALL_SUPPORTED {
                            "Download and install"
                        } else {
                            "Open release page"
                        },
                    )
                    .size(13),
                )
                .padding([9, 12])
                .style(look::primary)
                .on_press(Message::UpdateDownload),
            );
        }
        if matches!(self.updates.status, Status::Downloading(_)) {
            buttons = buttons.push(
                button(text(lang, "Cancel").size(13))
                    .padding([9, 12])
                    .style(look::secondary)
                    .on_press(Message::UpdateCancel),
            );
        }
        column![
            section_title(lang, "Updates", "Stable releases from GitHub. Installation always requires your confirmation."),
            text(lang, format!("{}: {}", lang.translate("Current version"), env!("CARGO_PKG_VERSION"))).size(12).color(look::MUTED),
            checkbox(settings.automatic_updates).label(lang.translate("Check daily in the background").into_owned()).font(look::font(lang)).size(16).text_size(14).on_toggle(Message::AutomaticUpdates),
            checkbox(settings.update_mirrors_enabled).label(lang.translate("Use mirrors if GitHub download fails").into_owned()).font(look::font(lang)).size(16).text_size(14).on_toggle(Message::UpdateMirrorsEnabled),
            text_input("https://gh-proxy.com", &self.form.update_mirrors).font(look::font(lang)).size(13).padding([9,12]).style(look::input).on_input(Message::UpdateMirrors),
            text(lang, "Comma-separated HTTPS mirror prefixes. Failed direct downloads try the fastest reachable mirror. Official GitHub SHA-256 must match.").size(12).color(look::MUTED),
            text(lang, label).size(13),
            buttons
        ].spacing(12).into()
    }
    fn player_controls(&self, presentation: bool) -> Element<'static, Message, Theme, Renderer> {
        let lang = self.language();
        let available = if presentation {
            (self.viewport.width - 32.).min(960.) - 28.
        } else {
            self.viewport.width - 24.
        };
        let style = if presentation {
            look::player
        } else {
            look::secondary
        };
        let actions = row![
            button(
                text(
                    lang,
                    if presentation {
                        if self.fullscreen {
                            "Exit fullscreen"
                        } else {
                            "Show controls"
                        }
                    } else {
                        "Full screen"
                    }
                )
                .size(13)
            )
            .padding([8, 12])
            .style(style)
            .on_press(if presentation {
                Message::LeavePresentation
            } else {
                Message::Fullscreen
            }),
            button(
                text(
                    lang,
                    if self.crop {
                        "Fit picture"
                    } else {
                        "Fill picture"
                    }
                )
                .size(13)
            )
            .padding([8, 12])
            .style(style)
            .on_press(Message::Crop),
            widget::space().width(Length::Fill),
            text(lang, self.client.shared.metrics.video_colour_label())
                .size(12)
                .color(if presentation {
                    look::SCREEN_MUTED
                } else {
                    look::MUTED
                })
        ]
        .spacing(10)
        .align_y(iced::Alignment::Center);
        let volume = row![
            button(
                text(
                    lang,
                    if self.status.volume_db <= -100. {
                        "Unmute"
                    } else {
                        "Mute"
                    }
                )
                .size(13)
            )
            .padding([8, 12])
            .style(style)
            .on_press(Message::Mute),
            slider(
                -60.0..=0.,
                self.status.volume_db.clamp(-60., 0.),
                Message::Volume
            )
            .width(Length::Fill),
            text(
                lang,
                if self.status.volume_db <= -100. {
                    "Muted".into()
                } else {
                    format!("{:.0} dB", self.status.volume_db)
                }
            )
            .size(12)
            .width(46)
        ]
        .spacing(10)
        .align_y(iced::Alignment::Center);
        if available < 680. {
            column![actions, volume].spacing(12).into()
        } else {
            row![actions, container(volume).width(260)]
                .spacing(16)
                .align_y(iced::Alignment::Center)
                .into()
        }
    }
    fn video_view(&self) -> Element<'static, Message, Theme, Renderer> {
        let lang = self.language();
        if let Some(frame) = &self.frame {
            if render::compositor::STATUS.load(Ordering::Acquire) == 3 {
                return container(
                    text(lang, "Video requires a compatible GPU")
                        .size(20)
                        .color(iced::Color::WHITE),
                )
                .center(Length::Fill)
                .into();
            }
            return widget::shader(render::Video {
                epoch: frame.epoch,
                shared: self.client.shared.clone(),
                crop: self.crop,
            })
            .width(Length::Fill)
            .height(Length::Fill)
            .into();
        }
        if self.fullscreen || (self.focus && self.page == 0) {
            return container(
                text(
                    lang,
                    if self.fullscreen {
                        "Waiting for video. Press Esc or F11 to leave fullscreen."
                    } else {
                        "Waiting for video. Press Ctrl+H to show controls."
                    },
                )
                .size(14)
                .color(look::SCREEN_MUTED),
            )
            .center(Length::Fill)
            .into();
        }
        let audio = !self.status.peer.is_empty() && self.status.kind == "Audio";
        let mut content = column![].spacing(14).align_x(iced::Alignment::Center);
        if let Some(cover) = &self.cover {
            content = content.push(widget::image(cover.clone()).width(150).height(150));
        } else {
            content = content.push(receiver_mark(96., look::SCREEN_MUTED));
        }
        content = content.push(
            plain_text(
                lang,
                if audio && !self.status.title.is_empty() {
                    self.status.title.clone()
                } else {
                    self.form.saved.name.clone()
                },
            )
            .font(look::strong(lang))
            .align_x(iced::alignment::Horizontal::Center)
            .size(30)
            .color(iced::Color::WHITE),
        );
        if audio {
            content = content.push(
                plain_text(
                    lang,
                    if self.status.artist.is_empty() {
                        lang.translate("Audio connected").into_owned()
                    } else {
                        self.status.artist.clone()
                    },
                )
                .size(14)
                .color(look::SCREEN_MUTED),
            );
        } else {
            content=content.push(text(lang, if self.status.peer.is_empty(){"Open Screen Mirroring on your iPhone or iPad.\nChoose this receiver from the list."}
                else{"Connected. Waiting for your device to send video."}).size(14).color(look::SCREEN_MUTED).align_x(iced::alignment::Horizontal::Center));
            if self.status.peer.is_empty() {
                content = content.push(
                    text(lang, "Use your Wi-Fi network or a Windows mobile hotspot.")
                        .size(12)
                        .color(look::SCREEN_MUTED),
                );
            }
        }
        container(content).center(Length::Fill).padding(24).into()
    }
}
fn section_title<'a>(
    lang: Language,
    title: &'static str,
    description: &'static str,
) -> Element<'a, Message, Theme, Renderer> {
    column![
        text(lang, title).size(18).font(look::strong(lang)),
        text(lang, description).size(12).color(look::MUTED)
    ]
    .spacing(6)
    .into()
}
fn detail_group(
    lang: Language,
    title: &'static str,
    content: String,
) -> Element<'static, Message, Theme, Renderer> {
    column![
        text(lang, title).size(14).font(look::strong(lang)),
        text(lang, content).size(12).color(look::MUTED)
    ]
    .spacing(8)
    .width(Length::Fill)
    .into()
}
fn receiver_mark(width: f32, color: iced::Color) -> Element<'static, Message, Theme, Renderer> {
    column![
        container(widget::space().width(width).height(width * 0.57)).style(move |_| {
            container::Style {
                border: iced::Border {
                    color,
                    width: if width > 40. { 2. } else { 1.5 },
                    radius: 4.into(),
                },
                ..Default::default()
            }
        }),
        raw_text("△")
            .font(iced::Font::DEFAULT)
            .size(width * 0.35)
            .color(color)
    ]
    .spacing(-width * 0.09)
    .align_x(iced::Alignment::Center)
    .width(width)
    .into()
}

fn text<'a>(lang: Language, value: impl Into<Cow<'a, str>>) -> widget::Text<'a, Theme, Renderer> {
    let content = match value.into() {
        Cow::Borrowed(value) => lang.translate(value),
        Cow::Owned(value) => Cow::Owned(lang.translate(&value).into_owned()),
    };
    plain_text(lang, content)
}
fn plain_text<'a>(
    lang: Language,
    content: impl Into<Cow<'a, str>>,
) -> widget::Text<'a, Theme, Renderer> {
    raw_text(content.into())
        .font(look::font(lang))
        .shaping(iced::advanced::text::Shaping::Advanced)
}
#[derive(Clone, PartialEq, Eq)]
struct UiOption<T> {
    value: T,
    label: String,
}
impl<T: std::fmt::Display> UiOption<T> {
    fn new(lang: Language, value: T) -> Self {
        let label = lang.translate(&value.to_string()).into_owned();
        Self { value, label }
    }
}
impl<T> std::fmt::Display for UiOption<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.label)
    }
}

impl UiOption<crate::audio::DeviceChoice> {
    fn device(lang: Language, value: crate::audio::DeviceChoice) -> Self {
        let label = if value.id == "default" {
            lang.translate(&value.to_string()).into_owned()
        } else {
            value.to_string()
        };
        Self { value, label }
    }
}
