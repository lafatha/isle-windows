param(
    [switch]$Foreground,
    [switch]$Kill,
    [switch]$Build
)

$scriptDir = Split-Path -Parent $MyInvocation.MyCommand.Definition
if (-not $scriptDir) { $scriptDir = (Get-Location).Path }
$exe = Join-Path $scriptDir "isle-native\target\release\isle.exe"

# Only one island at a time.
Get-Process isle -ErrorAction SilentlyContinue | Stop-Process -Force -ErrorAction SilentlyContinue

if ($Kill) {
    Write-Host "[Isle] Isle is not running." -ForegroundColor Yellow
    exit 0
}

# Build on first run (or with -Build), otherwise launch the existing binary.
if ($Build -or -not (Test-Path $exe)) {
    Write-Host "[Isle] Building the native island (cargo build --release)..." -ForegroundColor Green
    Push-Location (Join-Path $scriptDir "isle-native")
    cargo build --release
    $code = $LASTEXITCODE
    Pop-Location
    if ($code -ne 0 -or -not (Test-Path $exe)) {
        Write-Host "[Isle] Build failed - is Rust (cargo) installed and on PATH?" -ForegroundColor Red
        exit 1
    }
}

if ($Foreground) {
    Write-Host "[Isle] Starting the island in this window (Ctrl+C to stop)..." -ForegroundColor Green
    & $exe
    exit $LASTEXITCODE
}

Write-Host "[Isle] Starting the native island above the screen..." -ForegroundColor Green
$p = Start-Process -FilePath $exe -PassThru
Write-Host "==============================================================" -ForegroundColor Magenta
Write-Host "[Isle] Running (PID: $($p.Id))" -ForegroundColor Green
Write-Host "Music, Focus, Reminders, Shelf, Calendar, Notifications and" -ForegroundColor White
Write-Host "Usage are live on the notch at the top of the screen." -ForegroundColor White
Write-Host "Point at the black notch to open it." -ForegroundColor Gray
Write-Host "==============================================================" -ForegroundColor Magenta
