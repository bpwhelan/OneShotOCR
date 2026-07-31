<h1 align="center">OneShot OCR</h1>

<p align="center">
  <b>Select it. Recognize it. Paste it.</b><br>
  A tiny Windows tray utility for one-off OCR.
</p>

<p align="center">
  <img src=".github/files/oneshot_demo.avif" width="900" alt="OneShot OCR capture demo">
</p>

---

OneShot OCR is a tiny standalone **background tray utility** for one-off text
capture: press a global hotkey → drag-select a screen area → OCR it → the text
lands on your clipboard. That's it—there are no plans for extra functionality.

It supports Google Lens plus local OneOCR and ScreenAI runtimes. The local
engines are called directly through their native DLL APIs; no OCR web server or
Python process is required.

OCR results are language-agnostic and work with any language supported by the
selected engine (a lot of them).

## Installation & usage

**[Download the latest Windows release](../../releases/latest)**, then place
`OneShot-OCR-Windows-x64.exe` wherever you want to keep it and run it. There is
no installer; OneShot OCR stays running in the background with a tray icon.

The OneShot OCR icon appears in the system tray. To capture text:

1. Press the global hotkey (default **Ctrl+Shift+O**) — or right-click the tray
   icon and choose **Capture (OCR area)**.
2. The primary monitor freezes and dims. Drag a rectangle over the text.
3. Release the mouse — the selection is OCR'd and copied to your clipboard.

Cancel a capture with **Esc** or a **right-click**.

### Tray menu

- **OCR Engine** - Select between Google Lens, OneOCR, and ScreenAI
- **Capture (OCR area)** — start a capture immediately.
- **Set Hotkey...** — opens a small window; press the key combination you want.
  Supports modifiers (Ctrl/Shift/Alt/Win) and **F1-F24, including F13-F24**.
  Press **Esc** to cancel. The choice is saved.
- **Quit** — exit the app.

### Configuration

Settings are stored at `%APPDATA%\oneshot-ocr\config.txt`:

```
hotkey=control+shift+KeyO
engine=google_lens
```

- `hotkey` — any combination `global-hotkey` understands (e.g. `F13`,
  `control+shift+KeyO`, `alt+F20`).
- `engine` — `google_lens` (default), `oneocr`, or `screenai`. The tray menu can
  change this and saves it automatically.

Existing settings in `%APPDATA%\lens_adhoc\config.txt` are used until you save
a setting, which writes them to the new OneShot OCR location.

### Local OCR runtimes

On Windows, OneOCR is loaded from `%USERPROFILE%\.config\oneocr` and expects
`oneocr.dll`, `oneocr.onemodel`, and `onnxruntime.dll`. ScreenAI is loaded from
`%USERPROFILE%\.config\screen_ai\resources` and expects
`chrome_screen_ai.dll` plus the resource files and `files_list_ocr.txt`.

These files are downloaded automatically by OwOCR and GSM, but NOT by OneShot.

If a local runtime is missing, the app reports the missing file in its console
and continues running; selecting Google Lens remains available.

## Build from source

Install the latest stable [Rust toolchain](https://rustup.rs/), then run:

```powershell
cargo run --release --locked
```

The release executable is written to `target\release\oneshot-ocr.exe`.

## Notes

- Windows only in practice (uses the primary monitor via `xcap` and the Win32
  tray/hotkey APIs); the OCR/capture code is otherwise cross-platform.
- Large captures are automatically downscaled before upload, matching GSM's
  behavior.
- Because the app stays running, the clipboard contents persist normally after a
  capture.
- AI Generated Code, but Human Reviewed.
- Have **not** tested with Windows Scaling enabled, there may be some issues there, but *apparently* it should be fine.

## Credits

- Code Adapted from [GSM](https://github.com/bpwhelan/GameSentenceMiner)'s fork of https://github.com/AuroraWright/owocr, and https://github.com/AuroraWright/oneocr
