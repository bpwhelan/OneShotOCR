//! Screen capture spanning all monitors, stitched into one virtual-desktop image.

use anyhow::{Context, Result};
use image::RgbaImage;
use xcap::Monitor;

/// A stitched screenshot of the whole virtual desktop plus its top-left origin
/// in physical desktop pixels, used to position the overlay window.
pub struct VirtualScreenshot {
    pub image: RgbaImage,
    pub origin_x: i32,
    pub origin_y: i32,
}

/// Grab a screenshot spanning every monitor, stitched into a single image.
pub fn capture_all() -> Result<VirtualScreenshot> {
    let monitors = Monitor::all().context("enumerate monitors")?;
    if monitors.is_empty() {
        anyhow::bail!("no monitor found");
    }

    // Bounding box of the virtual desktop in physical pixels.
    let mut min_x = i32::MAX;
    let mut min_y = i32::MAX;
    let mut max_x = i32::MIN;
    let mut max_y = i32::MIN;
    for m in &monitors {
        let x = m.x().unwrap_or(0);
        let y = m.y().unwrap_or(0);
        let w = m.width().unwrap_or(0) as i32;
        let h = m.height().unwrap_or(0) as i32;
        min_x = min_x.min(x);
        min_y = min_y.min(y);
        max_x = max_x.max(x + w);
        max_y = max_y.max(y + h);
    }

    let width = (max_x - min_x).max(1) as u32;
    let height = (max_y - min_y).max(1) as u32;
    let mut canvas = RgbaImage::new(width, height);
    let mut captured_any = false;

    for m in &monitors {
        let shot = match m.capture_image() {
            Ok(img) => img,
            Err(e) => {
                eprintln!("capture monitor failed: {e}");
                continue;
            }
        };
        let (mw, mh) = (shot.width(), shot.height());
        // Convert via raw bytes so we are independent of xcap's internal image version.
        let Some(mon_img) = RgbaImage::from_raw(mw, mh, shot.into_raw()) else {
            continue;
        };
        let ox = (m.x().unwrap_or(0) - min_x).max(0) as i64;
        let oy = (m.y().unwrap_or(0) - min_y).max(0) as i64;
        image::imageops::overlay(&mut canvas, &mon_img, ox, oy);
        captured_any = true;
    }

    if !captured_any {
        anyhow::bail!("failed to capture any monitor");
    }

    Ok(VirtualScreenshot {
        image: canvas,
        origin_x: min_x,
        origin_y: min_y,
    })
}
