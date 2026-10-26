param(
    [switch]$Foreground,
    [switch]$Kill
)

$scriptDir = Split-Path -Parent $MyInvocation.MyCommand.Definition
if (-not $scriptDir) { $scriptDir = (Get-Location).Path }
Set-Location $scriptDir

# Any instance left over from last time is closed first, so the island opens in one place only.
Get-WmiObject Win32_Process -ErrorAction SilentlyContinue | Where-Object {
    $_.Name -like "*powershell*" -and $_.CommandLine -like "*isle-windows.ps1*"
} | ForEach-Object {
    Stop-Process -Id $_.ProcessId -Force -ErrorAction SilentlyContinue
}

if ($Kill) {
    Write-Host "[Isle] Isle is not running." -ForegroundColor Yellow
    exit 0
}

$targetScript = Join-Path $scriptDir "isle-windows.ps1"

if ($Foreground) {
    Write-Host "[Isle] Starting the island in this window (Esc or Ctrl+C to stop)..." -ForegroundColor Green
    & powershell.exe -STA -ExecutionPolicy Bypass -NoProfile -File $targetScript
    exit $LASTEXITCODE
}

Write-Host "[Isle] Starting the island above the screen..." -ForegroundColor Green
$cmd = "powershell.exe -STA -ExecutionPolicy Bypass -NoProfile -File `"$targetScript`""
$res = Invoke-CimMethod -ClassName Win32_Process -MethodName Create -Arguments @{ CommandLine = $cmd }
if ($null -ne $res -and $res.ReturnValue -eq 0) {
    Write-Host "==============================================================" -ForegroundColor Magenta
    Write-Host "[Isle] Running (PID: $($res.ProcessId))" -ForegroundColor Green
    Write-Host "Music, Focus, Reminders, Shelf, Calendar, Notifications and" -ForegroundColor White
    Write-Host "Usage are live on the notch at the top of the screen." -ForegroundColor White
    Write-Host "Point at the black notch to open it." -ForegroundColor Gray
    Write-Host "==============================================================" -ForegroundColor Magenta
} else {
    $p = Start-Process powershell.exe -ArgumentList @("-STA", "-ExecutionPolicy", "Bypass", "-NoProfile", "-File", $targetScript) -PassThru
    Write-Host "[Isle] Running via Start-Process (PID: $($p.Id))" -ForegroundColor Green
}
