# remote-exec-mcp

A Claude Code MCP server that gives Claude the ability to control remote machines over SSH or a lightweight HTTP agent daemon. Run commands, edit files, inspect processes, manage services, stream logs, control Windows UIs, and orchestrate fleets — all from a Claude conversation.

---

## Architecture

```
Claude Code
    │
    │  stdio (MCP protocol)
    ▼
┌─────────────────────┐
│     mcp-server      │   ← Rust binary, runs on your local machine
│  (crates/mcp-server)│     Translates Claude tool calls → HTTP or SSH
└─────────┬───────────┘
          │
          │  HMAC-signed HTTP  ──or──  SSH
          │
    ┌─────┴──────┐      ┌──────────────┐
    │   agent    │      │  SSH direct  │
    │(crates/    │      │  (no agent   │
    │  agent)    │      │  required)   │
    └────────────┘      └──────────────┘
    Runs on each
    remote machine
```

The **MCP server** runs locally alongside Claude Code. It maintains a SQLite registry of machines, signs every outgoing request with HMAC-SHA256, manages per-machine circuit breakers, and exposes 75+ tools to Claude.

The **agent** is a small HTTP daemon that runs on each remote machine. It executes commands, manages long-running jobs, hosts PTY sessions, streams logs, reports system metrics, and — on Windows — exposes UI automation and OCR. Install once per machine; the MCP server talks to it over HTTP.

SSH transport is a fallback — it works for basic operations without the agent installed.

---

## Pre-built Binaries

Static binaries are in the `dist/` directory — no Rust required on either the target machine or the MCP host.

### Agent (deploy to remote machines)

| File | Platform | Notes |
|---|---|---|
| `dist/agent-linux-x86_64` | Linux x86_64 | musl static — runs on any Linux, no glibc required |
| `dist/agent-linux-aarch64` | Linux ARM64 | musl static — Raspberry Pi, AWS Graviton, Apple M-series VMs |
| `dist/agent-windows-x86_64.exe` | Windows x86_64 | MSVC static CRT — includes UI automation and OCR |

### MCP Server (run on the Claude Code host)

| File | Platform | Notes |
|---|---|---|
| `dist/mcp-server-linux-x86_64` | Linux x86_64 | musl static — no glibc required |
| `dist/mcp-server-linux-aarch64` | Linux ARM64 | musl static — ARM hosts |
| `dist/mcp-server-windows-x86_64.exe` | Windows x86_64 | MSVC static CRT |

### Deploy the agent to a Linux machine

```bash
scp dist/agent-linux-x86_64 root@10.0.0.5:/usr/local/bin/remote-exec-agent
ssh root@10.0.0.5 'chmod +x /usr/local/bin/remote-exec-agent'
```

Then run the install script (which sets up config, token, and systemd service):

```bash
ssh root@10.0.0.5 'bash -s' < scripts/install-agent.sh -- --port 8765
```

The install script tries to download a fresh binary from GitHub Releases first. If the download fails (air-gapped machine, no internet), it falls back to the local binary you already copied.

### Deploy the agent to a Windows machine

```powershell
# Copy agent-windows-x86_64.exe to the target machine, then:
.\scripts\install-agent.ps1 -Port 8765
```

The PowerShell script creates a Windows Service, generates a secure token, writes the config, and health-checks the agent before returning.

### Rebuild the dist binaries

```bash
# Requires: cargo, cargo-zigbuild, zig (pip install ziglang)
bash scripts/build-dist.sh
```

---

## Quick Start

### 1. Build

```bash
# Requires Rust 1.70+
git clone <repo>
cd remote-exec-mcp
cargo build --release
```

Binaries land at `target/release/mcp-server` and `target/release/agent`.

### 2. Register with Claude Code

```bash
mcp-server init
# Creates ~/.config/remote-exec-mcp/
# Runs: claude mcp add remote-exec mcp-server
```

### 3. Install the agent on a remote machine

**Linux / macOS:**
```bash
curl -fsSL https://.../install-agent.sh | bash -s -- --port 8765
```

**Windows:**
```powershell
.\scripts\install-agent.ps1 -Port 8765
```

Both scripts:
1. Download or use the pre-built agent binary
2. Generate a secure random token
3. Write the config file
4. Install and start a systemd service (Linux) or Windows Service
5. Print the connection string

### 4. Register the machine with Claude

```
machine_connect remcp://root@10.0.0.5:8765?token=<token>&via=agent+ssh
```

Or register manually:

```
machine_add label="prod-web" host="10.0.0.5" transport="agent+ssh"
            agent_url="http://10.0.0.5:8765" agent_token="<token>"
            ssh_user="root" ssh_key_path="~/.ssh/id_rsa"
```

---

## Transport Modes

| Mode | Description |
|---|---|
| `ssh` | Pure SSH. Works everywhere, no agent needed. Limited to basic exec and file ops. |
| `agent` | HTTP agent only. Full feature set (jobs, PTY sessions, metrics, docker, UI automation, etc.). |
| `agent+ssh` | Agent preferred; automatic SSH fallback if agent unreachable. Recommended. |

---

## Tools Reference

All tools are available in Claude conversations once a machine is registered.

### Machine Management

| Tool | Description |
|---|---|
| `machine_add` | Register a machine (SSH or agent). Immediately fetches capabilities if agent_url provided. |
| `machine_list` | Table of all registered machines: id, label, host:port, transport, status, OS. |
| `machine_remove` | Delete a machine from the registry. |
| `machine_test` | Ping the machine, measure round-trip latency, refresh capabilities, display metrics. |

### Command Execution

| Tool | Description |
|---|---|
| `exec` | Run a command synchronously. Returns stdout, stderr, exit_code. Max 120s. |
| `job_start` | Start a command in the background. Returns a `job_id` immediately. |
| `job_status` | Check status, PID, exit code, and bytes of a running or finished job. |
| `job_logs` | Stream stdout+stderr of a job. Supports `tail=N` for last N lines. |
| `job_kill` | Send SIGTERM → wait 5s → SIGKILL. |
| `job_list` | Table of recent jobs on a machine. |

### File Operations

| Tool | Description |
|---|---|
| `file_upload` | Upload a local file to the remote machine. |
| `file_download` | Download a remote file to local disk. |
| `file_write` | Write text content directly to a remote path (no local file needed). |
| `file_read` | Read a remote file. Paginated at 64 KB by default. |
| `file_str_replace` | Exact-match string replacement. Errors if 0 or 2+ matches found. |
| `file_patch` | Apply a unified diff to a remote file. |
| `file_insert` | Insert content at a specific line number. |
| `file_delete_lines` | Delete a range of lines from a file. |

### PTY Sessions

Sessions maintain shell state (working directory, environment, shell variables) across multiple commands — unlike `exec` which starts fresh each time.

| Tool | Description |
|---|---|
| `session_open` | Open a persistent PTY shell. Returns a `session_id`. |
| `session_exec` | Run a command inside an existing session. Output includes exit code. |
| `session_close` | Close a PTY session. |
| `session_list` | List open sessions: id, cwd, idle time, status. |

### Filesystem

| Tool | Description |
|---|---|
| `fs_ls` | List directory: name, type, size, modified, permissions. |
| `fs_stat` | Stat a path: type, size, owner, permissions, symlink target. |
| `fs_find` | Find files by glob or regex pattern. |
| `fs_tree` | ASCII directory tree (default depth 3). |
| `fs_mkdir` | Create directory, optionally recursive. |
| `fs_rm` | Remove file or directory. |
| `fs_mv` | Move or rename. |
| `fs_cp` | Copy file or directory. |

### Processes

| Tool | Description |
|---|---|
| `ps_list` | List processes: PID, name, CPU%, memory MB, status, user. Filterable. |
| `ps_kill` | Kill by PID or process name. Supports custom signal. Returns killed PIDs. |
| `ps_tree` | ASCII process tree from root or a given PID. |

### System Information

| Tool | Description |
|---|---|
| `sys_info` | Hostname, OS, arch, CPU count, RAM used/total, uptime, load averages. |
| `disk_usage` | Per-mount: total, used, free, percent. |
| `net_ports` | Listening ports: port, proto, state, PID, process name. |
| `net_ping` | Ping a host from the remote machine: reachable, avg_ms, packet_loss%. |
| `net_interfaces` | Network interfaces: name, IP, MAC, up/down. |

### Logs

| Tool | Description |
|---|---|
| `log_tail` | Tail a log file. Supports `cursor` (byte offset) for incremental reads. |
| `log_grep` | Search a log file with regex. Returns matches with surrounding context lines. |

### Environment Variables

Environment variables are stored in-memory on the MCP server, scoped to `(machine_id, session_id)`, and injected automatically into `session_exec` calls.

| Tool | Description |
|---|---|
| `env_set` | Set one or more environment variables for a session. |
| `env_get` | Get all variables or a specific key. |
| `env_unset` | Remove a variable. |
| `env_load` | Fetch a `.env` file from the remote machine and parse it into the session env. |
| `env_clear` | Remove all variables for a session. |

### Services (systemd / Windows Services)

| Tool | Description |
|---|---|
| `service_list` | List services: name, status, enabled. |
| `service_status` | Full status block for a service. |
| `service_start` / `stop` / `restart` | Control a service. |
| `service_enable` / `disable` | Set service autostart. |
| `service_logs` | journald (Linux) or Event Log (Windows) output. |

### Docker

| Tool | Description |
|---|---|
| `docker_ps` | List containers: id, name, image, status, ports. |
| `docker_logs` | Stream container logs. Supports `cursor` for incremental reads. |
| `docker_exec` | Execute a command inside a running container. |
| `docker_start` / `stop` / `restart` | Container lifecycle. |
| `docker_inspect` | Full container inspect as pretty-printed JSON. |
| `docker_images` | List images: id, repo, tag, size, created. |

### Git

| Tool | Description |
|---|---|
| `git_status` | Branch, ahead/behind, staged, unstaged, untracked counts. |
| `git_log` | Recent commits: hash, author, date, message. |
| `git_diff` | Unified diff. Paginated at 100 KB. |
| `git_pull` | Pull latest changes. |
| `git_checkout` | Checkout a branch or commit. |

### Fleet Operations

Run a command or file operation across multiple machines in parallel (up to 10 concurrent).

| Tool | Description |
|---|---|
| `fleet_exec` | Run a command on a list of machines. Returns per-machine: status, exit_code, output preview, error. |
| `fleet_ls` | List a directory on multiple machines. |
| `fleet_upload` | Upload a file to multiple machines. |

### Windows UI Automation

Control Windows applications on remote machines using the Windows UI Automation (UIA) API and Windows Media OCR. All tools require an agent connection (`agent_url` must be set). Non-Windows agents return 501.

**Recommended workflow for understanding UI state (cheapest to most expensive):**
1. `ui_describe` — flat text list of named elements; activates Flutter's accessibility bridge
2. `ui_ocr` — screenshot + local OCR; returns only text, no image tokens; works on any app
3. `ui_screenshot` — raw base64 PNG; use only when OCR and describe are insufficient

| Tool | Description |
|---|---|
| `ui_describe` | Describe what is visible in a window as a flat text list: `[Button] OK`, `[Text] Score: 1250`. Activates Flutter's accessibility bridge first. Zero image tokens. |
| `ui_ocr` | Capture a screenshot and run Windows Media OCR locally on the agent. Returns only extracted text — zero image tokens. Works on any app including Flutter/game windows where `ui_describe` returns nothing useful. |
| `ui_windows` | List open windows: title, PID, class name. |
| `ui_tree` | Get the UIA element tree of a window (or the full desktop). Returns a structured text tree. |
| `ui_focus` | Bring a window to the foreground. |
| `ui_click` | Click at absolute screen coordinates. Supports `left` (default), `right`, and `double`. |
| `ui_move` | Move the mouse cursor to absolute coordinates. |
| `ui_type` | Type a string of text into the currently focused element. |
| `ui_key` | Send a key combination: `ctrl+c`, `alt+f4`, `win+r`, `enter`, `f5`, etc. |
| `ui_scroll` | Scroll at a position. Supports `up`/`down` direction and amount. |
| `ui_find_element` | Find a UI element by name or automation ID. Returns name, type, bounds, value, enabled. |
| `ui_click_element` | Click a UI element found by name or automation ID (more reliable than coordinate clicks). |
| `ui_get_value` | Read the current value or text of a UI element (e.g. a text field). |
| `ui_set_value` | Set the value of an input element directly (bypasses typing). |
| `ui_screenshot` | Take a screenshot of the full screen or a specific window. Returns base64 PNG. |

**Supported key names for `ui_key`:** `ctrl`, `alt`, `shift`, `win`, `enter`, `escape`, `tab`, `space`, `backspace`, `delete`, `home`, `end`, `pageup`, `pagedown`, `up`, `down`, `left`, `right`, `f1`–`f12`, or any single character. Combine with `+` (e.g. `ctrl+shift+esc`).

**Flutter app note:** Flutter uses a custom Skia renderer — the UIA tree is nearly empty without accessibility enabled. `ui_describe` performs a warm-up walk that signals Flutter to build its semantics tree, then reads it 400 ms later. If the result is still sparse, fall back to `ui_ocr`.

---

## Agent HTTP API

The agent exposes a REST API on the configured port (default: 8765). All routes except `/health` and `/metrics` require HMAC authentication.

### Authentication

Every request must include two headers:

```
X-Agent-Timestamp: <unix_seconds>
X-Agent-Signature: <hmac_sha256_hex>
```

Signature is computed as:

```
body_hash = sha256_hex(request_body)   # empty string for GET
message   = "{METHOD}\n{PATH_AND_QUERY}\n{timestamp}\n{body_hash}"
signature = hmac_sha256_hex(token, message)
```

The full path including query string is included in the message, so `GET /fs/ls?path=/tmp` and `GET /fs/ls?path=/etc` produce different signatures. Requests with a timestamp more than 60 seconds old are rejected (replay protection).

### Endpoints

| Method | Path | Description |
|---|---|---|
| `GET` | `/health` | `{"status":"ok"}` — no auth |
| `GET` | `/metrics` | Counters + uptime — no auth |
| `GET` | `/capabilities` | Detected OS, arch, hostname, has_docker, has_git, has_systemd, has_ui_automation |
| `POST` | `/exec` | Run command, wait for completion |
| `POST` | `/job/start` | Start background job, returns `job_id` |
| `GET` | `/job/:id` | Job status and counters |
| `GET` | `/job/:id/logs` | Job stdout+stderr |
| `POST` | `/job/:id/kill` | Kill a job |
| `GET` | `/jobs` | List recent jobs |
| `POST` | `/file/upload` | Upload file (multipart) |
| `GET` | `/file/download` | Download file |
| `POST` | `/file/write` | Write text to file |
| `GET` | `/file/read` | Read file |
| `POST` | `/file/str-replace` | Exact string replacement |
| `POST` | `/file/patch` | Apply unified diff |
| `POST` | `/file/insert` | Insert lines |
| `POST` | `/file/delete-lines` | Delete lines |
| `GET` | `/fs/ls` | List directory |
| `GET` | `/fs/stat` | Stat path |
| `POST` | `/fs/find` | Find files |
| `GET` | `/fs/tree` | Directory tree |
| `POST` | `/fs/mkdir` | Create directory |
| `DELETE` | `/fs/rm` | Remove path |
| `POST` | `/fs/mv` | Move path |
| `POST` | `/fs/cp` | Copy path |
| `GET` | `/process/list` | List processes |
| `POST` | `/process/kill` | Kill process |
| `GET` | `/process/tree` | Process tree |
| `GET` | `/service/list` | List services |
| `GET` | `/service/:name/status` | Service status |
| `POST` | `/service/:name/start` | Start service |
| `POST` | `/service/:name/stop` | Stop service |
| `POST` | `/service/:name/restart` | Restart service |
| `POST` | `/service/:name/enable` | Enable service |
| `POST` | `/service/:name/disable` | Disable service |
| `GET` | `/service/:name/logs` | Service logs |
| `GET` | `/log/tail` | Tail a log file |
| `GET` | `/log/grep` | Grep a log file |
| `GET` | `/sysinfo` | System info |
| `GET` | `/sysinfo/disk` | Disk usage |
| `GET` | `/sysinfo/ports` | Listening ports |
| `POST` | `/sysinfo/ping` | Ping a host |
| `GET` | `/sysinfo/interfaces` | Network interfaces |
| `POST` | `/session` | Open PTY session |
| `POST` | `/session/:id/exec` | Execute in session |
| `DELETE` | `/session/:id` | Close session |
| `GET` | `/sessions` | List sessions |
| `GET` | `/docker/ps` | Container list |
| `GET` | `/docker/:container/logs` | Container logs |
| `POST` | `/docker/:container/exec` | Exec in container |
| `POST` | `/docker/:container/start` | Start container |
| `POST` | `/docker/:container/stop` | Stop container |
| `POST` | `/docker/:container/restart` | Restart container |
| `GET` | `/docker/:container/inspect` | Inspect container |
| `GET` | `/docker/images` | List images |
| `GET` | `/git/status` | Git status |
| `GET` | `/git/log` | Git log |
| `GET` | `/git/diff` | Git diff |
| `POST` | `/git/pull` | Git pull |
| `POST` | `/git/checkout` | Git checkout |
| `GET` | `/ui/windows` | List open windows (Windows only) |
| `GET` | `/ui/tree` | UIA element tree (Windows only) |
| `POST` | `/ui/focus` | Focus a window (Windows only) |
| `POST` | `/ui/click` | Click at coordinates (Windows only) |
| `POST` | `/ui/move` | Move mouse (Windows only) |
| `POST` | `/ui/type` | Type text (Windows only) |
| `POST` | `/ui/key` | Send key combo (Windows only) |
| `POST` | `/ui/scroll` | Scroll at position (Windows only) |
| `GET` | `/ui/element` | Find element info (Windows only) |
| `POST` | `/ui/click-element` | Click element by name/id (Windows only) |
| `GET` | `/ui/get-value` | Read element value (Windows only) |
| `POST` | `/ui/set-value` | Write element value (Windows only) |
| `GET` | `/ui/screenshot` | Capture screenshot as base64 PNG (Windows only) |
| `GET` | `/ui/describe` | Describe visible UI as flat text, activates Flutter bridge (Windows only) |
| `GET` | `/ui/ocr` | Screenshot + Windows Media OCR, returns plain text (Windows only) |

Non-Windows agents return `501 Not Implemented` for all `/ui/*` routes.

---

## Configuration

### MCP Server

Configuration is stored at `~/.config/remote-exec-mcp/`. No manual editing required — tools manage the machine registry automatically.

The **audit log** lives at `~/.config/remote-exec-mcp/audit.log`. It rotates at 10 MB, keeping 5 files. Every tool call is logged as a JSONL entry:

```json
{"ts":1712000000,"tool":"exec","machine_id":"abc123","label":"prod","args":{"command":"ls -la"},"ok":true,"duration_ms":234,"exit_code":0}
```

Sensitive fields (`ssh_password`, `agent_token`, `content`, `unified_diff`, `password`, `token`) are always redacted before writing.

### Agent

The agent is configured by CLI flags or a JSON config file. Pass `--config /path/to/config.json` to use a file.

```json
{
  "port": 8765,
  "bind": "0.0.0.0",
  "token": "<secret>",
  "max_concurrent_execs": 32,
  "max_jobs": 100,
  "allowed_ips": [],
  "log_level": "info"
}
```

**Hot-reloadable fields** (send `SIGHUP` on Linux to apply without restart):
- `max_concurrent_execs`
- `max_jobs`
- `allowed_ips`
- `log_level`

**Requires restart** to change: `port`, `bind`, `token`.

### Agent CLI flags

```
Usage: agent [OPTIONS] --token <TOKEN>

Options:
      --bind <BIND>                     [default: 0.0.0.0]
  -p, --port <PORT>                     [default: 8765]
      --token <TOKEN>                   [env: AGENT_TOKEN]
      --config <CONFIG>                 Path to JSON config file
      --log-level <LOG_LEVEL>           [default: info]
      --max-concurrent-execs <N>        [default: 32]
      --max-jobs <N>                    [default: 100]
```

---

## Reliability Design

### Circuit Breaker

Each machine has an independent circuit breaker with three states:

```
Closed ──(3 failures in 60s)──► Open ──(30s)──► HalfOpen
  ▲                                                  │
  └──────────── success ─────────────────────────────┘
```

When **Open**, all tool calls for that machine fail immediately with a clear error message including the retry-after time. This prevents cascading failures when a machine is down.

### Heartbeat

A background task pings every registered machine every 30 seconds via `GET /health`. On success, it updates `status=online` and `last_seen`. On failure, it records a circuit breaker failure. Tool dispatch checks `last_seen` — if a machine hasn't been reachable for more than 5 minutes, the tool fails fast rather than hanging.

### Output Pagination

All tool outputs are capped at **100 KB** before being returned to Claude. If output is truncated, a message is appended with instructions on how to paginate using `log_tail` with a cursor or `job_logs` with `tail=N`.

### Job Buffer Backpressure

Background jobs buffer up to 10,000 lines or 50 MB per stream (stdout/stderr). When the buffer is full, new lines are **dropped** (not buffered to disk, not stalling the child process). The job status response includes `stdout_bytes_dropped` and `stderr_bytes_dropped` counters so Claude knows if output was lost.

### Graceful Shutdown

On `SIGTERM` or `Ctrl-C`, the agent drains in-flight requests for up to 30 seconds, then writes a `pids.json` file listing all running job PIDs. On next startup, the agent reads this file and marks any jobs whose PIDs are no longer alive as `failed`.

---

## Security

- **HMAC-SHA256** — every request from the MCP server to the agent is signed with the shared token. The token is never transmitted in plaintext.
- **Replay protection** — requests with a timestamp older than 60 seconds are rejected.
- **Constant-time comparison** — signature verification uses `subtle::ConstantTimeEq` to prevent timing attacks.
- **Audit log redaction** — `ssh_password`, `agent_token`, `content` (file writes), `unified_diff`, `password`, and `token` fields are always replaced with `[REDACTED]` in the audit log.
- **Token scoping** — each machine has its own token. Compromise of one machine's token does not affect others.
- **No root required** — the agent can run as any user. Its capabilities are limited to what that user can do on the OS.

---

## Project Layout

```
remote-exec-mcp/
├── Cargo.toml                  # workspace
├── crates/
│   ├── mcp-server/             # Claude Code MCP server (stdio)
│   │   └── src/
│   │       ├── main.rs         # startup, MCP tool dispatch
│   │       ├── error.rs        # RemoteExecError enum
│   │       ├── db.rs           # SQLite machine registry
│   │       ├── audit.rs        # rotating JSONL audit log
│   │       ├── auth.rs         # HMAC request signing
│   │       ├── heartbeat.rs    # background machine ping task
│   │       ├── transport/
│   │       │   ├── mod.rs      # circuit breaker, transport dispatch
│   │       │   ├── ssh.rs      # russh connection pool
│   │       │   └── agent.rs    # reqwest HTTP client
│   │       └── tools/          # one file per tool group
│   │           ├── mod.rs      # shared helpers, compute_enabled_tools
│   │           ├── exec.rs     # exec, job_*
│   │           ├── file.rs     # file_*
│   │           ├── fs.rs       # fs_*
│   │           ├── process.rs  # ps_*
│   │           ├── session.rs  # session_*
│   │           ├── sysinfo.rs  # sys_info, disk_usage, net_*
│   │           ├── logs.rs     # log_tail, log_grep
│   │           ├── env.rs      # env_*
│   │           ├── service.rs  # service_*
│   │           ├── docker.rs   # docker_*
│   │           ├── git.rs      # git_*
│   │           ├── fleet.rs    # fleet_*
│   │           ├── machine.rs  # machine_*
│   │           └── ui.rs       # ui_* (Windows UI automation)
│   └── agent/                  # HTTP daemon for remote machines
│       └── src/
│           ├── main.rs         # startup, graceful shutdown
│           ├── lib.rs          # public module exports
│           ├── auth.rs         # HMAC verification middleware
│           ├── config.rs       # CLI args + config file + SIGHUP reload
│           ├── capabilities.rs # detect docker/git/systemd/ui_automation at startup
│           ├── jobs.rs         # async job store with backpressure
│           ├── sessions.rs     # PTY session management
│           ├── metrics.rs      # atomic counters
│           └── routes/         # one file per route group
│               ├── mod.rs      # router, auth middleware wiring
│               ├── exec.rs     # /exec, /job/*
│               ├── file.rs     # /file/*
│               ├── fs.rs       # /fs/*
│               ├── process.rs  # /process/*
│               ├── session.rs  # /session/*
│               ├── sysinfo.rs  # /sysinfo/*
│               ├── logs.rs     # /log/*
│               ├── service.rs  # /service/*
│               ├── docker.rs   # /docker/*
│               ├── git.rs      # /git/*
│               └── ui.rs       # /ui/* (Windows: UIA + OCR; non-Windows: 501)
└── scripts/
    ├── build-dist.sh           # cross-compile all platform binaries
    ├── install-agent.sh        # Linux/macOS one-liner installer
    └── install-agent.ps1       # Windows installer (creates Windows Service)
```

---

## Development

```bash
# Build both crates
cargo build

# Run all tests
cargo test

# Run only mcp-server tests
cargo test -p mcp-server

# Run only agent tests
cargo test -p agent

# Build release binaries
cargo build --release

# Check for warnings
cargo clippy

# Cross-compile all dist binaries (requires cargo-zigbuild + zig)
bash scripts/build-dist.sh
```

### Test Coverage

| Crate | Module | What is tested |
|---|---|---|
| mcp-server | `auth` | SHA-256, HMAC, sign/verify roundtrip, replay/tamper rejection |
| mcp-server | `audit` | Redaction of all sensitive keys, nested objects, safe field preservation |
| mcp-server | `error` | Human-readable message formatting for all error variants |
| mcp-server | `db` | Full CRUD, upsert-overwrite, heartbeat, capabilities, in-memory SQLite |
| mcp-server | `tools/mod` | `paginate` at/over limit, `compute_enabled_tools` always returns all tools |
| mcp-server | `tools/ui` | AgentRequired for all ui_* tools without agent_url, urlenc correctness |
| mcp-server | `transport/mod` | Circuit breaker state machine, async `CircuitBreakers` with concurrent machines |
| agent | `auth` | SHA-256, HMAC correctness and output format |
| agent | `jobs` | Job lifecycle, stdout capture, exit codes, `StreamBuffer`, eviction logic |
| agent | `routes/ui` | Key parsing (modifiers, F-keys, aliases, unknown=err), UIA tree smoke test (Windows only) |
| agent | `tests/integration` | HTTP endpoints: health, metrics, 401 rejection (5 cases), exec, job start/status |

---

## Dependencies

| Crate | Purpose |
|---|---|
| `rmcp` | Official Rust MCP SDK (stdio transport) |
| `russh` + `russh-sftp` | SSH client with connection pooling |
| `reqwest` | HTTP client for agent communication |
| `axum` + `tower` + `tower-http` | HTTP server framework for the agent |
| `rusqlite` (bundled) | SQLite machine registry |
| `tokio` | Async runtime |
| `serde` + `serde_json` | Serialization |
| `hmac` + `sha2` | HMAC-SHA256 authentication |
| `dashmap` | Concurrent hash maps (circuit breakers, SSH pools) |
| `portable-pty` | Cross-platform PTY for session support |
| `strip-ansi-escapes` | Clean PTY output for Claude |
| `sysinfo` | System metrics (CPU, RAM, disks, networks, processes) |
| `clap` | CLI argument parsing |
| `tracing` + `tracing-subscriber` | Structured logging |
| `subtle` | Constant-time signature comparison |
| `thiserror` | Structured error enum |
| `uiautomation` | Windows UI Automation API bindings *(Windows agent only)* |
| `enigo` | Cross-platform keyboard/mouse simulation *(Windows agent only)* |
| `screenshots` | Screen capture *(Windows agent only)* |
| `windows` | WinRT APIs for Windows Media OCR *(Windows agent only)* |
| `base64` | Encode screenshots as base64 for transport *(Windows agent only)* |
