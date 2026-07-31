//! Minimal Google Lens OCR client.
//!
//! Talks to the same private Lens endpoint that GameSentenceMiner uses. The
//! protobuf messages below are hand-declared with `prost` derive macros (only
//! the handful of fields we actually need) so there is no `protoc`/build step.
//! Field numbers were taken from the Lens overlay protos shipped with GSM.

use anyhow::{Context, Result, bail};
use image::RgbaImage;
use prost::Message;
use rand::RngExt;

const ENDPOINT: &str = "https://lensfrontend-pa.googleapis.com/v1/crupload";
const PUBLIC_API_KEY: &str = "AIzaSyDr2UxVnv_U85AbhhY8XSHSIavUW0DC-sY";
const USER_AGENT: &str = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 \
     (KHTML, like Gecko) Chrome/134.0.0.0 Safari/537.36";

// Enum values (encoded on the wire as plain varints, same as int32).
const PLATFORM_WEB: i32 = 3;
const SURFACE_CHROMIUM: i32 = 4;
const AUTO_FILTER: i32 = 7;

// Lens downscales anything larger than this before upload.
const MAX_PIXELS: u64 = 3_000_000;

// ---------------------------------------------------------------------------
// Request messages
// ---------------------------------------------------------------------------

#[derive(Clone, PartialEq, Message)]
struct ServerRequest {
    #[prost(message, optional, tag = "1")]
    objects_request: Option<ObjectsRequest>,
}

#[derive(Clone, PartialEq, Message)]
struct ObjectsRequest {
    #[prost(message, optional, tag = "1")]
    request_context: Option<RequestContext>,
    #[prost(message, optional, tag = "3")]
    image_data: Option<ImageData>,
}

#[derive(Clone, PartialEq, Message)]
struct RequestContext {
    #[prost(message, optional, tag = "3")]
    request_id: Option<RequestId>,
    #[prost(message, optional, tag = "4")]
    client_context: Option<ClientContext>,
}

#[derive(Clone, PartialEq, Message)]
struct RequestId {
    #[prost(uint64, tag = "1")]
    uuid: u64,
    #[prost(int32, tag = "2")]
    sequence_id: i32,
    #[prost(int32, tag = "3")]
    image_sequence_id: i32,
    #[prost(bytes = "vec", tag = "4")]
    analytics_id: Vec<u8>,
}

#[derive(Clone, PartialEq, Message)]
struct ClientContext {
    #[prost(int32, tag = "1")]
    platform: i32,
    #[prost(int32, tag = "2")]
    surface: i32,
    #[prost(message, optional, tag = "4")]
    locale_context: Option<LocaleContext>,
    #[prost(string, tag = "6")]
    app_id: String,
    #[prost(message, optional, tag = "17")]
    client_filters: Option<AppliedFilters>,
}

#[derive(Clone, PartialEq, Message)]
struct LocaleContext {
    #[prost(string, tag = "1")]
    language: String,
    #[prost(string, tag = "2")]
    region: String,
    #[prost(string, tag = "3")]
    time_zone: String,
}

#[derive(Clone, PartialEq, Message)]
struct AppliedFilters {
    #[prost(message, repeated, tag = "1")]
    filter: Vec<AppliedFilter>,
}

#[derive(Clone, PartialEq, Message)]
struct AppliedFilter {
    #[prost(int32, tag = "1")]
    filter_type: i32,
}

#[derive(Clone, PartialEq, Message)]
struct ImageData {
    #[prost(message, optional, tag = "1")]
    payload: Option<ImagePayload>,
    #[prost(message, optional, tag = "3")]
    image_metadata: Option<ImageMetadata>,
}

#[derive(Clone, PartialEq, Message)]
struct ImagePayload {
    #[prost(bytes = "vec", tag = "1")]
    image_bytes: Vec<u8>,
}

#[derive(Clone, PartialEq, Message)]
struct ImageMetadata {
    #[prost(int32, tag = "1")]
    width: i32,
    #[prost(int32, tag = "2")]
    height: i32,
}

// ---------------------------------------------------------------------------
// Response messages (unknown fields are skipped by prost during decode)
// ---------------------------------------------------------------------------

#[derive(Clone, PartialEq, Message)]
struct ServerResponse {
    #[prost(message, optional, tag = "2")]
    objects_response: Option<ObjectsResponse>,
}

#[derive(Clone, PartialEq, Message)]
struct ObjectsResponse {
    #[prost(message, optional, tag = "3")]
    text: Option<TextField>,
}

#[derive(Clone, PartialEq, Message)]
struct TextField {
    #[prost(message, optional, tag = "1")]
    text_layout: Option<TextLayout>,
}

#[derive(Clone, PartialEq, Message)]
struct TextLayout {
    #[prost(message, repeated, tag = "1")]
    paragraphs: Vec<Paragraph>,
}

#[derive(Clone, PartialEq, Message)]
struct Paragraph {
    #[prost(message, repeated, tag = "2")]
    lines: Vec<Line>,
}

#[derive(Clone, PartialEq, Message)]
struct Line {
    #[prost(message, repeated, tag = "1")]
    words: Vec<Word>,
}

#[derive(Clone, PartialEq, Message)]
struct Word {
    #[prost(string, tag = "2")]
    plain_text: String,
    #[prost(string, tag = "3")]
    text_separator: String,
}

// ---------------------------------------------------------------------------

/// OCR a cropped image with Google Lens and return the recognized text.
pub fn ocr_image(img: &RgbaImage, language: &str) -> Result<String> {
    let (payload, width, height) = preprocess(img)?;
    let request = build_request(payload, width, height, language);
    let body = request.encode_to_vec();

    let client = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(20))
        .build()
        .context("build http client")?;

    let response = client
        .post(ENDPOINT)
        .header("X-Goog-Api-Key", PUBLIC_API_KEY)
        .header(reqwest::header::CONTENT_TYPE, "application/x-protobuf")
        .header(reqwest::header::USER_AGENT, USER_AGENT)
        .header("Sec-Fetch-Site", "none")
        .header("Sec-Fetch-Mode", "no-cors")
        .header("Sec-Fetch-Dest", "empty")
        .header(reqwest::header::ACCEPT_LANGUAGE, "ja-JP;q=0.6,ja;q=0.5")
        .body(body)
        .send()
        .context("send request to Google Lens")?;

    let status = response.status();
    let bytes = response.bytes().context("read Lens response body")?;
    if !status.is_success() {
        bail!("Google Lens returned HTTP {status}");
    }

    let parsed = ServerResponse::decode(bytes).context("decode Lens protobuf response")?;
    Ok(extract_text(&parsed))
}

fn build_request(image_bytes: Vec<u8>, width: i32, height: i32, language: &str) -> ServerRequest {
    let mut rng = rand::rng();
    let analytics_id: [u8; 16] = rng.random();

    ServerRequest {
        objects_request: Some(ObjectsRequest {
            request_context: Some(RequestContext {
                request_id: Some(RequestId {
                    uuid: rng.random(),
                    sequence_id: 0,
                    image_sequence_id: 0,
                    analytics_id: analytics_id.to_vec(),
                }),
                client_context: Some(ClientContext {
                    platform: PLATFORM_WEB,
                    surface: SURFACE_CHROMIUM,
                    locale_context: Some(LocaleContext {
                        language: language.to_string(),
                        region: "Asia/Tokyo".to_string(),
                        time_zone: String::new(),
                    }),
                    app_id: String::new(),
                    client_filters: Some(AppliedFilters {
                        filter: vec![AppliedFilter {
                            filter_type: AUTO_FILTER,
                        }],
                    }),
                }),
            }),
            image_data: Some(ImageData {
                payload: Some(ImagePayload { image_bytes }),
                image_metadata: Some(ImageMetadata { width, height }),
            }),
        }),
    }
}

/// Downscale oversized images (matching GSM) and PNG-encode for upload.
fn preprocess(img: &RgbaImage) -> Result<(Vec<u8>, i32, i32)> {
    let (mut w, mut h) = (img.width(), img.height());
    let pixels = w as u64 * h as u64;

    let encoded = if pixels > MAX_PIXELS {
        let aspect = w as f64 / h as f64;
        let new_w = ((MAX_PIXELS as f64 * aspect).sqrt()) as u32;
        let new_h = (new_w as f64 / aspect) as u32;
        w = new_w.max(1);
        h = new_h.max(1);
        let resized = image::imageops::resize(img, w, h, image::imageops::FilterType::Lanczos3);
        encode_png(&resized)?
    } else {
        encode_png(img)?
    };

    Ok((encoded, w as i32, h as i32))
}

fn encode_png(img: &RgbaImage) -> Result<Vec<u8>> {
    let mut buf = Vec::new();
    image::codecs::png::PngEncoder::new(&mut buf)
        .write_image(
            img.as_raw(),
            img.width(),
            img.height(),
            image::ExtendedColorType::Rgba8,
        )
        .context("encode PNG")?;
    Ok(buf)
}

/// Rebuild plain text from the Lens text layout using its word separators.
fn extract_text(response: &ServerResponse) -> String {
    let Some(layout) = response
        .objects_response
        .as_ref()
        .and_then(|o| o.text.as_ref())
        .and_then(|t| t.text_layout.as_ref())
    else {
        return String::new();
    };

    let mut paragraphs = Vec::new();
    for paragraph in &layout.paragraphs {
        let mut lines = Vec::new();
        for line in &paragraph.lines {
            let mut line_text = String::new();
            for word in &line.words {
                line_text.push_str(&word.plain_text);
                line_text.push_str(&word.text_separator);
            }
            let trimmed = line_text.trim_end();
            if !trimmed.is_empty() {
                lines.push(trimmed.to_string());
            }
        }
        if !lines.is_empty() {
            paragraphs.push(lines.join("\n"));
        }
    }

    paragraphs.join("\n").trim().to_string()
}

use image::ImageEncoder as _;
