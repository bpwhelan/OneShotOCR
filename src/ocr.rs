//! OCR backend selection and the native local OCR engines.

use anyhow::Result;
use image::RgbaImage;

use crate::config::OcrEngine;

#[cfg(feature = "oneocr")]
mod oneocr;
#[cfg(feature = "screenai")]
mod screenai;

/// Run the selected OCR engine against an RGBA screenshot crop.
pub fn ocr_image(img: &RgbaImage, engine: OcrEngine, language: &str) -> Result<String> {
    match engine {
        OcrEngine::GoogleLens => crate::lens::ocr_image(img, language),
        #[cfg(feature = "oneocr")]
        OcrEngine::OneOcr => oneocr::ocr_image(img),
        #[cfg(feature = "screenai")]
        OcrEngine::ScreenAi => screenai::ocr_image(img),
    }
}
