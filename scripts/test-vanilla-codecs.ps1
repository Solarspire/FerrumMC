param([string]$CaptureDirectory = "$PSScriptRoot/../run/smoke")
. "$PSScriptRoot/env.ps1"
$releases = @(
    @{ Version = '26.2'; Protocol = 776; Sha1 = '823e2250d24b3ddac457a60c92a6a941943fcd6a' },
    @{ Version = '26.3'; Protocol = 777; Sha1 = '33680f5f2ac32864d6d7cf5e56a705fdb3e05f4c' }
)
foreach ($release in $releases) {
    $version = $release.Version
    $cacheDir = Join-Path $env:USERPROFILE ".cache/ferrummc-vanilla/$version"
    New-Item -ItemType Directory -Force $cacheDir | Out-Null
    $jar = Join-Path $cacheDir 'server.jar'
    if (-not (Test-Path -LiteralPath $jar)) {
        Invoke-WebRequest "https://piston-data.mojang.com/v1/objects/$($release.Sha1)/server.jar" -OutFile $jar
    }
    if ((Get-FileHash -LiteralPath $jar -Algorithm SHA1).Hash.ToLowerInvariant() -ne $release.Sha1) {
        throw "Vanilla $version jar digest mismatch."
    }
    $serverJar = Join-Path $cacheDir "versions/$version/server-$version.jar"
    if (-not (Test-Path -LiteralPath $serverJar)) {
        # The bundler extracts dependencies; keep those and datagen output outside the repository.
        Push-Location $cacheDir
        try {
            & java '-DbundlerMainClass=net.minecraft.data.Main' -jar $jar --all --output generated > datagen.log 2>&1
            if ($LASTEXITCODE -ne 0) { throw "Vanilla $version data generation failed." }
        } finally { Pop-Location }
    }
    $classpath = (@($serverJar) + @(Get-ChildItem "$cacheDir/libraries" -Recurse -Filter *.jar | ForEach-Object FullName)) -join ';'
    $classes = Join-Path $FerrumRoot "target/oracle-$version"
    New-Item -ItemType Directory -Force $classes | Out-Null
    & javac -cp $classpath -d $classes "$FerrumRoot/tests/smoke/VanillaOracle.java"
    if ($LASTEXITCODE -ne 0) { throw "Vanilla $version oracle compilation failed." }
    foreach ($session in @('join', 'reconnect')) {
        $capture = Join-Path $CaptureDirectory "wire-$($release.Protocol)-$session.bin"
        $log = Join-Path $CaptureDirectory "vanilla-$version-$session.log"
        & java -cp "$classpath;$classes" VanillaOracle $capture > $log 2>&1
        if ($LASTEXITCODE -ne 0 -or -not (Select-String -LiteralPath $log -SimpleMatch "PASS vanilla $version")) {
            Get-Content -LiteralPath $log | Write-Output
            throw "Vanilla $version rejected $session traffic."
        }
        Get-Content -LiteralPath $log | Select-String -SimpleMatch 'PASS vanilla' | ForEach-Object { $_.Line }
    }
}
