<#
  Bantaba installer for Windows PowerShell (PowerShell 5.1+ / pwsh).
  Detects arch, downloads the Windows release zip, expands it to a
  user-writable dir, and puts `bantabad.exe` on your PATH. Does NOT run it.

  PHASE 0: `OWNER/REPO` below is a placeholder. This script is inert until a
  GitHub remote + a first `v*` release exist. See packaging/README.md.

  Usage (from an elevated-or-normal PowerShell):
    irm https://raw.githubusercontent.com/OWNER/REPO/main/packaging/install.ps1 | iex

  Env overrides:
    $env:BANTABA_VERSION = 'v0.1.0'   # pin a release tag (default: latest)
#>

#Requires -Version 5.1
$ErrorActionPreference = 'Stop'

$Repo = 'OWNER/REPO'  # <-- FILL IN once a remote exists (e.g. your-org/bantaba)
$Bin  = 'bantabad'

# --- detect arch ------------------------------------------------------------
# Only x86_64-pc-windows-msvc is built today; arm64 Windows runs it under
# emulation. Map both to the x86_64 target.
$archRaw = $env:PROCESSOR_ARCHITECTURE
switch ($archRaw) {
  'AMD64' { $target = 'x86_64-pc-windows-msvc' }
  'ARM64' {
    $target = 'x86_64-pc-windows-msvc'
    Write-Host 'note: no native arm64 build yet -- installing the x86_64 build (runs under emulation).'
  }
  default { throw "unsupported architecture: $archRaw" }
}

# --- resolve version --------------------------------------------------------
# Assets are versioned, so resolve the latest tag via the GitHub API unless
# $env:BANTABA_VERSION pins one.
$version = $env:BANTABA_VERSION
if (-not $version) {
  Write-Host "resolving latest release of $Repo ..."
  $rel = Invoke-RestMethod -Uri "https://api.github.com/repos/$Repo/releases/latest" `
    -Headers @{ 'User-Agent' = 'bantaba-installer' }
  $version = $rel.tag_name
  if (-not $version) { throw 'could not resolve latest version; set $env:BANTABA_VERSION to pin one.' }
}

$asset = "$Bin-$version-$target.zip"
$url   = "https://github.com/$Repo/releases/download/$version/$asset"

# --- download + expand ------------------------------------------------------
$installDir = Join-Path $env:LOCALAPPDATA 'Programs\Bantaba'
New-Item -ItemType Directory -Force -Path $installDir | Out-Null

$tmp = Join-Path ([System.IO.Path]::GetTempPath()) $asset
Write-Host "downloading $asset ..."
Invoke-WebRequest -Uri $url -OutFile $tmp -UseBasicParsing

Write-Host "extracting to $installDir ..."
Expand-Archive -Path $tmp -DestinationPath $installDir -Force
Remove-Item $tmp -Force

$exe = Join-Path $installDir "$Bin.exe"
if (-not (Test-Path $exe)) { throw "archive did not contain $Bin.exe" }

# --- add to user PATH -------------------------------------------------------
$userPath = [Environment]::GetEnvironmentVariable('Path', 'User')
if (-not $userPath) { $userPath = '' }
if (($userPath -split ';') -notcontains $installDir) {
  $newPath = if ($userPath) { "$userPath;$installDir" } else { $installDir }
  [Environment]::SetEnvironmentVariable('Path', $newPath, 'User')
  Write-Host "added $installDir to your user PATH -- restart your terminal to pick it up."
} else {
  Write-Host "$installDir is already on your user PATH."
}

Write-Host ''
Write-Host "installed $Bin $version -> $exe"
Write-Host "next: run '$Bin' -- it opens the Bantaba UI in your browser."
