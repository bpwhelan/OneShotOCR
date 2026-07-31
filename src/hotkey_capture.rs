//! Small window that captures the next key combination as a new global hotkey.
//!
//! The instructions live in the title bar (there is no text rendering), the
//! body is filled with a solid colour. Esc cancels. A key press builds a
//! [`HotKey`] string from the current modifiers plus the pressed key and parses
//! it, which naturally supports F1-F24.

use std::num::NonZeroU32;
use std::rc::Rc;
use std::str::FromStr;

use anyhow::{Context, Result};
use global_hotkey::hotkey::HotKey;
use winit::dpi::LogicalSize;
use winit::event::{ElementState, WindowEvent};
use winit::event_loop::ActiveEventLoop;
use winit::keyboard::{KeyCode, ModifiersState, PhysicalKey};
use winit::window::{Window, WindowId};

use crate::config::pretty_hotkey;

type Surface = softbuffer::Surface<Rc<Window>, Rc<Window>>;

pub enum CaptureOutcome {
    Pending,
    Cancelled,
    Captured(HotKey),
}

pub struct HotkeyCapture {
    window: Rc<Window>,
    surface: Surface,
    modifiers: ModifiersState,
}

impl HotkeyCapture {
    pub fn new(event_loop: &ActiveEventLoop, current: &HotKey) -> Result<Self> {
        let attrs = Window::default_attributes()
            .with_title(format!(
                "Press a hotkey  (current: {})   -   Esc to cancel",
                pretty_hotkey(current)
            ))
            .with_inner_size(LogicalSize::new(520.0, 130.0))
            .with_resizable(false);

        let window = Rc::new(
            event_loop
                .create_window(attrs)
                .context("create hotkey window")?,
        );
        window.focus_window();

        let context = softbuffer::Context::new(window.clone())
            .map_err(|e| anyhow::anyhow!("softbuffer context: {e}"))?;
        let surface = softbuffer::Surface::new(&context, window.clone())
            .map_err(|e| anyhow::anyhow!("softbuffer surface: {e}"))?;

        let mut capture = Self {
            window,
            surface,
            modifiers: ModifiersState::empty(),
        };
        capture.fill();
        Ok(capture)
    }

    pub fn id(&self) -> WindowId {
        self.window.id()
    }

    fn fill(&mut self) {
        let size = self.window.inner_size();
        let (w, h) = (size.width.max(1), size.height.max(1));
        if let (Some(nw), Some(nh)) = (NonZeroU32::new(w), NonZeroU32::new(h)) {
            let _ = self.surface.resize(nw, nh);
        }
        if let Ok(mut buffer) = self.surface.buffer_mut() {
            for px in buffer.iter_mut() {
                *px = 0x0020_2830;
            }
            let _ = buffer.present();
        }
    }

    pub fn handle(&mut self, event: &WindowEvent) -> CaptureOutcome {
        match event {
            WindowEvent::CloseRequested => CaptureOutcome::Cancelled,
            WindowEvent::ModifiersChanged(modifiers) => {
                self.modifiers = modifiers.state();
                CaptureOutcome::Pending
            }
            WindowEvent::RedrawRequested => {
                self.fill();
                CaptureOutcome::Pending
            }
            WindowEvent::KeyboardInput { event, .. } => {
                if event.state != ElementState::Pressed {
                    return CaptureOutcome::Pending;
                }
                let PhysicalKey::Code(code) = event.physical_key else {
                    return CaptureOutcome::Pending;
                };
                if code == KeyCode::Escape {
                    return CaptureOutcome::Cancelled;
                }
                if is_modifier(code) {
                    return CaptureOutcome::Pending;
                }
                match build_hotkey(self.modifiers, code) {
                    Some(hk) => CaptureOutcome::Captured(hk),
                    None => CaptureOutcome::Pending,
                }
            }
            _ => CaptureOutcome::Pending,
        }
    }
}

fn is_modifier(code: KeyCode) -> bool {
    matches!(
        code,
        KeyCode::ShiftLeft
            | KeyCode::ShiftRight
            | KeyCode::ControlLeft
            | KeyCode::ControlRight
            | KeyCode::AltLeft
            | KeyCode::AltRight
            | KeyCode::SuperLeft
            | KeyCode::SuperRight
    )
}

/// Build a [`HotKey`] from winit modifiers plus a physical key, by assembling
/// the string form that `global-hotkey` parses. The winit `KeyCode` debug name
/// (e.g. `F13`, `KeyA`, `Digit1`) matches the parser's key tokens.
fn build_hotkey(mods: ModifiersState, code: KeyCode) -> Option<HotKey> {
    let mut s = String::new();
    if mods.shift_key() {
        s.push_str("shift+");
    }
    if mods.control_key() {
        s.push_str("control+");
    }
    if mods.alt_key() {
        s.push_str("alt+");
    }
    if mods.super_key() {
        s.push_str("super+");
    }
    s.push_str(&format!("{code:?}"));
    HotKey::from_str(&s).ok()
}
