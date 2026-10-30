param([int] $TimeoutSeconds = 180)
$ErrorActionPreference = 'Stop'
$env:PATH = "$env:USERPROFILE\.cargo\bin;C:\Program Files\Eclipse Adoptium\jdk-25.0.2.10-hotspot\bin;$env:PATH"
$env:JAVA_HOME = 'C:\Program Files\Eclipse Adoptium\jdk-25.0.2.10-hotspot'
$repo = Split-Path $PSScriptRoot -Parent
$runtime = Join-Path $repo 'bridge/neoforge-runtime'
& (Join-Path $PSScriptRoot 'prepare-neoforge-runtime.ps1')
Push-Location $runtime
try {
    & ./gradlew.bat --console=plain assemble
    if ($LASTEXITCODE -ne 0) { throw 'NeoForge runtime build failed' }
} finally { Pop-Location }
$server = Join-Path $runtime '.cache/server'
$mods = Join-Path $server 'mods'
New-Item -ItemType Directory -Force $mods | Out-Null
$jars = @(
    (Join-Path $repo 'tests/mods/candidates/Draconic-Next-neoforge-1.0.0-alpha.1.jar'),
    (Join-Path $repo 'tests/mods/candidates/hbm-neoforge-1.0.3.jar'),
    (Join-Path $repo 'tests/mods/candidates/crankshaft-neoforge-1.6.1+mc26.2.jar'),
    (Join-Path $repo 'tests/mods/ferrum-testmod/build/libs/ferrum-testmod-1.0.0.jar'),
    (Join-Path $runtime 'build/libs/ferrum-neoforge-runtime-1.0.0.jar')
)
$hashes = @{}
foreach ($jar in $jars) {
    if (!(Test-Path -LiteralPath $jar)) { throw "Missing acceptance mod: $jar" }
    $hashes[$jar] = (Get-FileHash -LiteralPath $jar -Algorithm SHA256).Hash
    Copy-Item -LiteralPath $jar -Destination $mods -Force
}
$reportPath = Join-Path $runtime '.cache/report.json'
if (Test-Path -LiteralPath $reportPath) { Remove-Item -LiteralPath $reportPath }
$configPath = Join-Path $runtime '.cache/acceptance.json'
@{ gameDirectory = $server; reportPath = $reportPath } | ConvertTo-Json | Set-Content -LiteralPath $configPath -Encoding utf8NoBOM
$launch = Get-Content (Join-Path $runtime '.cache/jvm.json') -Raw | ConvertFrom-Json
$vmArgs = @($launch.vmOptions | ForEach-Object {
    if ($_.StartsWith('-Djava.class.path=')) { $_ + ';' + (Join-Path $runtime 'build/libs/ferrum-neoforge-runtime-1.0.0-smoke.jar').Replace('\', '/') } else { $_ }
})
$vmArgs += @('net.solarspire.ferrum.runtime.smoke.RuntimeAcceptance', $configPath.Replace('\', '/'))
$vmArgs = @($vmArgs | ForEach-Object { '"' + $_ + '"' })
$argFile = Join-Path $runtime '.cache/acceptance-args.txt'
$vmArgs | Set-Content -LiteralPath $argFile -Encoding utf8NoBOM
$stdout = Join-Path $runtime '.cache/acceptance.stdout.log'
$stderr = Join-Path $runtime '.cache/acceptance.stderr.log'
$process = Start-Process -FilePath "$env:JAVA_HOME/bin/java.exe" -ArgumentList ('@"' + $argFile + '"') -WorkingDirectory $server -WindowStyle Hidden -PassThru -RedirectStandardOutput $stdout -RedirectStandardError $stderr
if (!$process.WaitForExit($TimeoutSeconds * 1000)) {
    Stop-Process -Id $process.Id -Force
    throw "NeoForge bridge timed out; see $stdout and $stderr"
}
if ($process.ExitCode -ne 0) { throw "NeoForge bridge exited $($process.ExitCode); see $stdout and $stderr" }
$report = Get-Content -LiteralPath $reportPath -Raw | ConvertFrom-Json
if (!$report.ready) { throw 'NeoForge bridge did not become ready' }
if (!$report.compatible) { throw 'NeoForge bridge reported a compatibility failure' }
if (!$report.registriesFrozen) { throw 'Registries did not freeze' }
foreach ($modId in @('draconicnext', 'hbm', 'ferrum_testmod')) {
    $mod = $report.mods | Where-Object id -EQ $modId
    if (!$mod.constructed) { throw "$modId was not constructed" }
    $blocks = @($report.registries.'minecraft:block' | Where-Object { $_.StartsWith("${modId}:") }).Count
    $items = @($report.registries.'minecraft:item' | Where-Object { $_.StartsWith("${modId}:") }).Count
    if ($blocks -eq 0 -or $items -eq 0) { throw "$modId has empty block/item registries" }
    Write-Host "${modId}: constructed, $blocks blocks, $items items"
}
if (@($report.failedMixins).Count -ne 0) { throw "Failed mixins: $($report.failedMixins | ConvertTo-Json -Compress)" }
if (@($report.mixinAuditFailures).Count -ne 0) { throw 'Mixin target audit failed' }
if (@($report.appliedMixins).Count -eq 0) { throw 'No mixin applications were audited' }
if (@($report.skippedClientMixins).Count -eq 0) { throw 'Client mixin skip diagnostics are missing' }
if (@($report.headlessGuards).Count -lt 8) { throw 'World lifecycle guards were not installed' }
if (@($report.blockedWorldOperations).Count -ne 0 -or @($report.serverThreads).Count -ne 0) {
    throw 'A mod attempted to create or tick a Java world'
}
if ('ferrum_testmod:counter' -notin $report.payloads.PLAY.id) { throw 'Test mod payload is missing' }
if ('ferrum_testmod:counter_block' -notin $report.registries.'minecraft:block_entity_type') { throw 'Test mod block entity is missing' }
foreach ($jar in $jars) {
    if ((Get-FileHash -LiteralPath $jar -Algorithm SHA256).Hash -ne $hashes[$jar]) { throw "Mod changed: $jar" }
    $loadedJar = Join-Path $mods (Split-Path $jar -Leaf)
    if ((Get-FileHash -LiteralPath $loadedJar -Algorithm SHA256).Hash -ne $hashes[$jar]) { throw "Loaded mod changed: $loadedJar" }
}
if ((Test-Path (Join-Path $server 'world')) -or (Get-ChildItem $server -Recurse -Filter '*.mca')) {
    throw 'Bridge created a Java world or region file'
}
Write-Host "PASS: zero failed server mixins; report: $reportPath"
Write-Host "$(@($report.appliedMixins).Count) mixins applied; $(@($report.skippedClientMixins).Count) client mixins skipped; $(@($report.registries.PSObject.Properties).Count) frozen registries; $(@($report.payloads.CONFIGURATION).Count) configuration / $(@($report.payloads.PLAY).Count) play payloads"

# A required injection into an unavailable world method must remain visible without killing the host.
$probe = Join-Path $runtime 'build/libs/ferrum-neoforge-runtime-1.0.0-failure-probe.jar'
$probeCopy = Join-Path $mods (Split-Path $probe -Leaf)
$failureReport = Join-Path $runtime '.cache/failure-report.json'
$failureConfig = Join-Path $runtime '.cache/failure.json'
@{ gameDirectory = $server; reportPath = $failureReport } | ConvertTo-Json | Set-Content -LiteralPath $failureConfig -Encoding utf8NoBOM
$failureArgs = @($launch.vmOptions) + @($launch.entryClass, $failureConfig.Replace('\', '/'), '--once')
$failureArgFile = Join-Path $runtime '.cache/failure-args.txt'
$failureArgs | ForEach-Object { '"' + $_ + '"' } | Set-Content -LiteralPath $failureArgFile -Encoding utf8NoBOM
if (Test-Path -LiteralPath $failureReport) { Remove-Item -LiteralPath $failureReport }
Copy-Item -LiteralPath $probe -Destination $probeCopy -Force
try {
    $failureProcess = Start-Process -FilePath "$env:JAVA_HOME/bin/java.exe" -ArgumentList ('@"' + $failureArgFile + '"') -WorkingDirectory $server -WindowStyle Hidden -PassThru -RedirectStandardOutput (Join-Path $runtime '.cache/failure.stdout.log') -RedirectStandardError (Join-Path $runtime '.cache/failure.stderr.log')
    if (!$failureProcess.WaitForExit($TimeoutSeconds * 1000)) {
        Stop-Process -Id $failureProcess.Id -Force
        throw 'Failure-reporting regression timed out'
    }
    if ($failureProcess.ExitCode -ne 0) { throw 'A failed world mixin killed the diagnostic runtime' }
    $failure = Get-Content -LiteralPath $failureReport -Raw | ConvertFrom-Json
    if (!$failure.ready -or $failure.compatible -or @($failure.failedMixins).Count -ne 1 -or
        $failure.failedMixins[0].mixin -ne 'net.solarspire.ferrum.runtime.probe.mixin.UnavailableWorldMixin') {
        throw 'Failed world mixin was not accurately reported'
    }
    Write-Host 'PASS: unavailable world mixin reported; diagnostic runtime survived'
} finally {
    Remove-Item -LiteralPath $probeCopy -Force
}
