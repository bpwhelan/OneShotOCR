//! ScreenAI native runtime integration.
//!
//! ScreenAI exposes a small C ABI. Its OCR result is a protobuf payload; the
//! parser below intentionally reads only the line/word text fields needed by
//! this clipboard utility, while skipping fields added by newer runtimes.

use anyhow::{Context, Result, bail};
use image::RgbaImage;

#[cfg(windows)]
mod windows_impl {
    use super::*;
    use libloading::Library;
    use std::collections::HashMap;
    use std::ffi::{CStr, c_char, c_void};
    use std::os::windows::ffi::OsStrExt;
    use std::path::PathBuf;
    use std::sync::{Mutex, OnceLock};

    type SetFileContentFunctions = unsafe extern "C" fn(*mut c_void, *mut c_void);
    type InitOcrUsingCallback = unsafe extern "C" fn() -> u8;
    type SetOcrLightMode = unsafe extern "C" fn(u8);
    type PerformOcr = unsafe extern "C" fn(*const SkBitmap, *mut u32) -> *mut c_char;
    type FreeLibraryAllocatedCharArray = unsafe extern "C" fn(*mut c_char);

    #[repr(C)]
    #[derive(Clone, Copy)]
    struct SkColorInfo {
        color_space: *mut c_void,
        color_type: i32,
        alpha_type: i32,
    }

    #[repr(C)]
    #[derive(Clone, Copy)]
    struct SkImageSize {
        width: i32,
        height: i32,
    }

    #[repr(C)]
    #[derive(Clone, Copy)]
    struct SkImageInfo {
        color_info: SkColorInfo,
        dimensions: SkImageSize,
    }

    #[repr(C)]
    #[derive(Clone, Copy)]
    struct SkPixmap {
        pixels: *mut c_void,
        row_bytes: usize,
        info: SkImageInfo,
    }

    #[repr(C)]
    #[derive(Clone, Copy)]
    struct SkBitmap {
        pixel_ref: *mut c_void,
        pixmap: SkPixmap,
        flags: u32,
    }

    struct Api {
        _library: Library,
        set_file_content_functions: SetFileContentFunctions,
        init_ocr: InitOcrUsingCallback,
        set_light_mode: SetOcrLightMode,
        perform_ocr: PerformOcr,
        free_result: FreeLibraryAllocatedCharArray,
    }

    unsafe fn load_symbol<T: Copy>(library: &Library, name: &[u8]) -> Result<T> {
        Ok(*unsafe { library.get::<T>(name) }
            .with_context(|| format!("load ScreenAI symbol {}", String::from_utf8_lossy(name)))?)
    }

    impl Api {
        fn load(path: &PathBuf) -> Result<Self> {
            set_dll_directory(path.parent().unwrap_or(path));
            let library =
                unsafe { Library::new(path).with_context(|| format!("load {}", path.display()))? };
            Ok(Self {
                set_file_content_functions: unsafe {
                    load_symbol(&library, b"SetFileContentFunctions\0")?
                },
                init_ocr: unsafe { load_symbol(&library, b"InitOCRUsingCallback\0")? },
                set_light_mode: unsafe { load_symbol(&library, b"SetOCRLightMode\0")? },
                perform_ocr: unsafe { load_symbol(&library, b"PerformOCR\0")? },
                free_result: unsafe { load_symbol(&library, b"FreeLibraryAllocatedCharArray\0")? },
                _library: library,
            })
        }
    }

    struct ResourceState {
        root: PathBuf,
        cache: Mutex<HashMap<String, Vec<u8>>>,
    }

    static RESOURCES: OnceLock<ResourceState> = OnceLock::new();

    unsafe extern "C" fn resource_size(path: *const c_char) -> u32 {
        resource_bytes(path)
            .map(|bytes| bytes.len() as u32)
            .unwrap_or(0)
    }

    unsafe extern "C" fn resource_data(
        path: *const c_char,
        requested_size: u32,
        target: *mut c_void,
    ) {
        let Some(bytes) = resource_bytes(path) else {
            return;
        };
        let size = bytes.len().min(requested_size as usize);
        if size == 0 || target.is_null() {
            return;
        }
        unsafe {
            std::ptr::copy_nonoverlapping(bytes.as_ptr(), target as *mut u8, size);
        }
    }

    fn resource_bytes(path: *const c_char) -> Option<Vec<u8>> {
        if path.is_null() {
            return None;
        }
        let raw = unsafe { CStr::from_ptr(path) }.to_string_lossy();
        let normalized = normalize_resource_path(&raw)?;
        let state = RESOURCES.get()?;
        if let Ok(cache) = state.cache.lock() {
            if let Some(bytes) = cache.get(&normalized) {
                return Some(bytes.clone());
            }
        }
        let file_path = state.root.join(&normalized);
        let bytes = std::fs::read(file_path).ok()?;
        if let Ok(mut cache) = state.cache.lock() {
            cache.insert(normalized, bytes.clone());
        }
        Some(bytes)
    }

    fn normalize_resource_path(raw: &str) -> Option<String> {
        let normalized = raw.replace('\\', "/");
        let parts: Vec<&str> = normalized
            .trim_matches('/')
            .split('/')
            .filter(|part| !part.is_empty())
            .collect();
        if parts.is_empty() || parts.iter().any(|part| *part == "..") {
            return None;
        }
        Some(parts.join("/"))
    }

    struct Engine {
        api: Api,
    }

    impl Engine {
        fn new() -> Result<Self> {
            let resource_dir = screenai_resource_directory()?;
            let library_path = resource_dir.join("chrome_screen_ai.dll");
            if !library_path.is_file() {
                bail!("ScreenAI runtime not found: {}", library_path.display());
            }
            if !resource_dir.join("files_list_ocr.txt").is_file() {
                bail!(
                    "ScreenAI OCR resource manifest not found: {}",
                    resource_dir.display()
                );
            }
            let _ = RESOURCES.set(ResourceState {
                root: resource_dir,
                cache: Mutex::new(HashMap::new()),
            });

            let api = Api::load(&library_path)?;
            unsafe {
                (api.set_file_content_functions)(
                    resource_size as *mut c_void,
                    resource_data as *mut c_void,
                );
            }
            if unsafe { (api.init_ocr)() } == 0 {
                bail!("ScreenAI InitOCRUsingCallback returned false");
            }
            unsafe { (api.set_light_mode)(0) };
            Ok(Self { api })
        }

        fn recognize(&self, image: &RgbaImage) -> Result<String> {
            let mut pixels = image.as_raw().clone();
            let bitmap = SkBitmap {
                pixel_ref: std::ptr::null_mut(),
                pixmap: SkPixmap {
                    pixels: pixels.as_mut_ptr() as *mut c_void,
                    row_bytes: image.width() as usize * 4,
                    info: SkImageInfo {
                        color_info: SkColorInfo {
                            color_space: std::ptr::null_mut(),
                            color_type: 4,
                            alpha_type: 1,
                        },
                        dimensions: SkImageSize {
                            width: image.width() as i32,
                            height: image.height() as i32,
                        },
                    },
                },
                flags: 0,
            };
            let mut output_length = 0_u32;
            let result = unsafe { (self.api.perform_ocr)(&bitmap, &mut output_length) };
            if result.is_null() || output_length == 0 {
                return Ok(String::new());
            }
            let payload =
                unsafe { std::slice::from_raw_parts(result as *const u8, output_length as usize) };
            let parsed = parse_visual_annotation(payload);
            unsafe { (self.api.free_result)(result) };
            parsed
        }
    }

    fn screenai_resource_directory() -> Result<PathBuf> {
        let home = std::env::var_os("USERPROFILE")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(PathBuf::from))
            .context("find the user profile directory")?;
        let root = home.join(".config").join("screen_ai");
        let candidates = [root.join("resources"), root];
        candidates
            .into_iter()
            .find(|path| path.join("chrome_screen_ai.dll").is_file())
            .context("ScreenAI resources not found under ~/.config/screen_ai")
    }

    fn set_dll_directory(directory: &std::path::Path) {
        let mut wide: Vec<u16> = directory
            .as_os_str()
            .encode_wide()
            .chain(std::iter::once(0))
            .collect();
        unsafe {
            let _ = windows_sys::Win32::System::LibraryLoader::SetDllDirectoryW(wide.as_mut_ptr());
        }
    }

    #[derive(Default)]
    struct Rect {
        x: i64,
        y: i64,
    }

    struct Line {
        text: String,
        rect: Rect,
    }

    struct ProtoReader<'a> {
        bytes: &'a [u8],
        position: usize,
    }

    impl<'a> ProtoReader<'a> {
        fn new(bytes: &'a [u8]) -> Self {
            Self { bytes, position: 0 }
        }

        fn eof(&self) -> bool {
            self.position >= self.bytes.len()
        }

        fn varint(&mut self) -> Result<u64> {
            let mut value = 0_u64;
            let mut shift = 0_u32;
            loop {
                if self.position >= self.bytes.len() || shift > 63 {
                    bail!("invalid ScreenAI protobuf varint");
                }
                let byte = self.bytes[self.position];
                self.position += 1;
                value |= u64::from(byte & 0x7f) << shift;
                if byte & 0x80 == 0 {
                    return Ok(value);
                }
                shift += 7;
            }
        }

        fn bytes(&mut self) -> Result<&'a [u8]> {
            let length = self.varint()? as usize;
            let end = self
                .position
                .checked_add(length)
                .context("ScreenAI field length overflow")?;
            if end > self.bytes.len() {
                bail!("truncated ScreenAI protobuf field");
            }
            let result = &self.bytes[self.position..end];
            self.position = end;
            Ok(result)
        }

        fn fixed32(&mut self) -> Result<u32> {
            if self.position + 4 > self.bytes.len() {
                bail!("truncated ScreenAI protobuf float");
            }
            let result =
                u32::from_le_bytes(self.bytes[self.position..self.position + 4].try_into()?);
            self.position += 4;
            Ok(result)
        }

        fn skip(&mut self, wire_type: u64) -> Result<()> {
            match wire_type {
                0 => {
                    let _ = self.varint()?;
                }
                1 => {
                    if self.position + 8 > self.bytes.len() {
                        bail!("truncated ScreenAI fixed64 field");
                    }
                    self.position += 8;
                }
                2 => {
                    let _ = self.bytes()?;
                }
                5 => {
                    let _ = self.fixed32()?;
                }
                _ => bail!("unsupported ScreenAI protobuf wire type {wire_type}"),
            }
            Ok(())
        }
    }

    fn read_tag(reader: &mut ProtoReader<'_>) -> Result<(u64, u64)> {
        let tag = reader.varint()?;
        Ok((tag >> 3, tag & 7))
    }

    fn parse_rect(bytes: &[u8]) -> Result<Rect> {
        let mut reader = ProtoReader::new(bytes);
        let mut rect = Rect::default();
        while !reader.eof() {
            let (field, wire) = read_tag(&mut reader)?;
            if wire == 0 && (field == 1 || field == 2) {
                let value = reader.varint()? as i64;
                if field == 1 {
                    rect.x = value;
                } else {
                    rect.y = value;
                }
            } else {
                reader.skip(wire)?;
            }
        }
        Ok(rect)
    }

    fn parse_word(bytes: &[u8]) -> Result<(String, bool)> {
        let mut reader = ProtoReader::new(bytes);
        let mut text = String::new();
        let mut has_space_after = false;
        while !reader.eof() {
            let (field, wire) = read_tag(&mut reader)?;
            match (field, wire) {
                (3, 2) => text = sanitize(std::str::from_utf8(reader.bytes()?).unwrap_or_default()),
                (6, 0) => has_space_after = reader.varint()? != 0,
                _ => reader.skip(wire)?,
            }
        }
        Ok((text, has_space_after))
    }

    fn parse_line(bytes: &[u8]) -> Result<Line> {
        let mut reader = ProtoReader::new(bytes);
        let mut text = String::new();
        let mut rect = Rect::default();
        let mut words = Vec::new();
        while !reader.eof() {
            let (field, wire) = read_tag(&mut reader)?;
            match (field, wire) {
                (1, 2) => words.push(parse_word(reader.bytes()?)?),
                (2, 2) => rect = parse_rect(reader.bytes()?)?,
                (3, 2) => text = sanitize(std::str::from_utf8(reader.bytes()?).unwrap_or_default()),
                _ => reader.skip(wire)?,
            }
        }
        if text.is_empty() {
            text = words
                .iter()
                .map(|(word, space)| {
                    if *space {
                        format!("{word} ")
                    } else {
                        word.clone()
                    }
                })
                .collect::<String>()
                .trim_end()
                .to_string();
        }
        Ok(Line { text, rect })
    }

    fn parse_visual_annotation(bytes: &[u8]) -> Result<String> {
        let mut reader = ProtoReader::new(bytes);
        let mut lines = Vec::new();
        while !reader.eof() {
            let (field, wire) = read_tag(&mut reader)?;
            if field == 2 && wire == 2 {
                let line = parse_line(reader.bytes()?)?;
                if !line.text.trim().is_empty() {
                    lines.push(line);
                }
            } else {
                reader.skip(wire)?;
            }
        }
        lines.sort_by_key(|line| (line.rect.y, line.rect.x));
        Ok(lines
            .into_iter()
            .map(|line| line.text.trim().to_string())
            .filter(|line| !line.is_empty())
            .collect::<Vec<_>>()
            .join("\n"))
    }

    fn sanitize(text: &str) -> String {
        text.replace(['\r', '\n'], "")
    }

    static ENGINE: OnceLock<Mutex<Option<Engine>>> = OnceLock::new();

    pub fn ocr_image(image: &RgbaImage) -> Result<String> {
        let lock = ENGINE.get_or_init(|| Mutex::new(None));
        let mut guard = lock
            .lock()
            .map_err(|_| anyhow::anyhow!("ScreenAI engine lock poisoned"))?;
        if guard.is_none() {
            *guard = Some(Engine::new()?);
        }
        guard
            .as_ref()
            .expect("ScreenAI engine initialized")
            .recognize(image)
    }
}

#[cfg(windows)]
pub fn ocr_image(image: &RgbaImage) -> Result<String> {
    windows_impl::ocr_image(image)
}

#[cfg(not(windows))]
pub fn ocr_image(_image: &RgbaImage) -> Result<String> {
    bail!("ScreenAI is only available on Windows in this application")
}
