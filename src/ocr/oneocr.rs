//! OneOCR native runtime integration.
//!
//! This mirrors the small C ABI exposed by the `oneocr` Python package, but
//! calls the DLL directly so the tray application does not need an OCR HTTP
//! server or a Python process.

use anyhow::{Context, Result, bail};
use image::RgbaImage;

#[cfg(windows)]
mod windows_impl {
    use super::*;
    use libloading::Library;
    use std::ffi::{CStr, CString, c_char};
    use std::os::windows::ffi::OsStrExt;
    use std::path::PathBuf;
    use std::sync::{Mutex, OnceLock};

    const DLL_NAME: &str = "oneocr.dll";
    const MODEL_NAME: &str = "oneocr.onemodel";
    const MODEL_KEY: &[u8] = b"kj)TGtrK>f]b[Piow.gU+nC@s\"\"\"\"\"\"4";

    type CreateOcrInitOptions = unsafe extern "system" fn(*mut i64) -> i64;
    type OcrInitOptionsSetUseModelDelayLoad = unsafe extern "system" fn(i64, i8) -> i64;
    type CreateOcrPipeline =
        unsafe extern "system" fn(*const c_char, *const c_char, i64, *mut i64) -> i64;
    type CreateOcrProcessOptions = unsafe extern "system" fn(*mut i64) -> i64;
    type OcrProcessOptionsSetMaxRecognitionLineCount = unsafe extern "system" fn(i64, i64) -> i64;
    type RunOcrPipeline =
        unsafe extern "system" fn(i64, *const ImageStructure, i64, *mut i64) -> i64;
    type GetOcrLineCount = unsafe extern "system" fn(i64, *mut i64) -> i64;
    type GetOcrLine = unsafe extern "system" fn(i64, i64, *mut i64) -> i64;
    type GetOcrLineContent = unsafe extern "system" fn(i64, *mut *const c_char) -> i64;
    type ReleaseOcrResult = unsafe extern "system" fn(i64);
    type ReleaseOcrInitOptions = unsafe extern "system" fn(i64);
    type ReleaseOcrPipeline = unsafe extern "system" fn(i64);
    type ReleaseOcrProcessOptions = unsafe extern "system" fn(i64);

    #[repr(C)]
    struct ImageStructure {
        image_type: i32,
        width: i32,
        height: i32,
        reserved: i32,
        step_size: i64,
        data_ptr: *mut u8,
    }

    struct Api {
        _library: Library,
        create_init_options: CreateOcrInitOptions,
        set_delay_load: OcrInitOptionsSetUseModelDelayLoad,
        create_pipeline: CreateOcrPipeline,
        create_process_options: CreateOcrProcessOptions,
        set_max_line_count: OcrProcessOptionsSetMaxRecognitionLineCount,
        run_pipeline: RunOcrPipeline,
        get_line_count: GetOcrLineCount,
        get_line: GetOcrLine,
        get_line_content: GetOcrLineContent,
        release_result: ReleaseOcrResult,
        release_init_options: ReleaseOcrInitOptions,
        release_pipeline: ReleaseOcrPipeline,
        release_process_options: ReleaseOcrProcessOptions,
    }

    unsafe fn load_symbol<T: Copy>(library: &Library, name: &[u8]) -> Result<T> {
        // The symbol is copied out while the Library is retained in Api.
        Ok(*unsafe { library.get::<T>(name) }
            .with_context(|| format!("load OneOCR symbol {}", String::from_utf8_lossy(name)))?)
    }

    impl Api {
        fn load(directory: &PathBuf) -> Result<Self> {
            set_dll_directory(directory);
            let library = unsafe {
                Library::new(directory.join(DLL_NAME))
                    .with_context(|| format!("load {}", directory.join(DLL_NAME).display()))?
            };

            Ok(Self {
                create_init_options: unsafe { load_symbol(&library, b"CreateOcrInitOptions\0")? },
                set_delay_load: unsafe {
                    load_symbol(&library, b"OcrInitOptionsSetUseModelDelayLoad\0")?
                },
                create_pipeline: unsafe { load_symbol(&library, b"CreateOcrPipeline\0")? },
                create_process_options: unsafe {
                    load_symbol(&library, b"CreateOcrProcessOptions\0")?
                },
                set_max_line_count: unsafe {
                    load_symbol(&library, b"OcrProcessOptionsSetMaxRecognitionLineCount\0")?
                },
                run_pipeline: unsafe { load_symbol(&library, b"RunOcrPipeline\0")? },
                get_line_count: unsafe { load_symbol(&library, b"GetOcrLineCount\0")? },
                get_line: unsafe { load_symbol(&library, b"GetOcrLine\0")? },
                get_line_content: unsafe { load_symbol(&library, b"GetOcrLineContent\0")? },
                release_result: unsafe { load_symbol(&library, b"ReleaseOcrResult\0")? },
                release_init_options: unsafe { load_symbol(&library, b"ReleaseOcrInitOptions\0")? },
                release_pipeline: unsafe { load_symbol(&library, b"ReleaseOcrPipeline\0")? },
                release_process_options: unsafe {
                    load_symbol(&library, b"ReleaseOcrProcessOptions\0")?
                },
                _library: library,
            })
        }
    }

    struct Engine {
        api: Api,
        init_options: i64,
        pipeline: i64,
        process_options: i64,
    }

    impl Engine {
        fn new() -> Result<Self> {
            let directory = oneocr_directory()?;
            for required in [DLL_NAME, MODEL_NAME] {
                if !directory.join(required).is_file() {
                    bail!(
                        "OneOCR file not found: {}",
                        directory.join(required).display()
                    );
                }
            }

            let api = Api::load(&directory)?;
            let mut init_options = 0_i64;
            check(
                unsafe { (api.create_init_options)(&mut init_options) },
                "create init options",
            )?;
            check(
                unsafe { (api.set_delay_load)(init_options, 0) },
                "configure model loading",
            )?;

            let model_path = CString::new(directory.join(MODEL_NAME).to_string_lossy().as_bytes())
                .context("encode OneOCR model path")?;
            let model_key = CString::new(MODEL_KEY).context("encode OneOCR model key")?;
            let mut pipeline = 0_i64;
            if let Err(error) = check(
                unsafe {
                    (api.create_pipeline)(
                        model_path.as_ptr(),
                        model_key.as_ptr(),
                        init_options,
                        &mut pipeline,
                    )
                },
                "create pipeline",
            ) {
                unsafe { (api.release_init_options)(init_options) };
                return Err(error);
            }

            let mut process_options = 0_i64;
            if let Err(error) = check(
                unsafe { (api.create_process_options)(&mut process_options) },
                "create process options",
            ) {
                unsafe {
                    (api.release_pipeline)(pipeline);
                    (api.release_init_options)(init_options);
                }
                return Err(error);
            }
            if let Err(error) = check(
                unsafe { (api.set_max_line_count)(process_options, 1000) },
                "configure maximum line count",
            ) {
                unsafe {
                    (api.release_process_options)(process_options);
                    (api.release_pipeline)(pipeline);
                    (api.release_init_options)(init_options);
                }
                return Err(error);
            }

            Ok(Self {
                api,
                init_options,
                pipeline,
                process_options,
            })
        }

        fn recognize(&self, image: &RgbaImage) -> Result<String> {
            let image = prepare_image(image);
            let mut bgra = Vec::with_capacity(image.as_raw().len());
            for pixel in image.pixels() {
                let [r, g, b, a] = pixel.0;
                bgra.extend_from_slice(&[b, g, r, a]);
            }

            let structure = ImageStructure {
                image_type: 3,
                width: image.width() as i32,
                height: image.height() as i32,
                reserved: 0,
                step_size: (image.width() * 4) as i64,
                data_ptr: bgra.as_ptr() as *mut u8,
            };
            let mut result = 0_i64;
            check(
                unsafe {
                    (self.api.run_pipeline)(
                        self.pipeline,
                        &structure,
                        self.process_options,
                        &mut result,
                    )
                },
                "run OneOCR",
            )?;

            let parsed = self.read_result(result);
            unsafe { (self.api.release_result)(result) };
            parsed
        }

        fn read_result(&self, result: i64) -> Result<String> {
            let mut count = 0_i64;
            check(
                unsafe { (self.api.get_line_count)(result, &mut count) },
                "read OneOCR line count",
            )?;
            let mut lines = Vec::new();
            for index in 0..count.max(0) {
                let mut line = 0_i64;
                if unsafe { (self.api.get_line)(result, index, &mut line) } != 0 {
                    continue;
                }
                let mut content: *const c_char = std::ptr::null();
                if unsafe { (self.api.get_line_content)(line, &mut content) } == 0
                    && !content.is_null()
                {
                    let text = unsafe { CStr::from_ptr(content) }
                        .to_string_lossy()
                        .trim()
                        .to_string();
                    if !text.is_empty() {
                        lines.push(text);
                    }
                }
            }
            Ok(lines.join("\n"))
        }
    }

    impl Drop for Engine {
        fn drop(&mut self) {
            unsafe {
                if self.process_options != 0 {
                    (self.api.release_process_options)(self.process_options);
                }
                if self.pipeline != 0 {
                    (self.api.release_pipeline)(self.pipeline);
                }
                if self.init_options != 0 {
                    (self.api.release_init_options)(self.init_options);
                }
            }
        }
    }

    fn check(code: i64, operation: &str) -> Result<()> {
        if code == 0 {
            Ok(())
        } else {
            bail!("{operation} failed (OneOCR code {code})")
        }
    }

    fn prepare_image(image: &RgbaImage) -> RgbaImage {
        let mut width = image.width();
        let mut height = image.height();
        if width < 51 || height < 51 {
            width = width.max(51);
            height = height.max(51);
            let mut padded = RgbaImage::new(width, height);
            image::imageops::overlay(
                &mut padded,
                image,
                ((width - image.width()) / 2) as i64,
                ((height - image.height()) / 2) as i64,
            );
            return padded;
        }
        if width > 10_000 || height > 10_000 {
            let scale = (10_000.0 / width as f64).min(10_000.0 / height as f64);
            width = ((width as f64 * scale).round() as u32).max(1);
            height = ((height as f64 * scale).round() as u32).max(1);
            return image::imageops::resize(
                image,
                width,
                height,
                image::imageops::FilterType::Lanczos3,
            );
        }
        image.clone()
    }

    fn oneocr_directory() -> Result<PathBuf> {
        let home = std::env::var_os("USERPROFILE")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(PathBuf::from))
            .context("find the user profile directory")?;
        Ok(home.join(".config").join("oneocr"))
    }

    fn set_dll_directory(directory: &PathBuf) {
        let mut wide: Vec<u16> = directory
            .as_os_str()
            .encode_wide()
            .chain(std::iter::once(0))
            .collect();
        unsafe {
            let _ = windows_sys::Win32::System::LibraryLoader::SetDllDirectoryW(wide.as_mut_ptr());
        }
    }

    static ENGINE: OnceLock<Mutex<Option<Engine>>> = OnceLock::new();

    pub fn ocr_image(image: &RgbaImage) -> Result<String> {
        let lock = ENGINE.get_or_init(|| Mutex::new(None));
        let mut guard = lock
            .lock()
            .map_err(|_| anyhow::anyhow!("OneOCR engine lock poisoned"))?;
        if guard.is_none() {
            *guard = Some(Engine::new()?);
        }
        guard
            .as_ref()
            .expect("OneOCR engine initialized")
            .recognize(image)
    }
}

#[cfg(windows)]
pub fn ocr_image(image: &RgbaImage) -> Result<String> {
    windows_impl::ocr_image(image)
}

#[cfg(not(windows))]
pub fn ocr_image(_image: &RgbaImage) -> Result<String> {
    bail!("OneOCR is only available on Windows 10 and newer")
}
