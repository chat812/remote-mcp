#!/usr/bin/env bash
# Install remote-exec agent on Linux/macOS
# Usage: curl -fsSL https://.../install-agent.sh | bash -s -- --port 8765
set -euo pipefail

PORT=8765
INSTALL_DIR=/usr/local/bin
CONFIG_DIR=/etc/remote-exec-agent
RUN_DIR=/var/run/remote-exec-agent
SERVICE_NAME=remote-exec-agent
GITHUB_REPO="chat812/remote-mcp"
GITHUB_RELEASE_BASE="https://github.com/${GITHUB_REPO}/releases/latest/download"

# Parse arguments
while [[ $# -gt 0 ]]; do
    case $1 in
        --port)
            PORT="$2"
            shift 2
            ;;
        --install-dir)
            INSTALL_DIR="$2"
            shift 2
            ;;
        *)
            echo "Unknown argument: $1"
            exit 1
            ;;
    esac
done

# Must run as root
if [[ $EUID -ne 0 ]]; then
    echo "Please run as root (sudo)" >&2
    exit 1
fi

# 1. Detect OS and arch
OS=$(uname -s | tr '[:upper:]' '[:lower:]')
ARCH=$(uname -m)
case "$ARCH" in
    x86_64)  ARCH_SUFFIX="x86_64" ;;
    aarch64) ARCH_SUFFIX="aarch64" ;;
    arm64)   ARCH_SUFFIX="aarch64" ;;
    *)       ARCH_SUFFIX="$ARCH" ;;
esac

echo "Detected: $OS / $ARCH_SUFFIX"

# 2. Install binary — local script dir > GitHub release > cargo build
BINARY_PATH="$INSTALL_DIR/remote-exec-agent"
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
LOCAL_BINARY="$SCRIPT_DIR/agent-linux-${ARCH_SUFFIX}"
RELEASE_URL="${GITHUB_RELEASE_BASE}/agent-linux-${ARCH_SUFFIX}"

download_binary() {
    if command -v curl &>/dev/null; then
        curl -fsSL --retry 3 -o "$BINARY_PATH" "$1"
    elif command -v wget &>/dev/null; then
        wget -qO "$BINARY_PATH" "$1"
    else
        echo "Neither curl nor wget found. Please install one and retry." >&2
        exit 1
    fi
}

if [[ -f "$LOCAL_BINARY" ]]; then
    echo "Using local dist binary: $LOCAL_BINARY"
    cp "$LOCAL_BINARY" "$BINARY_PATH"
else
    echo "Downloading prebuilt static binary..."
    if download_binary "$RELEASE_URL"; then
        echo "Downloaded from ${RELEASE_URL}"
    else
        echo "Download failed. Falling back to building from source..."
        if command -v cargo &>/dev/null; then
            if [[ -f "$REPO_ROOT/Cargo.toml" ]]; then
                cargo build --release --manifest-path "$REPO_ROOT/Cargo.toml" -p agent
                cp "$REPO_ROOT/target/release/agent" "$BINARY_PATH"
            else
                cargo install --git "https://github.com/$GITHUB_REPO" agent --root /usr/local
                cp /usr/local/bin/agent "$BINARY_PATH" 2>/dev/null || true
            fi
        else
            echo "Error: download failed and cargo not found. Cannot install." >&2
            exit 1
        fi
    fi
fi

chmod +x "$BINARY_PATH"
echo "Binary installed at $BINARY_PATH"

# 3. Generate token
if command -v openssl &>/dev/null; then
    TOKEN=$(openssl rand -hex 32)
else
    TOKEN=$(cat /dev/urandom | tr -dc 'a-f0-9' | head -c 64)
fi

# 4. Write config
mkdir -p "$CONFIG_DIR"
mkdir -p "$RUN_DIR"

cat > "$CONFIG_DIR/config.json" <<EOF
{
  "port": $PORT,
  "bind": "0.0.0.0",
  "token": "$TOKEN",
  "max_concurrent_execs": 32,
  "max_jobs": 100,
  "allowed_ips": [],
  "log_level": "info"
}
EOF
chmod 600 "$CONFIG_DIR/config.json"
echo "Config written to $CONFIG_DIR/config.json"

# 5. Install systemd service
if command -v systemctl &>/dev/null; then
    cat > /etc/systemd/system/${SERVICE_NAME}.service <<EOF
[Unit]
Description=Remote Exec Agent
After=network.target

[Service]
Type=simple
ExecStart=$BINARY_PATH --port $PORT --token $TOKEN --config $CONFIG_DIR/config.json
Restart=always
RestartSec=5
StandardOutput=journal
StandardError=journal
SyslogIdentifier=$SERVICE_NAME

[Install]
WantedBy=multi-user.target
EOF

    systemctl daemon-reload
    systemctl enable "$SERVICE_NAME"
    systemctl start "$SERVICE_NAME"
    echo "systemd service installed and started"

    # 6. Wait 2s and verify health
    sleep 2
    if curl -sf "http://localhost:$PORT/health" >/dev/null 2>&1; then
        echo "Health check passed"
    else
        echo "Warning: health check failed. Check: journalctl -u $SERVICE_NAME -n 20"
    fi
else
    echo "systemd not found. Starting agent in background..."
    nohup "$BINARY_PATH" --port "$PORT" --token "$TOKEN" --config "$CONFIG_DIR/config.json" \
        > /var/log/remote-exec-agent.log 2>&1 &
    sleep 2
    if curl -sf "http://localhost:$PORT/health" >/dev/null 2>&1; then
        echo "Health check passed (PID: $!)"
    else
        echo "Warning: health check failed. Check /var/log/remote-exec-agent.log"
    fi
fi

# 7. Detect outbound IP
OUTBOUND_IP=""
if command -v curl &>/dev/null; then
    OUTBOUND_IP=$(curl -sf --max-time 5 https://api.ipify.org 2>/dev/null || true)
fi
if [[ -z "$OUTBOUND_IP" ]]; then
    OUTBOUND_IP=$(hostname -I | awk '{print $1}' 2>/dev/null || echo "<your-server-ip>")
fi

# 8. Print connection string
echo ""
echo "============================================"
echo "Agent running on :$PORT"
echo ""
echo "Register this machine in Claude:"
echo "  machine_connect remcp://root@${OUTBOUND_IP}:${PORT}?token=${TOKEN}&via=agent+ssh"
echo "============================================"
