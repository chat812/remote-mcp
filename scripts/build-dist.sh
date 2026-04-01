#!/usr/bin/env bash
# Build static agent binaries for all supported platforms.
# Requires: Rust, cargo-zigbuild, zig (via `pip install ziglang`)
#
# Usage: bash scripts/build-dist.sh
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
DIST="$REPO_ROOT/dist"

echo "=== remote-exec-mcp dist build ==="

# Ensure zig is on PATH (installed via pip install ziglang)
ZIG_WIN=$(python -c "import ziglang, os; print(os.path.dirname(ziglang.__file__))" 2>/dev/null || true)
# Convert Windows path to Unix path if cygpath is available (Git Bash / MSYS2)
if [[ -n "$ZIG_WIN" ]] && command -v cygpath &>/dev/null; then
    ZIG_DIR=$(cygpath "$ZIG_WIN")
else
    ZIG_DIR="$ZIG_WIN"
fi
if [[ -n "$ZIG_DIR" && ( -f "$ZIG_DIR/zig.exe" || -f "$ZIG_DIR/zig" ) ]]; then
    export PATH="$ZIG_DIR:$PATH"
    echo "Using zig from $ZIG_DIR"
elif ! command -v zig &>/dev/null; then
    echo "Error: zig not found. Install with: pip install ziglang" >&2
    exit 1
fi

# Ensure cargo is on PATH (needed in Git Bash on Windows)
if ! command -v cargo &>/dev/null; then
    export PATH="$HOME/.cargo/bin:$PATH"
fi

# Ensure cargo-zigbuild is installed
if ! command -v cargo-zigbuild &>/dev/null; then
    echo "Installing cargo-zigbuild..."
    cargo install cargo-zigbuild
fi

mkdir -p "$DIST"

build_target() {
    local TARGET=$1
    local OUT_NAME=$2
    local BINARY_NAME=${3:-agent}

    echo ""
    echo "--- Building $TARGET ---"
    cargo zigbuild --release -p agent \
        --target "$TARGET" \
        --manifest-path "$REPO_ROOT/Cargo.toml"

    local SRC="$REPO_ROOT/target/${TARGET}/release/${BINARY_NAME}"
    local DST="$DIST/${OUT_NAME}"
    cp "$SRC" "$DST"
    echo "  => $DST ($(du -sh "$DST" | cut -f1))"
}

build_native() {
    local TARGET=$1
    local OUT_NAME=$2
    local BINARY_NAME=${3:-agent}

    echo ""
    echo "--- Building $TARGET (native) ---"
    cargo build --release -p agent \
        --target "$TARGET" \
        --manifest-path "$REPO_ROOT/Cargo.toml"

    local SRC="$REPO_ROOT/target/${TARGET}/release/${BINARY_NAME}"
    local DST="$DIST/${OUT_NAME}"
    cp "$SRC" "$DST"
    echo "  => $DST ($(du -sh "$DST" | cut -f1))"
}

# Add musl targets if not already present
rustup target add x86_64-unknown-linux-musl aarch64-unknown-linux-musl 2>/dev/null || true

# Linux static binaries (musl = fully static, no glibc dependency)
build_target "x86_64-unknown-linux-musl"  "agent-linux-x86_64"
build_target "aarch64-unknown-linux-musl" "agent-linux-aarch64"

# MCP server Linux static binaries
build_target "x86_64-unknown-linux-musl"  "mcp-server-linux-x86_64"  "mcp-server"
build_target "aarch64-unknown-linux-musl" "mcp-server-linux-aarch64" "mcp-server"

# Windows (if building on Windows or with mingw cross-compiler)
if [[ "$(uname -s)" == *"MINGW"* || "$(uname -s)" == *"MSYS"* || "$(uname -s)" == *"CYGWIN"* ]]; then
    build_native "x86_64-pc-windows-msvc" "agent-windows-x86_64.exe" "agent.exe"
    build_native "x86_64-pc-windows-msvc" "mcp-server-windows-x86_64.exe" "mcp-server.exe"
fi

echo ""
echo "=== Build complete ==="
ls -lh "$DIST/"
echo ""
echo "Binaries are statically linked and ready to deploy."
echo "Deploy to a remote machine:"
echo "  scp dist/agent-linux-x86_64 user@host:/usr/local/bin/remote-exec-agent"
echo "  ssh user@host 'chmod +x /usr/local/bin/remote-exec-agent'"
