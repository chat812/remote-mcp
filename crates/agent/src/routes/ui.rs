/// Windows UI Automation routes.
///
/// All handlers compile on every platform.  On non-Windows they return
/// `501 Not Implemented`.  The actual implementation lives in the
/// `win` submodule which is only compiled on Windows.
use axum::{
    extract::{Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use serde::{Deserialize, Serialize};
use serde_json::json;

use super::AppState;

// ── helpers ───────────────────────────────────────────────────────────────────

fn ok(v: serde_json::Value) -> Response {
    Json(v).into_response()
}

fn err(status: StatusCode, code: &str, msg: &str) -> Response {
    (status, Json(json!({ "error": msg, "code": code }))).into_response()
}

#[cfg(not(windows))]
fn not_supported() -> Response {
    err(
        StatusCode::NOT_IMPLEMENTED,
        "NOT_SUPPORTED",
        "UI automation is only supported on Windows",
    )
}

// ── request / response types (always compiled) ────────────────────────────────

#[derive(Deserialize)]
pub struct TreeQuery {
    pub window: Option<String>,
    /// Maximum recursion depth (default 4)
    pub depth: Option<u32>,
}

#[derive(Deserialize)]
pub struct FocusRequest {
    pub window: String,
}

#[derive(Deserialize)]
pub struct ClickRequest {
    pub x: i32,
    pub y: i32,
    /// "left" (default), "right", or "double"
    pub button: Option<String>,
}

#[derive(Deserialize)]
pub struct MoveRequest {
    pub x: i32,
    pub y: i32,
}

#[derive(Deserialize)]
pub struct TypeRequest {
    pub text: String,
}

#[derive(Deserialize)]
pub struct KeyRequest {
    /// Key combo string, e.g. "ctrl+c", "alt+f4", "win+r", "enter", "f5"
    pub key: String,
}

#[derive(Deserialize)]
pub struct ScrollRequest {
    pub x: i32,
    pub y: i32,
    /// "up" or "down" (default "down")
    pub direction: Option<String>,
    /// Number of scroll ticks (default 3)
    pub amount: Option<i32>,
}

#[derive(Deserialize)]
pub struct ElementQuery {
    pub window: Option<String>,
    pub name: Option<String>,
    pub automation_id: Option<String>,
}

#[derive(Deserialize)]
pub struct ClickElementRequest {
    pub window: Option<String>,
    pub name: Option<String>,
    pub automation_id: Option<String>,
}

#[derive(Deserialize)]
pub struct SetValueRequest {
    pub window: Option<String>,
    pub name: Option<String>,
    pub automation_id: Option<String>,
    pub value: String,
}

#[derive(Deserialize)]
pub struct ScreenshotQuery {
    pub window: Option<String>,
}

// ── Windows implementation ────────────────────────────────────────────────────

#[cfg(windows)]
pub mod win {
    use anyhow::{anyhow, Result};
    use serde::Serialize;

    // ── Key parsing ───────────────────────────────────────────────────────────

    /// Parse a single key name into an `enigo::Key`.
    pub fn parse_single_key(s: &str) -> Result<enigo::Key> {
        use enigo::Key;
        Ok(match s.to_lowercase().as_str() {
            "ctrl" | "control" => Key::Control,
            "alt" => Key::Alt,
            "shift" => Key::Shift,
            "win" | "windows" | "super" | "meta" | "cmd" => Key::Meta,
            "enter" | "return" => Key::Return,
            "escape" | "esc" => Key::Escape,
            "tab" => Key::Tab,
            "space" => Key::Space,
            "backspace" => Key::Backspace,
            "delete" | "del" => Key::Delete,
            "home" => Key::Home,
            "end" => Key::End,
            "pageup" | "pgup" => Key::PageUp,
            "pagedown" | "pgdn" | "pgdown" => Key::PageDown,
            "up" => Key::UpArrow,
            "down" => Key::DownArrow,
            "left" => Key::LeftArrow,
            "right" => Key::RightArrow,
            "f1" => Key::F1,
            "f2" => Key::F2,
            "f3" => Key::F3,
            "f4" => Key::F4,
            "f5" => Key::F5,
            "f6" => Key::F6,
            "f7" => Key::F7,
            "f8" => Key::F8,
            "f9" => Key::F9,
            "f10" => Key::F10,
            "f11" => Key::F11,
            "f12" => Key::F12,
            s if s.chars().count() == 1 => Key::Unicode(s.chars().next().unwrap()),
            other => return Err(anyhow!("Unknown key: '{}'", other)),
        })
    }

    /// Send a key combo string via enigo (e.g. "ctrl+c", "alt+f4", "enter").
    pub fn send_key_combo(key_str: &str) -> Result<()> {
        use enigo::{Direction, Enigo, Keyboard, Settings};
        let mut enigo = Enigo::new(&Settings::default())?;
        let parts: Vec<&str> = key_str.split('+').collect();
        if parts.is_empty() {
            return Err(anyhow!("Empty key string"));
        }
        let (modifiers, main) = parts.split_at(parts.len() - 1);
        let main_key = parse_single_key(main[0].trim())?;

        // Press modifiers
        for m in modifiers {
            enigo.key(parse_single_key(m.trim())?, Direction::Press)?;
        }
        // Tap main key
        enigo.key(main_key, Direction::Click)?;
        // Release modifiers in reverse
        for m in modifiers.iter().rev() {
            enigo.key(parse_single_key(m.trim())?, Direction::Release)?;
        }
        Ok(())
    }

    // ── Window listing ────────────────────────────────────────────────────────

    #[derive(Serialize)]
    pub struct WindowInfo {
        pub title: String,
        pub pid: u32,
        pub class_name: String,
    }

    pub fn list_windows() -> Result<Vec<WindowInfo>> {
        use uiautomation::UIAutomation;
        let auto = UIAutomation::new()?;
        let root = auto.get_root_element()?;
        let walker = auto.create_tree_walker()?;
        let mut windows = Vec::new();

        if let Ok(child) = walker.get_first_child(&root) {
            let mut cur = child;
            loop {
                let title = cur.get_name().unwrap_or_default();
                if !title.is_empty() {
                    let pid = cur.get_process_id().unwrap_or(0) as u32;
                    let class_name = cur.get_classname().unwrap_or_default();
                    windows.push(WindowInfo { title, pid, class_name });
                }
                match walker.get_next_sibling(&cur) {
                    Ok(next) => cur = next,
                    Err(_) => break,
                }
            }
        }
        Ok(windows)
    }

    // ── Element tree ──────────────────────────────────────────────────────────

    /// Recursively build a text representation of the UIA element tree.
    pub fn build_tree_text(
        walker: &uiautomation::UITreeWalker,
        element: &uiautomation::UIElement,
        depth: u32,
        max_depth: u32,
        out: &mut String,
    ) {
        let indent = "  ".repeat(depth as usize);
        let name = element.get_name().unwrap_or_default();
        let ct = element
            .get_control_type()
            .map(|t| format!("{:?}", t))
            .unwrap_or_else(|_| "?".to_string());
        let aid = element
            .get_automation_id()
            .ok()
            .filter(|s| !s.is_empty())
            .map(|s| format!(" id={}", s))
            .unwrap_or_default();
        let display = if name.is_empty() { "<unnamed>" } else { name.as_str() };
        out.push_str(&format!("{}[{}]{} \"{}\"\n", indent, ct, aid, display));

        if depth < max_depth {
            if let Ok(child) = walker.get_first_child(element) {
                let mut cur = child;
                loop {
                    build_tree_text(walker, &cur, depth + 1, max_depth, out);
                    match walker.get_next_sibling(&cur) {
                        Ok(next) => cur = next,
                        Err(_) => break,
                    }
                }
            }
        }
    }

    pub fn get_tree(window_title: Option<&str>, max_depth: u32) -> Result<String> {
        use uiautomation::UIAutomation;
        let auto = UIAutomation::new()?;
        let root = auto.get_root_element()?;
        let walker = auto.create_tree_walker()?;

        let start_element = if let Some(title) = window_title {
            auto.create_matcher()
                .from(root)
                .timeout(2000)
                .name(title)
                .find_first()
                .map_err(|_| anyhow!("Window '{}' not found", title))?
        } else {
            root
        };

        let mut out = String::new();
        build_tree_text(&walker, &start_element, 0, max_depth, &mut out);
        Ok(out)
    }

    // ── Focus ─────────────────────────────────────────────────────────────────

    pub fn focus_window(title: &str) -> Result<()> {
        use uiautomation::UIAutomation;
        let auto = UIAutomation::new()?;
        let root = auto.get_root_element()?;
        let element = auto
            .create_matcher()
            .from(root)
            .timeout(2000)
            .name(title)
            .find_first()
            .map_err(|_| anyhow!("Window '{}' not found", title))?;
        element.set_focus()?;
        Ok(())
    }

    // ── Mouse ─────────────────────────────────────────────────────────────────

    pub fn mouse_click(x: i32, y: i32, button: &str) -> Result<()> {
        use enigo::{Button, Coordinate, Direction, Enigo, Mouse, Settings};
        let mut enigo = Enigo::new(&Settings::default())?;
        enigo.move_mouse(x, y, Coordinate::Abs)?;
        match button {
            "right" => {
                enigo.button(Button::Right, Direction::Click)?;
            }
            "double" => {
                enigo.button(Button::Left, Direction::Click)?;
                enigo.button(Button::Left, Direction::Click)?;
            }
            _ => {
                enigo.button(Button::Left, Direction::Click)?;
            }
        }
        Ok(())
    }

    pub fn mouse_move(x: i32, y: i32) -> Result<()> {
        use enigo::{Coordinate, Enigo, Mouse, Settings};
        let mut enigo = Enigo::new(&Settings::default())?;
        enigo.move_mouse(x, y, Coordinate::Abs)?;
        Ok(())
    }

    pub fn mouse_scroll(x: i32, y: i32, direction: &str, amount: i32) -> Result<()> {
        use enigo::{Axis, Coordinate, Enigo, Mouse, Settings};
        let mut enigo = Enigo::new(&Settings::default())?;
        enigo.move_mouse(x, y, Coordinate::Abs)?;
        // Positive = scroll down, negative = scroll up
        let delta = if direction == "up" { -amount } else { amount };
        enigo.scroll(delta, Axis::Vertical)?;
        Ok(())
    }

    // ── Keyboard ──────────────────────────────────────────────────────────────

    pub fn type_text(text: &str) -> Result<()> {
        use enigo::{Enigo, Keyboard, Settings};
        let mut enigo = Enigo::new(&Settings::default())?;
        enigo.text(text)?;
        Ok(())
    }

    // ── Element find / click / value ─────────────────────────────────────────

    fn find_element(
        auto: &uiautomation::UIAutomation,
        root: uiautomation::UIElement,
        name: Option<&str>,
        automation_id: Option<&str>,
    ) -> Result<uiautomation::UIElement> {
        let mut matcher = auto.create_matcher().from(root).timeout(3000);
        if let Some(n) = name {
            matcher = matcher.name(n);
        }
        // UIMatcher has no built-in automation_id filter; use filter_fn instead.
        if let Some(id) = automation_id {
            let id_owned = id.to_string();
            matcher = matcher.filter_fn(Box::new(move |el: &uiautomation::UIElement| {
                Ok(el.get_automation_id().ok().as_deref() == Some(&id_owned))
            }));
        }
        matcher
            .find_first()
            .map_err(|_| anyhow!("Element not found (name={:?}, id={:?})", name, automation_id))
    }

    fn get_root_or_window(
        auto: &uiautomation::UIAutomation,
        window: Option<&str>,
    ) -> Result<uiautomation::UIElement> {
        let root = auto.get_root_element()?;
        if let Some(title) = window {
            auto.create_matcher()
                .from(root)
                .timeout(2000)
                .name(title)
                .find_first()
                .map_err(|_| anyhow!("Window '{}' not found", title))
        } else {
            Ok(root)
        }
    }

    #[derive(Serialize)]
    pub struct ElementInfo {
        pub name: String,
        pub control_type: String,
        pub automation_id: String,
        pub value: Option<String>,
        pub enabled: bool,
        pub bounds: BoundsInfo,
    }

    #[derive(Serialize)]
    pub struct BoundsInfo {
        pub left: i32,
        pub top: i32,
        pub right: i32,
        pub bottom: i32,
    }

    pub fn find_element_info(
        window: Option<&str>,
        name: Option<&str>,
        automation_id: Option<&str>,
    ) -> Result<ElementInfo> {
        use uiautomation::UIAutomation;
        let auto = UIAutomation::new()?;
        let scope = get_root_or_window(&auto, window)?;
        let el = find_element(&auto, scope, name, automation_id)?;

        let bounds = el.get_bounding_rectangle()?;
        let value = {
            use uiautomation::patterns::UIValuePattern;
            el.get_pattern::<UIValuePattern>()
                .ok()
                .and_then(|p| p.get_value().ok())
        };

        Ok(ElementInfo {
            name: el.get_name().unwrap_or_default(),
            control_type: el
                .get_control_type()
                .map(|t| format!("{:?}", t))
                .unwrap_or_default(),
            automation_id: el.get_automation_id().unwrap_or_default(),
            value,
            enabled: el.is_enabled().unwrap_or(false),
            bounds: BoundsInfo {
                left: bounds.get_left(),
                top: bounds.get_top(),
                right: bounds.get_right(),
                bottom: bounds.get_bottom(),
            },
        })
    }

    pub fn click_element(
        window: Option<&str>,
        name: Option<&str>,
        automation_id: Option<&str>,
    ) -> Result<()> {
        use uiautomation::UIAutomation;
        let auto = UIAutomation::new()?;
        let scope = get_root_or_window(&auto, window)?;
        let el = find_element(&auto, scope, name, automation_id)?;
        el.click()?;
        Ok(())
    }

    pub fn get_element_value(
        window: Option<&str>,
        name: Option<&str>,
        automation_id: Option<&str>,
    ) -> Result<String> {
        use uiautomation::{patterns::UIValuePattern, UIAutomation};
        let auto = UIAutomation::new()?;
        let scope = get_root_or_window(&auto, window)?;
        let el = find_element(&auto, scope, name, automation_id)?;

        // Try ValuePattern first, fall back to element name
        if let Ok(vp) = el.get_pattern::<UIValuePattern>() {
            if let Ok(v) = vp.get_value() {
                return Ok(v);
            }
        }
        Ok(el.get_name().unwrap_or_default())
    }

    pub fn set_element_value(
        window: Option<&str>,
        name: Option<&str>,
        automation_id: Option<&str>,
        value: &str,
    ) -> Result<()> {
        use uiautomation::{patterns::UIValuePattern, UIAutomation};
        let auto = UIAutomation::new()?;
        let scope = get_root_or_window(&auto, window)?;
        let el = find_element(&auto, scope, name, automation_id)?;
        let vp: UIValuePattern = el.get_pattern()?;
        vp.set_value(value)?;
        Ok(())
    }

    // ── Describe (Flutter-friendly) ───────────────────────────────────────────

    /// Collect named, interactive UIA elements into a flat list.
    fn collect_named(
        walker: &uiautomation::UITreeWalker,
        element: &uiautomation::UIElement,
        depth: u32,
        max_depth: u32,
        out: &mut Vec<String>,
    ) {
        use uiautomation::types::ControlType;

        let name = element.get_name().unwrap_or_default();
        let ct = element.get_control_type().ok();

        if !name.is_empty() {
            let label = match ct {
                Some(ControlType::Button) => Some(format!("[Button] {}", name)),
                Some(ControlType::Edit) => {
                    use uiautomation::patterns::UIValuePattern;
                    let val = element
                        .get_pattern::<UIValuePattern>()
                        .ok()
                        .and_then(|p| p.get_value().ok())
                        .unwrap_or_default();
                    if val.is_empty() {
                        Some(format!("[Input] {} (empty)", name))
                    } else {
                        Some(format!("[Input] {} = \"{}\"", name, val))
                    }
                }
                Some(ControlType::Text) => Some(format!("[Text] {}", name)),
                Some(ControlType::CheckBox) => Some(format!("[CheckBox] {}", name)),
                Some(ControlType::RadioButton) => Some(format!("[RadioButton] {}", name)),
                Some(ControlType::ComboBox) => Some(format!("[ComboBox] {}", name)),
                Some(ControlType::ListItem) => Some(format!("[ListItem] {}", name)),
                Some(ControlType::MenuItem) => Some(format!("[MenuItem] {}", name)),
                Some(ControlType::TabItem) => Some(format!("[Tab] {}", name)),
                Some(ControlType::TreeItem) => Some(format!("[TreeItem] {}", name)),
                Some(ControlType::Hyperlink) => Some(format!("[Link] {}", name)),
                Some(ControlType::Image) => Some(format!("[Image] {}", name)),
                Some(ControlType::Header) | Some(ControlType::HeaderItem) => {
                    Some(format!("[Header] {}", name))
                }
                Some(ControlType::Window) | Some(ControlType::Pane) => {
                    Some(format!("--- {} ---", name))
                }
                _ => None,
            };
            if let Some(s) = label {
                out.push(s);
            }
        }

        if depth < max_depth {
            if let Ok(child) = walker.get_first_child(element) {
                let mut cur = child;
                loop {
                    collect_named(walker, &cur, depth + 1, max_depth, out);
                    match walker.get_next_sibling(&cur) {
                        Ok(next) => cur = next,
                        Err(_) => break,
                    }
                }
            }
        }
    }

    /// Describe what is visible in a window.
    ///
    /// For Flutter apps the first UIA walk is a "warm-up" that signals to
    /// Flutter's accessibility bridge that a UIA client is present; Flutter
    /// then builds its semantics tree.  A brief pause and a second walk
    /// returns the actual content.
    pub fn describe_window(window_title: Option<&str>, max_depth: u32) -> Result<String> {
        use uiautomation::UIAutomation;

        // Single automation + walker instance reused for both walks.
        let auto = UIAutomation::new()?;
        let walker = auto.create_tree_walker()?;

        let find_window = |r: uiautomation::UIElement| -> Result<uiautomation::UIElement> {
            if let Some(title) = window_title {
                auto.create_matcher()
                    .from(r)
                    .timeout(2000)
                    .name(title)
                    .find_first()
                    .map_err(|_| anyhow!("Window '{}' not found", title))
            } else {
                Ok(r)
            }
        };

        // Warm-up walk — triggers Flutter's accessibility bridge if present.
        let window = find_window(auto.get_root_element()?)?;
        let mut _warmup = Vec::new();
        collect_named(&walker, &window, 0, 2, &mut _warmup);

        // Give Flutter time to build its semantics tree.
        std::thread::sleep(std::time::Duration::from_millis(400));

        // Real walk — re-fetch root so we get the refreshed tree.
        let window2 = find_window(auto.get_root_element()?)?;
        let mut items = Vec::new();
        collect_named(&walker, &window2, 0, max_depth, &mut items);

        if items.is_empty() {
            return Ok("No named UI elements found. The app may not expose accessibility information.".into());
        }
        Ok(items.join("\n"))
    }

    // ── OCR ───────────────────────────────────────────────────────────────────

    // ── WinRT async helper ────────────────────────────────────────────────────

    /// Block until a WinRT async operation finishes by polling GetResults().
    ///
    /// WinRT returns E_ILLEGAL_METHOD_CALL (0x8000000E) when the operation is
    /// still in progress, so we sleep and retry until it succeeds or fails for
    /// a real reason.  Safe to call from spawn_blocking threads.
    macro_rules! wait_winrt {
        ($op:expr) => {{
            use windows::core::HRESULT;
            // E_ILLEGAL_METHOD_CALL — operation not yet complete
            const NOT_READY: HRESULT = HRESULT(0x8000000Eu32 as i32);
            let op = $op;
            loop {
                match op.GetResults() {
                    Ok(v) => break v,
                    Err(e) if e.code() == NOT_READY => {
                        std::thread::sleep(std::time::Duration::from_millis(5));
                    }
                    Err(e) => return Err(e.into()),
                }
            }
        }};
    }

    // ── OCR ───────────────────────────────────────────────────────────────────

    /// Capture a screenshot and run Windows Media OCR on it, returning plain text.
    ///
    /// Returns only text — no image tokens consumed.  Works on any app including
    /// Flutter/custom-rendered windows where the UIA tree is sparse.
    pub fn ocr_window(window_title: Option<&str>) -> Result<String> {
        use windows::Graphics::Imaging::{BitmapDecoder, BitmapPixelFormat, SoftwareBitmap};
        use windows::Media::Ocr::OcrEngine;
        use windows::Storage::Streams::{DataWriter, InMemoryRandomAccessStream};
        use windows::core::Interface;

        // 1. Capture PNG bytes (reuse existing screenshot logic)
        let png_bytes = if let Some(title) = window_title {
            use uiautomation::UIAutomation;
            let auto = UIAutomation::new()?;
            let root = auto.get_root_element()?;
            let el = auto
                .create_matcher()
                .from(root)
                .timeout(2000)
                .name(title)
                .find_first()
                .map_err(|_| anyhow!("Window '{}' not found", title))?;
            let r = el.get_bounding_rectangle()?;
            use screenshots::Screen;
            let screens = Screen::all()?;
            if screens.is_empty() {
                return Err(anyhow!("No screens found"));
            }
            let img = screens[0].capture_area(
                r.get_left(),
                r.get_top(),
                (r.get_right() - r.get_left()) as u32,
                (r.get_bottom() - r.get_top()) as u32,
            )?;
            img_to_png(img)?
        } else {
            use screenshots::Screen;
            let screens = Screen::all()?;
            if screens.is_empty() {
                return Err(anyhow!("No screens found"));
            }
            img_to_png(screens[0].capture()?)?
        };

        // 2. Write PNG bytes into an InMemoryRandomAccessStream
        let stream = InMemoryRandomAccessStream::new()?;
        {
            use windows::Storage::Streams::IOutputStream;
            let output: IOutputStream = stream.cast()?;
            let writer = DataWriter::CreateDataWriter(&output)?;
            writer.WriteBytes(&png_bytes)?;
            wait_winrt!(writer.StoreAsync()?); // returns u32 bytes stored — ignored
            writer.DetachStream()?;
        }
        {
            use windows::Storage::Streams::IRandomAccessStream;
            let ras: IRandomAccessStream = stream.cast()?;
            ras.Seek(0)?;
        }

        // 3. Decode PNG → SoftwareBitmap
        let decoder: BitmapDecoder = {
            use windows::Storage::Streams::IRandomAccessStream;
            let ras: IRandomAccessStream = stream.cast()?;
            wait_winrt!(BitmapDecoder::CreateAsync(&ras)?)
        };
        let bitmap: SoftwareBitmap = wait_winrt!(decoder.GetSoftwareBitmapAsync()?);

        // 4. Convert to Bgra8 (required by OcrEngine)
        let bitmap = if bitmap.BitmapPixelFormat()? != BitmapPixelFormat::Bgra8 {
            SoftwareBitmap::Convert(&bitmap, BitmapPixelFormat::Bgra8)?
        } else {
            bitmap
        };

        // 5. Run OCR
        let engine = OcrEngine::TryCreateFromUserProfileLanguages()
            .map_err(|e| anyhow!("OCR engine unavailable: {}", e))?;
        let result = wait_winrt!(engine.RecognizeAsync(&bitmap)?);
        let text = result.Text()?.to_string();

        if text.trim().is_empty() {
            Ok("No text detected.".into())
        } else {
            Ok(text)
        }
    }

    // ── Screenshot ────────────────────────────────────────────────────────────

    fn img_to_png(
        img: screenshots::image::ImageBuffer<screenshots::image::Rgba<u8>, Vec<u8>>,
    ) -> Result<Vec<u8>> {
        use screenshots::image::{ImageEncoder, codecs::png::PngEncoder};
        let mut buf = Vec::new();
        let enc = PngEncoder::new(&mut buf);
        enc.write_image(
            img.as_raw(),
            img.width(),
            img.height(),
            screenshots::image::ColorType::Rgba8.into(),
        )?;
        Ok(buf)
    }

    pub fn take_screenshot(window_title: Option<&str>) -> Result<String> {
        use base64::{engine::general_purpose::STANDARD, Engine as _};
        use screenshots::Screen;

        let png_bytes = if let Some(title) = window_title {
            // Find window bounds via UIA, then capture that area
            use uiautomation::UIAutomation;
            let auto = UIAutomation::new()?;
            let root = auto.get_root_element()?;
            let el = auto
                .create_matcher()
                .from(root)
                .timeout(2000)
                .name(title)
                .find_first()
                .map_err(|_| anyhow!("Window '{}' not found", title))?;
            let r = el.get_bounding_rectangle()?;
            let screens = Screen::all()?;
            if screens.is_empty() {
                return Err(anyhow!("No screens found"));
            }
            let img = screens[0].capture_area(
                r.get_left(),
                r.get_top(),
                (r.get_right() - r.get_left()) as u32,
                (r.get_bottom() - r.get_top()) as u32,
            )?;
            img_to_png(img)?
        } else {
            let screens = Screen::all()?;
            if screens.is_empty() {
                return Err(anyhow!("No screens found"));
            }
            img_to_png(screens[0].capture()?)?
        };

        Ok(STANDARD.encode(&png_bytes))
    }
}

// ── Axum handlers ─────────────────────────────────────────────────────────────

/// GET /ui/windows — list open windows
pub async fn get_ui_windows(_state: State<AppState>) -> Response {
    #[cfg(not(windows))]
    return not_supported();

    #[cfg(windows)]
    match tokio::task::spawn_blocking(win::list_windows).await {
        Ok(Ok(list)) => ok(json!(list)),
        Ok(Err(e)) => err(StatusCode::INTERNAL_SERVER_ERROR, "UI_ERROR", &e.to_string()),
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, "TASK_ERROR", &e.to_string()),
    }
}

/// GET /ui/tree?window=TITLE&depth=N — UIA element tree
pub async fn get_ui_tree(_state: State<AppState>, Query(q): Query<TreeQuery>) -> Response {
    #[cfg(not(windows))]
    return not_supported();

    #[cfg(windows)]
    {
        let window = q.window.clone();
        let depth = q.depth.unwrap_or(4).min(10);
        match tokio::task::spawn_blocking(move || win::get_tree(window.as_deref(), depth)).await {
            Ok(Ok(tree)) => ok(json!({ "tree": tree })),
            Ok(Err(e)) => err(StatusCode::INTERNAL_SERVER_ERROR, "UI_ERROR", &e.to_string()),
            Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, "TASK_ERROR", &e.to_string()),
        }
    }
}

/// POST /ui/focus — bring window to foreground
pub async fn post_ui_focus(_state: State<AppState>, Json(req): Json<FocusRequest>) -> Response {
    #[cfg(not(windows))]
    return not_supported();

    #[cfg(windows)]
    {
        let window = req.window.clone();
        match tokio::task::spawn_blocking(move || win::focus_window(&window)).await {
            Ok(Ok(())) => ok(json!({ "ok": true })),
            Ok(Err(e)) => err(StatusCode::INTERNAL_SERVER_ERROR, "UI_ERROR", &e.to_string()),
            Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, "TASK_ERROR", &e.to_string()),
        }
    }
}

/// POST /ui/click — click at x,y
pub async fn post_ui_click(_state: State<AppState>, Json(req): Json<ClickRequest>) -> Response {
    #[cfg(not(windows))]
    return not_supported();

    #[cfg(windows)]
    {
        let (x, y, button) = (req.x, req.y, req.button.unwrap_or_else(|| "left".into()));
        match tokio::task::spawn_blocking(move || win::mouse_click(x, y, &button)).await {
            Ok(Ok(())) => ok(json!({ "ok": true, "x": x, "y": y })),
            Ok(Err(e)) => err(StatusCode::INTERNAL_SERVER_ERROR, "UI_ERROR", &e.to_string()),
            Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, "TASK_ERROR", &e.to_string()),
        }
    }
}

/// POST /ui/move — move mouse cursor
pub async fn post_ui_move(_state: State<AppState>, Json(req): Json<MoveRequest>) -> Response {
    #[cfg(not(windows))]
    return not_supported();

    #[cfg(windows)]
    {
        let (x, y) = (req.x, req.y);
        match tokio::task::spawn_blocking(move || win::mouse_move(x, y)).await {
            Ok(Ok(())) => ok(json!({ "ok": true })),
            Ok(Err(e)) => err(StatusCode::INTERNAL_SERVER_ERROR, "UI_ERROR", &e.to_string()),
            Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, "TASK_ERROR", &e.to_string()),
        }
    }
}

/// POST /ui/type — type text into focused element
pub async fn post_ui_type(_state: State<AppState>, Json(req): Json<TypeRequest>) -> Response {
    #[cfg(not(windows))]
    return not_supported();

    #[cfg(windows)]
    {
        let text = req.text.clone();
        match tokio::task::spawn_blocking(move || win::type_text(&text)).await {
            Ok(Ok(())) => ok(json!({ "ok": true })),
            Ok(Err(e)) => err(StatusCode::INTERNAL_SERVER_ERROR, "UI_ERROR", &e.to_string()),
            Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, "TASK_ERROR", &e.to_string()),
        }
    }
}

/// POST /ui/key — send key combo (e.g. "ctrl+c")
pub async fn post_ui_key(_state: State<AppState>, Json(req): Json<KeyRequest>) -> Response {
    #[cfg(not(windows))]
    return not_supported();

    #[cfg(windows)]
    {
        let key = req.key.clone();
        match tokio::task::spawn_blocking(move || win::send_key_combo(&key)).await {
            Ok(Ok(())) => ok(json!({ "ok": true })),
            Ok(Err(e)) => err(StatusCode::BAD_REQUEST, "KEY_ERROR", &e.to_string()),
            Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, "TASK_ERROR", &e.to_string()),
        }
    }
}

/// POST /ui/scroll — scroll at position
pub async fn post_ui_scroll(_state: State<AppState>, Json(req): Json<ScrollRequest>) -> Response {
    #[cfg(not(windows))]
    return not_supported();

    #[cfg(windows)]
    {
        let (x, y) = (req.x, req.y);
        let direction = req.direction.unwrap_or_else(|| "down".into());
        let amount = req.amount.unwrap_or(3);
        match tokio::task::spawn_blocking(move || win::mouse_scroll(x, y, &direction, amount))
            .await
        {
            Ok(Ok(())) => ok(json!({ "ok": true })),
            Ok(Err(e)) => err(StatusCode::INTERNAL_SERVER_ERROR, "UI_ERROR", &e.to_string()),
            Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, "TASK_ERROR", &e.to_string()),
        }
    }
}

/// GET /ui/element?window=X&name=Y&automation_id=Z — find element info
pub async fn get_ui_element(
    _state: State<AppState>,
    Query(q): Query<ElementQuery>,
) -> Response {
    #[cfg(not(windows))]
    return not_supported();

    #[cfg(windows)]
    {
        let window = q.window.clone();
        let name = q.name.clone();
        let aid = q.automation_id.clone();
        match tokio::task::spawn_blocking(move || {
            win::find_element_info(window.as_deref(), name.as_deref(), aid.as_deref())
        })
        .await
        {
            Ok(Ok(info)) => ok(json!(info)),
            Ok(Err(e)) => err(StatusCode::NOT_FOUND, "ELEMENT_NOT_FOUND", &e.to_string()),
            Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, "TASK_ERROR", &e.to_string()),
        }
    }
}

/// POST /ui/click-element — click a UI element by name/automationId
pub async fn post_ui_click_element(
    _state: State<AppState>,
    Json(req): Json<ClickElementRequest>,
) -> Response {
    #[cfg(not(windows))]
    return not_supported();

    #[cfg(windows)]
    {
        let window = req.window.clone();
        let name = req.name.clone();
        let aid = req.automation_id.clone();
        match tokio::task::spawn_blocking(move || {
            win::click_element(window.as_deref(), name.as_deref(), aid.as_deref())
        })
        .await
        {
            Ok(Ok(())) => ok(json!({ "ok": true })),
            Ok(Err(e)) => err(StatusCode::NOT_FOUND, "ELEMENT_NOT_FOUND", &e.to_string()),
            Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, "TASK_ERROR", &e.to_string()),
        }
    }
}

/// GET /ui/get-value?window=X&name=Y — read value/text of a UI element
pub async fn get_ui_value(
    _state: State<AppState>,
    Query(q): Query<ElementQuery>,
) -> Response {
    #[cfg(not(windows))]
    return not_supported();

    #[cfg(windows)]
    {
        let window = q.window.clone();
        let name = q.name.clone();
        let aid = q.automation_id.clone();
        match tokio::task::spawn_blocking(move || {
            win::get_element_value(window.as_deref(), name.as_deref(), aid.as_deref())
        })
        .await
        {
            Ok(Ok(value)) => ok(json!({ "value": value })),
            Ok(Err(e)) => err(StatusCode::NOT_FOUND, "ELEMENT_NOT_FOUND", &e.to_string()),
            Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, "TASK_ERROR", &e.to_string()),
        }
    }
}

/// POST /ui/set-value — write value into an input element
pub async fn post_ui_set_value(
    _state: State<AppState>,
    Json(req): Json<SetValueRequest>,
) -> Response {
    #[cfg(not(windows))]
    return not_supported();

    #[cfg(windows)]
    {
        let window = req.window.clone();
        let name = req.name.clone();
        let aid = req.automation_id.clone();
        let value = req.value.clone();
        match tokio::task::spawn_blocking(move || {
            win::set_element_value(window.as_deref(), name.as_deref(), aid.as_deref(), &value)
        })
        .await
        {
            Ok(Ok(())) => ok(json!({ "ok": true })),
            Ok(Err(e)) => err(StatusCode::INTERNAL_SERVER_ERROR, "UI_ERROR", &e.to_string()),
            Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, "TASK_ERROR", &e.to_string()),
        }
    }
}

/// GET /ui/screenshot?window=TITLE — take a screenshot (base64 PNG)
pub async fn get_ui_screenshot(
    _state: State<AppState>,
    Query(q): Query<ScreenshotQuery>,
) -> Response {
    #[cfg(not(windows))]
    return not_supported();

    #[cfg(windows)]
    {
        let window = q.window.clone();
        match tokio::task::spawn_blocking(move || win::take_screenshot(window.as_deref())).await {
            Ok(Ok(b64)) => ok(json!({ "image": b64, "mime": "image/png" })),
            Ok(Err(e)) => err(StatusCode::INTERNAL_SERVER_ERROR, "UI_ERROR", &e.to_string()),
            Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, "TASK_ERROR", &e.to_string()),
        }
    }
}

/// GET /ui/describe?window=TITLE&depth=N — describe visible UI elements
pub async fn get_ui_describe(
    _state: State<AppState>,
    Query(q): Query<TreeQuery>,
) -> Response {
    #[cfg(not(windows))]
    return not_supported();

    #[cfg(windows)]
    {
        let window = q.window.clone();
        let depth = q.depth.unwrap_or(6).min(12);
        match tokio::task::spawn_blocking(move || win::describe_window(window.as_deref(), depth))
            .await
        {
            Ok(Ok(desc)) => ok(json!({ "description": desc })),
            Ok(Err(e)) => err(StatusCode::INTERNAL_SERVER_ERROR, "UI_ERROR", &e.to_string()),
            Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, "TASK_ERROR", &e.to_string()),
        }
    }
}

/// GET /ui/ocr?window=TITLE — screenshot + local OCR, returns plain text (no image tokens)
pub async fn get_ui_ocr(
    _state: State<AppState>,
    Query(q): Query<ScreenshotQuery>,
) -> Response {
    #[cfg(not(windows))]
    return not_supported();

    #[cfg(windows)]
    {
        let window = q.window.clone();
        match tokio::task::spawn_blocking(move || win::ocr_window(window.as_deref())).await {
            Ok(Ok(text)) => ok(json!({ "text": text })),
            Ok(Err(e)) => err(StatusCode::INTERNAL_SERVER_ERROR, "OCR_ERROR", &e.to_string()),
            Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, "TASK_ERROR", &e.to_string()),
        }
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ── Key parsing (platform-independent logic) ──────────────────────────────

    #[cfg(windows)]
    mod key_tests {
        use super::win;

        #[test]
        fn parse_modifier_keys() {
            assert!(win::parse_single_key("ctrl").is_ok());
            assert!(win::parse_single_key("alt").is_ok());
            assert!(win::parse_single_key("shift").is_ok());
            assert!(win::parse_single_key("win").is_ok());
        }

        #[test]
        fn parse_named_keys() {
            assert!(win::parse_single_key("enter").is_ok());
            assert!(win::parse_single_key("escape").is_ok());
            assert!(win::parse_single_key("tab").is_ok());
            assert!(win::parse_single_key("space").is_ok());
            assert!(win::parse_single_key("backspace").is_ok());
            assert!(win::parse_single_key("delete").is_ok());
            assert!(win::parse_single_key("home").is_ok());
            assert!(win::parse_single_key("end").is_ok());
            assert!(win::parse_single_key("pageup").is_ok());
            assert!(win::parse_single_key("pagedown").is_ok());
        }

        #[test]
        fn parse_function_keys() {
            for f in ["f1", "f2", "f5", "f10", "f12"] {
                assert!(win::parse_single_key(f).is_ok(), "Failed to parse {}", f);
            }
        }

        #[test]
        fn parse_single_char_key() {
            assert!(win::parse_single_key("c").is_ok());
            assert!(win::parse_single_key("r").is_ok());
            assert!(win::parse_single_key("a").is_ok());
        }

        #[test]
        fn parse_unknown_key_is_err() {
            assert!(win::parse_single_key("foobarkey").is_err());
        }

        #[test]
        fn aliases_work() {
            assert!(win::parse_single_key("esc").is_ok());
            assert!(win::parse_single_key("return").is_ok());
            assert!(win::parse_single_key("del").is_ok());
            assert!(win::parse_single_key("pgup").is_ok());
            assert!(win::parse_single_key("pgdn").is_ok());
            assert!(win::parse_single_key("meta").is_ok());
            assert!(win::parse_single_key("cmd").is_ok());
        }
    }

    // ── Tree formatter ────────────────────────────────────────────────────────

    /// A minimal smoke test for build_tree_text using real UIA — runs only on
    /// Windows and only when a display is available (CI may skip).
    #[cfg(windows)]
    #[test]
    fn tree_of_desktop_is_non_empty() {
        // Getting the root element should always work on a live desktop.
        use uiautomation::UIAutomation;
        let auto = UIAutomation::new().expect("UIAutomation::new");
        let root = auto.get_root_element().expect("get_root_element");
        let walker = auto.create_tree_walker().expect("create_tree_walker");
        let mut out = String::new();
        super::win::build_tree_text(&walker, &root, 0, 1, &mut out);
        // The desktop root always has at least one child on a running system.
        assert!(!out.is_empty(), "Tree output should not be empty");
    }

    // ── Non-Windows stub test ─────────────────────────────────────────────────

    // This is a compile-time check: the module must compile cleanly on all
    // platforms.  The actual 501 behavior is verified by integration tests.
}
