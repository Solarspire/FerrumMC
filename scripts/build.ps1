. "$PSScriptRoot/env.ps1"
Push-Location (Join-Path $FerrumRoot 'vendor/patchbukkit/java')
try {
    & ./gradlew.bat --no-daemon :patchbukkit:build :ferrummc-hello-plugin:build
    if ($LASTEXITCODE -ne 0) { throw 'Java build failed; see the Gradle output.' }
} finally { Pop-Location }
Push-Location $FerrumRoot
try {
    # Java must exist before PatchBukkit's Rust build script embeds it.
    & cargo build --locked
    if ($LASTEXITCODE -ne 0) { throw 'Rust debug build failed; see the Cargo output.' }
} finally { Pop-Location }
