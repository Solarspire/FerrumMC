param([switch]$PrepareOnly, [int]$Port = 25566, [int]$TimeoutSeconds = 600)
. "$PSScriptRoot/env.ps1"
if ($PrepareOnly) {
    $smokeDir = Join-Path $FerrumRoot 'run/smoke'
    New-Item -ItemType Directory -Force $smokeDir | Out-Null
    (Get-Content -Raw "$FerrumRoot/tests/smoke/ferrummc.toml").Replace('25566', "$Port") |
        Set-Content "$smokeDir/ferrummc.toml" -Encoding utf8NoBOM
    Write-Output "Scratch config prepared at $smokeDir/ferrummc.toml; an enabled NeoForge bridge is required."
    return
}
& cargo test --locked -p pumpkin-core net::java::registry_remap::tests
if ($LASTEXITCODE -ne 0) { throw 'Native registry remap tests failed' }
& "$PSScriptRoot/test-neoforge-client.ps1" -Port $Port -TimeoutSeconds $TimeoutSeconds -SkipBuild -VanillaOnly
Write-Output 'PASS native 776 status/codecs, vanilla NeoForge gate, and unsupported protocol rejection'
