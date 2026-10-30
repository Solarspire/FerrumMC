param([int] $Port = 25579, [int] $TimeoutSeconds = 180, [switch] $SkipBuild)
. "$PSScriptRoot/env.ps1"
if ($Port -lt 1024 -or $Port -gt 65535) { throw 'Use an unprivileged TCP port.' }
$runtime = Join-Path $FerrumRoot 'bridge/neoforge-runtime'
if (!$SkipBuild) {
    & "$runtime/gradlew.bat" -p $runtime --console=plain assemble
    if ($LASTEXITCODE -ne 0) { throw 'Runtime build failed' }
    & cargo build --locked -p pumpkin
    if ($LASTEXITCODE -ne 0) { throw 'FerrumMC build failed' }
}
$scratch = Join-Path $FerrumRoot ('run/neoforge-bridge-' + [Guid]::NewGuid().ToString('N'))
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
$result = @{ passed = $false; scratch = $scratch; counterBefore = $null; counterAfter = $null; exceptions = 0; optionalMixinClassWarnings = 0; javaExceptionLines = @() }

function Send-Console($process, [string] $command) {
    $process.StandardInput.WriteLine($command)
    $process.StandardInput.Flush()
}

function Read-Counter($process, $output) {
    $firstLine = $output.Lines.Count
    Send-Console $process 'ferrum be 0 100 0'
    $deadline = [DateTime]::UtcNow.AddSeconds(5)
    while ([DateTime]::UtcNow -lt $deadline -and !$process.HasExited) {
        $text = ($output.Lines.ToArray() | Select-Object -Skip $firstLine) -join "`n"
        if ($text -match 'Counter["'']?\s*[:=]\s*(\d+)') { return [long]$Matches[1] }
        Start-Sleep -Milliseconds 100
    }
    throw 'No counter NBT returned by ferrum be; inspect placement and command errors in the server log'
}

function Invoke-BridgeServer([bool] $restart) {
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
    try {
        $started = $process.Start()
        if (!$started) { throw 'Server process did not start' }
        $process.BeginOutputReadLine()
        $process.BeginErrorReadLine()
        $deadline = [DateTime]::UtcNow.AddSeconds($TimeoutSeconds)
        $ready = $false
        while ([DateTime]::UtcNow -lt $deadline -and !$process.HasExited) {
            $text = $output.Lines.ToArray() -join "`n"
            if ($text.Contains('Server is now running.') -and $text.Contains('NeoForge runtime attached; frozen registries ready')) {
                $ready = $true
                break
            }
            Start-Sleep -Milliseconds 100
        }
        if (!$ready) { throw 'Rust server or NeoForge runtime did not become ready' }
        Send-Console $process 'execute in minecraft:overworld run forceload add 0 0'
        $loadDeadline = [DateTime]::UtcNow.AddSeconds($TimeoutSeconds)
        $loaded = $false
        while ([DateTime]::UtcNow -lt $loadDeadline -and !$process.HasExited) {
            $loadLine = $output.Lines.Count
            Send-Console $process 'ferrum be 0 100 0'
            Start-Sleep -Milliseconds 250
            $loadText = ($output.Lines.ToArray() | Select-Object -Skip $loadLine) -join "`n"
            if ($loadText -match 'No block entity at this position|Counter["'']?\s*[:=]') { $loaded = $true; break }
        }
        if (!$loaded) { throw 'Force-loaded acceptance chunk did not become available' }
        if (!$restart) { Send-Console $process 'setblock 0 100 0 ferrum_testmod:counter_block' }
        Start-Sleep -Milliseconds 2000
        $counter = Read-Counter $process $output
        if (!$restart) {
            if ($counter -lt 20 -or $counter -gt 100) { throw "Counter after ~40 ticks: $counter" }
            $result.counterBefore = $counter
            Send-Console $process 'setblock 2 100 0 draconicnext:generator'
            Send-Console $process 'setblock 4 100 0 hbm:machine_electric_furnace'
            Send-Console $process 'setblock 6 100 0 hbm:machine_radar'
        } else {
            $result.counterAfter = $counter
            if ($counter -le $result.counterBefore) { throw 'Counter did not survive restart and continue ticking' }
        }
        Start-Sleep -Milliseconds 10000
        foreach ($position in @(2, 4, 6)) { Send-Console $process "ferrum be $position 100 0" }
        Start-Sleep -Milliseconds 250
        $text = $output.Lines.ToArray() -join "`n"
        foreach ($namespace in @('draconicnext:', 'hbm:')) {
            if ($text -notmatch ('id["'']?\s*[:=]\s*["'']?' + [regex]::Escape($namespace))) {
                throw "Missing candidate block entity NBT: $namespace"
            }
        }
        # Removing a block entity runs mod removal hooks that ask the level for its server.
        if (!$restart) { Send-Console $process 'setblock 6 100 0 minecraft:air'; Start-Sleep -Milliseconds 1500 }
        if (!$restart) { $result.counterBefore = Read-Counter $process $output }
    } finally {
        if ($started -and !$process.HasExited) {
            Send-Console $process 'stop'
            $process.StandardInput.Close()
            if (!$process.WaitForExit(30000)) { $process.Kill($true) }
        }
        if ($started) {
            $process.WaitForExit()
            $lines = $output.Lines.ToArray()
            $lines | Set-Content "$scratch/server-$restart.log"
            $exceptionLines = @($lines | Where-Object { $_ -match '^(?:Exception in thread|Caused by:)|java\.[\w.]+Exception:' })
            $result.javaExceptionLines += $exceptionLines
            $optionalWarnings = @($exceptionLines | Where-Object { $_ -match '\[mixin/\]: Error loading class:.*java.lang.ClassNotFoundException:' })
            $result.optionalMixinClassWarnings += $optionalWarnings.Count
            $result.exceptions += $exceptionLines.Count - $optionalWarnings.Count
            if ($process.ExitCode -ne 0) { throw "Server exited $($process.ExitCode); see $scratch" }
        }
        $process.Dispose()
    }
}

try {
    Invoke-BridgeServer $false
    Invoke-BridgeServer $true
    if ($result.exceptions -ne 0) { throw "Java exceptions: $($result.exceptions)" }
    $result.passed = $true
    Write-Output "PASS counter=$($result.counterBefore) restart=$($result.counterAfter) candidates=3 ticks>=200 exceptions=0"
} catch {
    $result.error = $_.Exception.Message
    throw
} finally {
    $result | ConvertTo-Json | Set-Content "$scratch/acceptance.json" -Encoding utf8NoBOM
    Write-Output "Acceptance evidence: $scratch"
}
