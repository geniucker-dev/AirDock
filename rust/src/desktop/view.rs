// SPDX-License-Identifier: MPL-2.0
//! Desktop view composition. Media frames are selected by the video primitive.
use super::appearance as look;
use super::*;
use iced::widget::column;

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
    pub(super) fn view(&self, _: window::Id) -> Element<'_, Message, Theme, Renderer> {
        if self.fullscreen || (self.focus && self.page == 0 && self.status.error.is_empty()) {
            let player = container(self.video_view())
                .width(Length::Fill)
                .height(Length::Fill)
                .style(|_| container::Style {
                    background: Some(iced::Color::BLACK.into()),
                    ..Default::default()
                });
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
        body.into()
    }
    fn topbar(&self) -> Element<'_, Message, Theme, Renderer> {
        let selected = self.page;
        let brand = row![
            receiver_mark(26., look::BLUE),
            text("AirPlay Windows").font(look::STRONG).size(18)
        ]
        .spacing(12)
        .align_y(iced::Alignment::Center)
        .width(210);
        let navigation = row![
            button(text("Receive").size(14).font(look::STRONG))
                .width(96)
                .padding([9, 12])
                .style(move |_, status| look::nav(selected == 0, status))
                .on_press(Message::Page(0)),
            button(text("Settings").size(14).font(look::STRONG))
                .width(96)
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
        let service = row![indicator, text(connection).size(12).color(look::MUTED)]
            .spacing(7)
            .align_y(iced::Alignment::Center);
        container(
            row![
                brand,
                navigation,
                widget::space().width(Length::Fill),
                service,
                button(text("Hide").size(13))
                    .width(58)
                    .padding([9, 10])
                    .style(look::secondary)
                    .on_press_maybe(self.tray_available().then_some(Message::Hide)),
                button(text("Quit").size(13))
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
        container(
            row![
                text(&self.status.error).size(13),
                widget::space().width(Length::Fill),
                button(text("Dismiss").size(13))
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
        self.client
            .shared
            .metrics
            .receiver_view_builds
            .fetch_add(1, Ordering::Relaxed);
        let video = container(self.video_view())
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
            });
        let mut stage = widget::stack![video]
            .width(Length::Fill)
            .height(Length::Fill);
        if self.diagnostics_expanded {
            let details = column![
                row![text("Playback details").size(14).font(look::STRONG),
                    widget::space().width(Length::Fill),
                    button(text("Close").size(12)).padding([5,8]).style(look::secondary)
                        .on_press(Message::Diagnostics)].align_y(iced::Alignment::Center),
                scrollable(column![
                    detail_group("Video", self.stats.video.clone()),
                    detail_group("Timing", self.stats.timing.clone()),
                    detail_group("Audio", self.stats.audio.clone()),
                    text(self.stats.network.clone()).size(12).color(look::MUTED),
                    text("FPS counts new video submissions. Processing excludes sender, network transit and physical screen latency.")
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
        let summary = text(format!(
            "{:.1} fps     Processing {}     {:.2} Mbps     Skipped {}",
            self.stats.fps,
            self.stats
                .processing_ms
                .map(|v| format!("{v:.1} ms"))
                .unwrap_or_else(|| "—".into()),
            self.stats.mbps,
            self.stats.dropped
        ))
        .size(12)
        .color(look::MUTED);
        let rail = row![
            summary,
            widget::space().width(Length::Fill),
            button(
                text(if self.diagnostics_expanded {
                    "Hide details"
                } else {
                    "Details"
                })
                .size(12)
            )
            .padding([5, 8])
            .style(look::secondary)
            .on_press(Message::Diagnostics),
            button(text("Disconnect").size(12))
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
        fn field<'a>(
            label: &'static str,
            value: &'a str,
            error: Option<&'a str>,
            on_input: fn(String) -> Message,
        ) -> Element<'a, Message, Theme, Renderer> {
            let mut content = column![
                text(label).size(12).color(look::MUTED),
                text_input("", value)
                    .size(14)
                    .on_input(on_input)
                    .padding([10, 12])
                    .style(look::input)
            ]
            .spacing(7)
            .width(Length::Fill);
            if let Some(error) = error {
                content = content.push(
                    text(error)
                        .size(12)
                        .color(iced::Color::from_rgb8(167, 69, 64)),
                );
            }
            content.into()
        }
        let f = &self.form;
        let receiver=column![
            section_title("Receiver","Set how this receiver appears and what your device sends."),
            field("Receiver name",&f.draft.name,f.errors.name.as_deref(),Message::Name),
            row![field("Width (px)",&f.width,f.errors.width.as_deref(),Message::Width),
                field("Height (px)",&f.height,f.errors.height.as_deref(),Message::Height),
                field("Frame rate (fps)",&f.fps,f.errors.fps.as_deref(),Message::Fps)].spacing(16),
            text("Use 0 for the device's default size. Receiver changes apply on the next connection.")
                .size(12).color(look::MUTED),
            widget::rule::horizontal(1),
            section_title("Playback","Keep decoding fast and the picture in sync with your display."),
            checkbox(f.draft.hardware_decode).label("Use hardware decoding").size(16).text_size(14).on_toggle(Message::Hardware),
            checkbox(f.draft.hevc_enabled).label("Allow HEVC video").size(16).text_size(14).on_toggle(Message::Hevc),
            checkbox(f.draft.hls_enabled).label("Allow HLS playback").size(16).text_size(14).on_toggle(Message::Hls),
            checkbox(f.draft.vsync).label("Synchronize with display (VSync)").size(16).text_size(14).on_toggle(Message::Vsync),
            text("Rendering GPU").size(12).color(look::MUTED),
            pick_list(vec!["balanced".to_owned(),"low-power".to_owned(),"high-performance".to_owned()],
                Some(f.draft.gpu_preference.clone()),Message::Gpu).style(look::picker).text_size(14).padding([10,12]).width(Length::Fill),
            text("Balanced follows the display's GPU. GPU and VSync changes require a restart.")
                .size(12).color(look::MUTED)
        ].spacing(14);
        let audio = column![
            section_title("Audio output", "Choose where you want to hear the stream."),
            pick_list(
                self.devices.clone(),
                self.devices
                    .iter()
                    .find(|d| d.id == f.draft.audio_device)
                    .cloned(),
                Message::Audio
            )
            .style(look::picker)
            .placeholder("Saved output unavailable")
            .text_size(14)
            .padding([10, 12])
            .width(Length::Fill),
            text("Device changes apply and save immediately, without saving other edits.")
                .size(12)
                .color(look::MUTED),
            text(if self.status.audio_status.is_empty() {
                "Ready when audio starts"
            } else {
                &self.status.audio_status
            })
            .size(12)
            .color(look::MUTED)
        ]
        .spacing(14);
        let desktop = column![
            section_title(
                "Window and startup",
                "Keep receiving while the player is out of the way."
            ),
            checkbox(f.draft.close_to_tray)
                .label("Close window to tray")
                .size(16)
                .text_size(14)
                .on_toggle(Message::CloseTray),
            text("Otherwise, closing minimizes to the taskbar. Use Quit to stop receiving.")
                .size(12)
                .color(look::MUTED),
            checkbox(f.draft.minimize_to_tray)
                .label("Minimize to tray")
                .size(16)
                .text_size(14)
                .on_toggle(Message::MinimizeTray),
            checkbox(f.draft.start_hidden)
                .label("Start in tray")
                .size(16)
                .text_size(14)
                .on_toggle(Message::StartHidden),
            checkbox(f.draft.autostart)
                .label("Start with Windows")
                .size(16)
                .text_size(14)
                .on_toggle(Message::Autostart),
            text("Window size and fullscreen preference are remembered automatically.")
                .size(12)
                .color(look::MUTED)
        ]
        .spacing(14);
        let right = column![audio, widget::rule::horizontal(1), desktop].spacing(24);
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
                text(if busy { "Saving…" } else { "Save settings" })
                    .size(14)
                    .font(look::STRONG)
            )
            .padding([10, 18])
            .style(look::primary)
            .on_press_maybe((!busy && f.dirty()).then_some(Message::Save)),
            button(text("Reset form").size(14))
                .padding([10, 14])
                .style(look::secondary)
                .on_press_maybe((!busy).then_some(Message::Reset)),
            widget::space().width(Length::Fill),
            text(state).size(12).color(look::MUTED)
        ]
        .spacing(12)
        .align_y(iced::Alignment::Center);
        column![
            row![text("Receiver settings").size(22).font(look::STRONG)],
            scrollable(form).height(Length::Fill),
            text(if f.feedback.is_empty() {
                " "
            } else {
                &f.feedback
            })
            .size(12),
            footer
        ]
        .spacing(14)
        .padding(24)
        .height(Length::Fill)
        .width(Length::Fill)
        .into()
    }
    fn player_controls(&self, presentation: bool) -> Element<'static, Message, Theme, Renderer> {
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
                text(if presentation {
                    if self.fullscreen {
                        "Exit fullscreen"
                    } else {
                        "Show controls"
                    }
                } else {
                    "Full screen"
                })
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
                text(if self.crop {
                    "Fit picture"
                } else {
                    "Fill picture"
                })
                .size(13)
            )
            .padding([8, 12])
            .style(style)
            .on_press(Message::Crop),
            widget::space().width(Length::Fill),
            text(self.client.shared.metrics.video_colour_label())
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
                text(if self.status.volume_db <= -100. {
                    "Unmute"
                } else {
                    "Mute"
                })
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
            text(if self.status.volume_db <= -100. {
                "Muted".into()
            } else {
                format!("{:.0} dB", self.status.volume_db)
            })
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
        if let Some(frame) = &self.frame {
            if render::compositor::STATUS.load(Ordering::Acquire) == 3 {
                return container(
                    text("Video requires a compatible GPU")
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
                text(if self.fullscreen {
                    "Waiting for video. Press Esc or F11 to leave fullscreen."
                } else {
                    "Waiting for video. Press Ctrl+H to show controls."
                })
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
            text(if audio && !self.status.title.is_empty() {
                self.status.title.clone()
            } else {
                self.form.saved.name.clone()
            })
            .font(look::STRONG)
            .size(30)
            .color(iced::Color::WHITE),
        );
        if audio {
            content = content.push(
                text(if self.status.artist.is_empty() {
                    "Audio connected".into()
                } else {
                    self.status.artist.clone()
                })
                .size(14)
                .color(look::SCREEN_MUTED),
            );
        } else {
            content=content.push(text(if self.status.peer.is_empty(){"Open Screen Mirroring on your iPhone or iPad.\nChoose this receiver from the list."}
                else{"Connected. Waiting for your device to send video."}).size(14).color(look::SCREEN_MUTED).align_x(iced::alignment::Horizontal::Center));
            if self.status.peer.is_empty() {
                content = content.push(
                    text("Use your Wi-Fi network or a Windows mobile hotspot.")
                        .size(12)
                        .color(look::SCREEN_MUTED),
                );
            }
        }
        container(content).center(Length::Fill).padding(24).into()
    }
}
fn section_title<'a>(
    title: &'static str,
    description: &'static str,
) -> Element<'a, Message, Theme, Renderer> {
    column![
        text(title).size(18).font(look::STRONG),
        text(description).size(12).color(look::MUTED)
    ]
    .spacing(6)
    .into()
}
fn detail_group(
    title: &'static str,
    content: String,
) -> Element<'static, Message, Theme, Renderer> {
    column![
        text(title).size(14).font(look::STRONG),
        text(content).size(12).color(look::MUTED)
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
        text("△")
            .font(iced::Font::DEFAULT)
            .size(width * 0.35)
            .color(color)
    ]
    .spacing(-width * 0.09)
    .align_x(iced::Alignment::Center)
    .width(width)
    .into()
}
