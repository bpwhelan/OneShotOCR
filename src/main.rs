#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

//! OneShot OCR: a background tray utility that grabs a screen region on a
//! global hotkey, OCRs it with the selected engine, and copies the text to the
//! clipboard.

mod app;
mod capture;
mod config;
mod hotkey_capture;
mod lens;
mod ocr;
mod overlay;

use anyhow::{Context, Result};
use tray_icon::menu::MenuEvent;
use winit::event_loop::{ControlFlow, EventLoop};

use app::{App, UserEvent};

fn main() -> Result<()> {
    let event_loop = EventLoop::<UserEvent>::with_user_event()
        .build()
        .context("create event loop")?;
    event_loop.set_control_flow(ControlFlow::Wait);

    // Forward tray menu clicks into the winit loop, waking it immediately.
    let proxy = event_loop.create_proxy();
    MenuEvent::set_event_handler(Some(move |event| {
        let _ = proxy.send_event(UserEvent::Menu(event));
    }));

    let mut app = App::new();
    event_loop.run_app(&mut app).context("run event loop")?;
    Ok(())
}
