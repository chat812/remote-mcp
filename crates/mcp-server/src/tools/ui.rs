/// MCP tool implementations for Windows UI Automation.
///
/// All tools require an agent connection (`machine.agent_url` must be set).
/// They delegate to the agent's `/ui/*` HTTP routes, which return 501 on
/// non-Windows agents.
use crate::db::Db;
use crate::error::RemoteExecError;
use crate::transport::agent;
use anyhow::Result;
use serde_json::{json, Value};

fn agent_required(tool: &str) -> anyhow::Error {
    RemoteExecError::AgentRequired { tool: tool.to_string() }.into()
}

// ── helpers ───────────────────────────────────────────────────────────────────

fn get_machine_with_agent(db: &Db, machine_id: &str, tool: &str) -> Result<crate::db::Machine> {
    let machine = db
        .get(machine_id)?
        .ok_or_else(|| RemoteExecError::MachineNotFound(machine_id.to_string()))?;
    if machine.agent_url.is_none() {
        return Err(agent_required(tool));
    }
    Ok(machine)
}

fn fmt(v: Value) -> String {
    match v {
        Value::String(s) => s,
        other => other.to_string(),
    }
}

// ── tools ─────────────────────────────────────────────────────────────────────

/// List open windows (title, pid, class_name).
pub async fn ui_windows(db: &Db, machine_id: &str) -> Result<String> {
    let m = get_machine_with_agent(db, machine_id, "ui_windows")?;
    let v: Value = agent::agent_get_json(&m, "/ui/windows", 10).await?;
    Ok(v.to_string())
}

/// Get the UIA element tree of a window (or the full desktop if no window).
pub async fn ui_tree(
    db: &Db,
    machine_id: &str,
    window: Option<&str>,
    depth: Option<u32>,
) -> Result<String> {
    let m = get_machine_with_agent(db, machine_id, "ui_tree")?;
    let mut url = "/ui/tree".to_string();
    let mut sep = '?';
    if let Some(w) = window {
        url.push_str(&format!("{}window={}", sep, urlenc(w)));
        sep = '&';
    }
    if let Some(d) = depth {
        url.push_str(&format!("{}depth={}", sep, d));
    }
    let v: Value = agent::agent_get_json(&m, &url, 15).await?;
    Ok(fmt(v.get("tree").cloned().unwrap_or(v)))
}

/// Bring a window to the foreground.
pub async fn ui_focus(db: &Db, machine_id: &str, window: &str) -> Result<String> {
    let m = get_machine_with_agent(db, machine_id, "ui_focus")?;
    let body = json!({ "window": window });
    let v: Value = agent::agent_post_json(&m, "/ui/focus", &body, 10).await?;
    Ok(v.to_string())
}

/// Click at absolute screen coordinates.
pub async fn ui_click(
    db: &Db,
    machine_id: &str,
    x: i32,
    y: i32,
    button: Option<&str>,
) -> Result<String> {
    let m = get_machine_with_agent(db, machine_id, "ui_click")?;
    let body = json!({ "x": x, "y": y, "button": button.unwrap_or("left") });
    let v: Value = agent::agent_post_json(&m, "/ui/click", &body, 10).await?;
    Ok(v.to_string())
}

/// Move the mouse cursor to absolute coordinates.
pub async fn ui_move(db: &Db, machine_id: &str, x: i32, y: i32) -> Result<String> {
    let m = get_machine_with_agent(db, machine_id, "ui_move")?;
    let body = json!({ "x": x, "y": y });
    let v: Value = agent::agent_post_json(&m, "/ui/move", &body, 10).await?;
    Ok(v.to_string())
}

/// Type a string of text into the currently focused element.
pub async fn ui_type(db: &Db, machine_id: &str, text: &str) -> Result<String> {
    let m = get_machine_with_agent(db, machine_id, "ui_type")?;
    let body = json!({ "text": text });
    let v: Value = agent::agent_post_json(&m, "/ui/type", &body, 15).await?;
    Ok(v.to_string())
}

/// Send a key combination (e.g. "ctrl+c", "alt+f4", "win+r", "enter").
pub async fn ui_key(db: &Db, machine_id: &str, key: &str) -> Result<String> {
    let m = get_machine_with_agent(db, machine_id, "ui_key")?;
    let body = json!({ "key": key });
    let v: Value = agent::agent_post_json(&m, "/ui/key", &body, 10).await?;
    Ok(v.to_string())
}

/// Scroll at a position.
pub async fn ui_scroll(
    db: &Db,
    machine_id: &str,
    x: i32,
    y: i32,
    direction: Option<&str>,
    amount: Option<i32>,
) -> Result<String> {
    let m = get_machine_with_agent(db, machine_id, "ui_scroll")?;
    let body = json!({
        "x": x, "y": y,
        "direction": direction.unwrap_or("down"),
        "amount": amount.unwrap_or(3)
    });
    let v: Value = agent::agent_post_json(&m, "/ui/scroll", &body, 10).await?;
    Ok(v.to_string())
}

/// Find a UI element and return its info (name, type, bounds, value).
pub async fn ui_find_element(
    db: &Db,
    machine_id: &str,
    window: Option<&str>,
    name: Option<&str>,
    automation_id: Option<&str>,
) -> Result<String> {
    let m = get_machine_with_agent(db, machine_id, "ui_find_element")?;
    let mut url = "/ui/element".to_string();
    let mut sep = '?';
    if let Some(w) = window {
        url.push_str(&format!("{}window={}", sep, urlenc(w)));
        sep = '&';
    }
    if let Some(n) = name {
        url.push_str(&format!("{}name={}", sep, urlenc(n)));
        sep = '&';
    }
    if let Some(id) = automation_id {
        url.push_str(&format!("{}automation_id={}", sep, urlenc(id)));
    }
    let v: Value = agent::agent_get_json(&m, &url, 10).await?;
    Ok(v.to_string())
}

/// Click a UI element found by name or automation ID.
pub async fn ui_click_element(
    db: &Db,
    machine_id: &str,
    window: Option<&str>,
    name: Option<&str>,
    automation_id: Option<&str>,
) -> Result<String> {
    let m = get_machine_with_agent(db, machine_id, "ui_click_element")?;
    let body = json!({
        "window": window,
        "name": name,
        "automation_id": automation_id
    });
    let v: Value = agent::agent_post_json(&m, "/ui/click-element", &body, 10).await?;
    Ok(v.to_string())
}

/// Read the current value/text of a UI element.
pub async fn ui_get_value(
    db: &Db,
    machine_id: &str,
    window: Option<&str>,
    name: Option<&str>,
    automation_id: Option<&str>,
) -> Result<String> {
    let m = get_machine_with_agent(db, machine_id, "ui_get_value")?;
    let mut url = "/ui/get-value".to_string();
    let mut sep = '?';
    if let Some(w) = window {
        url.push_str(&format!("{}window={}", sep, urlenc(w)));
        sep = '&';
    }
    if let Some(n) = name {
        url.push_str(&format!("{}name={}", sep, urlenc(n)));
        sep = '&';
    }
    if let Some(id) = automation_id {
        url.push_str(&format!("{}automation_id={}", sep, urlenc(id)));
    }
    let v: Value = agent::agent_get_json(&m, &url, 10).await?;
    Ok(fmt(v.get("value").cloned().unwrap_or(v)))
}

/// Set the value of an input element (e.g. a text field).
pub async fn ui_set_value(
    db: &Db,
    machine_id: &str,
    window: Option<&str>,
    name: Option<&str>,
    automation_id: Option<&str>,
    value: &str,
) -> Result<String> {
    let m = get_machine_with_agent(db, machine_id, "ui_set_value")?;
    let body = json!({
        "window": window,
        "name": name,
        "automation_id": automation_id,
        "value": value
    });
    let v: Value = agent::agent_post_json(&m, "/ui/set-value", &body, 10).await?;
    Ok(v.to_string())
}

/// Describe what is currently visible in a window as a flat text list.
///
/// Activates the accessibility bridge first (so Flutter apps populate their
/// semantics tree) then returns named, interactive elements: buttons, inputs,
/// text, checkboxes, tabs, etc.  Much cheaper than ui_screenshot.
pub async fn ui_describe(
    db: &Db,
    machine_id: &str,
    window: Option<&str>,
    depth: Option<u32>,
) -> Result<String> {
    let m = get_machine_with_agent(db, machine_id, "ui_describe")?;
    let mut url = "/ui/describe".to_string();
    let mut sep = '?';
    if let Some(w) = window {
        url.push_str(&format!("{}window={}", sep, urlenc(w)));
        sep = '&';
    }
    if let Some(d) = depth {
        url.push_str(&format!("{}depth={}", sep, d));
    }
    let v: Value = agent::agent_get_json(&m, &url, 15).await?;
    Ok(fmt(v.get("description").cloned().unwrap_or(v)))
}

/// Run Windows Media OCR on a screenshot and return extracted text.
/// No image tokens consumed — only text is returned to the caller.
pub async fn ui_ocr(db: &Db, machine_id: &str, window: Option<&str>) -> Result<String> {
    let m = get_machine_with_agent(db, machine_id, "ui_ocr")?;
    let url = if let Some(w) = window {
        format!("/ui/ocr?window={}", urlenc(w))
    } else {
        "/ui/ocr".to_string()
    };
    let v: Value = agent::agent_get_json(&m, &url, 60).await?;
    Ok(fmt(v.get("text").cloned().unwrap_or(v)))
}

/// Take a screenshot of the full screen or a specific window.
/// Returns base64-encoded PNG in JSON: `{"image":"...","mime":"image/png"}`.
pub async fn ui_screenshot(
    db: &Db,
    machine_id: &str,
    window: Option<&str>,
) -> Result<String> {
    let m = get_machine_with_agent(db, machine_id, "ui_screenshot")?;
    let url = if let Some(w) = window {
        format!("/ui/screenshot?window={}", urlenc(w))
    } else {
        "/ui/screenshot".to_string()
    };
    let v: Value = agent::agent_get_json(&m, &url, 30).await?;
    Ok(v.to_string())
}

// ── URL encoding ──────────────────────────────────────────────────────────────

fn urlenc(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{:02X}", b)),
        }
    }
    out
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::{Capabilities, Db, Machine};

    fn make_machine_no_agent() -> Machine {
        Machine {
            id: "m1".into(),
            label: "test".into(),
            host: "127.0.0.1".into(),
            port: 22,
            os: "windows".into(),
            transport: "ssh".into(),
            ssh_user: Some("user".into()),
            ssh_key_path: None,
            ssh_password: None,
            agent_url: None,
            agent_token: None,
            capabilities: None,
            last_seen: None,
            status: "online".into(),
            created_at: 0,
        }
    }

    fn make_machine_with_agent() -> Machine {
        Machine {
            agent_url: Some("http://127.0.0.1:8765".into()),
            agent_token: Some("tok".into()),
            ..make_machine_no_agent()
        }
    }

    fn in_memory_db_with(m: Machine) -> Db {
        let db = Db::open_in_memory().expect("in-memory db");
        db.upsert(&m).expect("upsert");
        db
    }

    // All UI tools must return AgentRequired when no agent_url is set.

    #[tokio::test]
    async fn ui_windows_requires_agent() {
        let db = in_memory_db_with(make_machine_no_agent());
        let err = ui_windows(&db, "m1").await.unwrap_err();
        assert!(err.to_string().contains("agent"), "expected agent error, got: {err}");
    }

    #[tokio::test]
    async fn ui_tree_requires_agent() {
        let db = in_memory_db_with(make_machine_no_agent());
        let err = ui_tree(&db, "m1", None, None).await.unwrap_err();
        assert!(err.to_string().contains("agent"));
    }

    #[tokio::test]
    async fn ui_click_requires_agent() {
        let db = in_memory_db_with(make_machine_no_agent());
        let err = ui_click(&db, "m1", 100, 200, None).await.unwrap_err();
        assert!(err.to_string().contains("agent"));
    }

    #[tokio::test]
    async fn ui_type_requires_agent() {
        let db = in_memory_db_with(make_machine_no_agent());
        let err = ui_type(&db, "m1", "hello").await.unwrap_err();
        assert!(err.to_string().contains("agent"));
    }

    #[tokio::test]
    async fn ui_key_requires_agent() {
        let db = in_memory_db_with(make_machine_no_agent());
        let err = ui_key(&db, "m1", "ctrl+c").await.unwrap_err();
        assert!(err.to_string().contains("agent"));
    }

    #[tokio::test]
    async fn ui_screenshot_requires_agent() {
        let db = in_memory_db_with(make_machine_no_agent());
        let err = ui_screenshot(&db, "m1", None).await.unwrap_err();
        assert!(err.to_string().contains("agent"));
    }

    #[tokio::test]
    async fn unknown_machine_is_not_found() {
        let db = Db::open_in_memory().expect("db");
        let err = ui_windows(&db, "does-not-exist").await.unwrap_err();
        assert!(err.to_string().to_lowercase().contains("not found") ||
                err.to_string().contains("MachineNotFound"));
    }

    // URL encoding helper
    #[test]
    fn urlenc_basic() {
        assert_eq!(urlenc("hello"), "hello");
        assert_eq!(urlenc("hello world"), "hello%20world");
        assert_eq!(urlenc("Notepad"), "Notepad");
        assert_eq!(urlenc("a+b"), "a%2Bb");
    }

    #[test]
    fn urlenc_slash_preserved() {
        // slashes are percent-encoded in this implementation
        assert_eq!(urlenc("a/b"), "a%2Fb");
    }
}
