$ErrorActionPreference = "Stop"

$sccache = $env:LAB_SCCACHE_EXE
if (-not $sccache) {
  $sccache = Join-Path $env:USERPROFILE ".cargo\bin\sccache.exe"
}

if (-not (Test-Path $sccache)) {
  Write-Error "sccache.exe not found at $sccache; run the Windows CI install step first"
  exit 1
}

if (-not $env:SCCACHE_DIR) {
  $env:SCCACHE_DIR = Join-Path $env:USERPROFILE "actions-runner\labby\_sccache\labby"
}

if (-not $env:SCCACHE_CACHE_SIZE) {
  $env:SCCACHE_CACHE_SIZE = "20G"
}

& $sccache @args
exit $LASTEXITCODE
