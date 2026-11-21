# Exercises the actual web-build script without compiling Rust or running wasm-bindgen.
# Usage: pwsh scripts/test-build-web.ps1
param([string]$BuildScript = (Join-Path $PSScriptRoot 'build-web.ps1'))
$ErrorActionPreference = 'Stop'
$source = [IO.File]::ReadAllText($BuildScript)
$scratch = Join-Path $PSScriptRoot ('web-path-check-' + [guid]::NewGuid().ToString('N'))
$previousTarget = $env:CARGO_TARGET_DIR
$previousLocation = Get-Location
function Assert([bool]$Condition, [string]$Message) {
    if (-not $Condition) { throw $Message }
}
function cargo {
    Assert ((Get-Location).Path -eq $checkout) "cargo ran outside the literal checkout: $(Get-Location) expected $checkout"
    $calls.Add(@($args))
    if ($args[0] -eq 'run') {
        $out = $args[-1]
        New-Item -ItemType Directory -Force -Path $out | Out-Null
        Set-Content -LiteralPath (Join-Path $out 'index.html') -Value 'guide'
    }
    $global:LASTEXITCODE = 0
}
function rustup { 'wasm32-unknown-unknown'; $global:LASTEXITCODE = 0 }
function wasm-bindgen {
    $global:LASTEXITCODE = 0
    if ($args[0] -eq '--version') { 'wasm-bindgen 0.2.100'; return }
    $out = $args[[array]::IndexOf($args, '--out-dir') + 1]
    Assert ($args[-1] -eq $expectedWasm) 'wrong wasm input'
    Set-Content -LiteralPath (Join-Path $out 'anicraft_bg.wasm') -Value 'wasm'
    Set-Content -LiteralPath (Join-Path $out 'anicraft.js') -Value 'glue'
}
try {
    foreach ($dev in @($false, $true)) {
        $script:checkout = [IO.Path]::GetFullPath((Join-Path $scratch ('checkout[1]-' + $dev)))
        $neighbor = Join-Path $scratch ('checkout1-' + $dev)
        foreach ($dir in @('scripts', 'packaging/web', 'assets/app-icon/hicolor/128x128/apps', 'web')) {
            New-Item -ItemType Directory -Force -Path (Join-Path $checkout $dir) | Out-Null
        }
        New-Item -ItemType Directory -Force -Path (Join-Path $neighbor 'web') | Out-Null
        Set-Content -LiteralPath (Join-Path $neighbor 'web/keep.txt') -Value 'neighbor'
        Set-Content -LiteralPath (Join-Path $checkout 'web/stale.txt') -Value 'stale'
        Set-Content -LiteralPath (Join-Path $checkout 'Cargo.lock') -Value "name = `"wasm-bindgen`"`nversion = `"0.2.100`""
        Set-Content -LiteralPath (Join-Path $checkout 'packaging/web/index.html') -Value 'editor'
        Set-Content -LiteralPath (Join-Path $checkout 'assets/app-icon/hicolor/128x128/apps/ai.storyteller.anicraft.png') -Value 'icon'
        $scriptPath = Join-Path $checkout 'scripts/build-web.ps1'
        [IO.File]::WriteAllText($scriptPath, $source)
        $env:CARGO_TARGET_DIR = if ($dev) { Join-Path $scratch 'target[2]' } else { $null }
        $target = if ($dev) { $env:CARGO_TARGET_DIR } else { Join-Path $checkout 'target' }
        $profileDir = if ($dev) { 'debug' } else { 'web' }
        $script:expectedWasm = Join-Path $target "wasm32-unknown-unknown/$profileDir/anicraft.wasm"
        $calls = [Collections.Generic.List[object]]::new()
        & $scriptPath -Dev:$dev
        Assert ($calls.Count -eq 2 -and $calls[0][0] -eq 'build' -and $calls[1][0] -eq 'run') 'missing build or guide call'
        Assert ($calls[0][-1] -eq $(if ($dev) { 'dev' } else { 'web' })) 'wrong cargo profile'
        foreach ($file in @('index.html', 'anicraft.png', 'anicraft.js', 'anicraft_bg.wasm', 'guide/index.html')) {
            Assert (Test-Path -LiteralPath (Join-Path $checkout "web/$file")) "missing output $file"
        }
        Assert (-not (Test-Path -LiteralPath (Join-Path $checkout 'web/stale.txt'))) 'stale output remains'
        Assert ((Get-Content -Raw -LiteralPath (Join-Path $neighbor 'web/keep.txt')).Trim() -eq 'neighbor') 'neighbor changed'
    }
    'PASS: release/dev builds use literal checkout/output paths and preserve wildcard neighbors'
} finally {
    Set-Location -LiteralPath $previousLocation.Path
    $env:CARGO_TARGET_DIR = $previousTarget
    $base = [IO.Path]::GetFullPath($PSScriptRoot) + [IO.Path]::DirectorySeparatorChar
    $owned = [IO.Path]::GetFullPath($scratch)
    if (-not $owned.StartsWith($base, [StringComparison]::OrdinalIgnoreCase)) { throw 'scratch escaped test directory' }
    if (Test-Path -LiteralPath $owned) { Remove-Item -LiteralPath $owned -Recurse -Force }
}
