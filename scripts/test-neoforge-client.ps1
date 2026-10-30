param([int] $Port = 25581, [string] $Block = 'hbm:machine_electric_furnace', [string] $PlaceItem = 'hbm:machine_radar', [string] $GunItem = 'hbm:gun_greasegun', [int] $TimeoutSeconds = 180, [int] $ViewDistance = 2, [switch] $SkipBuild, [switch] $VanillaOnly)
. "$PSScriptRoot/env.ps1"
if ($Port -lt 1024 -or $Port -gt 65535) { throw 'Use an unprivileged TCP port.' }
$runtime = Join-Path $FerrumRoot 'bridge/neoforge-runtime'
if (!$SkipBuild) {
    & "$runtime/gradlew.bat" -p $runtime --console=plain assemble
    if ($LASTEXITCODE -ne 0) { throw 'Runtime build failed' }
    & cargo build --locked -p pumpkin
    if ($LASTEXITCODE -ne 0) { throw 'FerrumMC build failed' }
}
$scratch = Join-Path $FerrumRoot ('run/neoforge-client-' + [Guid]::NewGuid().ToString('N'))
$game = Join-Path $scratch 'neoforge'
New-Item -ItemType Directory -Force "$game/mods" | Out-Null
New-Item -ItemType Junction -Path "$game/libraries" -Target "$runtime/.cache/server/libraries" | Out-Null
foreach ($jar in @(
    "$FerrumRoot/tests/mods/candidates/Draconic-Next-neoforge-1.0.0-alpha.1.jar",
    "$FerrumRoot/tests/mods/candidates/hbm-neoforge-1.0.3.jar",
    "$FerrumRoot/tests/mods/candidates/crankshaft-neoforge-1.6.1+mc26.2.jar",
    "$FerrumRoot/tests/mods/ferrum-testmod/build/libs/ferrum-testmod-1.0.0.jar",
    "$runtime/build/libs/ferrum-neoforge-runtime-1.0.0.jar"
)) {
    Copy-Item -LiteralPath $jar -Destination "$game/mods"
}
@{ gameDirectory = $game; reportPath = "$scratch/loader-report.json" } |
    ConvertTo-Json | Set-Content "$scratch/runtime.json" -Encoding utf8NoBOM
@{ enabled = $true; launch = "$runtime/.cache/jvm.json"; runtime = "$scratch/runtime.json" } |
    ConvertTo-Json | Set-Content "$scratch/ferrum-neoforge.json" -Encoding utf8NoBOM
@"
seed = "1"
allow_nether = false
allow_end = false
spawn_protection = 0
[networking.java]
address = "127.0.0.1:$Port"
online_mode = false
encryption = false
view_distance = $ViewDistance
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
if ($VanillaOnly) {
    ((Get-Content "$scratch/ferrummc.toml" -Raw) -replace '(\[networking\.bedrock\]\s+)enabled = false', '${1}enabled = true' -replace '(\[networking\.lan_broadcast\]\s+)enabled = false', '${1}enabled = true') |
        Set-Content "$scratch/ferrummc.toml" -Encoding utf8NoBOM
    Add-Content "$scratch/ferrummc.toml" "`n[networking.java.compression]`nenabled = false"
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
$listener = [Net.Sockets.TcpListener]::new([Net.IPAddress]::Loopback, $Port)
try { $listener.Start() } finally { $listener.Stop() }

function Send-Console($process, [string] $command) {
    $process.StandardInput.WriteLine($command)
    $process.StandardInput.Flush()
}

# The probe client boots its own NeoForge runtime with the same mods, as a modded client would.
$clientGame = Join-Path $scratch 'client'
New-Item -ItemType Directory -Force "$clientGame/mods" | Out-Null
New-Item -ItemType Junction -Path "$clientGame/libraries" -Target "$runtime/.cache/server/libraries" | Out-Null
Get-ChildItem "$game/mods" | Copy-Item -Destination "$clientGame/mods"
@{ gameDirectory = $clientGame; reportPath = "$scratch/client-loader-report.json" } |
    ConvertTo-Json | Set-Content "$scratch/client-runtime.json" -Encoding utf8NoBOM
$launch = Get-Content "$runtime/.cache/jvm.json" -Raw | ConvertFrom-Json
$vmArgs = @($launch.vmOptions | ForEach-Object {
    if ($_.StartsWith('-Djava.class.path=')) { $_ + ';' + "$runtime/build/libs/ferrum-neoforge-runtime-1.0.0-smoke.jar".Replace('\', '/') } else { $_ }
})
$vmArgs += @('net.solarspire.ferrum.runtime.smoke.ClientAcceptance', "$scratch/client-runtime.json".Replace('\', '/'),
    "$runtime/build/libs/ferrum-neoforge-runtime-1.0.0-client-probe.jar".Replace('\', '/'), '127.0.0.1', "$Port", $Block, '0', '100', '0',
    "$scratch/client-report.json".Replace('\', '/'), $PlaceItem, $GunItem)
$vmArgs | ForEach-Object { '"' + $_ + '"' } | Set-Content "$scratch/client-args.txt" -Encoding utf8NoBOM

$process = [Diagnostics.Process]::new()
$process.StartInfo.FileName = "$FerrumRoot/target/debug/ferrummc.exe"
$process.StartInfo.WorkingDirectory = $scratch
$process.StartInfo.UseShellExecute = $false
$process.StartInfo.CreateNoWindow = $true
$process.StartInfo.RedirectStandardInput = $true
$process.StartInfo.RedirectStandardOutput = $true
$process.StartInfo.RedirectStandardError = $true
$output = [FerrumBridgeOutput]::new($process)
$started = $false
$passed = $false
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
    if ($VanillaOnly) {
        $probe = Join-Path $FerrumRoot 'target/ferrummc-smoke.exe'
        & rustc --edition=2024 "$FerrumRoot/tests/smoke/client.rs" -o $probe
        if ($LASTEXITCODE -ne 0) { throw 'Smoke client compilation failed' }
        & $probe --self-test
        if ($LASTEXITCODE -ne 0) { throw 'Smoke codec self-test failed' }
        & $probe '127.0.0.1' $Port --reject-vanilla
        if ($LASTEXITCODE -ne 0) { throw 'Vanilla admission rejection failed' }
        if ($text.Contains('Bedrock server-list status is listening') -or $text.Contains('LAN broadcast is enabled')) {
            throw 'Unsupported Bedrock/LAN listener policy failed'
        }
        $passed = $true
        return
    }
    Send-Console $process 'execute in minecraft:overworld run forceload add 0 0'
    $loaded = $false
    while ([DateTime]::UtcNow -lt $deadline -and !$process.HasExited) {
        $loadLine = $output.Lines.Count
        Send-Console $process 'ferrum be 0 100 0'
        Start-Sleep -Milliseconds 250
        $loadText = ($output.Lines.ToArray() | Select-Object -Skip $loadLine) -join "`n"
        if ($loadText -match 'No block entity at this position|Counter["'']?\s*[:=]|id["'']?\s*[:=]') { $loaded = $true; break }
    }
    if (!$loaded) { throw 'Force-loaded acceptance chunk did not become available' }
    Send-Console $process 'setworldspawn 0 101 0'
    Send-Console $process "setblock 0 100 0 $Block"
    Start-Sleep -Milliseconds 1000
    $env:FERRUM_PROBE_VIEW_DISTANCE = "$ViewDistance"
    $client = Start-Process -FilePath "$env:JAVA_HOME/bin/java.exe" -ArgumentList ('@"' + "$scratch/client-args.txt" + '"') -WorkingDirectory $scratch -WindowStyle Hidden -PassThru -RedirectStandardOutput "$scratch/client.stdout.log" -RedirectStandardError "$scratch/client.stderr.log"
    # Once the client is in play, drop a leaves item next to it and chat, so entity data and chat get translated.
    $clientDeadline = [DateTime]::UtcNow.AddSeconds($TimeoutSeconds)
    $sent = 0
    $zombie = $false
    while (!$client.HasExited -and [DateTime]::UtcNow -lt $clientDeadline) {
        if ($sent -lt 20 -and (Test-Path "$scratch/client.stdout.log") -and (Select-String -Path "$scratch/client.stdout.log" -Pattern 'play <- chunk' -Quiet)) {
            Send-Console $process 'summon item 0 101 0 {Item:{id:"minecraft:oak_leaves",count:1}}'
            Send-Console $process 'say probe-chat'
            Send-Console $process 'gamemode creative NeoProbe'
            Send-Console $process 'tp NeoProbe 0.5 101 2.5'
            if ($GunItem -and -not $zombie) { Send-Console $process "summon zombie 0.5 101 9.5 {NoAI:1b}"; $zombie = $true }
            $sent++
        }
        Start-Sleep -Milliseconds 1000
    }
    if (!$client.HasExited) {
        Stop-Process -Id $client.Id -Force
        throw "Client probe timed out; see $scratch/client.stdout.log"
    }
    if ($client.ExitCode -ne 0) { throw "Client probe exited $($client.ExitCode); see $scratch/client.stdout.log" }
    $report = Get-Content "$scratch/client-report.json" -Raw | ConvertFrom-Json
    if (!$report.collectedGameRegistries -or $report.jukeboxSongs -notcontains "hbm:lc") { throw "Client registry collection failed or hbm:lc missing from jukebox_song" }
    if (!$report.negotiated -or $report.frozenRegistries -lt 1 -or $report.registryMismatches -ne 0) { throw 'NeoForge negotiation or registry sync incomplete' }
    if ($report.actualState -ne $report.expectedState) { throw 'Chunk state mismatch' }
    if ($report.droppedItem -ne 'minecraft:oak_leaves') { throw "Dropped item entity data decoded as '$($report.droppedItem)', expected minecraft:oak_leaves" }
    if ($report.chat -notmatch 'probe-chat') { throw "Chat message did not decode on the client: '$($report.chat)'" }
    if ($PlaceItem -and $report.placedBlock -ne $PlaceItem) { throw "Placing $PlaceItem produced '$($report.placedBlock)'" }
    if ($PlaceItem -and (!$report.broken -or !$report.dropped)) { throw 'Breaking or dropping the placed modded block item failed' }
    if ($PlaceItem -eq 'hbm:machine_radar') {
        $radar = $report.menus.placed
        if (!$radar.opened -or $radar.menuType -ne 'hbm:machine_radar') { throw "Right-clicking the radar did not open its Java menu: $($radar | ConvertTo-Json -Compress)" }
        if ($radar.extraPos -notmatch 'x=1, y=100, z=0') { throw "The radar's advanced open-screen data carried '$($radar.extraPos)'" }
        if (@($radar.hbmPayloads).Count -lt 1) { throw 'The radar menu got no HBM block entity sync payload' }
        if (!$radar.closed) { throw 'The radar menu probe did not finish' }
    }
    if ($Block -eq 'hbm:machine_electric_furnace') {
        $machine = $report.menus.block
        if (!$machine.opened -or $machine.menuType -ne 'hbm:machine_electric_furnace' -or $machine.contentSlots -lt 36) { throw "Right-clicking $Block did not open its menu with content: $($machine | ConvertTo-Json -Compress)" }
        if (!$machine.clickAck0 -or !$machine.clickAck1) { throw "Container clicks on the Java menu were not answered: $($machine | ConvertTo-Json -Compress)" }
        if ($machine.thrownItemEntity -lt 1) { throw 'Throwing a stack out of the Java menu spawned no item entity' }
        if (!$machine.closed) { throw 'The machine menu probe did not finish' }
    }
    if ($GunItem) {
        if (!$report.gunStateChanged) { throw "Gun $GunItem stack state did not change after its keybinds: '$($report.gunInitialData)' -> '$($report.gunData)'" }
        if (@($report.gunPayloads).Count -lt 1) { throw "No hbm clientbound payload reached the client after the $GunItem keybinds" }
        if (!$report.gunComponent) { throw "The client never saw hbm:gun_state as a typed component on $GunItem" }
        if ($report.bulletType -notmatch "^hbm:") { throw "No hbm bullet entity was added on the client (bulletType=$($report.bulletType))" }
        if ($report.bulletMoves -lt 1 -or !$report.bulletData) { throw "The bullet did not move or carry data on the client (moves=$($report.bulletMoves) data=$($report.bulletData))" }
        if (!$report.bulletRemoved) { throw "The bullet was never removed on the client" }
        if (!$report.zombieDamaged) { throw "The zombie in the bullet's line was never hurt (zombieId=$($report.zombieId))" }
        if (!$report.gunSound -and !$report.gunParticles) { throw "No sound or particle packet reached the client for $GunItem" }
        $serverText = $output.Lines.ToArray() -join "`n"
        if ($serverText -match "NeoForge entity pass failed|failed its tick|Mod player event .* failed") { throw "Server logged a bridge failure while the bullet flew" }
    }
    Start-Sleep -Milliseconds 500
    if ($process.HasExited) { throw 'Server exited during the client probe' }
    $passed = $true
    Write-Output "PASS gunPayloads=$(@($report.gunPayloads) -join ',') gunData=$($report.gunData) block=$Block state=$($report.actualState) frozenRegistries=$($report.frozenRegistries) jukeboxSongs=$($report.jukeboxSongs.Count) globalBits=$($report.globalStateBits)"
} finally {
    if ($started -and !$process.HasExited) {
        Send-Console $process 'stop'
        $process.StandardInput.Close()
        if (!$process.WaitForExit(30000)) { $process.Kill($true) }
    }
    if ($started) { $output.Lines.ToArray() | Set-Content "$scratch/server.log" }
    if ($VanillaOnly -and $passed -and $process.ExitCode -ne 0) { throw 'Server did not stop cleanly' }
    $process.Dispose()
    Write-Output "Client acceptance evidence: $scratch (passed=$passed)"
}
