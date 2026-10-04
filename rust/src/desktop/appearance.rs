// SPDX-License-Identifier: MPL-2.0
//! Desktop visual tokens; independent of receiving and media scheduling.
use iced::{
    Border, Color, Font, Theme, font,
    widget::{button, container, pick_list, text_input},
};
pub const FONT: Font = Font::with_name("Manrope");
pub const STRONG: Font = Font {
    weight: font::Weight::Semibold,
    ..FONT
};
pub const SHELL: Color = Color::from_rgb8(238, 243, 247);
pub const PAPER: Color = Color::WHITE;
pub const INK: Color = Color::from_rgb8(24, 53, 72);
pub const MUTED: Color = Color::from_rgb8(96, 119, 134);
pub const BLUE: Color = Color::from_rgb8(47, 107, 147);
pub const LINE: Color = Color::from_rgb8(211, 223, 230);
pub const SCREEN_MUTED: Color = Color::from_rgb8(158, 181, 196);
pub fn desktop_theme() -> Theme {
    static THEME: std::sync::OnceLock<Theme> = std::sync::OnceLock::new();
    THEME
        .get_or_init(|| {
            Theme::custom(
                "Receiver",
                iced::theme::Palette {
                    background: SHELL,
                    text: INK,
                    primary: BLUE,
                    success: Color::from_rgb8(52, 126, 112),
                    warning: Color::from_rgb8(156, 108, 38),
                    danger: Color::from_rgb8(167, 69, 64),
                },
            )
        })
        .clone()
}
pub fn panel(_: &Theme) -> container::Style {
    container::Style {
        background: Some(PAPER.into()),
        text_color: Some(INK),
        border: Border {
            color: LINE,
            width: 1.,
            radius: 12.into(),
        },
        ..Default::default()
    }
}
pub fn topbar(_: &Theme) -> container::Style {
    container::Style {
        background: Some(PAPER.into()),
        text_color: Some(INK),
        ..Default::default()
    }
}
pub fn overlay(_: &Theme) -> container::Style {
    container::Style {
        background: Some(Color::from_rgba8(25, 37, 47, 0.96).into()),
        text_color: Some(Color::WHITE),
        border: Border {
            color: Color::from_rgb8(65, 83, 96),
            width: 1.,
            radius: 12.into(),
        },
        ..Default::default()
    }
}
pub fn nav(selected: bool, status: button::Status) -> button::Style {
    let active = selected || matches!(status, button::Status::Hovered | button::Status::Pressed);
    button::Style {
        background: active.then(|| Color::from_rgb8(227, 238, 246).into()),
        text_color: if selected { BLUE } else { MUTED },
        border: Border {
            radius: 8.into(),
            ..Default::default()
        },
        ..Default::default()
    }
}
pub fn secondary(_: &Theme, status: button::Status) -> button::Style {
    let disabled = matches!(status, button::Status::Disabled);
    button::Style {
        background: Some(
            if matches!(status, button::Status::Hovered | button::Status::Pressed) {
                Color::from_rgb8(229, 239, 246)
            } else {
                PAPER
            }
            .into(),
        ),
        text_color: if disabled {
            Color::from_rgb8(147, 164, 175)
        } else {
            INK
        },
        border: Border {
            color: if disabled {
                LINE
            } else {
                Color::from_rgb8(193, 209, 221)
            },
            width: 1.,
            radius: 7.into(),
        },
        ..Default::default()
    }
}
pub fn primary(_: &Theme, status: button::Status) -> button::Style {
    button::Style {
        background: Some(
            if matches!(status, button::Status::Disabled) {
                Color::from_rgb8(178, 196, 209)
            } else if matches!(status, button::Status::Hovered | button::Status::Pressed) {
                Color::from_rgb8(35, 83, 119)
            } else {
                BLUE
            }
            .into(),
        ),
        text_color: Color::WHITE,
        border: Border {
            radius: 7.into(),
            ..Default::default()
        },
        ..Default::default()
    }
}
pub fn player(_: &Theme, status: button::Status) -> button::Style {
    button::Style {
        background: Some(
            if matches!(status, button::Status::Hovered | button::Status::Pressed) {
                Color::from_rgb8(68, 91, 110)
            } else {
                Color::from_rgb8(44, 63, 78)
            }
            .into(),
        ),
        text_color: Color::WHITE,
        border: Border {
            color: Color::from_rgb8(83, 106, 124),
            width: 1.,
            radius: 7.into(),
        },
        ..Default::default()
    }
}
pub fn input(theme: &Theme, status: text_input::Status) -> text_input::Style {
    let mut style = text_input::default(theme, status);
    style.background = Color::from_rgb8(249, 251, 253).into();
    style.border = Border {
        color: if matches!(status, text_input::Status::Focused { .. }) {
            BLUE
        } else {
            LINE
        },
        width: 1.,
        radius: 7.into(),
    };
    style
}

pub fn picker(_: &Theme, status: pick_list::Status) -> pick_list::Style {
    pick_list::Style {
        text_color: INK,
        placeholder_color: MUTED,
        handle_color: BLUE,
        background: Color::from_rgb8(249, 251, 253).into(),
        border: Border {
            color: if matches!(status, pick_list::Status::Active) {
                LINE
            } else {
                BLUE
            },
            width: 1.,
            radius: 7.into(),
        },
    }
}
