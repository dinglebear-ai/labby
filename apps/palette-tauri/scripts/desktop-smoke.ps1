param(
  [string]$EnvFile = $env:LABBY_PALETTE_ENV_FILE,
  [switch]$KeepApp
)

$ErrorActionPreference = 'Stop'

function Import-DotEnv($Path) {
  if (-not $Path -or -not (Test-Path $Path)) { return }
  Get-Content $Path | ForEach-Object {
    if ($_ -match '^\s*([^#=]+)\s*=\s*(.*)\s*$') {
      $name = $matches[1].Trim()
      $value = $matches[2].Trim().Trim('"')
      [Environment]::SetEnvironmentVariable($name, $value, 'Process')
    }
  }
}

function First-Env([string[]]$Names) {
  foreach ($name in $Names) {
    $value = [Environment]::GetEnvironmentVariable($name, 'Process')
    if ($value -and $value.Trim().Length -gt 0) { return $value.Trim() }
  }
  return $null
}

Import-DotEnv $EnvFile

$exe = First-Env @('LABBY_PALETTE_EXE')
$apiUrl = First-Env @('LABBY_PALETTE_API_URL', 'LABBY_API_URL')
$token = First-Env @('LABBY_PALETTE_TOKEN', 'LABBY_MCP_HTTP_TOKEN', 'LAB_MCP_HTTP_TOKEN')
$query = First-Env @('LABBY_PALETTE_QUERY')
$evidenceDir = First-Env @('LABBY_PALETTE_EVIDENCE_DIR')
$settingsDir = First-Env @('LABBY_PALETTE_SETTINGS_DIR')

if (-not $exe) { throw 'LABBY_PALETTE_EXE is required' }
if (-not (Test-Path $exe)) { throw "LABBY_PALETTE_EXE does not exist: $exe" }
if (-not $apiUrl) { throw 'LABBY_PALETTE_API_URL or LABBY_API_URL is required' }
if (-not $token) { throw 'LABBY_PALETTE_TOKEN or LABBY_MCP_HTTP_TOKEN is required' }
if (-not $query) { $query = 'gateway' }
if (-not $evidenceDir) { $evidenceDir = Join-Path $PWD 'palette-smoke-evidence' }
if (-not $settingsDir) {
  $settingsDir = Join-Path $env:APPDATA 'tv.tootie.lab.palette'
}

New-Item -ItemType Directory -Force -Path $evidenceDir | Out-Null
New-Item -ItemType Directory -Force -Path $settingsDir | Out-Null

$apiUrl = $apiUrl.TrimEnd('/')
$catalog = Invoke-RestMethod `
  -Uri ($apiUrl + '/v1/palette/catalog') `
  -Headers @{ Authorization = ('Bearer ' + $token); Accept = 'application/json' } `
  -TimeoutSec 30

$entries = @($catalog.entries)
$matches = @($entries | Where-Object {
  ($_.id -like "*$query*") -or
  ($_.label -like "*$query*") -or
  ($_.source -like "*$query*") -or
  ($_.description -like "*$query*")
})
if ($matches.Count -lt 1) {
  throw "catalog has $($entries.Count) entries, but query '$query' matched no launcher rows"
}

$settings = [ordered]@{
  serverUrl = $apiUrl
  staticToken = $token
  shortcut = 'Ctrl+Shift+Space'
  theme = 'system'
  hideOnBlur = $false
  openResultsInline = $true
  showFooterHints = $false
}
$settings | ConvertTo-Json | Set-Content -Path (Join-Path $settingsDir 'settings.json') -Encoding UTF8

$process = $null
try {
  Get-Process labby-palette-tauri -ErrorAction SilentlyContinue |
    Stop-Process -Force -ErrorAction SilentlyContinue
  Start-Sleep -Milliseconds 500
  $process = Start-Process -FilePath $exe -PassThru
  Start-Sleep -Seconds 8

  Add-Type -AssemblyName System.Windows.Forms
  Add-Type -AssemblyName System.Drawing
  [System.Windows.Forms.SendKeys]::SendWait('^+ ')
  Start-Sleep -Seconds 2
  [System.Windows.Forms.SendKeys]::SendWait($query)
  Start-Sleep -Seconds 4

  $bounds = [System.Windows.Forms.SystemInformation]::VirtualScreen
  $bitmap = New-Object System.Drawing.Bitmap $bounds.Width, $bounds.Height
  $graphics = [System.Drawing.Graphics]::FromImage($bitmap)
  $graphics.CopyFromScreen($bounds.Location, [System.Drawing.Point]::Empty, $bounds.Size)
  $screenshot = Join-Path $evidenceDir 'palette-smoke.png'
  $bitmap.Save($screenshot, [System.Drawing.Imaging.ImageFormat]::Png)
  $graphics.Dispose()
  $bitmap.Dispose()

  $screenshotInfo = Get-Item $screenshot
  if ($screenshotInfo.Length -lt 1024) {
    throw "screenshot was unexpectedly small: $($screenshotInfo.Length) bytes"
  }
  $procInfo = Get-Process -Id $process.Id -ErrorAction SilentlyContinue |
    Select-Object -First 1 Id, MainWindowTitle, Path, Responding
  if (-not $procInfo) { throw 'palette process exited before smoke assertions completed' }
  if ($procInfo.Responding -eq $false) { throw 'palette process is not responding' }

  $result = [ordered]@{
    ok = $true
    apiUrl = $apiUrl
    query = $query
    catalogEntries = $entries.Count
    matchedEntries = $matches.Count
    matchedIds = @($matches | Select-Object -First 10 -ExpandProperty id)
    screenshot = $screenshot
    screenshotBytes = $screenshotInfo.Length
    process = $procInfo
  }
  $result | ConvertTo-Json -Depth 5 |
    Set-Content -Path (Join-Path $evidenceDir 'result.json') -Encoding UTF8
  $result | ConvertTo-Json -Depth 5
}
finally {
  if (-not $KeepApp -and $process) {
    Stop-Process -Id $process.Id -Force -ErrorAction SilentlyContinue
  }
}
