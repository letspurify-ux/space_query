use fltk::{enums::ColorDepth, image::RgbImage, prelude::WindowExt};

use crate::utils::arithmetic::safe_div;

include!("icon_fill.rs");

const BIG_ICON_SIZE: usize = 64;

pub fn apply_window_icon<W: WindowExt>(window: &mut W) {
    if let Some(image) = fltk_icon_image() {
        window.set_icon(Some(image));
    }
}

fn fltk_icon_image() -> Option<RgbImage> {
    let mut pixels = [0u8; BIG_ICON_SIZE * BIG_ICON_SIZE * 4];
    fill_icon(&mut pixels, BIG_ICON_SIZE);
    let image = RgbImage::new(
        &pixels,
        BIG_ICON_SIZE as i32,
        BIG_ICON_SIZE as i32,
        ColorDepth::Rgba8,
    )
    .ok()?;
    Some(image)
}
