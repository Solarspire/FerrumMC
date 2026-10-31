param([int] $Port = 25581, [string] $Block = 'hbm:machine_electric_furnace', [string] $PlaceItem = 'hbm:machine_radar', [string] $GunItem = 'hbm:gun_greasegun', [string] $CrateItem = '', [int] $TimeoutSeconds = 180, [int] $ViewDistance = 2, [switch] $SkipBuild, [switch] $VanillaOnly)
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
    "$scratch/client-report.json".Replace('\', '/'), $PlaceItem, $GunItem, $CrateItem)
$vmArgs | ForEach-Object { '"' + $_ + '"' } | Set-Content "$scratch/client-args.txt" -Encoding utf8NoBOM

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
    if ($CrateItem) {
        # A placed crate for the probe to open, break and replace.
        Send-Console $process 'setblock 3 100 3 minecraft:stone'
        Send-Console $process "setblock 3 101 3 $CrateItem"
    }
    if ($GunItem) {
        # The test mod's counter feeds the vanilla chest above it through the item capability.
        Send-Console $process 'setblock 6 100 6 ferrum_testmod:counter_block'
        Send-Console $process 'setblock 6 101 6 minecraft:chest'
    }
    Start-Sleep -Milliseconds 1000
    $env:FERRUM_PROBE_VIEW_DISTANCE = "$ViewDistance"
    $client = Start-Process -FilePath "$env:JAVA_HOME/bin/java.exe" -ArgumentList ('@"' + "$scratch/client-args.txt" + '"') -WorkingDirectory $scratch -WindowStyle Hidden -PassThru -RedirectStandardOutput "$scratch/client.stdout.log" -RedirectStandardError "$scratch/client.stderr.log"
    # Once the client is in play, drop a leaves item next to it and chat, so entity data and chat get translated.
    $clientDeadline = [DateTime]::UtcNow.AddSeconds($TimeoutSeconds)
    $sent = 0
    $zombie = $false
    $meteorSent = $false
    $dumpsSent = 0
    $sentLimit = if ($CrateItem) { 6 } else { 20 }
    while (!$client.HasExited -and [DateTime]::UtcNow -lt $clientDeadline) {
        if ($sent -lt $sentLimit -and (Test-Path "$scratch/client.stdout.log") -and (Select-String -Path "$scratch/client.stdout.log" -Pattern 'play <- chunk' -Quiet)) {
            Send-Console $process 'summon item 0 101 0 {Item:{id:"minecraft:oak_leaves",count:1}}'
            Send-Console $process 'say probe-chat'
            Send-Console $process 'gamemode creative NeoProbe'
            Send-Console $process 'op NeoProbe'
            Send-Console $process 'tp NeoProbe 0.5 101 2.5'
            if ($GunItem -and -not $meteorSent) { Send-Console $process 'ferrum summon 40 130 8 hbm:entity_meteor'; $meteorSent = $true }
            if ($GunItem -and -not $zombie) { Send-Console $process "summon husk 1.5 101 2.5 {NoAI:1b}"; Send-Console $process "summon husk 0.5 101 9.5 {NoAI:1b}"; $zombie = $true }
            $sent++
        }
        if (Test-Path "$scratch/client.stdout.log") {
            # The probe asks for the server's copy of a slot by printing "DUMP <slot>".
            $dumps = @(Select-String -Path "$scratch/client.stdout.log" -Pattern '^DUMP (\d+)')
            while ($dumpsSent -lt $dumps.Count) { Send-Console $process "ferrum item NeoProbe $($dumps[$dumpsSent].Matches[0].Groups[1].Value)"; $dumpsSent++ }
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
    if ($Block -eq 'hbm:machine_electric_furnace' -and $PlaceItem) {
        $machine = $report.menus.block
        if (!$machine.opened -or $machine.menuType -ne 'hbm:machine_electric_furnace' -or $machine.contentSlots -lt 36) { throw "Right-clicking $Block did not open its menu with content: $($machine | ConvertTo-Json -Compress)" }
        if (!$machine.clickAck0 -or !$machine.clickAck1) { throw "Container clicks on the Java menu were not answered: $($machine | ConvertTo-Json -Compress)" }
        if ($machine.thrownItemEntity -lt 1) { throw 'Throwing a stack out of the Java menu spawned no item entity' }
        if (!$machine.closed) { throw 'The machine menu probe did not finish' }
    }
    if ($Block -eq 'hbm:machine_electric_furnace' -and $PlaceItem) {
        $rejoin = $report.rejoin
        if (!$report.carriedAtDisconnect) { throw 'The carry probe never got the stack onto the cursor of the open machine menu' }
        if (!$rejoin.carriedBack) { throw "The stack carried in the machine menu at disconnect was lost on rejoin: $($rejoin | ConvertTo-Json -Compress)" }
        if (!$rejoin.machineOpened -or $rejoin.machineFilledSlots -lt 1) { throw "The machine could not be filled after rejoin: $($rejoin | ConvertTo-Json -Compress)" }
        if ($rejoin.machineDrops -lt 1) { throw "Breaking the filled machine dropped none of its items: $($rejoin | ConvertTo-Json -Compress)" }
    }
    if ($CrateItem) {
        $crate = $report.crate
        if (!$crate.heldOpened) { throw "Using the held crate item did not open its menu: $($crate | ConvertTo-Json -Compress)" }
        if (!$crate.heldInserted -or $crate.heldReopenedStone -lt 1) { throw "Items put into the held crate were gone after close and reopen: $($crate | ConvertTo-Json -Compress)" }
        if (!$crate.blockOpened) { throw "Right-clicking the placed crate did not open its menu: $($crate | ConvertTo-Json -Compress)" }
        if (!$crate.blockInserted -or $crate.blockReopenedStone -lt 1) { throw "Items put into the placed crate were gone after close and reopen: $($crate | ConvertTo-Json -Compress)" }
        if (!$crate.blockBroken -or !$crate.droppedCratePickedUp -or !$crate.droppedCrateHasStone) { throw "Breaking the filled crate did not drop an item holding its contents: $($crate | ConvertTo-Json -Compress)" }
        if (!$crate.blockPlacedAgain -or $crate.placedAgainStone -lt 1) { throw "Placing the dropped crate item did not restore its contents: $($crate | ConvertTo-Json -Compress)" }
    }
    if ($GunItem) {
        if (!$report.gunStateChanged) { throw "Gun $GunItem stack state did not change after its keybinds: '$($report.gunInitialData)' -> '$($report.gunData)'" }
        if (@($report.gunPayloads).Count -lt 1) { throw "No hbm clientbound payload reached the client after the $GunItem keybinds" }
        if (!$report.gunComponent) { throw "The client never saw hbm:gun_state as a typed component on $GunItem" }
        if ($report.bulletType -notmatch "^hbm:") { throw "No hbm bullet entity was added on the client (bulletType=$($report.bulletType))" }
        if (($report.bulletMoves -lt 1 -and !$report.zombieDamaged) -or !$report.bulletData) { throw "The bullet did not move or carry data on the client (moves=$($report.bulletMoves) data=$($report.bulletData))" }
        if (!$report.bulletRemoved) { throw "The bullet was never removed on the client" }
        if (!$report.zombieDamaged) { throw "The zombie in the bullet's line was never hurt (zombieId=$($report.zombieId))" }
        if (!$report.gunSound -and !$report.gunParticles) { throw "No sound or particle packet reached the client for $GunItem" }
        $armorLine = $output.Lines.ToArray() | Where-Object { $_ -match 'slot 38:' } | Select-Object -Last 1
        if (!$report.armorProbed -or !$armorLine -or $armorLine -notmatch 'draconicnext:draconic_chest' -or $armorLine -notmatch 'portable_energy') { throw "Charged Draconic armor lost its item or energy on the server: $armorLine" }
        if (!($report.meleeSwordDamage -gt $report.meleeFistDamage)) { throw "Draconic sword did no more damage than a fist (fist=$($report.meleeFistDamage) sword=$($report.meleeSwordDamage))" }
        if (!$report.toolTargetBroken -or $report.toolBlocksBroken -lt 2 -or $report.toolItemDrops -lt 1) { throw "Draconic staff of power did not break its area and drop items (target=$($report.toolTargetBroken) blocks=$($report.toolBlocksBroken) drops=$($report.toolItemDrops))" }
        if (!($report.spearLunge -gt 0.3)) { throw "A jab with the charged Draconic spear did not lunge the player (push=$($report.spearLunge))" }
        if (!$report.modMobHurt) { throw "Punching the hbm:entity_test_dummy did not hurt it through Java (modMobId=$($report.modMobId))" }
        if (!$report.modCraft) { throw 'Putting hbm:asphalt_light in the inventory crafting grid did not offer the mod recipe result hbm:asphalt' }
        if (!$report.creativeEnchant) { throw "A creative slot with an enchanted stack was not accepted (creativeEnchant=$($report.creativeEnchant))" }
        if (!$report.maceCooldown -or !$report.maceSound) { throw "Using the Draconic mace in the air did not run its meteor strike (cooldown=$($report.maceCooldown) sound=$($report.maceSound))" }
        if (@($report.decodeFailures).Count -gt 0) { throw "The client could not decode packets: $(@($report.decodeFailures) -join '; ')" }
        if (!($report.armorShieldMax -gt 0)) { throw "The client never saw a charged armor shield on the worn armor (armorShieldMax=$($report.armorShieldMax))" }
        if (!$report.armorShieldHit -or ($report.armorHealthAfterHit -ge 0 -and $report.armorHealthAfterHit -lt 20)) { throw "A hit on the charged Draconic shield was not absorbed (shieldHit=$($report.armorShieldHit) health=$($report.armorHealthAfterHit))" }
        if ($armorLine -notmatch 'armor_shield') { throw "Draconic armor shield never charged on the server: $armorLine" }
        # The falling meteor must finish its impact through the bridge and leave the meteorite's blocks.
        $meteor = 0
        for ($i = 0; $i -lt 40 -and $meteor -lt 1; $i++) {
            foreach ($y in 36, 43, 50, 57, 64, 71) {
                $meteorLine = $output.Lines.Count
                Send-Console $process "ferrum blocks 40 $y 8 7 hbm:block_meteor"
                Start-Sleep -Milliseconds 300
                $meteorText = ($output.Lines.ToArray() | Select-Object -Skip $meteorLine) -join "`n"
                if ($meteorText -match 'Blocks hbm:block_meteor: (\d+)') { $meteor += [int]$Matches[1] }
            }
            if ($meteor -lt 1) { Start-Sleep -Milliseconds 1000 }
        }
        if ($meteor -lt 1) {
            $meteorLine = $output.Lines.Count
            Send-Console $process 'ferrum count hbm:entity_meteor'
            Start-Sleep -Milliseconds 500
            $left = ($output.Lines.ToArray() | Select-Object -Skip $meteorLine) -join ' '
            throw "The hbm meteor landed without leaving any hbm:block_meteor blocks ($left)"
        }
        $chestLine = $output.Lines.Count
        Send-Console $process 'ferrum be 6 101 6'
        Start-Sleep -Milliseconds 1000
        $chestText = ($output.Lines.ToArray() | Select-Object -Skip $chestLine) -join "`n"
        if ($chestText -notmatch 'minecraft:stick') { throw "A mod block entity's insert into the vanilla chest above it never reached Rust: $chestText" }
        $serverText = $output.Lines.ToArray() -join "`n"
        if ($serverText -match "NeoForge entity pass failed|failed its tick|Mod player event .* failed|Mod (player tick|inventory tick in slot \d+) failed|A mod listener failed|NeoForge player step failed|NeoForge server tick failed") { throw "Server logged a bridge failure while the bullet flew" }
        if ($serverText -match 'Ignoring unreadable|Dropping client item component') { throw 'The server could not read a client item stack' }
        if ($serverText -match 'PlayerAdvancements|environmentAttributes" is null|Generation view: ') { throw 'A stand-in server, level or generation view lacked state mod code needed' }
        if ($serverText -match 'getCurrentServer\(\)" is null') { throw 'HBM could not find the server (ServerLifecycleHooks.getCurrentServer is null)' }
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
