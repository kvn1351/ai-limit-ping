# ai-limit-ping (alp) 1-line installer for Windows
# Repository: https://github.com/kvn1351/ai-limit-ping

$ErrorActionPreference = "Stop"

$Repo = "kvn1351/ai-limit-ping"
$Target = "x86_64-pc-windows-msvc"
$InstallDir = if ($env:ALP_INSTALL_DIR) { $env:ALP_INSTALL_DIR } else { "$env:LOCALAPPDATA\Programs\alp" }
$DownloadUrl = "https://github.com/$Repo/releases/latest/download/alp-$Target.zip"

Write-Host "→ Installing ai-limit-ping (alp)..." -ForegroundColor Cyan

if (-not (Test-Path $InstallDir)) {
    New-Item -ItemType Directory -Path $InstallDir -Force | Out-Null
}

$TempZip = [System.IO.Path]::GetTempFileName() + ".zip"
$TempExtract = Join-Path ([System.IO.Path]::GetTempPath()) ([System.Guid]::NewGuid().ToString())

try {
    Write-Host "  Downloading latest release from GitHub..."
    Invoke-WebRequest -Uri $DownloadUrl -OutFile $TempZip -UseBasicParsing

    Expand-Archive -Path $TempZip -DestinationPath $TempExtract -Force
    Copy-Item -Path (Join-Path $TempExtract "alp.exe") -Destination (Join-Path $InstallDir "alp.exe") -Force

    Write-Host "`n✓ alp successfully installed to $InstallDir\alp.exe`n" -ForegroundColor Green

    # Check PATH
    $UserPath = [Environment]::GetEnvironmentVariable("Path", "User")
    if ($UserPath -notlike "*$InstallDir*") {
        Write-Host "Adding $InstallDir to user PATH..." -ForegroundColor Yellow
        [Environment]::SetEnvironmentVariable("Path", "$UserPath;$InstallDir", "User")
        $env:Path = "$env:Path;$InstallDir"
        Write-Host "PATH updated successfully.`n" -ForegroundColor Green
    }

    Write-Host "Get started:"
    Write-Host "  alp status          # View 5-hour limit windows"
    Write-Host "  alp daemon install  # Enable unattended background service"
}
finally {
    if (Test-Path $TempZip) { Remove-Item -Force $TempZip }
    if (Test-Path $TempExtract) { Remove-Item -Recurse -Force $TempExtract }
}
