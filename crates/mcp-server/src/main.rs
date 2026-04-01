mod audit;
mod auth;
mod db;
mod error;
mod heartbeat;
mod tools;
mod transport;

use anyhow::Result;
use clap::Parser;
use rmcp::{
    model::{
        CallToolRequestParam, CallToolResult, Content, Implementation, ProtocolVersion,
        ServerCapabilities, ServerInfo,
    },
    service::RequestContext,
    ServerHandler, ServiceExt,
};
use serde_json::Value;
use std::sync::Arc;
use tracing::{info, warn};
use tracing_subscriber::EnvFilter;

#[derive(Parser, Debug)]
#[command(name = "mcp-server", version, about = "Remote Exec MCP Server")]
struct Args {
    /// Log level (used in serve mode)
    #[arg(long, default_value = "info")]
    log_level: String,

    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(clap::Subcommand, Debug)]
enum Command {
    /// Register this binary with Claude Code as an MCP server and create config directory.
    Init {
        /// Name to register under (default: remote-exec)
        #[arg(long, default_value = "remote-exec")]
        name: String,
    },
}

#[derive(Clone)]
struct McpService {
    ctx: tools::ToolContext,
}

impl McpService {
    fn new(ctx: tools::ToolContext) -> Self {
        Self { ctx }
    }

    async fn dispatch_tool(&self, name: &str, args: Value) -> Result<String> {
        let ctx = &self.ctx;
        let db = &ctx.db;
        let circuits = ctx.circuits.clone();

        macro_rules! get_str {
            ($key:expr) => {
                args.get($key)
                    .and_then(|v| v.as_str())
                    .unwrap_or_default()
                    .to_string()
            };
        }
        macro_rules! get_opt_str {
            ($key:expr) => {
                args.get($key).and_then(|v| v.as_str()).map(|s| s.to_string())
            };
        }
        macro_rules! get_opt_u64 {
            ($key:expr) => {
                args.get($key).and_then(|v| v.as_u64())
            };
        }
        macro_rules! get_bool {
            ($key:expr) => {
                args.get($key)
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false)
            };
        }

        match name {
            // Machine tools
            "machine_add" => {
                tools::machine::machine_add(
                    db,
                    get_str!("label"),
                    get_str!("host"),
                    args.get("port").and_then(|v| v.as_i64()),
                    get_opt_str!("os"),
                    get_opt_str!("transport"),
                    get_opt_str!("ssh_user"),
                    get_opt_str!("ssh_key_path"),
                    get_opt_str!("ssh_password"),
                    get_opt_str!("agent_url"),
                    get_opt_str!("agent_token"),
                )
                .await
            }
            "machine_list" => tools::machine::machine_list(db).await,
            "machine_remove" => {
                tools::machine::machine_remove(db, &get_str!("id")).await
            }
            "machine_test" => {
                tools::machine::machine_test(db, &get_str!("id"), circuits).await
            }
            "machine_connect" => {
                tools::machine::machine_connect(db, &get_str!("uri")).await
            }
            // Exec tools
            "exec" => {
                tools::exec::exec(
                    db,
                    &get_str!("machine_id"),
                    &get_str!("command"),
                    get_opt_str!("workdir").as_deref(),
                    get_opt_u64!("timeout_secs"),
                    circuits,
                )
                .await
            }
            "job_start" => {
                tools::exec::job_start(
                    db,
                    &get_str!("machine_id"),
                    &get_str!("command"),
                    get_opt_str!("workdir").as_deref(),
                    circuits,
                )
                .await
            }
            "job_status" => {
                tools::exec::job_status(db, &get_str!("machine_id"), &get_str!("job_id")).await
            }
            "job_logs" => {
                tools::exec::job_logs(
                    db,
                    &get_str!("machine_id"),
                    &get_str!("job_id"),
                    args.get("tail").and_then(|v| v.as_u64()).map(|n| n as usize),
                    get_opt_str!("stream"),
                )
                .await
            }
            "job_kill" => {
                tools::exec::job_kill(db, &get_str!("machine_id"), &get_str!("job_id")).await
            }
            "job_list" => tools::exec::job_list(db, &get_str!("machine_id")).await,
            // File tools
            "file_write" => {
                tools::file::file_write(
                    db,
                    &get_str!("machine_id"),
                    &get_str!("path"),
                    &get_str!("content"),
                    get_opt_str!("mode").as_deref(),
                    circuits,
                )
                .await
            }
            "file_read" => {
                tools::file::file_read(db, &get_str!("machine_id"), &get_str!("path"), circuits)
                    .await
            }
            "file_str_replace" => {
                tools::file::file_str_replace(
                    db,
                    &get_str!("machine_id"),
                    &get_str!("path"),
                    &get_str!("old_str"),
                    &get_str!("new_str"),
                    circuits,
                )
                .await
            }
            "file_patch" => {
                tools::file::file_patch(
                    db,
                    &get_str!("machine_id"),
                    &get_str!("path"),
                    &get_str!("unified_diff"),
                    circuits,
                )
                .await
            }
            "file_insert" => {
                tools::file::file_insert(
                    db,
                    &get_str!("machine_id"),
                    &get_str!("path"),
                    args.get("line").and_then(|v| v.as_u64()).unwrap_or(0) as usize,
                    &get_str!("content"),
                    circuits,
                )
                .await
            }
            "file_delete_lines" => {
                tools::file::file_delete_lines(
                    db,
                    &get_str!("machine_id"),
                    &get_str!("path"),
                    args.get("start_line").and_then(|v| v.as_u64()).unwrap_or(1) as usize,
                    args.get("end_line").and_then(|v| v.as_u64()).unwrap_or(1) as usize,
                    circuits,
                )
                .await
            }
            "file_upload" => {
                tools::file::file_upload(
                    db,
                    &get_str!("machine_id"),
                    &get_str!("local_path"),
                    &get_str!("remote_path"),
                )
                .await
            }
            "file_download" => {
                tools::file::file_download(
                    db,
                    &get_str!("machine_id"),
                    &get_str!("remote_path"),
                    &get_str!("local_path"),
                )
                .await
            }
            // Session tools
            "session_open" => {
                tools::session::session_open(
                    db,
                    &get_str!("machine_id"),
                    get_opt_str!("workdir").as_deref(),
                    get_opt_str!("shell").as_deref(),
                )
                .await
            }
            "session_exec" => {
                tools::session::session_exec(
                    db,
                    &get_str!("machine_id"),
                    &get_str!("session_id"),
                    &get_str!("command"),
                    get_opt_u64!("timeout_secs"),
                )
                .await
            }
            "session_close" => {
                tools::session::session_close(
                    db,
                    &get_str!("machine_id"),
                    &get_str!("session_id"),
                )
                .await
            }
            "session_list" => {
                tools::session::session_list(db, &get_str!("machine_id")).await
            }
            // FS tools
            "fs_ls" => {
                tools::fs::fs_ls(
                    db,
                    &get_str!("machine_id"),
                    &get_str!("path"),
                    get_bool!("all"),
                    circuits,
                )
                .await
            }
            "fs_stat" => {
                tools::fs::fs_stat(db, &get_str!("machine_id"), &get_str!("path"), circuits).await
            }
            "fs_find" => {
                tools::fs::fs_find(
                    db,
                    &get_str!("machine_id"),
                    &get_str!("path"),
                    get_opt_str!("pattern").as_deref(),
                    get_opt_str!("file_type").as_deref(),
                    args.get("max_depth").and_then(|v| v.as_i64()).map(|n| n as i32),
                    circuits,
                )
                .await
            }
            "fs_tree" => {
                tools::fs::fs_tree(
                    db,
                    &get_str!("machine_id"),
                    &get_str!("path"),
                    args.get("max_depth").and_then(|v| v.as_i64()).map(|n| n as i32),
                    circuits,
                )
                .await
            }
            "fs_mkdir" => {
                tools::fs::fs_mkdir(
                    db,
                    &get_str!("machine_id"),
                    &get_str!("path"),
                    get_bool!("parents"),
                    circuits,
                )
                .await
            }
            "fs_rm" => {
                tools::fs::fs_rm(
                    db,
                    &get_str!("machine_id"),
                    &get_str!("path"),
                    get_bool!("recursive"),
                    circuits,
                )
                .await
            }
            "fs_mv" => {
                tools::fs::fs_mv(
                    db,
                    &get_str!("machine_id"),
                    &get_str!("src"),
                    &get_str!("dst"),
                    circuits,
                )
                .await
            }
            "fs_cp" => {
                tools::fs::fs_cp(
                    db,
                    &get_str!("machine_id"),
                    &get_str!("src"),
                    &get_str!("dst"),
                    get_bool!("recursive"),
                    circuits,
                )
                .await
            }
            // Process tools
            "ps_list" => {
                tools::process::ps_list(
                    db,
                    &get_str!("machine_id"),
                    get_opt_str!("filter").as_deref(),
                    circuits,
                )
                .await
            }
            "ps_kill" => {
                tools::process::ps_kill(
                    db,
                    &get_str!("machine_id"),
                    args.get("pid").and_then(|v| v.as_u64()).unwrap_or(0) as u32,
                    get_opt_str!("signal").as_deref(),
                    circuits,
                )
                .await
            }
            "ps_tree" => {
                tools::process::ps_tree(
                    db,
                    &get_str!("machine_id"),
                    args.get("pid").and_then(|v| v.as_u64()).map(|n| n as u32),
                    circuits,
                )
                .await
            }
            // Service tools
            "service_list" => {
                tools::service::service_list(db, &get_str!("machine_id"), circuits).await
            }
            "service_status" => {
                tools::service::service_status(
                    db,
                    &get_str!("machine_id"),
                    &get_str!("service"),
                    circuits,
                )
                .await
            }
            "service_start" => {
                tools::service::service_start(
                    db,
                    &get_str!("machine_id"),
                    &get_str!("service"),
                    circuits,
                )
                .await
            }
            "service_stop" => {
                tools::service::service_stop(
                    db,
                    &get_str!("machine_id"),
                    &get_str!("service"),
                    circuits,
                )
                .await
            }
            "service_restart" => {
                tools::service::service_restart(
                    db,
                    &get_str!("machine_id"),
                    &get_str!("service"),
                    circuits,
                )
                .await
            }
            "service_enable" => {
                tools::service::service_enable(
                    db,
                    &get_str!("machine_id"),
                    &get_str!("service"),
                    circuits,
                )
                .await
            }
            "service_disable" => {
                tools::service::service_disable(
                    db,
                    &get_str!("machine_id"),
                    &get_str!("service"),
                    circuits,
                )
                .await
            }
            "service_logs" => {
                tools::service::service_logs(
                    db,
                    &get_str!("machine_id"),
                    &get_str!("service"),
                    args.get("tail").and_then(|v| v.as_u64()).map(|n| n as usize),
                    circuits,
                )
                .await
            }
            // Log tools
            "log_tail" => {
                tools::logs::log_tail(
                    db,
                    &get_str!("machine_id"),
                    &get_str!("path"),
                    args.get("tail").and_then(|v| v.as_u64()).map(|n| n as usize),
                    get_opt_str!("cursor").as_deref(),
                    circuits,
                )
                .await
            }
            "log_grep" => {
                tools::logs::log_grep(
                    db,
                    &get_str!("machine_id"),
                    &get_str!("path"),
                    &get_str!("pattern"),
                    args.get("context").and_then(|v| v.as_u64()).map(|n| n as usize),
                    circuits,
                )
                .await
            }
            // Sysinfo tools
            "sys_info" => tools::sysinfo::sys_info(db, &get_str!("machine_id"), circuits).await,
            "disk_usage" => {
                tools::sysinfo::disk_usage(
                    db,
                    &get_str!("machine_id"),
                    get_opt_str!("path").as_deref(),
                    circuits,
                )
                .await
            }
            "net_ports" => tools::sysinfo::net_ports(db, &get_str!("machine_id"), circuits).await,
            "net_ping" => {
                tools::sysinfo::net_ping(
                    db,
                    &get_str!("machine_id"),
                    &get_str!("target"),
                    args.get("count").and_then(|v| v.as_u64()).map(|n| n as u32),
                    circuits,
                )
                .await
            }
            "net_interfaces" => {
                tools::sysinfo::net_interfaces(db, &get_str!("machine_id"), circuits).await
            }
            // Env tools
            "env_get" => {
                tools::env::env_get(
                    db,
                    &get_str!("machine_id"),
                    get_opt_str!("key").as_deref(),
                    circuits,
                )
                .await
            }
            "env_set" => {
                tools::env::env_set(
                    db,
                    &get_str!("machine_id"),
                    &get_str!("key"),
                    &get_str!("value"),
                    get_opt_str!("scope").as_deref(),
                    circuits,
                )
                .await
            }
            "env_unset" => {
                tools::env::env_unset(db, &get_str!("machine_id"), &get_str!("key"), circuits)
                    .await
            }
            "env_load" => {
                tools::env::env_load(db, &get_str!("machine_id"), &get_str!("path"), circuits)
                    .await
            }
            "env_clear" => {
                tools::env::env_clear(
                    db,
                    &get_str!("machine_id"),
                    get_opt_str!("scope").as_deref(),
                    circuits,
                )
                .await
            }
            // Fleet tools
            "fleet_exec" => {
                let ids: Vec<String> = args
                    .get("machine_ids")
                    .and_then(|v| v.as_array())
                    .map(|a| a.iter().filter_map(|v| v.as_str().map(|s| s.to_string())).collect())
                    .unwrap_or_default();
                tools::fleet::fleet_exec(db, ids, &get_str!("command"), get_opt_u64!("timeout_secs"), circuits).await
            }
            "fleet_ls" => {
                let ids: Vec<String> = args
                    .get("machine_ids")
                    .and_then(|v| v.as_array())
                    .map(|a| a.iter().filter_map(|v| v.as_str().map(|s| s.to_string())).collect())
                    .unwrap_or_default();
                tools::fleet::fleet_ls(db, ids, &get_str!("path"), circuits).await
            }
            "fleet_upload" => {
                let ids: Vec<String> = args
                    .get("machine_ids")
                    .and_then(|v| v.as_array())
                    .map(|a| a.iter().filter_map(|v| v.as_str().map(|s| s.to_string())).collect())
                    .unwrap_or_default();
                tools::fleet::fleet_upload(db, ids, &get_str!("local_path"), &get_str!("remote_path")).await
            }
            // Docker tools
            "docker_ps" => {
                tools::docker::docker_ps(db, &get_str!("machine_id"), get_bool!("all"), circuits).await
            }
            "docker_logs" => {
                tools::docker::docker_logs(
                    db,
                    &get_str!("machine_id"),
                    &get_str!("container"),
                    args.get("tail").and_then(|v| v.as_u64()).map(|n| n as usize),
                    get_bool!("follow"),
                    circuits,
                )
                .await
            }
            "docker_exec" => {
                tools::docker::docker_exec(
                    db,
                    &get_str!("machine_id"),
                    &get_str!("container"),
                    &get_str!("command"),
                    circuits,
                )
                .await
            }
            "docker_start" => {
                tools::docker::docker_start(db, &get_str!("machine_id"), &get_str!("container"), circuits).await
            }
            "docker_stop" => {
                tools::docker::docker_stop(db, &get_str!("machine_id"), &get_str!("container"), circuits).await
            }
            "docker_restart" => {
                tools::docker::docker_restart(db, &get_str!("machine_id"), &get_str!("container"), circuits).await
            }
            "docker_inspect" => {
                tools::docker::docker_inspect(db, &get_str!("machine_id"), &get_str!("container"), circuits).await
            }
            "docker_images" => {
                tools::docker::docker_images(db, &get_str!("machine_id"), circuits).await
            }
            // Git tools
            "git_status" => {
                tools::git::git_status(db, &get_str!("machine_id"), &get_str!("workdir"), circuits).await
            }
            "git_log" => {
                tools::git::git_log(
                    db,
                    &get_str!("machine_id"),
                    &get_str!("workdir"),
                    args.get("n").and_then(|v| v.as_u64()).map(|n| n as u32),
                    circuits,
                )
                .await
            }
            "git_diff" => {
                tools::git::git_diff(
                    db,
                    &get_str!("machine_id"),
                    &get_str!("workdir"),
                    get_opt_str!("target").as_deref(),
                    circuits,
                )
                .await
            }
            "git_pull" => {
                tools::git::git_pull(
                    db,
                    &get_str!("machine_id"),
                    &get_str!("workdir"),
                    get_opt_str!("remote").as_deref(),
                    get_opt_str!("branch").as_deref(),
                    circuits,
                )
                .await
            }
            "git_checkout" => {
                tools::git::git_checkout(
                    db,
                    &get_str!("machine_id"),
                    &get_str!("workdir"),
                    &get_str!("branch"),
                    get_bool!("create"),
                    circuits,
                )
                .await
            }
            // UI Automation tools
            "ui_describe" => {
                tools::ui::ui_describe(
                    db,
                    &get_str!("machine_id"),
                    get_opt_str!("window").as_deref(),
                    args.get("depth").and_then(|v| v.as_u64()).map(|n| n as u32),
                )
                .await
            }
            "ui_windows" => {
                tools::ui::ui_windows(db, &get_str!("machine_id")).await
            }
            "ui_tree" => {
                tools::ui::ui_tree(
                    db,
                    &get_str!("machine_id"),
                    get_opt_str!("window").as_deref(),
                    args.get("depth").and_then(|v| v.as_u64()).map(|n| n as u32),
                )
                .await
            }
            "ui_focus" => {
                tools::ui::ui_focus(db, &get_str!("machine_id"), &get_str!("window")).await
            }
            "ui_click" => {
                tools::ui::ui_click(
                    db,
                    &get_str!("machine_id"),
                    args.get("x").and_then(|v| v.as_i64()).unwrap_or(0) as i32,
                    args.get("y").and_then(|v| v.as_i64()).unwrap_or(0) as i32,
                    get_opt_str!("button").as_deref(),
                )
                .await
            }
            "ui_move" => {
                tools::ui::ui_move(
                    db,
                    &get_str!("machine_id"),
                    args.get("x").and_then(|v| v.as_i64()).unwrap_or(0) as i32,
                    args.get("y").and_then(|v| v.as_i64()).unwrap_or(0) as i32,
                )
                .await
            }
            "ui_type" => {
                tools::ui::ui_type(db, &get_str!("machine_id"), &get_str!("text")).await
            }
            "ui_key" => {
                tools::ui::ui_key(db, &get_str!("machine_id"), &get_str!("key")).await
            }
            "ui_scroll" => {
                tools::ui::ui_scroll(
                    db,
                    &get_str!("machine_id"),
                    args.get("x").and_then(|v| v.as_i64()).unwrap_or(0) as i32,
                    args.get("y").and_then(|v| v.as_i64()).unwrap_or(0) as i32,
                    get_opt_str!("direction").as_deref(),
                    args.get("amount").and_then(|v| v.as_i64()).map(|n| n as i32),
                )
                .await
            }
            "ui_find_element" => {
                tools::ui::ui_find_element(
                    db,
                    &get_str!("machine_id"),
                    get_opt_str!("window").as_deref(),
                    get_opt_str!("name").as_deref(),
                    get_opt_str!("automation_id").as_deref(),
                )
                .await
            }
            "ui_click_element" => {
                tools::ui::ui_click_element(
                    db,
                    &get_str!("machine_id"),
                    get_opt_str!("window").as_deref(),
                    get_opt_str!("name").as_deref(),
                    get_opt_str!("automation_id").as_deref(),
                )
                .await
            }
            "ui_get_value" => {
                tools::ui::ui_get_value(
                    db,
                    &get_str!("machine_id"),
                    get_opt_str!("window").as_deref(),
                    get_opt_str!("name").as_deref(),
                    get_opt_str!("automation_id").as_deref(),
                )
                .await
            }
            "ui_set_value" => {
                tools::ui::ui_set_value(
                    db,
                    &get_str!("machine_id"),
                    get_opt_str!("window").as_deref(),
                    get_opt_str!("name").as_deref(),
                    get_opt_str!("automation_id").as_deref(),
                    &get_str!("value"),
                )
                .await
            }
            "ui_screenshot" => {
                tools::ui::ui_screenshot(
                    db,
                    &get_str!("machine_id"),
                    get_opt_str!("window").as_deref(),
                )
                .await
            }
            "ui_ocr" => {
                tools::ui::ui_ocr(
                    db,
                    &get_str!("machine_id"),
                    get_opt_str!("window").as_deref(),
                )
                .await
            }
            other => Err(anyhow::anyhow!("Unknown tool: {}", other)),
        }
    }
}

impl ServerHandler for McpService {
    fn get_info(&self) -> ServerInfo {
        ServerInfo {
            protocol_version: ProtocolVersion::LATEST,
            capabilities: ServerCapabilities::builder()
                .enable_tools()
                .build(),
            server_info: Implementation {
                name: "remote-exec-mcp".to_string(),
                version: env!("CARGO_PKG_VERSION").to_string(),
            },
            instructions: Some("Remote execution MCP server. Use machine_add to register machines, then use exec/file/fs tools to interact with them.".to_string()),
        }
    }

    async fn list_tools(
        &self,
        _request: rmcp::model::PaginatedRequestParam,
        _context: RequestContext<rmcp::RoleServer>,
    ) -> Result<rmcp::model::ListToolsResult, rmcp::Error> {
        let machines = self.ctx.db.list().unwrap_or_default();
        let enabled = tools::compute_enabled_tools(&machines);

        let tool_defs = all_tool_definitions();
        let filtered: Vec<_> = tool_defs
            .into_iter()
            .filter(|t| enabled.contains(t.name.as_ref()))
            .collect();

        Ok(rmcp::model::ListToolsResult {
            tools: filtered,
            next_cursor: None,
        })
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParam,
        _context: RequestContext<rmcp::RoleServer>,
    ) -> Result<CallToolResult, rmcp::Error> {
        let name = request.name.as_ref();
        let args = request
            .arguments
            .map(|v| serde_json::Value::Object(v.into_iter().collect()))
            .unwrap_or(serde_json::Value::Object(serde_json::Map::new()));

        let result = self.dispatch_tool(name, args).await;

        match result {
            Ok(text) => Ok(CallToolResult {
                content: vec![Content::text(text)],
                is_error: Some(false),
            }),
            Err(e) => Ok(CallToolResult {
                content: vec![Content::text(format!("Error: {}", e))],
                is_error: Some(true),
            }),
        }
    }
}

fn schema(v: serde_json::Value) -> std::sync::Arc<serde_json::Map<String, serde_json::Value>> {
    std::sync::Arc::new(
        serde_json::from_value::<serde_json::Map<String, serde_json::Value>>(v).unwrap(),
    )
}

fn all_tool_definitions() -> Vec<rmcp::model::Tool> {
    use rmcp::model::Tool;
    use serde_json::json;

    vec![
        Tool {
            name: "machine_add".into(),
            description: "Register a new remote machine".into(),
            input_schema: schema(json!({
                "type": "object",
                "properties": {
                    "label": { "type": "string", "description": "Human-readable label" },
                    "host": { "type": "string", "description": "Hostname or IP" },
                    "port": { "type": "integer", "description": "SSH port (default 22)" },
                    "os": { "type": "string", "description": "OS type (linux/windows/macos)" },
                    "transport": { "type": "string", "description": "Transport type (ssh/agent/agent+ssh)" },
                    "ssh_user": { "type": "string" },
                    "ssh_key_path": { "type": "string" },
                    "ssh_password": { "type": "string" },
                    "agent_url": { "type": "string" },
                    "agent_token": { "type": "string" }
                },
                "required": ["label", "host"]
            })),
        },
        Tool {
            name: "machine_list".into(),
            description: "List all registered machines".into(),
            input_schema: schema(json!({ "type": "object", "properties": {} })),
        },
        Tool {
            name: "machine_remove".into(),
            description: "Remove a registered machine".into(),
            input_schema: schema(json!({
                "type": "object",
                "properties": { "id": { "type": "string" } },
                "required": ["id"]
            })),
        },
        Tool {
            name: "machine_test".into(),
            description: "Test connectivity to a machine".into(),
            input_schema: schema(json!({
                "type": "object",
                "properties": { "id": { "type": "string" } },
                "required": ["id"]
            })),
        },
        Tool {
            name: "machine_connect".into(),
            description: "Register a machine from a remcp:// URI (printed by `agent init` or install scripts). Example: machine_connect uri=\"remcp://192.168.1.42:8765?token=abc&label=my-pc&via=agent\"".into(),
            input_schema: schema(json!({
                "type": "object",
                "properties": {
                    "uri": { "type": "string", "description": "remcp:// URI from agent init output" }
                },
                "required": ["uri"]
            })),
        },
        Tool {
            name: "exec".into(),
            description: "Execute a shell command on a remote machine. Use only when no dedicated tool exists for the operation — prefer fs_ls over 'ls'/'dir', file_read over 'cat'/'type', fs_find over 'find', ps_list over 'ps'/'tasklist', etc.".into(),
            input_schema: schema(json!({
                "type": "object",
                "properties": {
                    "machine_id": { "type": "string" },
                    "command": { "type": "string" },
                    "workdir": { "type": "string" },
                    "timeout_secs": { "type": "integer" }
                },
                "required": ["machine_id", "command"]
            })),
        },
        Tool {
            name: "job_start".into(),
            description: "Start a background job on a machine (requires agent)".into(),
            input_schema: schema(json!({
                "type": "object",
                "properties": {
                    "machine_id": { "type": "string" },
                    "command": { "type": "string" },
                    "workdir": { "type": "string" }
                },
                "required": ["machine_id", "command"]
            })),
        },
        Tool {
            name: "job_status".into(),
            description: "Get status of a background job".into(),
            input_schema: schema(json!({
                "type": "object",
                "properties": {
                    "machine_id": { "type": "string" },
                    "job_id": { "type": "string" }
                },
                "required": ["machine_id", "job_id"]
            })),
        },
        Tool {
            name: "job_logs".into(),
            description: "Get logs from a background job".into(),
            input_schema: schema(json!({
                "type": "object",
                "properties": {
                    "machine_id": { "type": "string" },
                    "job_id": { "type": "string" },
                    "tail": { "type": "integer" },
                    "stream": { "type": "string", "enum": ["stdout", "stderr", "both"], "description": "Which stream to return (default: both)" }
                },
                "required": ["machine_id", "job_id"]
            })),
        },
        Tool {
            name: "job_kill".into(),
            description: "Kill a background job".into(),
            input_schema: schema(json!({
                "type": "object",
                "properties": {
                    "machine_id": { "type": "string" },
                    "job_id": { "type": "string" }
                },
                "required": ["machine_id", "job_id"]
            })),
        },
        Tool {
            name: "job_list".into(),
            description: "List all background jobs on a machine".into(),
            input_schema: schema(json!({
                "type": "object",
                "properties": { "machine_id": { "type": "string" } },
                "required": ["machine_id"]
            })),
        },
        Tool {
            name: "file_write".into(),
            description: "Write content to a remote file. Prefer this over exec with 'echo >', 'tee', or similar shell redirections.".into(),
            input_schema: schema(json!({
                "type": "object",
                "properties": {
                    "machine_id": { "type": "string" },
                    "path": { "type": "string" },
                    "content": { "type": "string" },
                    "mode": { "type": "string" }
                },
                "required": ["machine_id", "path", "content"]
            })),
        },
        Tool {
            name: "file_read".into(),
            description: "Read a remote file's contents. Prefer this over exec with 'cat', 'type', or similar shell commands.".into(),
            input_schema: schema(json!({
                "type": "object",
                "properties": {
                    "machine_id": { "type": "string" },
                    "path": { "type": "string" }
                },
                "required": ["machine_id", "path"]
            })),
        },
        Tool {
            name: "file_str_replace".into(),
            description: "Replace a string in a remote file".into(),
            input_schema: schema(json!({
                "type": "object",
                "properties": {
                    "machine_id": { "type": "string" },
                    "path": { "type": "string" },
                    "old_str": { "type": "string" },
                    "new_str": { "type": "string" }
                },
                "required": ["machine_id", "path", "old_str", "new_str"]
            })),
        },
        Tool {
            name: "file_patch".into(),
            description: "Apply a unified diff patch to a remote file".into(),
            input_schema: schema(json!({
                "type": "object",
                "properties": {
                    "machine_id": { "type": "string" },
                    "path": { "type": "string" },
                    "unified_diff": { "type": "string" }
                },
                "required": ["machine_id", "path", "unified_diff"]
            })),
        },
        Tool {
            name: "file_insert".into(),
            description: "Insert content at a line in a remote file".into(),
            input_schema: schema(json!({
                "type": "object",
                "properties": {
                    "machine_id": { "type": "string" },
                    "path": { "type": "string" },
                    "line": { "type": "integer" },
                    "content": { "type": "string" }
                },
                "required": ["machine_id", "path", "line", "content"]
            })),
        },
        Tool {
            name: "file_delete_lines".into(),
            description: "Delete lines from a remote file".into(),
            input_schema: schema(json!({
                "type": "object",
                "properties": {
                    "machine_id": { "type": "string" },
                    "path": { "type": "string" },
                    "start_line": { "type": "integer" },
                    "end_line": { "type": "integer" }
                },
                "required": ["machine_id", "path", "start_line", "end_line"]
            })),
        },
        Tool {
            name: "file_upload".into(),
            description: "Upload a local file to a remote machine".into(),
            input_schema: schema(json!({
                "type": "object",
                "properties": {
                    "machine_id": { "type": "string" },
                    "local_path": { "type": "string" },
                    "remote_path": { "type": "string" }
                },
                "required": ["machine_id", "local_path", "remote_path"]
            })),
        },
        Tool {
            name: "file_download".into(),
            description: "Download a remote file to local".into(),
            input_schema: schema(json!({
                "type": "object",
                "properties": {
                    "machine_id": { "type": "string" },
                    "remote_path": { "type": "string" },
                    "local_path": { "type": "string" }
                },
                "required": ["machine_id", "remote_path", "local_path"]
            })),
        },
        Tool {
            name: "session_open".into(),
            description: "Open an interactive session on a machine".into(),
            input_schema: schema(json!({
                "type": "object",
                "properties": {
                    "machine_id": { "type": "string" },
                    "workdir": { "type": "string" },
                    "shell": { "type": "string" }
                },
                "required": ["machine_id"]
            })),
        },
        Tool {
            name: "session_exec".into(),
            description: "Execute a command in an open session".into(),
            input_schema: schema(json!({
                "type": "object",
                "properties": {
                    "machine_id": { "type": "string" },
                    "session_id": { "type": "string" },
                    "command": { "type": "string" },
                    "timeout_secs": { "type": "integer" }
                },
                "required": ["machine_id", "session_id", "command"]
            })),
        },
        Tool {
            name: "session_close".into(),
            description: "Close an interactive session".into(),
            input_schema: schema(json!({
                "type": "object",
                "properties": {
                    "machine_id": { "type": "string" },
                    "session_id": { "type": "string" }
                },
                "required": ["machine_id", "session_id"]
            })),
        },
        Tool {
            name: "session_list".into(),
            description: "List active sessions on a machine".into(),
            input_schema: schema(json!({
                "type": "object",
                "properties": { "machine_id": { "type": "string" } },
                "required": ["machine_id"]
            })),
        },
        Tool {
            name: "fs_ls".into(),
            description: "List directory contents on a remote machine. Prefer this over exec with 'ls', 'dir', or similar shell commands.".into(),
            input_schema: schema(json!({
                "type": "object",
                "properties": {
                    "machine_id": { "type": "string" },
                    "path": { "type": "string" },
                    "all": { "type": "boolean" }
                },
                "required": ["machine_id", "path"]
            })),
        },
        Tool {
            name: "fs_stat".into(),
            description: "Get file/directory metadata (size, permissions, timestamps) on a remote machine. Prefer this over exec with 'stat', 'ls -la', or similar.".into(),
            input_schema: schema(json!({
                "type": "object",
                "properties": {
                    "machine_id": { "type": "string" },
                    "path": { "type": "string" }
                },
                "required": ["machine_id", "path"]
            })),
        },
        Tool {
            name: "fs_find".into(),
            description: "Find files by name or pattern on a remote machine. Prefer this over exec with 'find', 'dir /s', or similar shell commands.".into(),
            input_schema: schema(json!({
                "type": "object",
                "properties": {
                    "machine_id": { "type": "string" },
                    "path": { "type": "string" },
                    "pattern": { "type": "string" },
                    "file_type": { "type": "string" },
                    "max_depth": { "type": "integer" }
                },
                "required": ["machine_id", "path"]
            })),
        },
        Tool {
            name: "fs_tree".into(),
            description: "Show a recursive directory tree on a remote machine. Prefer this over exec with 'tree' or similar shell commands.".into(),
            input_schema: schema(json!({
                "type": "object",
                "properties": {
                    "machine_id": { "type": "string" },
                    "path": { "type": "string" },
                    "max_depth": { "type": "integer" }
                },
                "required": ["machine_id", "path"]
            })),
        },
        Tool {
            name: "fs_mkdir".into(),
            description: "Create a directory on a remote machine".into(),
            input_schema: schema(json!({
                "type": "object",
                "properties": {
                    "machine_id": { "type": "string" },
                    "path": { "type": "string" },
                    "parents": { "type": "boolean" }
                },
                "required": ["machine_id", "path"]
            })),
        },
        Tool {
            name: "fs_rm".into(),
            description: "Remove a file or directory on a remote machine".into(),
            input_schema: schema(json!({
                "type": "object",
                "properties": {
                    "machine_id": { "type": "string" },
                    "path": { "type": "string" },
                    "recursive": { "type": "boolean" }
                },
                "required": ["machine_id", "path"]
            })),
        },
        Tool {
            name: "fs_mv".into(),
            description: "Move/rename a file on a remote machine".into(),
            input_schema: schema(json!({
                "type": "object",
                "properties": {
                    "machine_id": { "type": "string" },
                    "src": { "type": "string" },
                    "dst": { "type": "string" }
                },
                "required": ["machine_id", "src", "dst"]
            })),
        },
        Tool {
            name: "fs_cp".into(),
            description: "Copy a file on a remote machine".into(),
            input_schema: schema(json!({
                "type": "object",
                "properties": {
                    "machine_id": { "type": "string" },
                    "src": { "type": "string" },
                    "dst": { "type": "string" },
                    "recursive": { "type": "boolean" }
                },
                "required": ["machine_id", "src", "dst"]
            })),
        },
        Tool {
            name: "ps_list".into(),
            description: "List running processes on a remote machine. Prefer this over exec with 'ps', 'tasklist', or similar shell commands.".into(),
            input_schema: schema(json!({
                "type": "object",
                "properties": {
                    "machine_id": { "type": "string" },
                    "filter": { "type": "string" }
                },
                "required": ["machine_id"]
            })),
        },
        Tool {
            name: "ps_kill".into(),
            description: "Send a signal to a process".into(),
            input_schema: schema(json!({
                "type": "object",
                "properties": {
                    "machine_id": { "type": "string" },
                    "pid": { "type": "integer" },
                    "signal": { "type": "string" }
                },
                "required": ["machine_id", "pid"]
            })),
        },
        Tool {
            name: "ps_tree".into(),
            description: "Show process tree".into(),
            input_schema: schema(json!({
                "type": "object",
                "properties": {
                    "machine_id": { "type": "string" },
                    "pid": { "type": "integer" }
                },
                "required": ["machine_id"]
            })),
        },
        Tool {
            name: "service_list".into(),
            description: "List systemd services".into(),
            input_schema: schema(json!({
                "type": "object",
                "properties": { "machine_id": { "type": "string" } },
                "required": ["machine_id"]
            })),
        },
        Tool {
            name: "service_status".into(),
            description: "Get status of a systemd service".into(),
            input_schema: schema(json!({
                "type": "object",
                "properties": {
                    "machine_id": { "type": "string" },
                    "service": { "type": "string" }
                },
                "required": ["machine_id", "service"]
            })),
        },
        Tool {
            name: "service_start".into(),
            description: "Start a systemd service".into(),
            input_schema: schema(json!({
                "type": "object",
                "properties": {
                    "machine_id": { "type": "string" },
                    "service": { "type": "string" }
                },
                "required": ["machine_id", "service"]
            })),
        },
        Tool {
            name: "service_stop".into(),
            description: "Stop a systemd service".into(),
            input_schema: schema(json!({
                "type": "object",
                "properties": {
                    "machine_id": { "type": "string" },
                    "service": { "type": "string" }
                },
                "required": ["machine_id", "service"]
            })),
        },
        Tool {
            name: "service_restart".into(),
            description: "Restart a systemd service".into(),
            input_schema: schema(json!({
                "type": "object",
                "properties": {
                    "machine_id": { "type": "string" },
                    "service": { "type": "string" }
                },
                "required": ["machine_id", "service"]
            })),
        },
        Tool {
            name: "service_enable".into(),
            description: "Enable a systemd service".into(),
            input_schema: schema(json!({
                "type": "object",
                "properties": {
                    "machine_id": { "type": "string" },
                    "service": { "type": "string" }
                },
                "required": ["machine_id", "service"]
            })),
        },
        Tool {
            name: "service_disable".into(),
            description: "Disable a systemd service".into(),
            input_schema: schema(json!({
                "type": "object",
                "properties": {
                    "machine_id": { "type": "string" },
                    "service": { "type": "string" }
                },
                "required": ["machine_id", "service"]
            })),
        },
        Tool {
            name: "service_logs".into(),
            description: "Get logs from a systemd service".into(),
            input_schema: schema(json!({
                "type": "object",
                "properties": {
                    "machine_id": { "type": "string" },
                    "service": { "type": "string" },
                    "tail": { "type": "integer" }
                },
                "required": ["machine_id", "service"]
            })),
        },
        Tool {
            name: "log_tail".into(),
            description: "Tail a log file on a remote machine (last N lines). Prefer this over exec with 'tail', 'Get-Content -Tail', or similar.".into(),
            input_schema: schema(json!({
                "type": "object",
                "properties": {
                    "machine_id": { "type": "string" },
                    "path": { "type": "string" },
                    "tail": { "type": "integer" },
                    "cursor": { "type": "string" }
                },
                "required": ["machine_id", "path"]
            })),
        },
        Tool {
            name: "log_grep".into(),
            description: "Search a log file for a pattern on a remote machine. Prefer this over exec with 'grep', 'Select-String', or similar.".into(),
            input_schema: schema(json!({
                "type": "object",
                "properties": {
                    "machine_id": { "type": "string" },
                    "path": { "type": "string" },
                    "pattern": { "type": "string" },
                    "context": { "type": "integer" }
                },
                "required": ["machine_id", "path", "pattern"]
            })),
        },
        Tool {
            name: "sys_info".into(),
            description: "Get system info from a remote machine".into(),
            input_schema: schema(json!({
                "type": "object",
                "properties": { "machine_id": { "type": "string" } },
                "required": ["machine_id"]
            })),
        },
        Tool {
            name: "disk_usage".into(),
            description: "Get disk usage on a remote machine".into(),
            input_schema: schema(json!({
                "type": "object",
                "properties": {
                    "machine_id": { "type": "string" },
                    "path": { "type": "string" }
                },
                "required": ["machine_id"]
            })),
        },
        Tool {
            name: "net_ports".into(),
            description: "List listening ports on a remote machine".into(),
            input_schema: schema(json!({
                "type": "object",
                "properties": { "machine_id": { "type": "string" } },
                "required": ["machine_id"]
            })),
        },
        Tool {
            name: "net_ping".into(),
            description: "Ping a host from a remote machine".into(),
            input_schema: schema(json!({
                "type": "object",
                "properties": {
                    "machine_id": { "type": "string" },
                    "target": { "type": "string" },
                    "count": { "type": "integer" }
                },
                "required": ["machine_id", "target"]
            })),
        },
        Tool {
            name: "net_interfaces".into(),
            description: "List network interfaces on a remote machine".into(),
            input_schema: schema(json!({
                "type": "object",
                "properties": { "machine_id": { "type": "string" } },
                "required": ["machine_id"]
            })),
        },
        Tool {
            name: "env_get".into(),
            description: "Get environment variables on a remote machine".into(),
            input_schema: schema(json!({
                "type": "object",
                "properties": {
                    "machine_id": { "type": "string" },
                    "key": { "type": "string" }
                },
                "required": ["machine_id"]
            })),
        },
        Tool {
            name: "env_set".into(),
            description: "Set an environment variable on a remote machine".into(),
            input_schema: schema(json!({
                "type": "object",
                "properties": {
                    "machine_id": { "type": "string" },
                    "key": { "type": "string" },
                    "value": { "type": "string" },
                    "scope": { "type": "string" }
                },
                "required": ["machine_id", "key", "value"]
            })),
        },
        Tool {
            name: "env_unset".into(),
            description: "Unset an environment variable on a remote machine".into(),
            input_schema: schema(json!({
                "type": "object",
                "properties": {
                    "machine_id": { "type": "string" },
                    "key": { "type": "string" }
                },
                "required": ["machine_id", "key"]
            })),
        },
        Tool {
            name: "env_load".into(),
            description: "Load environment from a file on a remote machine".into(),
            input_schema: schema(json!({
                "type": "object",
                "properties": {
                    "machine_id": { "type": "string" },
                    "path": { "type": "string" }
                },
                "required": ["machine_id", "path"]
            })),
        },
        Tool {
            name: "env_clear".into(),
            description: "Clear environment on a remote machine".into(),
            input_schema: schema(json!({
                "type": "object",
                "properties": {
                    "machine_id": { "type": "string" },
                    "scope": { "type": "string" }
                },
                "required": ["machine_id"]
            })),
        },
        Tool {
            name: "fleet_exec".into(),
            description: "Execute a command on multiple machines".into(),
            input_schema: schema(json!({
                "type": "object",
                "properties": {
                    "machine_ids": { "type": "array", "items": { "type": "string" } },
                    "command": { "type": "string" },
                    "timeout_secs": { "type": "integer" }
                },
                "required": ["machine_ids", "command"]
            })),
        },
        Tool {
            name: "fleet_ls".into(),
            description: "List directory contents on multiple machines".into(),
            input_schema: schema(json!({
                "type": "object",
                "properties": {
                    "machine_ids": { "type": "array", "items": { "type": "string" } },
                    "path": { "type": "string" }
                },
                "required": ["machine_ids", "path"]
            })),
        },
        Tool {
            name: "fleet_upload".into(),
            description: "Upload a file to multiple machines".into(),
            input_schema: schema(json!({
                "type": "object",
                "properties": {
                    "machine_ids": { "type": "array", "items": { "type": "string" } },
                    "local_path": { "type": "string" },
                    "remote_path": { "type": "string" }
                },
                "required": ["machine_ids", "local_path", "remote_path"]
            })),
        },
        Tool {
            name: "docker_ps".into(),
            description: "List Docker containers".into(),
            input_schema: schema(json!({
                "type": "object",
                "properties": {
                    "machine_id": { "type": "string" },
                    "all": { "type": "boolean" }
                },
                "required": ["machine_id"]
            })),
        },
        Tool {
            name: "docker_logs".into(),
            description: "Get Docker container logs".into(),
            input_schema: schema(json!({
                "type": "object",
                "properties": {
                    "machine_id": { "type": "string" },
                    "container": { "type": "string" },
                    "tail": { "type": "integer" },
                    "follow": { "type": "boolean" }
                },
                "required": ["machine_id", "container"]
            })),
        },
        Tool {
            name: "docker_exec".into(),
            description: "Execute a command in a Docker container".into(),
            input_schema: schema(json!({
                "type": "object",
                "properties": {
                    "machine_id": { "type": "string" },
                    "container": { "type": "string" },
                    "command": { "type": "string" }
                },
                "required": ["machine_id", "container", "command"]
            })),
        },
        Tool {
            name: "docker_start".into(),
            description: "Start a Docker container".into(),
            input_schema: schema(json!({
                "type": "object",
                "properties": {
                    "machine_id": { "type": "string" },
                    "container": { "type": "string" }
                },
                "required": ["machine_id", "container"]
            })),
        },
        Tool {
            name: "docker_stop".into(),
            description: "Stop a Docker container".into(),
            input_schema: schema(json!({
                "type": "object",
                "properties": {
                    "machine_id": { "type": "string" },
                    "container": { "type": "string" }
                },
                "required": ["machine_id", "container"]
            })),
        },
        Tool {
            name: "docker_restart".into(),
            description: "Restart a Docker container".into(),
            input_schema: schema(json!({
                "type": "object",
                "properties": {
                    "machine_id": { "type": "string" },
                    "container": { "type": "string" }
                },
                "required": ["machine_id", "container"]
            })),
        },
        Tool {
            name: "docker_inspect".into(),
            description: "Inspect a Docker container".into(),
            input_schema: schema(json!({
                "type": "object",
                "properties": {
                    "machine_id": { "type": "string" },
                    "container": { "type": "string" }
                },
                "required": ["machine_id", "container"]
            })),
        },
        Tool {
            name: "docker_images".into(),
            description: "List Docker images".into(),
            input_schema: schema(json!({
                "type": "object",
                "properties": { "machine_id": { "type": "string" } },
                "required": ["machine_id"]
            })),
        },
        Tool {
            name: "git_status".into(),
            description: "Get git status in a workdir".into(),
            input_schema: schema(json!({
                "type": "object",
                "properties": {
                    "machine_id": { "type": "string" },
                    "workdir": { "type": "string" }
                },
                "required": ["machine_id", "workdir"]
            })),
        },
        Tool {
            name: "git_log".into(),
            description: "Get git log in a workdir".into(),
            input_schema: schema(json!({
                "type": "object",
                "properties": {
                    "machine_id": { "type": "string" },
                    "workdir": { "type": "string" },
                    "n": { "type": "integer" }
                },
                "required": ["machine_id", "workdir"]
            })),
        },
        Tool {
            name: "git_diff".into(),
            description: "Get git diff in a workdir".into(),
            input_schema: schema(json!({
                "type": "object",
                "properties": {
                    "machine_id": { "type": "string" },
                    "workdir": { "type": "string" },
                    "target": { "type": "string" }
                },
                "required": ["machine_id", "workdir"]
            })),
        },
        Tool {
            name: "git_pull".into(),
            description: "Git pull in a workdir".into(),
            input_schema: schema(json!({
                "type": "object",
                "properties": {
                    "machine_id": { "type": "string" },
                    "workdir": { "type": "string" },
                    "remote": { "type": "string" },
                    "branch": { "type": "string" }
                },
                "required": ["machine_id", "workdir"]
            })),
        },
        Tool {
            name: "git_checkout".into(),
            description: "Git checkout in a workdir".into(),
            input_schema: schema(json!({
                "type": "object",
                "properties": {
                    "machine_id": { "type": "string" },
                    "workdir": { "type": "string" },
                    "branch": { "type": "string" },
                    "create": { "type": "boolean" }
                },
                "required": ["machine_id", "workdir", "branch"]
            })),
        },
        // ── Windows UI Automation ──────────────────────────────────────────────
        Tool {
            name: "ui_describe".into(),
            description: "Describe what is currently visible in a window as a flat text list of named UI elements (buttons, inputs, text, tabs, etc.). Works with Flutter/custom-rendered apps by activating the accessibility bridge first. Use this as the primary way to understand UI state — no image tokens needed.".into(),
            input_schema: schema(json!({
                "type": "object",
                "properties": {
                    "machine_id": { "type": "string" },
                    "window": { "type": "string", "description": "Window title (omit for full desktop)" },
                    "depth": { "type": "integer", "description": "Max element depth (default 6)" }
                },
                "required": ["machine_id"]
            })),
        },
        Tool {
            name: "ui_windows".into(),
            description: "List all open windows on a remote Windows machine (title, pid, class). Use this first to discover what is on screen before interacting.".into(),
            input_schema: schema(json!({
                "type": "object",
                "properties": { "machine_id": { "type": "string" } },
                "required": ["machine_id"]
            })),
        },
        Tool {
            name: "ui_tree".into(),
            description: "Get the Windows UI Automation element tree of a window (or the full desktop). Returns a structured text tree — prefer this over ui_screenshot to understand UI state without image tokens.".into(),
            input_schema: schema(json!({
                "type": "object",
                "properties": {
                    "machine_id": { "type": "string" },
                    "window": { "type": "string", "description": "Window title (omit for full desktop)" },
                    "depth": { "type": "integer", "description": "Max recursion depth (default 4, max 10)" }
                },
                "required": ["machine_id"]
            })),
        },
        Tool {
            name: "ui_focus".into(),
            description: "Bring a window to the foreground on a remote Windows machine.".into(),
            input_schema: schema(json!({
                "type": "object",
                "properties": {
                    "machine_id": { "type": "string" },
                    "window": { "type": "string", "description": "Window title" }
                },
                "required": ["machine_id", "window"]
            })),
        },
        Tool {
            name: "ui_click".into(),
            description: "Click at absolute screen coordinates on a remote Windows machine. Use ui_click_element instead when you know the element name.".into(),
            input_schema: schema(json!({
                "type": "object",
                "properties": {
                    "machine_id": { "type": "string" },
                    "x": { "type": "integer" },
                    "y": { "type": "integer" },
                    "button": { "type": "string", "description": "\"left\" (default), \"right\", or \"double\"" }
                },
                "required": ["machine_id", "x", "y"]
            })),
        },
        Tool {
            name: "ui_move".into(),
            description: "Move the mouse cursor to absolute coordinates on a remote Windows machine.".into(),
            input_schema: schema(json!({
                "type": "object",
                "properties": {
                    "machine_id": { "type": "string" },
                    "x": { "type": "integer" },
                    "y": { "type": "integer" }
                },
                "required": ["machine_id", "x", "y"]
            })),
        },
        Tool {
            name: "ui_type".into(),
            description: "Type a string of text into the currently focused UI element on a remote Windows machine. Focus the target first with ui_focus or ui_click_element.".into(),
            input_schema: schema(json!({
                "type": "object",
                "properties": {
                    "machine_id": { "type": "string" },
                    "text": { "type": "string" }
                },
                "required": ["machine_id", "text"]
            })),
        },
        Tool {
            name: "ui_key".into(),
            description: "Send a key or key combination on a remote Windows machine. Examples: \"enter\", \"escape\", \"ctrl+c\", \"alt+f4\", \"win+r\", \"ctrl+shift+i\", \"f5\".".into(),
            input_schema: schema(json!({
                "type": "object",
                "properties": {
                    "machine_id": { "type": "string" },
                    "key": { "type": "string", "description": "Key combo, e.g. \"ctrl+c\", \"alt+f4\", \"enter\", \"f5\"" }
                },
                "required": ["machine_id", "key"]
            })),
        },
        Tool {
            name: "ui_scroll".into(),
            description: "Scroll at a position on a remote Windows machine.".into(),
            input_schema: schema(json!({
                "type": "object",
                "properties": {
                    "machine_id": { "type": "string" },
                    "x": { "type": "integer" },
                    "y": { "type": "integer" },
                    "direction": { "type": "string", "description": "\"up\" or \"down\" (default \"down\")" },
                    "amount": { "type": "integer", "description": "Scroll ticks (default 3)" }
                },
                "required": ["machine_id", "x", "y"]
            })),
        },
        Tool {
            name: "ui_find_element".into(),
            description: "Find a UI element on a remote Windows machine and return its info (name, type, bounds, current value). Use ui_tree first to discover element names.".into(),
            input_schema: schema(json!({
                "type": "object",
                "properties": {
                    "machine_id": { "type": "string" },
                    "window": { "type": "string", "description": "Narrow search to this window title" },
                    "name": { "type": "string", "description": "Element name/label" },
                    "automation_id": { "type": "string", "description": "AutomationId property" }
                },
                "required": ["machine_id"]
            })),
        },
        Tool {
            name: "ui_click_element".into(),
            description: "Click a UI element by name or AutomationId on a remote Windows machine. Prefer this over ui_click when the element name is known — it is more reliable than coordinates.".into(),
            input_schema: schema(json!({
                "type": "object",
                "properties": {
                    "machine_id": { "type": "string" },
                    "window": { "type": "string" },
                    "name": { "type": "string", "description": "Element name/label" },
                    "automation_id": { "type": "string", "description": "AutomationId property" }
                },
                "required": ["machine_id"]
            })),
        },
        Tool {
            name: "ui_get_value".into(),
            description: "Read the current value or text of a UI element on a remote Windows machine (e.g. text field content, checkbox state).".into(),
            input_schema: schema(json!({
                "type": "object",
                "properties": {
                    "machine_id": { "type": "string" },
                    "window": { "type": "string" },
                    "name": { "type": "string" },
                    "automation_id": { "type": "string" }
                },
                "required": ["machine_id"]
            })),
        },
        Tool {
            name: "ui_set_value".into(),
            description: "Set the value of an input element (text field, etc.) on a remote Windows machine using the UI Automation ValuePattern.".into(),
            input_schema: schema(json!({
                "type": "object",
                "properties": {
                    "machine_id": { "type": "string" },
                    "window": { "type": "string" },
                    "name": { "type": "string" },
                    "automation_id": { "type": "string" },
                    "value": { "type": "string" }
                },
                "required": ["machine_id", "value"]
            })),
        },
        Tool {
            name: "ui_screenshot".into(),
            description: "Take a screenshot of the full screen or a specific window on a remote Windows machine. Returns base64 PNG. Use as a last resort — prefer ui_ocr or ui_describe first to understand UI state without image tokens.".into(),
            input_schema: schema(json!({
                "type": "object",
                "properties": {
                    "machine_id": { "type": "string" },
                    "window": { "type": "string", "description": "Window title (omit for full screen)" }
                },
                "required": ["machine_id"]
            })),
        },
        Tool {
            name: "ui_ocr".into(),
            description: "Capture a screenshot on the remote Windows machine and run local OCR (Windows Media OCR), returning only the extracted text — no image tokens consumed. Works on any app including Flutter/game windows where ui_describe gives insufficient data. Use when ui_describe returns nothing useful.".into(),
            input_schema: schema(json!({
                "type": "object",
                "properties": {
                    "machine_id": { "type": "string" },
                    "window": { "type": "string", "description": "Window title to capture (omit for full screen)" }
                },
                "required": ["machine_id"]
            })),
        },
    ]
}

fn cmd_init(mcp_name: &str) -> Result<()> {
    // 1. Resolve path to this binary
    let exe = std::env::current_exe()?;
    let exe_str = exe.to_string_lossy().into_owned();

    // 2. Write entry into ~/.claude.json
    let claude_json_path = dirs::home_dir()
        .ok_or_else(|| anyhow::anyhow!("Cannot determine home directory"))?
        .join(".claude.json");

    // Read existing file or start with empty object
    let mut root: serde_json::Value = if claude_json_path.exists() {
        let raw = std::fs::read_to_string(&claude_json_path)?;
        serde_json::from_str(&raw).unwrap_or(serde_json::json!({}))
    } else {
        serde_json::json!({})
    };

    // Insert/overwrite the server entry under mcpServers
    let entry = serde_json::json!({
        "type": "stdio",
        "command": exe_str
    });
    root["mcpServers"][mcp_name] = entry;

    // Write back with pretty formatting
    let json_out = serde_json::to_string_pretty(&root)?;
    std::fs::write(&claude_json_path, json_out)?;

    println!("Registered '{}' in {}", mcp_name, claude_json_path.display());
    println!("Command: {}", exe_str);
    println!("Restart Claude Code to pick up the new server.");

    Ok(())
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();

    if let Some(Command::Init { name }) = args.command {
        return cmd_init(&name);
    }

    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| EnvFilter::new(&args.log_level)),
        )
        .with_writer(std::io::stderr)
        .init();

    info!("Starting remote-exec-mcp server");

    let db = db::Db::open()?;
    let audit = audit::AuditLog::open()?;
    let circuits = transport::CircuitBreakers::new();

    // Start heartbeat
    heartbeat::start_heartbeat(db.clone(), circuits.clone());

    // Start weekly maintenance
    {
        let db_clone = db.clone();
        tokio::spawn(async move {
            let mut ticker = tokio::time::interval(tokio::time::Duration::from_secs(7 * 24 * 3600));
            ticker.tick().await; // skip first immediate tick
            loop {
                ticker.tick().await;
                let _ = db_clone.maintenance();
            }
        });
    }

    let ctx = tools::ToolContext::new(db, audit, circuits);
    let service = McpService::new(ctx);

    let transport = rmcp::transport::stdio();
    let server = service.serve(transport).await?;
    server.waiting().await?;

    info!("MCP server shut down");
    Ok(())
}
