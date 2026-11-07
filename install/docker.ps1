$ErrorActionPreference = 'Stop'
$docker = Get-Command docker -ErrorAction SilentlyContinue
if ($docker) {
  & $docker.Source info *> $null
  if ($LASTEXITCODE -eq 0) { exit 0 }
}
$desktop = Join-Path $env:LOCALAPPDATA 'Programs\DockerDesktop\Docker Desktop.exe'
$systemDesktop = Join-Path $env:ProgramFiles 'Docker\Docker\Docker Desktop.exe'
if (!(Test-Path $desktop) -and !(Test-Path $systemDesktop)) {
  $arch = if ($env:PROCESSOR_ARCHITECTURE -eq 'ARM64') { 'arm64' } else { 'amd64' }
  $temporary = Join-Path ([System.IO.Path]::GetTempPath()) ([System.Guid]::NewGuid().ToString())
  New-Item -ItemType Directory -Path $temporary | Out-Null
  try {
    $installer = Join-Path $temporary 'Docker Desktop Installer.exe'
    Invoke-WebRequest -Uri "https://desktop.docker.com/win/main/$arch/Docker%20Desktop%20Installer.exe" -OutFile $installer
    $signature = Get-AuthenticodeSignature $installer
    if ($signature.Status -ne 'Valid' -or $signature.SignerCertificate.Subject -notmatch 'Docker') { throw 'Docker installer signature validation failed.' }
    $process = Start-Process -FilePath $installer -ArgumentList @('install','--user') -Wait -PassThru
    if ($process.ExitCode -ne 0) { throw 'Complete Docker installation and any requested restart, then retry.' }
  } finally { Remove-Item -LiteralPath $temporary -Recurse -Force }
}
if (Test-Path $desktop) { Start-Process -FilePath $desktop } else { Start-Process -FilePath $systemDesktop }
Write-Output 'Complete Docker Desktop first-run setup if prompted, then retry database setup.'
