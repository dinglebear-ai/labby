<#
.SYNOPSIS
    Install or roll back the Labby binary on Windows.
.DESCRIPTION
    Resolves an immutable release containing the Windows asset, requires and
    verifies its SHA-256 sidecar, retains verified artifacts and an install
    receipt, then atomically replaces labby.exe. Source fallback is opt-in.
#>
[CmdletBinding()]
param(
    [string]$InstallDir = $(if ($env:LABBY_INSTALL_DIR) { $env:LABBY_INSTALL_DIR } else { Join-Path $env:LOCALAPPDATA 'labby\bin' }),
    [string]$Version = $(if ($env:LABBY_INSTALL_VERSION) { $env:LABBY_INSTALL_VERSION } else { 'latest' }),
    [string]$Repo = $(if ($env:LABBY_INSTALL_REPO) { $env:LABBY_INSTALL_REPO } else { 'dinglebear-ai/labby' }),
    [switch]$Rollback = $($env:LABBY_INSTALL_ROLLBACK -eq '1'),
    [switch]$AllowSourceFallback = $($env:LABBY_ALLOW_SOURCE_FALLBACK -eq '1'),
    [string]$LocalBinary = $env:LABBY_INSTALL_LOCAL_BINARY,
    [string]$LocalSha256 = $env:LABBY_INSTALL_LOCAL_SHA256,
    [switch]$NoPathUpdate
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
$runningOnWindows = if (Get-Variable IsWindows -ErrorAction SilentlyContinue) {
    $IsWindows
} else {
    $env:OS -eq 'Windows_NT'
}

function Write-Info([string]$Message) { Write-Information $Message -InformationAction Continue }
function Test-LabbyRunningOnWindows { return $runningOnWindows }

function Resolve-LabbyReleaseVersion {
    param([string]$Repo, [string]$RequestedVersion, [string]$AssetName)
    if ($RequestedVersion -ne 'latest') { return $RequestedVersion }
    $releases = Invoke-RestMethod -Uri "https://api.github.com/repos/$Repo/releases?per_page=20" -TimeoutSec 300
    $release = $releases | Where-Object {
        @($_.assets | ForEach-Object { $_.name }) -contains $AssetName
    } | Select-Object -First 1
    if (-not $release -or -not $release.tag_name) {
        throw "no published release contains required asset $AssetName"
    }
    return [string]$release.tag_name
}

function Test-LabbyChecksum {
    param([string]$ArtifactPath, [string]$ChecksumPath)
    $expected = ((Get-Content $ChecksumPath -Raw) -split '\s+')[0].Trim().ToLowerInvariant()
    if ($expected -notmatch '^[0-9a-f]{64}$') {
        $trustError = [System.Security.SecurityException]::new("checksum file for $(Split-Path $ArtifactPath -Leaf) is malformed")
        $trustError.Data['LabbyTrustFailure'] = $true
        throw $trustError
    }
    $actual = (Get-FileHash $ArtifactPath -Algorithm SHA256).Hash.ToLowerInvariant()
    if ($expected -ne $actual) {
        $trustError = [System.Security.SecurityException]::new(
            "checksum verification FAILED for $(Split-Path $ArtifactPath -Leaf) (expected $expected, got $actual)"
        )
        $trustError.Data['LabbyTrustFailure'] = $true
        throw $trustError
    }
}

function Test-LabbyReleaseProvenance {
    param([string]$ArtifactPath, [string]$Repo, [string]$ResolvedVersion)
    if (-not (Get-Command gh -ErrorAction SilentlyContinue)) {
        $trustError = [System.Security.SecurityException]::new('GitHub CLI (gh) is required to verify release provenance')
        $trustError.Data['LabbyTrustFailure'] = $true
        throw $trustError
    }
    & gh attestation verify $ArtifactPath --repo $Repo `
        --signer-workflow "$Repo/.github/workflows/release.yml" `
        --source-ref "refs/tags/$ResolvedVersion" --deny-self-hosted-runners | Out-Null
    if ($LASTEXITCODE -ne 0) {
        $trustError = [System.Security.SecurityException]::new("GitHub provenance verification FAILED for $(Split-Path $ArtifactPath -Leaf)")
        $trustError.Data['LabbyTrustFailure'] = $true
        throw $trustError
    }
}

function Get-LabbyCargoInstallArgumentList {
    param([string]$Repo, [string]$RequestedVersion, [string]$Root, [string]$ResolvedRevision)
    $arguments = @('install', '--git', "https://github.com/$Repo")
    if ($RequestedVersion -ne 'latest') {
        $arguments += @('--tag', $RequestedVersion)
    } elseif ($ResolvedRevision -match '^[0-9a-f]{40}([0-9a-f]{24})?$') {
        $arguments += @('--rev', $ResolvedRevision)
    } else {
        throw 'latest source fallback requires a resolved immutable revision'
    }
    return $arguments + @('labby', '--bin', 'labby', '--all-features', '--root', $Root)
}

function Resolve-LabbySourceRevision {
    param([string]$Repo)
    $commit = Invoke-RestMethod -Uri "https://api.github.com/repos/$Repo/commits/HEAD" -TimeoutSec 300
    $revision = [string]$commit.sha
    if ($revision -notmatch '^[0-9a-f]{40}([0-9a-f]{24})?$') {
        throw 'source repository returned an invalid HEAD revision'
    }
    return $revision
}

function Protect-LabbyMetadataFile {
    param([string]$Path)
    if (Test-LabbyRunningOnWindows) {
        & icacls.exe $Path '/inheritance:r' '/grant:r' "${env:USERNAME}:(F)" | Out-Null
        if ($LASTEXITCODE -ne 0) { throw "failed to restrict installer metadata permissions: $Path" }
    } else {
        & chmod 600 $Path
    }
}

function Get-CurrentLabbyWindowsSid {
    return [Security.Principal.WindowsIdentity]::GetCurrent().User.Value
}

function Invoke-LabbyIcacl {
    param([string]$Path, [string[]]$Arguments)
    & icacls.exe $Path @Arguments | Out-Null
}

function Protect-LabbyMetadataDirectory {
    param([string]$Path)
    if (Test-LabbyRunningOnWindows) {
        $identity = Get-CurrentLabbyWindowsSid
        Invoke-LabbyIcacl -Path $Path -Arguments @('/inheritance:r', '/grant:r', "*${identity}:(OI)(CI)(F)")
        if ($LASTEXITCODE -ne 0) { throw "failed to restrict installer metadata directory permissions: $Path" }
    } else {
        & chmod 700 $Path
    }
}

function Repair-LabbyActivation {
    param([string]$InstallDir)
    $metadata = Join-Path $InstallDir '.labby-install'
    $journal = Join-Path $metadata 'activation-journal'
    if (-not (Test-Path $journal -PathType Container)) { return }
    if (-not (Test-Path (Join-Path $journal 'state') -PathType Leaf)) {
        Remove-Item $journal -Recurse -Force
        return
    }
    $failures = [Collections.Generic.List[string]]::new()
    foreach ($entry in @(
        @{ Name = 'binary'; Target = (Join-Path $InstallDir 'labby.exe'); Snapshot = 'old-binary.exe' },
        @{ Name = 'receipt'; Target = (Join-Path $metadata 'receipt.json'); Snapshot = 'old-receipt.json' },
        @{ Name = 'previous'; Target = (Join-Path $metadata 'previous-receipt.json'); Snapshot = 'old-previous.json' }
    )) {
        try {
            if (Test-Path (Join-Path $journal "old-$($entry.Name).present") -PathType Leaf) {
                $snapshot = Join-Path $journal $entry.Snapshot
                if (-not (Test-Path $snapshot -PathType Leaf)) { throw 'snapshot is missing' }
                Copy-Item $snapshot "$($entry.Target).restore" -Force
                Move-LabbyFileAtomically -Source "$($entry.Target).restore" -Destination $entry.Target
            } else {
                Remove-Item $entry.Target -Force -ErrorAction Stop
            }
        } catch {
            if (-not (Test-Path $entry.Target)) {
                # Removing an already absent target is successful recovery.
                if (-not (Test-Path (Join-Path $journal "old-$($entry.Name).present"))) { continue }
            }
            $failures.Add("$($entry.Name): $($_.Exception.Message)")
        }
    }
    if ($failures.Count -gt 0) {
        throw "activation recovery FAILED; journal retained at ${journal}: $($failures -join '; ')"
    }
    Remove-Item $journal -Recurse -Force
    Write-Info 'interrupted installation transaction restored'
}

function Write-LabbyActivationState {
    param([string]$Journal, [string]$State)
    $temporary = Join-Path $Journal '.state.tmp'
    [IO.File]::WriteAllText($temporary, "$State`n", [Text.UTF8Encoding]::new($false))
    Move-LabbyFileAtomically -Source $temporary -Destination (Join-Path $Journal 'state')
}

function Move-LabbyFileAtomically {
    param([string]$Source, [string]$Destination)
    if (Test-Path $Destination -PathType Leaf) {
        $backup = "$Destination.replace-backup-$([guid]::NewGuid().ToString('N'))"
        [System.IO.File]::Replace($Source, $Destination, $backup, $true)
        Remove-Item -Force $backup
    } else {
        [System.IO.File]::Move($Source, $Destination)
    }
}

function Write-LabbyReceipt {
    param([string]$Path, [string]$Source, [string]$RequestedVersion, [string]$ResolvedVersion, [string]$Sha256)
    $temporary = "$Path.$([guid]::NewGuid().ToString('N')).tmp"
    $json = [ordered]@{
        format = 1
        source = $Source
        requested_version = $RequestedVersion
        resolved_version = $ResolvedVersion
        sha256 = $Sha256
        installed_at = [DateTimeOffset]::UtcNow.ToString('o')
    } | ConvertTo-Json
    [IO.File]::WriteAllText($temporary, $json, [Text.UTF8Encoding]::new($false))
    Protect-LabbyMetadataFile $temporary
    Move-LabbyFileAtomically -Source $temporary -Destination $Path
}

function Install-LabbyVerifiedBinary {
    param(
        [string]$SourcePath, [string]$InstallDir,
        [ValidateSet('release', 'source', 'local')][string]$Source,
        [string]$RequestedVersion, [string]$ResolvedVersion
    )
    $metadata = Join-Path $InstallDir '.labby-install'
    $artifacts = Join-Path $metadata 'artifacts'
    $receipt = Join-Path $metadata 'receipt.json'
    $previous = Join-Path $metadata 'previous-receipt.json'
    New-Item -ItemType Directory -Force -Path $InstallDir, $artifacts | Out-Null
    Protect-LabbyMetadataDirectory $metadata
    Protect-LabbyMetadataDirectory $artifacts
    Repair-LabbyActivation -InstallDir $InstallDir

    $digest = (Get-FileHash $SourcePath -Algorithm SHA256).Hash.ToLowerInvariant()
    $artifactDir = Join-Path $artifacts $digest
    $artifact = Join-Path $artifactDir 'labby.exe'
    New-Item -ItemType Directory -Force -Path $artifactDir | Out-Null
    Protect-LabbyMetadataDirectory $artifactDir
    if (-not (Test-Path $artifact -PathType Leaf)) {
        $artifactTemp = Join-Path $artifactDir ".$([guid]::NewGuid().ToString('N')).tmp"
        Copy-Item $SourcePath $artifactTemp
        Move-LabbyFileAtomically -Source $artifactTemp -Destination $artifact
    } elseif ((Get-FileHash $artifact -Algorithm SHA256).Hash.ToLowerInvariant() -ne $digest) {
        throw "cached artifact digest does not match its content: $digest"
    }

    $target = Join-Path $InstallDir 'labby.exe'
    $transaction = Join-Path $metadata 'activation-journal'
    New-Item -ItemType Directory -Path $transaction | Out-Null
    Protect-LabbyMetadataDirectory $transaction
    $staged = Join-Path $transaction 'new-labby.exe'
    Copy-Item $artifact $staged
    Write-LabbyReceipt -Path (Join-Path $transaction 'new-receipt.json') -Source $Source `
        -RequestedVersion $RequestedVersion -ResolvedVersion $ResolvedVersion -Sha256 $digest
    if (Test-Path $target -PathType Leaf) {
        Copy-Item $target (Join-Path $transaction 'old-binary.exe')
        New-Item -ItemType File (Join-Path $transaction 'old-binary.present') | Out-Null
    }
    if (Test-Path $receipt -PathType Leaf) {
        Copy-Item $receipt (Join-Path $transaction 'old-receipt.json')
        Copy-Item $receipt (Join-Path $transaction 'new-previous.json')
        New-Item -ItemType File (Join-Path $transaction 'old-receipt.present') | Out-Null
    }
    if (Test-Path $previous -PathType Leaf) {
        Copy-Item $previous (Join-Path $transaction 'old-previous.json')
        New-Item -ItemType File (Join-Path $transaction 'old-previous.present') | Out-Null
    }
    Write-LabbyActivationState -Journal $transaction -State prepared
    try {
        Move-LabbyFileAtomically -Source $staged -Destination $target
        Write-LabbyActivationState -Journal $transaction -State binary-activated
        if (Test-Path (Join-Path $transaction 'new-previous.json')) {
            Move-LabbyFileAtomically -Source (Join-Path $transaction 'new-previous.json') -Destination $previous
        }
        Write-LabbyActivationState -Journal $transaction -State previous-receipt-activated
        Move-LabbyFileAtomically -Source (Join-Path $transaction 'new-receipt.json') -Destination $receipt
        Write-LabbyActivationState -Journal $transaction -State receipt-activated
    } catch {
        $activationError = $_
        try { Repair-LabbyActivation -InstallDir $InstallDir } catch { throw }
        throw $activationError
    } finally {
        if ((Test-Path $transaction -PathType Container) -and
            (Test-Path (Join-Path $transaction 'state') -PathType Leaf) -and
            ((Get-Content -Raw (Join-Path $transaction 'state')).Trim() -eq 'receipt-activated')) {
            Remove-Item $transaction -Recurse -Force
        }
    }
}

function Install-LabbyLocalCandidate {
    param([string]$Path, [string]$ExpectedSha256, [string]$InstallDir, [string]$Version)
    if (-not (Test-Path $Path -PathType Leaf)) { throw 'local candidate is not a regular file' }
    if ($ExpectedSha256 -notmatch '^[0-9a-f]{64}$') { throw 'local candidate SHA-256 must be lowercase hexadecimal' }
    $staging = Join-Path ([IO.Path]::GetTempPath()) "labby-local-$([guid]::NewGuid().ToString('N'))"
    New-Item -ItemType Directory -Path $staging | Out-Null
    try {
        $staged = Join-Path $staging 'labby.exe'
        Copy-Item $Path $staged
        $actual = (Get-FileHash $staged -Algorithm SHA256).Hash.ToLowerInvariant()
        if ($actual -ne $ExpectedSha256) { throw 'local candidate checksum verification FAILED' }
        Install-LabbyVerifiedBinary -SourcePath $staged -InstallDir $InstallDir -Source local `
            -RequestedVersion $Version -ResolvedVersion $Version
    } finally {
        Remove-Item $staging -Recurse -Force -ErrorAction SilentlyContinue
    }
}

function Restore-LabbyPreviousInstall {
    param([string]$InstallDir)
    $metadata = Join-Path $InstallDir '.labby-install'
    $previous = Join-Path $metadata 'previous-receipt.json'
    if (-not (Test-Path $previous -PathType Leaf)) { throw 'no previous verified installation is available for offline rollback' }
    $prior = Get-Content -Raw $previous | ConvertFrom-Json
    if ($prior.sha256 -notmatch '^[0-9a-f]{64}$') { throw 'previous install receipt has an invalid artifact digest' }
    $artifact = Join-Path $metadata "artifacts/$($prior.sha256)/labby.exe"
    if (-not (Test-Path $artifact -PathType Leaf)) { throw "previous verified artifact is unavailable: $($prior.sha256)" }
    $actual = (Get-FileHash $artifact -Algorithm SHA256).Hash.ToLowerInvariant()
    if ($actual -ne $prior.sha256) { throw 'previous artifact digest does not match its receipt' }

    Install-LabbyVerifiedBinary -SourcePath $artifact -InstallDir $InstallDir -Source $prior.source `
        -RequestedVersion $prior.requested_version -ResolvedVersion $prior.resolved_version
    Write-Info "restored verified installation $($prior.resolved_version) ($($prior.sha256)) without network access"
}

function Install-LabbyFromRelease {
    param([string]$InstallDir, [string]$Version, [string]$Repo)
    $asset = 'lab-x86_64-pc-windows-msvc.zip'
    $resolved = Resolve-LabbyReleaseVersion -Repo $Repo -RequestedVersion $Version -AssetName $asset
    Write-Info "resolved binary release to $resolved"
    $base = "https://github.com/$Repo/releases/download/$resolved"
    $temporary = Join-Path ([System.IO.Path]::GetTempPath()) "labby-install-$([guid]::NewGuid().ToString('N'))"
    New-Item -ItemType Directory -Path $temporary | Out-Null
    try {
        $zip = Join-Path $temporary $asset
        $checksum = "$zip.sha256"
        Invoke-WebRequest -Uri "$base/$asset" -OutFile $zip -UseBasicParsing -TimeoutSec 300
        Invoke-WebRequest -Uri "$base/$asset.sha256" -OutFile $checksum -UseBasicParsing -TimeoutSec 300
        Test-LabbyChecksum -ArtifactPath $zip -ChecksumPath $checksum
        Write-Info 'sha256 verified'
        Test-LabbyReleaseProvenance -ArtifactPath $zip -Repo $Repo -ResolvedVersion $resolved
        Expand-Archive -Path $zip -DestinationPath $temporary -Force
        $binary = Get-ChildItem -Path $temporary -Recurse -Filter 'labby.exe' | Select-Object -First 1
        if (-not $binary) { throw "archive $asset did not contain labby.exe" }
        Install-LabbyVerifiedBinary -SourcePath $binary.FullName -InstallDir $InstallDir -Source release `
            -RequestedVersion $Version -ResolvedVersion $resolved
    } finally {
        Remove-Item -Recurse -Force $temporary -ErrorAction SilentlyContinue
    }
}

function Install-LabbyFromSource {
    param([string]$InstallDir, [string]$Version, [string]$Repo)
    if (-not (Get-Command cargo -ErrorAction SilentlyContinue)) { throw 'cargo is unavailable' }
    $root = Join-Path ([System.IO.Path]::GetTempPath()) "labby-source-$([guid]::NewGuid().ToString('N'))"
    try {
        $revision = if ($Version -eq 'latest') { Resolve-LabbySourceRevision -Repo $Repo } else { $null }
        $resolvedVersion = if ($revision) { "rev:$revision" } else { $Version }
        $arguments = Get-LabbyCargoInstallArgumentList -Repo $Repo -RequestedVersion $Version -Root $root `
            -ResolvedRevision $revision
        & cargo @arguments
        if ($LASTEXITCODE -ne 0) { throw "cargo install failed with exit code $LASTEXITCODE" }
        Install-LabbyVerifiedBinary -SourcePath (Join-Path $root 'bin/labby.exe') -InstallDir $InstallDir `
            -Source source -RequestedVersion $Version -ResolvedVersion $resolvedVersion
    } finally {
        Remove-Item -Recurse -Force $root -ErrorAction SilentlyContinue
    }
}

function Add-LabbyToUserPath([string]$Directory) {
    $userPath = [Environment]::GetEnvironmentVariable('Path', 'User')
    if (($userPath -split ';') -notcontains $Directory) {
        [Environment]::SetEnvironmentVariable('Path', (($userPath, $Directory) -join ';').TrimStart(';'), 'User')
        Write-Info "added $Directory to your user PATH (restart the shell to pick it up)"
    }
}

function Invoke-LabbyInstall {
    param(
        [string]$InstallDir,
        [string]$Version,
        [string]$Repo,
        [switch]$Rollback,
        [switch]$AllowSourceFallback,
        [string]$LocalBinary,
        [string]$LocalSha256,
        [switch]$NoPathUpdate
    )
    Repair-LabbyActivation -InstallDir $InstallDir
    $architecture = [Runtime.InteropServices.RuntimeInformation]::OSArchitecture
    if ($architecture -ne [Runtime.InteropServices.Architecture]::X64) {
        throw "unsupported Windows architecture $architecture; supported: x86_64"
    }
    if ($Rollback) {
        Restore-LabbyPreviousInstall -InstallDir $InstallDir
    } elseif ($LocalBinary) {
        Install-LabbyLocalCandidate -Path $LocalBinary -ExpectedSha256 $LocalSha256 `
            -InstallDir $InstallDir -Version $Version
    } elseif ($LocalSha256) {
        throw 'LocalSha256 requires LocalBinary'
    } else {
        try {
            Install-LabbyFromRelease -InstallDir $InstallDir -Version $Version -Repo $Repo
        } catch {
            if ($_.Exception.Data['LabbyTrustFailure']) { throw }
            if (-not $AllowSourceFallback) {
                throw "release install failed and source fallback is disabled; re-run with -AllowSourceFallback to build the exact requested ref: $($_.Exception.Message)"
            }
            Write-Info 'release install failed; explicit source fallback is enabled'
            Install-LabbyFromSource -InstallDir $InstallDir -Version $Version -Repo $Repo
        }
    }
    if (-not $NoPathUpdate) { Add-LabbyToUserPath $InstallDir }
    $executable = Join-Path $InstallDir 'labby.exe'
    $installed = if (Test-Path $executable) { & $executable --version } else { $executable }
    Write-Info "labby installed: $installed"
    Write-Info "next: run 'labby setup' to start the first-run flow"
}

if ($env:LABBY_INSTALL_NO_MAIN -ne '1') {
    try {
        Invoke-LabbyInstall -InstallDir $InstallDir -Version $Version -Repo $Repo `
            -Rollback:$Rollback -AllowSourceFallback:$AllowSourceFallback -LocalBinary $LocalBinary `
            -LocalSha256 $LocalSha256 -NoPathUpdate:$NoPathUpdate
    } catch {
        Write-Error "install.ps1: $($_.Exception.Message)"
        exit 1
    }
}
