# Install remote-exec agent on Windows as a managed service.
# Usage:
#   .\install-agent.ps1                          # defaults
#   .\install-agent.ps1 -Port 9000 -ServiceName MyAgent
#   iwr -useb https://.../install-agent.ps1 | iex
param(
    [int]$Port = 8765,
    [string]$InstallDir  = "C:\ProgramData\remote-exec-agent",
    [string]$ServiceName = "RemoteExecAgent",
    [string]$GithubRepo  = "chat812/remote-mcp"
)

$ErrorActionPreference = "Stop"
$ConfigDir = $InstallDir

# ── 0. Require Administrator ──────────────────────────────────────────────────
if (-not ([Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent()).IsInRole(
        [Security.Principal.WindowsBuiltInRole]::Administrator)) {
    Write-Error "Please run as Administrator"
    exit 1
}

# ── 1. Detect arch ───────────────────────────────────────────────────────────
$ArchSuffix = switch ($env:PROCESSOR_ARCHITECTURE) {
    "AMD64" { "x86_64" }
    "ARM64" { "aarch64" }
    default { $env:PROCESSOR_ARCHITECTURE.ToLower() }
}
Write-Host "Detected: windows / $ArchSuffix"

# ── 2. Install binary ────────────────────────────────────────────────────────
New-Item -ItemType Directory -Force -Path $InstallDir | Out-Null
$BinaryPath  = Join-Path $InstallDir "remote-exec-agent.exe"
$LocalBinary = Join-Path $PSScriptRoot "agent-windows-$ArchSuffix.exe"
$RepoRoot    = if ($PSScriptRoot) { Split-Path $PSScriptRoot -Parent } else { "." }

$Installed = $false

if (Test-Path $LocalBinary) {
    Write-Host "Using local binary: $LocalBinary"
    Copy-Item $LocalBinary $BinaryPath -Force
    $Installed = $true
}

if (-not $Installed) {
    $ReleaseUrl = "https://github.com/$GithubRepo/releases/latest/download/agent-windows-$ArchSuffix.exe"
    Write-Host "Downloading from: $ReleaseUrl"
    try {
        Invoke-WebRequest -Uri $ReleaseUrl -OutFile $BinaryPath -UseBasicParsing -TimeoutSec 60
        $Installed = $true
    } catch {
        Write-Warning "Download failed: $_"
    }
}

if (-not $Installed) {
    $cargo = Get-Command cargo -ErrorAction SilentlyContinue
    if ($cargo -and (Test-Path (Join-Path $RepoRoot "Cargo.toml"))) {
        Write-Host "Building from source..."
        cargo build --release --manifest-path (Join-Path $RepoRoot "Cargo.toml") -p agent
        Copy-Item (Join-Path $RepoRoot "target\release\agent.exe") $BinaryPath -Force
        $Installed = $true
    }
}

if (-not $Installed) {
    Write-Error "Could not obtain binary. Aborting."
    exit 1
}

# Remove MOTW / zone flag that can block service execution
Unblock-File -Path $BinaryPath -ErrorAction SilentlyContinue
Write-Host "Binary: $BinaryPath"

# ── 3. Generate token ────────────────────────────────────────────────────────
$rng   = [System.Security.Cryptography.RandomNumberGenerator]::Create()
$bytes = New-Object byte[] 32
$rng.GetBytes($bytes)
$Token = ([System.BitConverter]::ToString($bytes) -replace '-').ToLower()

# ── 4. Write config ──────────────────────────────────────────────────────────
$ConfigPath = Join-Path $ConfigDir "config.json"
$LogFile    = Join-Path $ConfigDir "agent.log"

$Config = [ordered]@{
    port                 = $Port
    bind                 = "0.0.0.0"
    token                = $Token
    max_concurrent_execs = 32
    max_jobs             = 100
    allowed_ips          = @()
    log_level            = "info"
} | ConvertTo-Json -Depth 5

Set-Content -Path $ConfigPath -Value $Config -Encoding UTF8
icacls $ConfigPath /inheritance:r /grant:r "SYSTEM:F" "Administrators:F" | Out-Null
Write-Host "Config: $ConfigPath"

# ── 5. Install service ───────────────────────────────────────────────────────
$existing = Get-Service -Name $ServiceName -ErrorAction SilentlyContinue
if ($existing) {
    Write-Host "Removing existing service '$ServiceName'..."
    Stop-Service -Name $ServiceName -Force -ErrorAction SilentlyContinue
    sc.exe delete $ServiceName | Out-Null
    Start-Sleep -Seconds 2
}

# Create service entry (binary path only — args set via registry below)
sc.exe create $ServiceName binPath= $BinaryPath start= auto obj= LocalSystem `
    DisplayName= "Remote Exec Agent" | Out-Null
sc.exe description $ServiceName "Remote execution agent for Claude Code MCP" | Out-Null

# Write the full command line directly into ImagePath — the only reliable way
# to pass arguments to a Windows service.
$FullCmd = "`"$BinaryPath`" --port $Port --token $Token --config `"$ConfigPath`" " +
           "--log-file `"$LogFile`" --service --service-name $ServiceName"
Set-ItemProperty `
    -Path "HKLM:\SYSTEM\CurrentControlSet\Services\$ServiceName" `
    -Name "ImagePath" `
    -Value $FullCmd

Write-Host "Service '$ServiceName' registered"

# ── 6. Start and verify ──────────────────────────────────────────────────────
sc.exe start $ServiceName | Out-Null
Write-Host "Starting service..."
Start-Sleep -Seconds 3

$svc = Get-Service -Name $ServiceName -ErrorAction SilentlyContinue
if ($svc -and $svc.Status -eq "Running") {
    Write-Host "Service status: Running"
} else {
    Write-Warning "Service did not reach Running state. Check the log:"
    Write-Warning "  Get-Content '$LogFile' -Tail 50"
    exit 1
}

try {
    $h = Invoke-WebRequest -Uri "http://localhost:$Port/health" -UseBasicParsing -TimeoutSec 5
    if ($h.StatusCode -eq 200) { Write-Host "Health check: OK" }
} catch {
    Write-Warning "Health check failed. Log: Get-Content '$LogFile' -Tail 50"
}

# ── 7. Print connection string ───────────────────────────────────────────────
$OutboundIp = try {
    (Invoke-WebRequest -Uri "https://api.ipify.org" -UseBasicParsing -TimeoutSec 5).Content.Trim()
} catch {
    (Get-NetIPAddress -AddressFamily IPv4 |
        Where-Object { $_.IPAddress -notmatch '^127\.' } |
        Select-Object -First 1).IPAddress
}
if (-not $OutboundIp) { $OutboundIp = "<your-server-ip>" }

Write-Host ""
Write-Host "============================================"
Write-Host "Agent running on :$Port"
Write-Host ""
Write-Host "Register in Claude Code:"
Write-Host "  machine_connect remcp://Administrator@${OutboundIp}:${Port}?token=${Token}&via=agent"
Write-Host "============================================"
