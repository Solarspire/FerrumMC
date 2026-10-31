param([int] $Port = 25583, [string] $Seed = '1', [int] $Runs = 2, [int] $Radius = 96, [int] $TimeoutSeconds = 240, [switch] $SkipBuild)
. "$PSScriptRoot/env.ps1"
if ($Port -lt 1024 -or $Port -gt 65535) { throw 'Use an unprivileged TCP port.' }
$runtime = Join-Path $FerrumRoot 'bridge/neoforge-runtime'
if (!$SkipBuild) {
    & "$runtime/gradlew.bat" -p $runtime --console=plain assemble
    if ($LASTEXITCODE -ne 0) { throw 'Runtime build failed' }
    & cargo build --locked -p pumpkin
    if ($LASTEXITCODE -ne 0) { throw 'FerrumMC build failed' }
}
if (-not ('FerrumBridgeOutput' -as [type])) {
    Add-Type -TypeDefinition @'
using System.Collections.Concurrent;
using System.Diagnostics;
public sealed class FerrumBridgeOutput {
    public readonly ConcurrentQueue<string> Lines = new ConcurrentQueue<string>();
    public FerrumBridgeOutput(Process process) {
        process.OutputDataReceived += (_, e) => { if (e.Data != null) Lines.Enqueue(e.Data); };
        process.ErrorDataReceived += (_, e) => { if (e.Data != null) Lines.Enqueue(e.Data); };
    }
}
'@
}
function Send-Console($process, [string] $command) {
    $process.StandardInput.WriteLine($command)
    $process.StandardInput.Flush()
}

# Generates the area around the origin with the bridge on and returns the structure and block entity trace.
function Invoke-WorldgenRun([int] $run) {
    $scratch = Join-Path $FerrumRoot ('run/neoforge-wg-' + [Guid]::NewGuid().ToString('N'))
    $game = Join-Path $scratch 'neoforge'
    New-Item -ItemType Directory -Force "$game/mods" | Out-Null
    New-Item -ItemType Junction -Path "$game/libraries" -Target "$runtime/.cache/server/libraries" | Out-Null
    foreach ($jar in @(
        "$FerrumRoot/tests/mods/candidates/hbm-neoforge-1.0.3.jar",
        "$runtime/build/libs/ferrum-neoforge-runtime-1.0.0.jar"
    )) { Copy-Item -LiteralPath $jar -Destination "$game/mods" }
    @{ gameDirectory = $game; reportPath = "$scratch/loader-report.json" } |
        ConvertTo-Json | Set-Content "$scratch/runtime.json" -Encoding utf8NoBOM
    @{ enabled = $true; launch = "$runtime/.cache/jvm.json"; runtime = "$scratch/runtime.json" } |
        ConvertTo-Json | Set-Content "$scratch/ferrum-neoforge.json" -Encoding utf8NoBOM
    @"
seed = "$Seed"
allow_nether = false
allow_end = false
spawn_protection = 0
[networking.java]
address = "127.0.0.1:$Port"
online_mode = false
encryption = false
view_distance = 2
simulation_distance = 2
[networking.bedrock]
enabled = false
[networking.lan_broadcast]
enabled = false
[telemetry]
enabled = false
[logging]
color = false
[commands]
use_tty = false
"@ | Set-Content "$scratch/ferrummc.toml" -Encoding utf8NoBOM

    $env:FERRUM_WORLDGEN_TRACE = '1'
    $process = [Diagnostics.Process]::new()
    $process.StartInfo.FileName = if ($env:FERRUM_TEST_EXE) { $env:FERRUM_TEST_EXE } else { "$FerrumRoot/target/debug/ferrummc.exe" }
    $process.StartInfo.WorkingDirectory = $scratch
    $process.StartInfo.UseShellExecute = $false
    $process.StartInfo.CreateNoWindow = $true
    $process.StartInfo.RedirectStandardInput = $true
    $process.StartInfo.RedirectStandardOutput = $true
    $process.StartInfo.RedirectStandardError = $true
    $output = [FerrumBridgeOutput]::new($process)
    $started = $false
    $trace = @()
    $probe = $null
    try {
        $started = $process.Start()
        if (!$started) { throw 'Server process did not start' }
        $process.BeginOutputReadLine()
        $process.BeginErrorReadLine()
        $deadline = [DateTime]::UtcNow.AddSeconds($TimeoutSeconds)
        while ([DateTime]::UtcNow -lt $deadline -and !$process.HasExited) {
            $text = $output.Lines.ToArray() -join "`n"
            if ($text.Contains('Server is now running.') -and $text.Contains('NeoForge runtime attached; frozen registries ready')) { break }
            Start-Sleep -Milliseconds 100
        }
        if ($process.HasExited -or [DateTime]::UtcNow -ge $deadline) { throw 'Rust server or NeoForge runtime did not become ready' }
        Send-Console $process "execute in minecraft:overworld run forceload add $(-$Radius) $(-$Radius) $Radius $Radius"
        # Generation is done once the trace stops growing.
        $last = -1
        $stable = 0
        while ([DateTime]::UtcNow -lt $deadline -and !$process.HasExited -and $stable -lt 8) {
            Start-Sleep -Milliseconds 1000
            $count = @($output.Lines.ToArray() | Where-Object { $_ -match 'Placed structure|Generated block entity' }).Count
            if ($count -gt 0 -and $count -eq $last) { $stable++ } else { $stable = 0 }
            $last = $count
        }
        $trace = @($output.Lines.ToArray() | Where-Object { $_ -match 'Structure start|Placed structure|Generated block entity' } |
            ForEach-Object { $_ -replace '^\[[0-9:]+\] \[[^\]]*\] \[[^\]]*\]: ', '' })
        # The generated block entity must live in the Rust world, not only in the Java trace.
        $entity = @($trace | Where-Object { $_ -match '^Generated block entity \S+ at (-?\d+) (-?\d+) (-?\d+)' } | ForEach-Object {
            if ($_ -match 'Generated block entity (\S+) at (-?\d+) (-?\d+) (-?\d+)') { [pscustomobject]@{ Id = $Matches[1]; Pos = "$($Matches[2]) $($Matches[3]) $($Matches[4])" } }
        }) | Sort-Object { $_.Id -ne 'minecraft:chest' } | Select-Object -First 1
        if ($entity) {
            $mark = $output.Lines.Count
            Send-Console $process "ferrum be $($entity.Pos)"
            Start-Sleep -Milliseconds 1500
            $reply = ($output.Lines.ToArray() | Select-Object -Skip $mark) -join "`n"
            $probe = @{ entity = $entity; reply = $reply }
        }
    } finally {
        if ($started -and !$process.HasExited) {
            Send-Console $process 'stop'
            $process.StandardInput.Close()
            if (!$process.WaitForExit(30000)) { $process.Kill($true) }
        }
        if ($started) {
            $process.WaitForExit()
            $output.Lines.ToArray() | Set-Content "$scratch/server.log"
        }
        $process.Dispose()
    }
    $failures = @(Get-Content "$scratch/server.log" | Where-Object { $_ -match 'Generation (structure|feature) .* failed|modded chunk decoration failed|Generated block entity .* failed' })
    if ($failures.Count) { throw "Worldgen failures in ${scratch}: $($failures[0])" }
    $errors = @(Get-Content "$scratch/server.log" | Where-Object { $_ -match '/ERROR\]' })
    if ($errors.Count) { Get-Content "$scratch/server.log" | Select-String -Pattern '/ERROR\]' -Context 0,6 | Select-Object -First 2 | ForEach-Object { Write-Warning ($_.Line + "`n" + ($_.Context.PostContext -join "`n")) } }
    Remove-Item -Recurse -Force $scratch
    return @{ trace = $trace; probe = $probe; errors = $errors.Count }
}

$hashes = @()
for ($run = 1; $run -le $Runs; $run++) {
    $result = Invoke-WorldgenRun $run
    $starts = @($result.trace | Where-Object { $_ -like 'Structure start*' })
    $placed = @($result.trace | Where-Object { $_ -like 'Placed structure*' -and $_ -notmatch ' 0 block writes' })
    $entities = @($result.trace | Where-Object { $_ -like 'Generated block entity*' })
    if ($starts.Count -lt 1) { throw "Run ${run}: no hbm structure start was generated around the origin for seed $Seed" }
    if ($placed.Count -lt 1) { throw "Run ${run}: structures started but none wrote blocks" }
    if ($entities.Count -lt 1) { throw "Run ${run}: no generated block entity reached the trace" }
    if (!$result.probe -or $result.probe.reply -match 'No block entity') { throw "Run ${run}: generated block entity $($result.probe.entity.Id) is missing from the Rust world" }
    # The trace order depends on worker scheduling; the set of results must not.
    $sorted = ($result.trace | Sort-Object) -join "`n"
    $hash = [BitConverter]::ToString([Security.Cryptography.SHA256]::HashData([Text.Encoding]::UTF8.GetBytes($sorted))).Replace('-', '').Substring(0, 16)
    $hashes += $hash
    $kinds = ($entities | ForEach-Object { if ($_ -match 'Generated block entity (\S+)') { $Matches[1] } } | Group-Object | ForEach-Object { "$($_.Name)x$($_.Count)" }) -join ' '
    $reply = ([string]$result.probe.reply) -replace "\s+", ' '
    Write-Output "run $run : probed $($result.probe.entity.Id) at $($result.probe.entity.Pos): $($reply.Substring(0, [Math]::Min(300, $reply.Length)))"
    Write-Output "run $run : block entity types: $kinds"
    Write-Output "run $run : starts=$($starts.Count) placed=$($placed.Count) blockEntities=$($entities.Count) hash=$hash"
}
if (($hashes | Select-Object -Unique).Count -ne 1) { throw "Generation is not deterministic for seed ${Seed}: $($hashes -join ', ')" }
Write-Output "PASS seed=$Seed deterministic=$($hashes[0])"
