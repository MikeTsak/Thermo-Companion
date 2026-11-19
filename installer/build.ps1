# Builds the release exe and the Setup.exe. Output: dist\ThermoCompanion-Setup-<version>.exe
param([string]$Version = "")

$ErrorActionPreference = "Stop"
Set-Location (Split-Path $PSScriptRoot -Parent)

if (-not $Version) {
    $Version = (Select-String -Path Cargo.toml -Pattern '^version\s*=\s*"(.+)"').Matches[0].Groups[1].Value
}

# With the GNU toolchain a full MinGW (winget: BrechtSanders.WinLibs.POSIX.UCRT) must be on PATH.
if (-not (Get-Command gcc -ErrorAction SilentlyContinue)) {
    $mingw = Get-ChildItem "$env:LOCALAPPDATA\Microsoft\WinGet\Packages" -Directory -Filter "BrechtSanders.WinLibs*" -ErrorAction SilentlyContinue |
        ForEach-Object { Join-Path $_.FullName "mingw64\bin" } | Where-Object { Test-Path $_ } | Select-Object -First 1
    if ($mingw) { $env:Path = "$mingw;$env:Path" }
}

cargo build --release --locked
if ($LASTEXITCODE -ne 0) { throw "cargo build failed" }

$iscc = @(
    "$env:LOCALAPPDATA\Programs\Inno Setup 6\ISCC.exe",
    "${env:ProgramFiles(x86)}\Inno Setup 6\ISCC.exe",
    "$env:ProgramFiles\Inno Setup 6\ISCC.exe"
) | Where-Object { Test-Path $_ } | Select-Object -First 1
if (-not $iscc) { throw "Inno Setup 6 not found. Install it with: winget install JRSoftware.InnoSetup" }

& $iscc "/DAppVersion=$Version" installer\thermo-companion.iss
if ($LASTEXITCODE -ne 0) { throw "ISCC failed" }
Get-ChildItem dist\ThermoCompanion-Setup-$Version.exe
