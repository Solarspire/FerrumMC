. "$PSScriptRoot/env.ps1"
$serverDir = Join-Path $FerrumRoot 'run/server'
New-Item -ItemType Directory -Force "$serverDir/plugins" | Out-Null
Copy-Item -LiteralPath "$FerrumRoot/target/debug/patchbukkit.dll" -Destination "$serverDir/plugins/patchbukkit.dll"
Push-Location $serverDir
try {
    & "$FerrumRoot/target/debug/ferrummc.exe"
    if ($LASTEXITCODE -ne 0) { throw "Server exited with code $LASTEXITCODE" }
} finally { Pop-Location }
