param(
    [switch]$DebugBuild,
    [switch]$SkipFrontend,
    [switch]$RunTests
)

$ErrorActionPreference = "Stop"
$root = Split-Path -Parent $PSScriptRoot
Set-Location $root

Write-Host "== MitmRs build ==" -ForegroundColor Cyan
Write-Host "Root: $root"

if (-not $SkipFrontend) {
    Write-Host "[1/4] Installing frontend dependencies..." -ForegroundColor Yellow
    if (Test-Path "web\pnpm-lock.yaml") {
        pnpm --dir web install --frozen-lockfile
    } else {
        pnpm --dir web install
    }
    if ($LASTEXITCODE -ne 0) { throw "pnpm install failed" }

    Write-Host "[2/4] Building embedded frontend..." -ForegroundColor Yellow
    pnpm --dir web build
    if ($LASTEXITCODE -ne 0) { throw "frontend build failed" }
} else {
    Write-Host "[1-2/4] Skipping frontend build" -ForegroundColor DarkGray
}

if (-not (Test-Path "web\dist\index.html")) {
    throw "web/dist/index.html is missing. Build the frontend first."
}

if ($RunTests) {
    Write-Host "[3/4] Running tests..." -ForegroundColor Yellow
    cargo test
    if ($LASTEXITCODE -ne 0) { throw "tests failed" }
} else {
    Write-Host "[3/4] Skipping tests (-RunTests to enable)" -ForegroundColor DarkGray
}

Write-Host "[4/4] Building Rust executable..." -ForegroundColor Yellow
if ($DebugBuild) {
    cargo build --bin mitmrs
    $profile = "debug"
} else {
    cargo build --release --bin mitmrs
    $profile = "release"
}
if ($LASTEXITCODE -ne 0) { throw "cargo build failed" }

$target = Join-Path $root "target\$profile"
$exe = Join-Path $target "mitmrs.exe"
if (-not (Test-Path $exe)) { throw "mitmrs.exe not found at $exe" }

$dist = Join-Path $root "dist"
New-Item -ItemType Directory -Force -Path $dist | Out-Null
Copy-Item -Force $exe (Join-Path $dist "mitmrs.exe")

$windivert = Join-Path $root "third_part\WinDivert-2.2.2-A\x64"
foreach ($name in @("WinDivert.dll", "WinDivert64.sys")) {
    $src = Join-Path $windivert $name
    if (-not (Test-Path $src)) { throw "WinDivert runtime file missing: $src" }
    Copy-Item -Force $src (Join-Path $dist $name)
}

foreach ($name in @("README.md", "LICENSE", "NOTICE.md")) {
    $src = Join-Path $root $name
    if (Test-Path $src) { Copy-Item -Force $src (Join-Path $dist $name) }
}
$windivertLicense = Join-Path $windivert "..\LICENSE"
if (Test-Path $windivertLicense) {
    Copy-Item -Force $windivertLicense (Join-Path $dist "WinDivert-LICENSE.txt")
}

Write-Host ""
Write-Host "Build complete: $dist" -ForegroundColor Green
Get-ChildItem $dist | Select-Object Name, Length | Format-Table -AutoSize
Write-Host "Run:" -ForegroundColor Cyan
Write-Host "  $dist\mitmrs.exe"
