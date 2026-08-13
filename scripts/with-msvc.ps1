param(
    [Parameter(Mandatory = $true)]
    [ValidateSet('check', 'clippy', 'dev', 'build', 'build-cutover', 'test', 'dev-isolated', 'build-isolated', 'host-build-isolated')]
    [string]$Action
)

$ErrorActionPreference = 'Stop'
$projectRoot = Split-Path -Parent $PSScriptRoot
$candidateInstalls = @(
    'C:\Program Files\Microsoft Visual Studio\18\Insiders',
    'C:\Program Files\Microsoft Visual Studio\2022\Preview',
    'C:\Program Files\Microsoft Visual Studio\2022\Community',
    'C:\Program Files (x86)\Microsoft Visual Studio\2022\BuildTools'
)

$vsInstall = $candidateInstalls |
    Where-Object { Test-Path (Join-Path $_ 'Common7\Tools\Microsoft.VisualStudio.DevShell.dll') } |
    Select-Object -First 1

if (-not $vsInstall) {
    throw 'Visual Studio C++ tools were not found. Install the Desktop development with C++ workload.'
}

$devShell = Join-Path $vsInstall 'Common7\Tools\Microsoft.VisualStudio.DevShell.dll'
Import-Module $devShell
Enter-VsDevShell -VsInstallPath $vsInstall -SkipAutomaticLocation -DevCmdArguments '-arch=x64 -host_arch=x64'

$toolRoot = Get-ChildItem (Join-Path $vsInstall 'VC\Tools\MSVC') -Directory -ErrorAction SilentlyContinue |
    Sort-Object Name -Descending |
    Where-Object { Test-Path (Join-Path $_.FullName 'bin\Hostx64\x64\link.exe') } |
    Select-Object -First 1

if (-not $toolRoot) {
    throw 'The x64 Microsoft linker was not found.'
}

$linkerDirectory = Join-Path $toolRoot.FullName 'bin\Hostx64\x64'
$standardInclude = Join-Path $toolRoot.FullName 'include'
$standardLibrary = Join-Path $toolRoot.FullName 'lib\x64'
$scopeRoot = Join-Path $vsInstall 'SDK\ScopeCppSDK\vc15\VC'
$scopeInclude = Join-Path $scopeRoot 'include'
$scopeLibrary = Join-Path $scopeRoot 'lib'

$env:Path = "$linkerDirectory;$env:USERPROFILE\.cargo\bin;$env:Path"
if (Test-Path (Join-Path $standardInclude 'excpt.h')) {
    $env:INCLUDE = "$standardInclude;$env:INCLUDE"
} elseif (Test-Path (Join-Path $scopeInclude 'excpt.h')) {
    $env:INCLUDE = "$scopeInclude;$env:INCLUDE"
}
if (Test-Path (Join-Path $standardLibrary 'msvcrt.lib')) {
    $env:LIB = "$standardLibrary;$env:LIB"
} elseif (Test-Path (Join-Path $scopeLibrary 'msvcrt.lib')) {
    $env:LIB = "$scopeLibrary;$env:LIB"
}

$manifest = Join-Path $projectRoot 'src-tauri\Cargo.toml'
$isolatedConfig = Join-Path $projectRoot 'src-tauri\tauri.isolated.conf.json'
Set-Location $projectRoot

switch ($Action) {
    'check' { & cargo check --manifest-path $manifest }
    'clippy' { & cargo clippy --manifest-path $manifest --all-targets -- -D warnings }
    'test' { & cargo test --manifest-path $manifest }
    'dev' {
        & cargo build --manifest-path $manifest --bin ai-dock-session-host
        if ($LASTEXITCODE -eq 0) {
            & "$projectRoot\node_modules\.bin\tauri.cmd" dev
        }
    }
    'build' {
        & "$projectRoot\node_modules\.bin\tauri.cmd" build
        if ($LASTEXITCODE -eq 0) {
            & cargo build --release --manifest-path $manifest --bin ai-dock-session-host
        }
    }
    'build-cutover' {
        $env:CARGO_TARGET_DIR = Join-Path $projectRoot 'src-tauri\target-cutover'
        & "$projectRoot\node_modules\.bin\tauri.cmd" build
        if ($LASTEXITCODE -eq 0) {
            & cargo build --release --manifest-path $manifest --bin ai-dock-session-host
        }
    }
    'dev-isolated' {
        $env:AI_DOCK_BUILD_FLAVOR = 'test'
        $env:VITE_AI_DOCK_FLAVOR = 'test'
        $env:CARGO_TARGET_DIR = Join-Path $projectRoot 'src-tauri\target-isolated'
        & cargo build --manifest-path $manifest --bin ai-dock-session-host
        if ($LASTEXITCODE -eq 0) {
            & "$projectRoot\node_modules\.bin\tauri.cmd" dev --config $isolatedConfig
        }
    }
    'build-isolated' {
        $env:AI_DOCK_BUILD_FLAVOR = 'test'
        $env:VITE_AI_DOCK_FLAVOR = 'test'
        $env:CARGO_TARGET_DIR = Join-Path $projectRoot 'src-tauri\target-isolated'
        & "$projectRoot\node_modules\.bin\tauri.cmd" build --config $isolatedConfig
        if ($LASTEXITCODE -eq 0) {
            & cargo build --release --manifest-path $manifest --bin ai-dock-session-host
        }
    }
    'host-build-isolated' {
        $env:AI_DOCK_BUILD_FLAVOR = 'test'
        $env:VITE_AI_DOCK_FLAVOR = 'test'
        $env:CARGO_TARGET_DIR = Join-Path $projectRoot 'src-tauri\target-isolated'
        & cargo build --release --manifest-path $manifest --bin ai-dock-session-host
    }
}

if ($LASTEXITCODE -ne 0) {
    exit $LASTEXITCODE
}
