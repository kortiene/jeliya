<#
  Jeliya installer for Windows PowerShell (PowerShell 5.1+ / pwsh).
  Detects arch, downloads the Windows release zip, expands it to a
  user-writable dir, and puts `jeliyad.exe` on your PATH. Does NOT run it.

  Requires a published GitHub Release with jeliyad assets attached.

  Usage (from an elevated-or-normal PowerShell):
    irm https://raw.githubusercontent.com/kortiene/jeliya/main/packaging/install.ps1 | iex

  Env overrides:
    $env:JELIYA_VERSION = 'v0.1.0'   # pin a release tag (default: latest)
#>

#Requires -Version 5.1
$ErrorActionPreference = 'Stop'

$Repo = 'kortiene/jeliya'
$Bin  = 'jeliyad'

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
# $env:JELIYA_VERSION pins one.
$version = $env:JELIYA_VERSION
if (-not $version) {
  Write-Host "resolving latest release of $Repo ..."
  $rel = Invoke-RestMethod -Uri "https://api.github.com/repos/$Repo/releases/latest" `
    -Headers @{ 'User-Agent' = 'jeliya-installer' }
  $version = $rel.tag_name
  if (-not $version) { throw 'could not resolve latest version; set $env:JELIYA_VERSION to pin one.' }
}

$asset = "$Bin-$version-$target.zip"
$url   = "https://github.com/$Repo/releases/download/$version/$asset"

# --- download + verify + expand ---------------------------------------------
$installDir = Join-Path $env:LOCALAPPDATA 'Programs\Jeliya'
$tmpDir = Join-Path ([System.IO.Path]::GetTempPath()) "jeliya-install-$PID-$([guid]::NewGuid().ToString('N'))"
$tmpArchive = Join-Path $tmpDir $asset
$tmpChecksum = "$tmpArchive.sha256"
$tmpExtract = Join-Path $tmpDir 'extract'

New-Item -ItemType Directory -Force -Path $tmpDir | Out-Null
try {
  Write-Host "downloading $asset ..."
  Invoke-WebRequest -Uri $url -OutFile $tmpArchive -UseBasicParsing
  if (-not (Test-Path $tmpArchive) -PathType Leaf) { throw "downloaded archive is missing: $url" }

  Write-Host "downloading and verifying $asset.sha256 ..."
  Invoke-WebRequest -Uri "$url.sha256" -OutFile $tmpChecksum -UseBasicParsing
  $checksumLines = @(Get-Content -LiteralPath $tmpChecksum | Where-Object { $_.Trim().Length -gt 0 })
  if ($checksumLines.Count -ne 1) { throw 'checksum sidecar must contain exactly one non-empty line' }
  if ($checksumLines[0] -notmatch '^([0-9a-fA-F]{64})  ([^\s]+)$') {
    throw 'checksum sidecar must contain a 64-hex digest, two spaces, and one filename'
  }
  $expectedHash = $Matches[1].ToLowerInvariant()
  $listedAsset = $Matches[2]
  if ($listedAsset -cne $asset) {
    throw "checksum sidecar names '$listedAsset', expected '$asset'"
  }
  $actualHash = (Get-FileHash -LiteralPath $tmpArchive -Algorithm SHA256).Hash.ToLowerInvariant()
  if ($actualHash -ne $expectedHash) { throw "checksum mismatch for $asset" }
  Write-Host 'checksum verified'

  New-Item -ItemType Directory -Force -Path $tmpExtract | Out-Null
  Write-Host "extracting verified archive ..."
  Expand-Archive -LiteralPath $tmpArchive -DestinationPath $tmpExtract -Force
  $stagedExe = Join-Path $tmpExtract "$Bin.exe"
  $members = @(Get-ChildItem -LiteralPath $tmpExtract -Recurse -File)
  if ($members.Count -ne 1 -or -not (Test-Path $stagedExe -PathType Leaf)) {
    throw "archive must contain exactly $Bin.exe"
  }
  if (($members[0].Attributes -band [System.IO.FileAttributes]::ReparsePoint) -ne 0) {
    throw "$Bin.exe must be a regular file, not a reparse point"
  }

  New-Item -ItemType Directory -Force -Path $installDir | Out-Null
  Copy-Item -LiteralPath $stagedExe -Destination (Join-Path $installDir "$Bin.exe") -Force
} finally {
  Remove-Item -LiteralPath $tmpDir -Recurse -Force -ErrorAction SilentlyContinue
}

$exe = Join-Path $installDir "$Bin.exe"
if (-not (Test-Path $exe -PathType Leaf)) { throw "installation did not produce $exe" }

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
Write-Host "next: run '$Bin' -- it opens the Jeliya UI in your browser."
