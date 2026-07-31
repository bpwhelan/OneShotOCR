//! OCR backend selection and the native local OCR engines.

use anyhow::Result;
use image::RgbaImage;

use crate::config::OcrEngine;

mod oneocr;
mod screenai;

/// Run the selected OCR engine against an RGBA screenshot crop.
pub fn ocr_image(img: &RgbaImage, engine: OcrEngine, language: &str) -> Result<String> {
    match engine {
        OcrEngine::GoogleLens => crate::lens::ocr_image(img, language),
        OcrEngine::OneOcr => oneocr::ocr_image(img),
        OcrEngine::ScreenAi => screenai::ocr_image(img),
    }
}
