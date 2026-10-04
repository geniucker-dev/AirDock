// SPDX-License-Identifier: MPL-2.0
//! Static application identity; icons are decoded once outside media workers.
use std::sync::OnceLock;

pub const NAME: &str = "AirDock";

pub fn window_icon() -> iced::window::Icon {
    static ICON: OnceLock<iced::window::Icon> = OnceLock::new();
    ICON.get_or_init(|| {
        let image = image::load_from_memory(include_bytes!("../assets/icons/airdock-128.png"))
            .expect("embedded AirDock PNG is validated by packaging")
            .into_rgba8();
        let (width, height) = image.dimensions();
        iced::window::icon::from_rgba(image.into_raw(), width, height)
            .expect("embedded icon has complete RGBA pixels")
    })
    .clone()
}

pub fn image_handle() -> iced::widget::image::Handle {
    static HANDLE: OnceLock<iced::widget::image::Handle> = OnceLock::new();
    HANDLE
        .get_or_init(|| {
            iced::widget::image::Handle::from_bytes(
                include_bytes!("../assets/icons/airdock-128.png").as_slice(),
            )
        })
        .clone()
}

#[cfg(windows)]
pub fn tray_icon() -> anyhow::Result<tray_icon::Icon> {
    let image =
        image::load_from_memory(include_bytes!("../assets/icons/airdock-32.png"))?.into_rgba8();
    let (width, height) = image.dimensions();
    Ok(tray_icon::Icon::from_rgba(image.into_raw(), width, height)?)
}
