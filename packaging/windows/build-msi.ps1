# Build a .msi installer for PhazeAI IDE (Windows)
# Requirements:
#   winget install WixToolset.WixToolset  (or scoop install wixtoolset)
#   cargo install cargo-wix
param(
    [string]$Version = "0.1.0",
    [switch]$NoBuild
)

$ErrorActionPreference = "Stop"
$ProjectRoot = Split-Path -Parent (Split-Path -Parent $PSScriptRoot)

Write-Host "==> Building PhazeAI IDE v$Version MSI installer" -ForegroundColor Cyan

# 1. Compile the binary
if (-not $NoBuild) {
    Write-Host "==> Compiling release binary..."
    Push-Location $ProjectRoot
    cargo build --release -p phazeai-ide
    if ($LASTEXITCODE -ne 0) { Write-Error "cargo build failed"; exit 1 }
    Pop-Location
}

$Binary = Join-Path $ProjectRoot "target\release\phazeai-ide.exe"
if (-not (Test-Path $Binary)) {
    Write-Error "Binary not found at: $Binary"
    exit 1
}

# 2. Build MSI with cargo-wix (simplest approach)
Write-Host "==> Building MSI with cargo-wix..."
New-Item -ItemType Directory -Force -Path "$ProjectRoot\dist" | Out-Null

Push-Location $ProjectRoot
try {
    # cargo-wix will use the .wxs file in the packaging/windows directory
    cargo wix --no-build --nocapture `
        --package phazeai-ide `
        --output "dist\PhazeAI-IDE-$Version-x64.msi"
    if ($LASTEXITCODE -ne 0) {
        Write-Warning "cargo-wix failed. Trying wix directly..."

        # Fallback: direct wix build
        $WxsFile = Join-Path $PSScriptRoot "phazeai-ide.wxs"
        $OutputMsi = Join-Path $ProjectRoot "dist\PhazeAI-IDE-$Version-x64.msi"
        wix build $WxsFile -o $OutputMsi -d "Version=$Version"
    }
}
finally {
    Pop-Location
}

$Output = Join-Path $ProjectRoot "dist\PhazeAI-IDE-$Version-x64.msi"
if (Test-Path $Output) {
    $Size = (Get-Item $Output).Length / 1MB
    Write-Host ""
    Write-Host "==> MSI built: $Output" -ForegroundColor Green
    Write-Host "    Size: $([math]::Round($Size, 1)) MB"
    Write-Host ""
    Write-Host "Install with:"
    Write-Host "  msiexec /i `"$Output`""
} else {
    Write-Error "MSI build failed - output not found"
}
