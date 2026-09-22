# auximap Build Script (PowerShell)
param (
    [switch]$SkipTests
)

Write-Host "===================================================" -ForegroundColor Cyan
Write-Host "  auximap Build Script (PowerShell)" -ForegroundColor Cyan
Write-Host "===================================================" -ForegroundColor Cyan

# 1. Run tests
if (-not $SkipTests) {
    Write-Host "[1/3] Running tests..." -ForegroundColor Yellow
    cargo test
    if ($LASTEXITCODE -ne 0) {
        Write-Host "[ERROR] Tests failed! Aborting." -ForegroundColor Red
        exit $LASTEXITCODE
    }
}

# 2. Build release binary
Write-Host "[2/3] Building release binary (LTO + Strip)..." -ForegroundColor Yellow
cargo build --release
if ($LASTEXITCODE -ne 0) {
    Write-Host "[ERROR] Build failed!" -ForegroundColor Red
    exit $LASTEXITCODE
}

# 3. Deploy binary
Write-Host "[3/3] Deploying binary to current directory..." -ForegroundColor Yellow
$targetBin = "target\release\auximap.exe"

if (Test-Path $targetBin) {
    Copy-Item $targetBin ".\auximap.exe" -Force
    $fileInfo = Get-Item ".\auximap.exe"
    $sizeKb = [math]::Round($fileInfo.Length / 1KB, 1)
    Write-Host "===================================================" -ForegroundColor Green
    Write-Host "  BUILD SUCCESS!" -ForegroundColor Green
    Write-Host "  Binary: .\auximap.exe" -ForegroundColor Green
    Write-Host "  Size  : $($fileInfo.Length) bytes ($sizeKb KB)" -ForegroundColor Green
    Write-Host "===================================================" -ForegroundColor Green
} else {
    Write-Warning "Build finished, but could not locate release binary."
}