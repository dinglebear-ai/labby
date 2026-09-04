BeforeAll {
    $env:LABBY_INSTALL_NO_MAIN = '1'
    $env:LOCALAPPDATA = Join-Path $TestDrive 'localappdata'
    . "$PSScriptRoot/../install.ps1"
}

AfterAll {
    Remove-Item Env:LABBY_INSTALL_NO_MAIN -ErrorAction SilentlyContinue
}

Describe 'Labby Windows installer contracts' {
    BeforeEach {
        Mock Test-LabbyReleaseProvenance {}
    }
    It 'fails when the release API cannot resolve latest' {
        Mock Invoke-RestMethod { throw 'fixture API unavailable' }
        { Resolve-LabbyReleaseVersion -Repo example/labby -RequestedVersion latest `
            -AssetName lab-x86_64-pc-windows-msvc.zip } | Should -Throw '*API unavailable*'
    }

    It 'fails latest resolution when no release contains the Windows asset' {
        Mock Invoke-RestMethod { @([pscustomobject]@{ tag_name = 'incus-v9'; assets = @([pscustomobject]@{ name = 'other.tar.gz' }) }) }
        { Resolve-LabbyReleaseVersion -Repo example/labby -RequestedVersion latest `
            -AssetName lab-x86_64-pc-windows-msvc.zip } | Should -Throw '*no published release contains*'
    }

    It 'does not invoke source fallback unless explicitly enabled' {
        Mock Install-LabbyFromRelease { throw 'fixture asset unavailable' }
        Mock Install-LabbyFromSource { throw 'source fallback must not run' }
        { Invoke-LabbyInstall -InstallDir (Join-Path $TestDrive 'disabled') -Version v1 `
            -Repo example/labby -NoPathUpdate } | Should -Throw '*source fallback is disabled*'
        Assert-MockCalled Install-LabbyFromSource -Times 0
    }

    It 'fails when the required checksum sidecar is unavailable' {
        Mock Invoke-WebRequest {
            param($Uri, $OutFile)
            if ($Uri -like '*.sha256') { throw 'fixture sidecar unavailable' }
            Set-Content -NoNewline -Path $OutFile -Value 'not-expanded'
        }
        { Install-LabbyFromRelease -InstallDir (Join-Path $TestDrive 'sidecar') `
            -Version v1 -Repo example/labby } | Should -Throw '*sidecar unavailable*'
    }
    It 'fails closed when a published checksum does not match' {
        $root = Join-Path $TestDrive 'checksum'
        New-Item -ItemType Directory -Path $root | Out-Null
        $artifact = Join-Path $root 'labby.zip'
        $checksum = "$artifact.sha256"
        Set-Content -NoNewline -Path $artifact -Value 'artifact bytes'
        Set-Content -Path $checksum -Value (('0' * 64) + '  labby.zip')

        { Test-LabbyChecksum -ArtifactPath $artifact -ChecksumPath $checksum } |
            Should -Throw '*checksum verification FAILED*'
    }

    It 'fails closed on a malformed checksum sidecar' {
        $artifact = Join-Path $TestDrive 'malformed.zip'
        Set-Content -NoNewline -Path $artifact -Value 'artifact bytes'
        Set-Content -Path "$artifact.sha256" -Value 'not-a-digest  malformed.zip'
        { Test-LabbyChecksum -ArtifactPath $artifact -ChecksumPath "$artifact.sha256" } |
            Should -Throw '*malformed*'
    }

    It 'classifies missing provenance verification as a trust failure' {
        Mock Get-Command { $null } -ParameterFilter { $Name -eq 'gh' }
        try {
            Test-LabbyReleaseProvenance -ArtifactPath artifact.zip -Repo example/labby -ResolvedVersion v1.2.3
            throw 'provenance verification unexpectedly succeeded'
        } catch {
            $_.Exception.Data['LabbyTrustFailure'] | Should -BeTrue
        }
    }

    It 'accepts named and bare checksum digests for the requested archive' {
        $artifact = Join-Path $TestDrive 'valid.zip'
        Set-Content -NoNewline -Path $artifact -Value 'valid artifact bytes'
        $digest = (Get-FileHash $artifact -Algorithm SHA256).Hash.ToLowerInvariant()
        foreach ($contents in @($digest, "$digest  unrelated-name.zip")) {
            Set-Content -NoNewline -Path "$artifact.sha256" -Value $contents
            { Test-LabbyChecksum -ArtifactPath $artifact -ChecksumPath "$artifact.sha256" } | Should -Not -Throw
        }
    }

    It 'selects the newest release that contains the Windows asset' {
        Mock Invoke-RestMethod {
            @(
                [pscustomobject]@{
                    tag_name = 'incus-v9'
                    assets = @([pscustomobject]@{ name = 'labby-incus.tar.gz' })
                },
                [pscustomobject]@{
                    tag_name = 'v1.2.3'
                    assets = @([pscustomobject]@{ name = 'lab-x86_64-pc-windows-msvc.zip' })
                }
            )
        }

        Resolve-LabbyReleaseVersion -Repo 'example/labby' -RequestedVersion latest `
            -AssetName 'lab-x86_64-pc-windows-msvc.zip' | Should -Be 'v1.2.3'
    }

    It 'keeps a requested tag in explicit source fallback arguments' {
        $arguments = Get-LabbyCargoInstallArgumentList -Repo 'example/labby' -RequestedVersion 'v2.0.0' -Root 'C:\labby'
        ($arguments -join ' ') | Should -Match '--tag v2\.0\.0'
    }

    It 'pins latest source fallback to a resolved commit' {
        $arguments = Get-LabbyCargoInstallArgumentList -Repo 'example/labby' -RequestedVersion latest `
            -Root 'C:\labby' -ResolvedRevision ('a' * 40)
        ($arguments -join ' ') | Should -Match ('--rev ' + ('a' * 40))
    }

    It 'builds a pinned fallback and records its tag and exact binary digest' {
        $toolDir = Join-Path $TestDrive 'cargo-bin'
        $installDir = Join-Path $TestDrive 'source-install'
        New-Item -ItemType Directory -Path $toolDir | Out-Null
        $cargo = Join-Path $toolDir $(if ($IsWindows) { 'cargo.cmd' } else { 'cargo' })
        if ($IsWindows) {
            @'
@echo off
echo %* > "%LABBY_TEST_CARGO_ARGS%"
:next
if "%~1"=="" exit /b 64
if "%~1"=="--root" goto root
shift
goto next
:root
mkdir "%~2\bin" 2>nul
<nul set /p="source-pinned" > "%~2\bin\labby.exe"
'@ | Set-Content -NoNewline $cargo
        } else {
        @'
#!/bin/sh
printf '%s\n' "$*" >"$LABBY_TEST_CARGO_ARGS"
root=
while [ "$#" -gt 0 ]; do
  if [ "$1" = --root ]; then root=$2; break; fi
  shift
done
mkdir -p "$root/bin"
printf source-pinned >"$root/bin/labby.exe"
'@ | Set-Content -NoNewline $cargo
            & chmod 755 $cargo
        }
        $oldPath = $env:PATH
        $env:PATH = "$toolDir$([IO.Path]::PathSeparator)$oldPath"
        $env:LABBY_TEST_CARGO_ARGS = Join-Path $TestDrive 'cargo.args'
        try {
            Install-LabbyFromSource -InstallDir $installDir -Version v3.0.0 -Repo example/labby
        } finally { $env:PATH = $oldPath }
        Get-Content -Raw $env:LABBY_TEST_CARGO_ARGS | Should -Match '--tag v3\.0\.0'
        $receipt = Get-Content -Raw (Join-Path $installDir '.labby-install/receipt.json') | ConvertFrom-Json
        $receipt.requested_version | Should -Be v3.0.0
        $receipt.resolved_version | Should -Be v3.0.0
        $receipt.sha256 | Should -Be ((Get-FileHash (Join-Path $installDir 'labby.exe') -Algorithm SHA256).Hash.ToLowerInvariant())
    }

    It 'preserves the prior installation after release checksum failure' {
        $installDir = Join-Path $TestDrive 'checksum-preserve'
        $prior = Join-Path $TestDrive 'prior.exe'
        Set-Content -NoNewline -Path $prior -Value 'known-good'
        Install-LabbyVerifiedBinary -SourcePath $prior -InstallDir $installDir -Source release `
            -RequestedVersion v1 -ResolvedVersion v1
        Mock Invoke-WebRequest {
            param($Uri, $OutFile)
            $contents = if ($Uri -like '*.sha256') { ('0' * 64) + '  labby.zip' } else { 'untrusted archive' }
            [IO.File]::WriteAllText($OutFile, $contents, [Text.UTF8Encoding]::new($false))
        }
        { Install-LabbyFromRelease -InstallDir $installDir -Version v2 -Repo example/labby } |
            Should -Throw '*checksum verification FAILED*'
        Get-Content -Raw (Join-Path $installDir 'labby.exe') | Should -Be 'known-good'
        (Get-Content -Raw (Join-Path $installDir '.labby-install/receipt.json') | ConvertFrom-Json).resolved_version |
            Should -Be v1
    }

    It 'retains exact known-good bytes and rolls back without a network call' {
        $installDir = Join-Path $TestDrive 'bin'
        $v1 = Join-Path $TestDrive 'v1.exe'
        $v2 = Join-Path $TestDrive 'v2.exe'
        Set-Content -NoNewline -Path $v1 -Value 'release-v1'
        Set-Content -NoNewline -Path $v2 -Value 'release-v2'

        Install-LabbyVerifiedBinary -SourcePath $v1 -InstallDir $installDir -Source release `
            -RequestedVersion v1.0.0 -ResolvedVersion v1.0.0
        Install-LabbyVerifiedBinary -SourcePath $v2 -InstallDir $installDir -Source release `
            -RequestedVersion v2.0.0 -ResolvedVersion v2.0.0

        Get-Content -Raw (Join-Path $installDir 'labby.exe') | Should -Be 'release-v2'
        Mock Invoke-WebRequest { throw 'network must not be used for rollback' }
        Restore-LabbyPreviousInstall -InstallDir $installDir
        Get-Content -Raw (Join-Path $installDir 'labby.exe') | Should -Be 'release-v1'
        Assert-MockCalled Invoke-WebRequest -Times 0

        $receipt = Get-Item (Join-Path $installDir '.labby-install/receipt.json')
        $receipt | Should -Not -BeNullOrEmpty
        (Get-Content -Raw $receipt | ConvertFrom-Json).resolved_version | Should -Be 'v1.0.0'
    }

    It 'rehashes cached artifacts before reuse' {
        $installDir = Join-Path $TestDrive 'tamper-bin'
        $candidate = Join-Path $TestDrive 'tamper.exe'
        Set-Content -NoNewline -Path $candidate -Value 'candidate'
        Install-LabbyVerifiedBinary -SourcePath $candidate -InstallDir $installDir -Source release `
            -RequestedVersion v1 -ResolvedVersion v1
        $cached = Get-ChildItem (Join-Path $installDir '.labby-install/artifacts') -Recurse -Filter labby.exe
        Set-Content -NoNewline -Path $cached.FullName -Value 'tampered'
        { Install-LabbyVerifiedBinary -SourcePath $candidate -InstallDir $installDir -Source release `
            -RequestedVersion v1 -ResolvedVersion v1 } | Should -Throw '*cached artifact digest*'
    }

    It 'installs only an exact locally supplied candidate digest' {
        $installDir = Join-Path $TestDrive 'local-bin'
        $candidate = Join-Path $TestDrive 'local.exe'
        Set-Content -NoNewline -Path $candidate -Value 'local-candidate'
        $digest = (Get-FileHash $candidate -Algorithm SHA256).Hash.ToLowerInvariant()
        Install-LabbyLocalCandidate -Path $candidate -ExpectedSha256 $digest -InstallDir $installDir -Version v2
        Get-Content -Raw (Join-Path $installDir 'labby.exe') | Should -Be 'local-candidate'
        (Get-Content -Raw (Join-Path $installDir '.labby-install/receipt.json') | ConvertFrom-Json).source |
            Should -Be local
        { Install-LabbyLocalCandidate -Path $candidate -ExpectedSha256 ('0' * 64) `
            -InstallDir $installDir -Version v2 } | Should -Throw '*checksum verification FAILED*'
    }

    It 'stages a local candidate once before verifying and activating it' {
        $installDir = Join-Path $TestDrive 'local-toctou-bin'
        $candidate = Join-Path $TestDrive 'local-toctou.exe'
        Set-Content -NoNewline -Path $candidate -Value 'original-candidate'
        $digest = (Get-FileHash $candidate -Algorithm SHA256).Hash.ToLowerInvariant()
        Mock Get-FileHash {
            param($Path, $Algorithm)
            $stream = [IO.File]::OpenRead($Path)
            try {
                $hasher = [Security.Cryptography.SHA256]::Create()
                $hash = [Convert]::ToHexString($hasher.ComputeHash($stream))
            } finally { $stream.Dispose() }
            $result = [pscustomobject]@{ Hash = $hash }
            if ($Path -eq $candidate) { Set-Content -NoNewline -Path $candidate -Value 'mutated-candidate' }
            $result
        }
        Install-LabbyLocalCandidate -Path $candidate -ExpectedSha256 $digest -InstallDir $installDir -Version v2
        Get-Content -Raw (Join-Path $installDir 'labby.exe') | Should -Be 'original-candidate'
    }

    It 'recovers a durable activation journal at every live-file boundary' {
        foreach ($state in @('binary-activated', 'previous-receipt-activated', 'receipt-activated')) {
            $installDir = Join-Path $TestDrive "crash-$state"
            $metadata = Join-Path $installDir '.labby-install'
            $journal = Join-Path $metadata 'activation-journal'
            New-Item -ItemType Directory -Force -Path $journal | Out-Null
            Set-Content -NoNewline -Path (Join-Path $journal 'old-binary.exe') -Value 'old-binary'
            Set-Content -NoNewline -Path (Join-Path $journal 'old-receipt.json') -Value '{"resolved_version":"old"}'
            Set-Content -NoNewline -Path (Join-Path $journal 'old-binary.present') -Value ''
            Set-Content -NoNewline -Path (Join-Path $journal 'old-receipt.present') -Value ''
            Set-Content -NoNewline -Path (Join-Path $journal 'state') -Value $state
            Set-Content -NoNewline -Path (Join-Path $installDir 'labby.exe') -Value 'new-binary'
            Set-Content -NoNewline -Path (Join-Path $metadata 'receipt.json') -Value '{"resolved_version":"new"}'
            Set-Content -NoNewline -Path (Join-Path $metadata 'previous-receipt.json') -Value '{"resolved_version":"old"}'
            Repair-LabbyActivation -InstallDir $installDir
            Get-Content -Raw (Join-Path $installDir 'labby.exe') | Should -Be 'old-binary'
            Get-Content -Raw (Join-Path $metadata 'receipt.json') | Should -Be '{"resolved_version":"old"}'
            Test-Path (Join-Path $metadata 'previous-receipt.json') | Should -BeFalse
            Test-Path $journal | Should -BeFalse
        }
    }

    It 'retains the activation journal and reports restoration failure' {
        $installDir = Join-Path $TestDrive 'crash-unrestorable'
        $journal = Join-Path $installDir '.labby-install/activation-journal'
        New-Item -ItemType Directory -Force -Path $journal | Out-Null
        Set-Content -NoNewline -Path (Join-Path $journal 'old-binary.present') -Value ''
        Set-Content -NoNewline -Path (Join-Path $journal 'state') -Value 'prepared'
        { Repair-LabbyActivation -InstallDir $installDir } | Should -Throw '*activation recovery FAILED*'
        Test-Path $journal | Should -BeTrue
    }

    It 'discards an unprepared journal without changing the live installation' {
        $installDir = Join-Path $TestDrive 'unprepared'
        $metadata = Join-Path $installDir '.labby-install'
        $journal = Join-Path $metadata 'activation-journal'
        New-Item -ItemType Directory -Force -Path $journal | Out-Null
        Set-Content -NoNewline -Path (Join-Path $installDir 'labby.exe') -Value 'known-good'
        Set-Content -NoNewline -Path (Join-Path $metadata 'receipt.json') -Value 'known-good-receipt'
        Set-Content -NoNewline -Path (Join-Path $journal 'new-labby.exe') -Value 'partial'
        Repair-LabbyActivation -InstallDir $installDir
        Get-Content -Raw (Join-Path $installDir 'labby.exe') | Should -Be 'known-good'
        Get-Content -Raw (Join-Path $metadata 'receipt.json') | Should -Be 'known-good-receipt'
        Test-Path $journal | Should -BeFalse
    }

    It 'restricts Windows metadata directories to the current SID' {
        Mock Test-LabbyRunningOnWindows { $true }
        Mock Invoke-LabbyIcacl { $global:LASTEXITCODE = 0 }
        Mock Get-CurrentLabbyWindowsSid { 'S-1-5-21-1234' }
        Protect-LabbyMetadataDirectory 'C:\labby\metadata'
        Assert-MockCalled Invoke-LabbyIcacl -Times 1 -ParameterFilter {
            $Arguments -contains '/inheritance:r' -and $Arguments -contains '*S-1-5-21-1234:(OI)(CI)(F)'
        }
    }

    It 'restores binary and receipt snapshots after post-activation failure' {
        $installDir = Join-Path $TestDrive 'transaction-bin'
        $v1 = Join-Path $TestDrive 'transaction-v1.exe'
        $v2 = Join-Path $TestDrive 'transaction-v2.exe'
        Set-Content -NoNewline -Path $v1 -Value 'transaction-v1'
        Set-Content -NoNewline -Path $v2 -Value 'transaction-v2'
        Install-LabbyVerifiedBinary -SourcePath $v1 -InstallDir $installDir -Source release `
            -RequestedVersion v1 -ResolvedVersion v1
        $receiptPath = Join-Path $installDir '.labby-install/receipt.json'
        $receiptBefore = Get-Content -Raw $receiptPath

        Mock Move-LabbyFileAtomically {
            param($Source, $Destination)
            if ($Destination -eq $receiptPath -and $Source -like '*new-receipt.json') {
                throw 'injected receipt activation failure'
            }
            if (Test-Path $Destination) { Remove-Item -Force $Destination }
            Move-Item $Source $Destination
        }

        { Install-LabbyVerifiedBinary -SourcePath $v2 -InstallDir $installDir -Source release `
            -RequestedVersion v2 -ResolvedVersion v2 } | Should -Throw '*injected receipt activation failure*'
        Get-Content -Raw (Join-Path $installDir 'labby.exe') | Should -Be 'transaction-v1'
        Get-Content -Raw $receiptPath | Should -Be $receiptBefore
        Test-Path (Join-Path $installDir '.labby-install/previous-receipt.json') | Should -BeFalse
    }
}
