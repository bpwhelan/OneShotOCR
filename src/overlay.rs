//! Fullscreen drag-to-select overlay, driven by the shared event loop.
//!
//! Freezes a captured screenshot on a borderless fullscreen window, dims it,
//! and lets the user drag a rectangle. Reports the selection in the coordinate
//! space of the original screenshot. Esc / right-click cancels.

use std::num::NonZeroU32;
use std::rc::Rc;

use anyhow::{Context, Result};
use image::RgbaImage;
use winit::dpi::{PhysicalPosition, PhysicalSize};
use winit::event::{ElementState, MouseButton, WindowEvent};
use winit::event_loop::ActiveEventLoop;
use winit::keyboard::{Key, NamedKey};
use winit::window::{CursorIcon, Window, WindowId, WindowLevel};

const BORDER_COLOR: u32 = 0x00FF_3B30; // red-ish, 0RGB
const DIM_PERCENT: u32 = 45; // brightness of the non-selected area

type Surface = softbuffer::Surface<Rc<Window>, Rc<Window>>;

/// Selection in original screenshot pixels: (x, y, width, height).
pub type Selection = (u32, u32, u32, u32);

/// Result of feeding an event to the overlay.
pub enum Outcome {
    /// Still selecting.
    Pending,
    /// User cancelled (Esc, right-click, or window closed).
    Cancelled,
    /// User picked a region.
    Selected(Selection),
}

/// A live selection overlay owning its own window and pixel buffers.
pub struct Overlay {
    screenshot: Rc<RgbaImage>,
    window: Rc<Window>,
    surface: Surface,
    disp_w: u32,
    disp_h: u32,
    bright: Vec<u32>,
    dark: Vec<u32>,
    start: Option<(f64, f64)>,
    current: Option<(f64, f64)>,
    dragging: bool,
}

impl Overlay {
    /// Create the overlay window inside the shared event loop, spanning the
    /// whole virtual desktop starting at `(origin_x, origin_y)` (physical px).
    pub fn new(
        event_loop: &ActiveEventLoop,
        screenshot: Rc<RgbaImage>,
        origin_x: i32,
        origin_y: i32,
    ) -> Result<Self> {
        let attrs = Window::default_attributes()
            .with_title("OneShot OCR")
            .with_decorations(false)
            .with_visible(false)
            .with_resizable(false)
            .with_window_level(WindowLevel::AlwaysOnTop)
            .with_position(PhysicalPosition::new(origin_x, origin_y))
            .with_inner_size(PhysicalSize::new(screenshot.width(), screenshot.height()));

        let window = Rc::new(
            event_loop
                .create_window(attrs)
                .context("create overlay window")?,
        );
        window.set_cursor(CursorIcon::Crosshair);
        disable_window_animations(&window);

        let context = softbuffer::Context::new(window.clone())
            .map_err(|e| anyhow::anyhow!("softbuffer context: {e}"))?;
        let surface = softbuffer::Surface::new(&context, window.clone())
            .map_err(|e| anyhow::anyhow!("softbuffer surface: {e}"))?;

        let mut overlay = Self {
            screenshot,
            window,
            surface,
            disp_w: 0,
            disp_h: 0,
            bright: Vec::new(),
            dark: Vec::new(),
            start: None,
            current: None,
            dragging: false,
        };

        let size = overlay.window.inner_size();
        let (w, h) = (size.width.max(1), size.height.max(1));
        overlay.resize_surface(w, h);
        overlay.build_buffers(w, h);
        // Paint the frozen screenshot before the window is shown so it appears
        // instantly with no fade/scale transition.
        overlay.redraw();
        overlay.window.set_visible(true);
        overlay.window.focus_window();
        Ok(overlay)
    }

    pub fn id(&self) -> WindowId {
        self.window.id()
    }

    fn resize_surface(&mut self, w: u32, h: u32) {
        if let (Some(nw), Some(nh)) = (NonZeroU32::new(w), NonZeroU32::new(h)) {
            let _ = self.surface.resize(nw, nh);
        }
    }

    fn build_buffers(&mut self, w: u32, h: u32) {
        use std::borrow::Cow;
        let src: Cow<RgbaImage> = if w == self.screenshot.width() && h == self.screenshot.height() {
            Cow::Borrowed(&self.screenshot)
        } else {
            Cow::Owned(image::imageops::resize(
                &*self.screenshot,
                w,
                h,
                image::imageops::FilterType::Triangle,
            ))
        };

        let n = (w as usize) * (h as usize);
        let mut bright = vec![0u32; n];
        let mut dark = vec![0u32; n];
        for (i, px) in src.pixels().enumerate() {
            let [r, g, b, _] = px.0;
            bright[i] = ((r as u32) << 16) | ((g as u32) << 8) | b as u32;
            let dr = (r as u32 * DIM_PERCENT) / 100;
            let dg = (g as u32 * DIM_PERCENT) / 100;
            let db = (b as u32 * DIM_PERCENT) / 100;
            dark[i] = (dr << 16) | (dg << 8) | db;
        }

        self.disp_w = w;
        self.disp_h = h;
        self.bright = bright;
        self.dark = dark;
    }

    /// Current selection in display coordinates, normalized and clamped.
    fn selection_rect(&self) -> Option<(u32, u32, u32, u32)> {
        let (sx, sy) = self.start?;
        let (cx, cy) = self.current?;
        let max_x = (self.disp_w.saturating_sub(1)) as f64;
        let max_y = (self.disp_h.saturating_sub(1)) as f64;
        let x0 = sx.min(cx).clamp(0.0, max_x) as u32;
        let y0 = sy.min(cy).clamp(0.0, max_y) as u32;
        let x1 = sx.max(cx).clamp(0.0, max_x) as u32;
        let y1 = sy.max(cy).clamp(0.0, max_y) as u32;
        if x1 > x0 && y1 > y0 {
            Some((x0, y0, x1 + 1, y1 + 1))
        } else {
            None
        }
    }

    fn redraw(&mut self) {
        let (w, h) = (self.disp_w, self.disp_h);
        if w == 0 || h == 0 {
            return;
        }
        let selection = self.selection_rect();
        let mut buffer = match self.surface.buffer_mut() {
            Ok(b) => b,
            Err(_) => return,
        };
        if buffer.len() != self.dark.len() {
            return;
        }
        buffer.copy_from_slice(&self.dark);

        if let Some((x0, y0, x1, y1)) = selection {
            for y in y0..y1 {
                let row = (y * w) as usize;
                let s = row + x0 as usize;
                let e = row + x1 as usize;
                buffer[s..e].copy_from_slice(&self.bright[s..e]);
            }
            for x in x0..x1 {
                buffer[(y0 * w + x) as usize] = BORDER_COLOR;
                buffer[((y1 - 1) * w + x) as usize] = BORDER_COLOR;
            }
            for y in y0..y1 {
                buffer[(y * w + x0) as usize] = BORDER_COLOR;
                buffer[(y * w + (x1 - 1)) as usize] = BORDER_COLOR;
            }
        }

        let _ = buffer.present();
    }

    /// Map the current display-space rect to original screenshot coordinates.
    fn finalize(&self) -> Outcome {
        let Some((x0, y0, x1, y1)) = self.selection_rect() else {
            return Outcome::Cancelled;
        };
        if x1 - x0 < 3 || y1 - y0 < 3 {
            return Outcome::Cancelled;
        }
        let scale_x = self.screenshot.width() as f64 / self.disp_w as f64;
        let scale_y = self.screenshot.height() as f64 / self.disp_h as f64;
        let ox = (x0 as f64 * scale_x).round() as u32;
        let oy = (y0 as f64 * scale_y).round() as u32;
        let ex = (x1 as f64 * scale_x).round() as u32;
        let ey = (y1 as f64 * scale_y).round() as u32;
        let ox = ox.min(self.screenshot.width().saturating_sub(1));
        let oy = oy.min(self.screenshot.height().saturating_sub(1));
        let ew = (ex.min(self.screenshot.width()) - ox).max(1);
        let eh = (ey.min(self.screenshot.height()) - oy).max(1);
        Outcome::Selected((ox, oy, ew, eh))
    }

    /// Feed a window event; returns the outcome of the selection.
    pub fn handle(&mut self, event: &WindowEvent) -> Outcome {
        match event {
            WindowEvent::CloseRequested => Outcome::Cancelled,
            WindowEvent::Resized(size) => {
                let (w, h) = (size.width.max(1), size.height.max(1));
                self.resize_surface(w, h);
                if w != self.disp_w || h != self.disp_h {
                    self.build_buffers(w, h);
                }
                self.window.request_redraw();
                Outcome::Pending
            }
            WindowEvent::CursorMoved { position, .. } => {
                self.current = Some((position.x, position.y));
                if self.dragging {
                    self.window.request_redraw();
                }
                Outcome::Pending
            }
            WindowEvent::MouseInput { state, button, .. } => match (button, state) {
                (MouseButton::Left, ElementState::Pressed) => {
                    self.start = self.current;
                    self.dragging = true;
                    Outcome::Pending
                }
                (MouseButton::Left, ElementState::Released) => {
                    self.dragging = false;
                    self.finalize()
                }
                (MouseButton::Right, ElementState::Pressed) => Outcome::Cancelled,
                _ => Outcome::Pending,
            },
            WindowEvent::KeyboardInput { event, .. } => {
                if event.state == ElementState::Pressed
                    && event.logical_key == Key::Named(NamedKey::Escape)
                {
                    Outcome::Cancelled
                } else {
                    Outcome::Pending
                }
            }
            WindowEvent::RedrawRequested => {
                self.redraw();
                Outcome::Pending
            }
            _ => Outcome::Pending,
        }
    }
}

/// Disable the DWM open/close transition so the overlay appears instantly.
#[cfg(target_os = "windows")]
fn disable_window_animations(window: &Window) {
    use windows_sys::Win32::Graphics::Dwm::{
        DWMWA_TRANSITIONS_FORCEDISABLED, DwmSetWindowAttribute,
    };
    use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};

    let Ok(handle) = window.window_handle() else {
        return;
    };
    let RawWindowHandle::Win32(win32) = handle.as_raw() else {
        return;
    };
    let hwnd = win32.hwnd.get() as *mut core::ffi::c_void;
    let disable: i32 = 1; // TRUE
    unsafe {
        DwmSetWindowAttribute(
            hwnd,
            DWMWA_TRANSITIONS_FORCEDISABLED as u32,
            &disable as *const i32 as *const core::ffi::c_void,
            std::mem::size_of::<i32>() as u32,
        );
    }
}

#[cfg(not(target_os = "windows"))]
fn disable_window_animations(_window: &Window) {}
