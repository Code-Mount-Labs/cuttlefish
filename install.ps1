$ErrorActionPreference = 'Stop'
$version = if ($env:CUTTLEFISH_VERSION) { $env:CUTTLEFISH_VERSION } else { 'v0.2.0-alpha.4' }
if ($version -notmatch '^[a-zA-Z0-9._-]+$') { throw 'Invalid release version.' }
$root = if ($env:CUTTLEFISH_INSTALL_DIR) { $env:CUTTLEFISH_INSTALL_DIR } else { Join-Path $env:LOCALAPPDATA 'Cuttlefish' }
$asset = "cuttlefish-$version-x86_64-pc-windows-msvc.zip"
$base = "https://github.com/Code-Mount-Labs/cuttlefish/releases/download/$version"
$temporary = Join-Path ([System.IO.Path]::GetTempPath()) ([System.Guid]::NewGuid().ToString())
New-Item -ItemType Directory -Path $temporary | Out-Null
try {
  $archive = Join-Path $temporary $asset
  Invoke-WebRequest -Uri "$base/$asset" -OutFile $archive
  Invoke-WebRequest -Uri "$base/SHA256SUMS" -OutFile (Join-Path $temporary 'SHA256SUMS')
  $lines = @(Get-Content (Join-Path $temporary 'SHA256SUMS') | Where-Object { $_ -match ('^[a-f0-9]{64}\s+' + [regex]::Escape($asset) + '$') })
  if ($lines.Count -ne 1) { throw 'Missing release checksum.' }
  $expected = ($lines[0] -split '\s+')[0]
  if ((Get-FileHash $archive -Algorithm SHA256).Hash.ToLowerInvariant() -ne $expected) { throw 'Release checksum verification failed.' }
  $unpack = Join-Path $temporary 'unpack'
  Expand-Archive -LiteralPath $archive -DestinationPath $unpack
  $bin = Join-Path $root 'bin'
  New-Item -ItemType Directory -Force -Path $bin | Out-Null
  foreach ($name in @('cuttlefish.exe','orchestrator-node.exe','orchestrator-coordinator.exe')) {
    Copy-Item -LiteralPath (Join-Path $unpack $name) -Destination (Join-Path $bin $name) -Force
  }
  Copy-Item -LiteralPath (Join-Path $unpack 'prompts') -Destination $root -Recurse -Force
  Write-Output "Installed Cuttlefish $version in $bin"
  if ($env:CUTTLEFISH_NO_SETUP -ne '1') { & (Join-Path $bin 'cuttlefish.exe') setup @args }
} finally { Remove-Item -LiteralPath $temporary -Recurse -Force }
