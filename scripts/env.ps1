$ErrorActionPreference = 'Stop'
$env:PATH = "$env:USERPROFILE\.cargo\bin;C:\Program Files\Eclipse Adoptium\jdk-25.0.2.10-hotspot\bin;$env:PATH"
$env:JAVA_HOME = 'C:\Program Files\Eclipse Adoptium\jdk-25.0.2.10-hotspot'
$env:RUSTUP_TOOLCHAIN = 'stable'
$FerrumRoot = Split-Path -Parent $PSScriptRoot
# Keep dependency downloads in the writable workspace, not in the reference checkouts.
$env:CARGO_HOME = Join-Path $FerrumRoot '.build-cache/cargo'
$env:GRADLE_USER_HOME = Join-Path $FerrumRoot '.build-cache/gradle'
$env:TEMP = Join-Path $FerrumRoot '.build-cache/tmp'
$env:TMP = $env:TEMP
New-Item -ItemType Directory -Force $env:TEMP | Out-Null
if ((& rustc --version) -notmatch '^rustc 1\.99\.1 ') {
    throw 'Build both native artifacts with Rust stable 1.99.1 MSVC.'
}
if ($LASTEXITCODE -ne 0) { throw 'rustc failed' }
if ((& javac -version 2>&1 | Out-String) -notmatch 'javac 25\.0\.2') {
    throw 'JDK 25.0.2 is required.'
}
if ($LASTEXITCODE -ne 0) { throw 'javac failed' }
