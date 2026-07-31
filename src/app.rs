//! Background application: system tray + global hotkey driving an on-demand
//! capture overlay. A single winit event loop hosts everything.

use std::rc::Rc;
use std::time::{Duration, Instant};

use anyhow::Result;
use global_hotkey::hotkey::HotKey;
use global_hotkey::{GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState};
use image::RgbaImage;
use tray_icon::menu::{
    CheckMenuItem, Menu, MenuEvent, MenuId, MenuItem, PredefinedMenuItem, Submenu,
};
use tray_icon::{Icon, TrayIcon, TrayIconBuilder};
use winit::application::ApplicationHandler;
use winit::event::WindowEvent;
use winit::event_loop::{ActiveEventLoop, ControlFlow};
use winit::window::WindowId;

use crate::capture;
use crate::config::{Config, OcrEngine, pretty_hotkey};
use crate::hotkey_capture::{CaptureOutcome, HotkeyCapture};
use crate::ocr;
use crate::overlay::{Outcome, Overlay, Selection};

/// How often we poll the global-hotkey channel, as a safety net.
const POLL_INTERVAL: Duration = Duration::from_millis(100);

/// Events forwarded from tray/menu callbacks into the winit loop.
#[derive(Debug)]
pub enum UserEvent {
    Menu(MenuEvent),
}

enum Mode {
    Idle,
    Selecting(Overlay),
    SettingHotkey(HotkeyCapture),
}

struct TrayHandles {
    _tray: TrayIcon,
    hotkey_label: MenuItem,
    _ocr_submenu: Submenu,
    capture_item: MenuItem,
    google_lens_item: CheckMenuItem,
    oneocr_item: CheckMenuItem,
    screenai_item: CheckMenuItem,
    set_hotkey_item: MenuItem,
    quit_item: MenuItem,
}

pub struct App {
    config: Config,
    hotkey: HotKey,
    manager: Option<GlobalHotKeyManager>,
    tray: Option<TrayHandles>,
    mode: Mode,
    screenshot: Option<Rc<RgbaImage>>,
    initialized: bool,
}

impl App {
    pub fn new() -> Self {
        let config = Config::load();
        let hotkey = config.hotkey;
        Self {
            config,
            hotkey,
            manager: None,
            tray: None,
            mode: Mode::Idle,
            screenshot: None,
            initialized: false,
        }
    }

    fn tooltip(&self) -> String {
        format!(
            "OneShot OCR  -  {} ({})",
            pretty_hotkey(&self.hotkey),
            self.config.engine.display_name()
        )
    }

    fn initialize(&mut self, event_loop: &ActiveEventLoop) {
        if self.initialized {
            return;
        }
        self.initialized = true;

        match GlobalHotKeyManager::new() {
            Ok(manager) => {
                if let Err(e) = manager.register(self.hotkey) {
                    eprintln!("failed to register hotkey: {e}");
                }
                self.manager = Some(manager);
            }
            Err(e) => eprintln!("failed to create hotkey manager: {e}"),
        }

        match self.build_tray() {
            Ok(handles) => self.tray = Some(handles),
            Err(e) => eprintln!("failed to build tray icon: {e}"),
        }

        event_loop.set_control_flow(ControlFlow::WaitUntil(Instant::now() + POLL_INTERVAL));
    }

    fn build_tray(&self) -> Result<TrayHandles> {
        let hotkey_label = MenuItem::new(
            format!("Hotkey: {}", pretty_hotkey(&self.hotkey)),
            false,
            None,
        );
        let capture_item = MenuItem::new("Capture (OCR area)", true, None);
        let google_lens_item = CheckMenuItem::new(
            "Google Lens",
            true,
            self.config.engine == OcrEngine::GoogleLens,
            None,
        );
        let oneocr_item = CheckMenuItem::new(
            "OneOCR",
            true,
            self.config.engine == OcrEngine::OneOcr,
            None,
        );
        let screenai_item = CheckMenuItem::new(
            "ScreenAI",
            true,
            self.config.engine == OcrEngine::ScreenAi,
            None,
        );
        let ocr_submenu = Submenu::new("OCR engine", true);
        ocr_submenu.append(&google_lens_item)?;
        ocr_submenu.append(&oneocr_item)?;
        ocr_submenu.append(&screenai_item)?;
        let set_hotkey_item = MenuItem::new("Set Hotkey...", true, None);
        let quit_item = MenuItem::new("Quit", true, None);

        let menu = Menu::new();
        menu.append(&hotkey_label)?;
        menu.append(&PredefinedMenuItem::separator())?;
        menu.append(&ocr_submenu)?;
        menu.append(&PredefinedMenuItem::separator())?;
        menu.append(&capture_item)?;
        menu.append(&set_hotkey_item)?;
        menu.append(&PredefinedMenuItem::separator())?;
        menu.append(&quit_item)?;

        let tray = TrayIconBuilder::new()
            .with_menu(Box::new(menu))
            .with_tooltip(self.tooltip())
            .with_icon(make_icon())
            .build()?;

        Ok(TrayHandles {
            _tray: tray,
            hotkey_label,
            _ocr_submenu: ocr_submenu,
            capture_item,
            google_lens_item,
            oneocr_item,
            screenai_item,
            set_hotkey_item,
            quit_item,
        })
    }

    fn poll_hotkey(&mut self, event_loop: &ActiveEventLoop) {
        while let Ok(ev) = GlobalHotKeyEvent::receiver().try_recv() {
            if ev.state == HotKeyState::Pressed && ev.id == self.hotkey.id() {
                self.start_capture(event_loop);
            }
        }
    }

    fn start_capture(&mut self, event_loop: &ActiveEventLoop) {
        if !matches!(self.mode, Mode::Idle) {
            return;
        }
        let (screenshot, origin_x, origin_y) = match capture::capture_all() {
            Ok(shot) => (Rc::new(shot.image), shot.origin_x, shot.origin_y),
            Err(e) => {
                eprintln!("screen capture failed: {e}");
                return;
            }
        };
        match Overlay::new(event_loop, screenshot.clone(), origin_x, origin_y) {
            Ok(overlay) => {
                self.screenshot = Some(screenshot);
                self.mode = Mode::Selecting(overlay);
            }
            Err(e) => eprintln!("overlay failed: {e}"),
        }
    }

    fn start_set_hotkey(&mut self, event_loop: &ActiveEventLoop) {
        if !matches!(self.mode, Mode::Idle) {
            return;
        }
        match HotkeyCapture::new(event_loop, &self.hotkey) {
            Ok(capture) => self.mode = Mode::SettingHotkey(capture),
            Err(e) => eprintln!("hotkey capture failed: {e}"),
        }
    }

    fn run_ocr(&self, screenshot: Rc<RgbaImage>, selection: Selection) {
        let (x, y, w, h) = selection;
        let cropped = image::imageops::crop_imm(&*screenshot, x, y, w, h).to_image();
        let language = self.config.language.clone();
        let engine = self.config.engine;
        std::thread::spawn(move || match ocr::ocr_image(&cropped, engine, &language) {
            Ok(text) if !text.trim().is_empty() => match arboard::Clipboard::new() {
                Ok(mut clipboard) => {
                    if let Err(e) = clipboard.set_text(text) {
                        eprintln!("clipboard write failed: {e}");
                    }
                }
                Err(e) => eprintln!("clipboard open failed: {e}"),
            },
            Ok(_) => eprintln!("no text recognized"),
            Err(e) => eprintln!("OCR failed: {e}"),
        });
    }

    fn apply_new_hotkey(&mut self, hotkey: HotKey) {
        if hotkey == self.hotkey {
            return;
        }
        let Some(manager) = &self.manager else {
            self.hotkey = hotkey;
            return;
        };
        let _ = manager.unregister(self.hotkey);
        if let Err(e) = manager.register(hotkey) {
            eprintln!("failed to register new hotkey: {e}");
            let _ = manager.register(self.hotkey);
            return;
        }
        self.hotkey = hotkey;
        self.config.hotkey = hotkey;
        if let Err(e) = self.config.save() {
            eprintln!("failed to save config: {e}");
        }
        if let Some(tray) = &self.tray {
            tray.hotkey_label
                .set_text(format!("Hotkey: {}", pretty_hotkey(&hotkey)));
            let _ = tray._tray.set_tooltip(Some(self.tooltip()));
        }
    }

    fn handle_menu(&mut self, event_loop: &ActiveEventLoop, id: MenuId) {
        let Some(tray) = &self.tray else {
            return;
        };
        if id == *tray.google_lens_item.id() {
            self.select_engine(OcrEngine::GoogleLens);
        } else if id == *tray.oneocr_item.id() {
            self.select_engine(OcrEngine::OneOcr);
        } else if id == *tray.screenai_item.id() {
            self.select_engine(OcrEngine::ScreenAi);
        } else if id == *tray.capture_item.id() {
            self.start_capture(event_loop);
        } else if id == *tray.set_hotkey_item.id() {
            self.start_set_hotkey(event_loop);
        } else if id == *tray.quit_item.id() {
            event_loop.exit();
        }
    }

    fn select_engine(&mut self, engine: OcrEngine) {
        self.config.engine = engine;
        if let Err(e) = self.config.save() {
            eprintln!("failed to save OCR engine: {e}");
        }
        if let Some(tray) = &self.tray {
            tray.google_lens_item
                .set_checked(engine == OcrEngine::GoogleLens);
            tray.oneocr_item.set_checked(engine == OcrEngine::OneOcr);
            tray.screenai_item
                .set_checked(engine == OcrEngine::ScreenAi);
            let _ = tray._tray.set_tooltip(Some(self.tooltip()));
        }
    }
}

impl ApplicationHandler<UserEvent> for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        self.initialize(event_loop);
    }

    fn user_event(&mut self, event_loop: &ActiveEventLoop, event: UserEvent) {
        match event {
            UserEvent::Menu(e) => self.handle_menu(event_loop, e.id),
        }
    }

    fn window_event(&mut self, _event_loop: &ActiveEventLoop, id: WindowId, event: WindowEvent) {
        match &mut self.mode {
            Mode::Selecting(overlay) if overlay.id() == id => match overlay.handle(&event) {
                Outcome::Pending => {}
                Outcome::Cancelled => {
                    self.mode = Mode::Idle;
                    self.screenshot = None;
                }
                Outcome::Selected(selection) => {
                    let screenshot = self.screenshot.take();
                    self.mode = Mode::Idle;
                    if let Some(screenshot) = screenshot {
                        self.run_ocr(screenshot, selection);
                    }
                }
            },
            Mode::SettingHotkey(capture) if capture.id() == id => match capture.handle(&event) {
                CaptureOutcome::Pending => {}
                CaptureOutcome::Cancelled => self.mode = Mode::Idle,
                CaptureOutcome::Captured(hotkey) => {
                    self.apply_new_hotkey(hotkey);
                    self.mode = Mode::Idle;
                }
            },
            _ => {}
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        self.poll_hotkey(event_loop);
        event_loop.set_control_flow(ControlFlow::WaitUntil(Instant::now() + POLL_INTERVAL));
    }
}

/// A simple 32x32 RGBA "lens" icon: a blue disc with a white centre.
fn make_icon() -> Icon {
    const SIZE: u32 = 32;
    let (cx, cy) = (15.5_f32, 15.5_f32);
    let mut rgba = vec![0u8; (SIZE * SIZE * 4) as usize];
    for y in 0..SIZE {
        for x in 0..SIZE {
            let dx = x as f32 - cx;
            let dy = y as f32 - cy;
            let dist = (dx * dx + dy * dy).sqrt();
            let idx = ((y * SIZE + x) * 4) as usize;
            if dist <= 15.0 {
                let (r, g, b) = if dist <= 6.5 {
                    (255, 255, 255)
                } else {
                    (40, 120, 220)
                };
                rgba[idx] = r;
                rgba[idx + 1] = g;
                rgba[idx + 2] = b;
                rgba[idx + 3] = 255;
            }
        }
    }
    Icon::from_rgba(rgba, SIZE, SIZE).expect("valid icon dimensions")
}
