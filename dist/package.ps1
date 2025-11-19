Param(
    [ValidateSet('auto', 'sfx', 'zip')]
    [string]$PackageFormat = 'auto'
)

$ErrorActionPreference = 'Stop'

function Detect-Version {
    $metadata = cargo metadata --no-deps --format-version 1 | ConvertFrom-Json
    return $metadata.packages[0].version
}

function Build-Binaries {
    cargo build --release --bins
}

function New-StagingLayout {
    param(
        [string]$StageDir
    )

    if (Test-Path $StageDir) { Remove-Item -Recurse -Force $StageDir }
    New-Item -ItemType Directory -Path "$StageDir\bin" | Out-Null

    Copy-Item "$PSScriptRoot/../target/release/dbgen.exe" "$StageDir/bin/dbgen.exe"
    Copy-Item "$PSScriptRoot/../target/release/rdbinspect.exe" "$StageDir/bin/rdbinspect.exe"

    @'
@echo off
setlocal
set "DEST=%ProgramFiles%\NorenTools"
set "BINDIR=%DEST%\bin"

if not exist "%BINDIR%" mkdir "%BINDIR%"
copy /Y "%~dp0bin\dbgen.exe" "%BINDIR%\dbgen.exe" >nul
copy /Y "%~dp0bin\rdbinspect.exe" "%BINDIR%\rdbinspect.exe" >nul

echo Installed dbgen and rdbinspect to %BINDIR%
echo Add %BINDIR% to your PATH to invoke the tools from any shell.
endlocal
exit /b 0
'@ | Set-Content -Path "$StageDir/install.bat" -Encoding ASCII
}

function Build-SfxInstaller {
    param(
        [string]$StageDir,
        [string]$OutputFile
    )

    if (-not (Get-Command 7z.exe -ErrorAction SilentlyContinue)) {
        throw '7z.exe is required to build the self-extracting installer. Install 7-Zip and ensure it is on PATH.'
    }

    if (Test-Path $OutputFile) { Remove-Item $OutputFile -Force }
    & 7z a -sfx $OutputFile "$StageDir\*" | Out-Null
}

function Build-Zip {
    param(
        [string]$StageDir,
        [string]$OutputFile
    )

    if (Test-Path $OutputFile) { Remove-Item $OutputFile -Force }
    Compress-Archive -Path "$StageDir/*" -DestinationPath $OutputFile
}

function Detect-Format {
    param(
        [string]$Requested
    )

    if ($Requested -ne 'auto') { return $Requested }

    if (Get-Command 7z.exe -ErrorAction SilentlyContinue) {
        return 'sfx'
    }

    return 'zip'
}

$root = Resolve-Path "$PSScriptRoot/.."
$dist = Join-Path $root 'dist'
$stage = Join-Path $dist 'noren-tools-windows'
$version = Detect-Version

Build-Binaries
New-StagingLayout -StageDir $stage

$format = Detect-Format -Requested $PackageFormat

switch ($format) {
    'sfx' {
        $output = Join-Path $dist "noren-tools-installer.exe"
        Build-SfxInstaller -StageDir $stage -OutputFile $output
        Write-Host "Created Windows installer: $output"
    }
    'zip' {
        $output = Join-Path $dist "noren-tools-$version.zip"
        Build-Zip -StageDir $stage -OutputFile $output
        Write-Host "Created zip archive: $output"
    }
}
