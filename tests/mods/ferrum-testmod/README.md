# Ferrum validation mod

A normal NeoForge mod for **Minecraft 26.2 / protocol 776**, pinned to **NeoForge
26.2.0.93** (latest published 26.2 build checked 2026-10-29). It uses ModDevGradle
2.0.148, Gradle 9.2.1 and Java 25. It has no FerrumMC dependency. Use the same built
JAR on stock NeoForge and later on FerrumMC as the ground-truth comparison.

## Build and automated tests

From this directory in PowerShell:

```powershell
$env:PATH = "$env:USERPROFILE\.cargo\bin;C:\Program Files\Eclipse Adoptium\jdk-25.0.2.10-hotspot\bin;$env:PATH"
$env:JAVA_HOME = 'C:\Program Files\Eclipse Adoptium\jdk-25.0.2.10-hotspot'
$env:GRADLE_USER_HOME = "$PWD\.gradle-user-home"
$env:TEMP = "$PWD\.tmp"
$env:TMP = $env:TEMP
New-Item -ItemType Directory -Force .tmp,.home | Out-Null
$env:JAVA_TOOL_OPTIONS = "-Duser.home=$PWD\.home -Djava.io.tmpdir=$PWD\.tmp"
.\gradlew.bat build
.\gradlew.bat runGameTestServer
```

On Unix, use Java 25, set the equivalent local cache/temp/home variables, and run
`./gradlew build` / `./gradlew runGameTestServer`. The build's native helper also uses
Java 25, so no additional Java 21 installation is needed. Initial setup downloads
Minecraft/NeoForge libraries and client assets; keep caches, prepared Mojang JARs,
server worlds and logs out of source control and distributions.

Output: **`build/libs/ferrum-testmod-1.0.0.jar`**. This contains only this mod's code,
resources and GPL license, with references to Minecraft's existing copper-block texture.
There are no shaded Minecraft/NeoForge classes or copied vanilla textures.

Two mod game tests run against real NeoForge/Minecraft:

- `ferrum_testmod:ticks_once`: a placed counter advances exactly five times in five ticks.
- `ferrum_testmod:persists_inventory`: resolve the one-slot capability; abort an insertion
  and verify no items remain; commit seven iron ingots; serialize/load a fresh BE and
  verify the counter and items; check the client update tag contains the counter.

The launcher also runs Minecraft's built-in test, so the observed total is three.
The tests do not establish real packet delivery, rendering or player protection behavior.

## Mod behavior

- `ferrum_testmod:counter_block`: cube block, block item and Functional Blocks tab entry.
- BE type `ferrum_testmod:counter_block`: server-only ticker increments `Counter` once
  each active simulation tick, marks dirty every tick, and requests a BE packet every 20 ticks.
- Disk NBT has `Counter` (long) and `Inventory` (one-slot resource-handler serialization).
  Contents drop when the block is removed; the dropped block item does not retain the counter.
- Client chunk update tags and `getUpdatePacket()` carry `Counter` only. Clients do not
  independently increment it. The private inventory is omitted from client update NBT.
- `Capabilities.Item.BLOCK` exposes the slot from all sides, using NeoForge 26.2's current
  `ItemStacksResourceHandler` and transactional transfer API. The older `ItemStackHandler`
  is deprecated; this mod intentionally uses the supported item-handler capability.
- Empty-hand right-click sends the clicking player a custom S2C payload on
  `ferrum_testmod:counter`, required channel version `1`. Its codec is BlockPos followed by
  VAR_LONG. Client chat reports `server=<value>, client=<last BE snapshot>` so delivery
  of the payload and BE updates can be checked independently. The snapshot may trail the
  server by up to a sync interval plus transport time. A vanilla client is unsupported.

## Run a normal dedicated NeoForge server

Keep this scratch installation under `run/stock` to contain every generated file.
Use a fresh directory/world if repeating the exact five-tick check.

```powershell
New-Item -ItemType Directory -Force run/stock | Out-Null
Invoke-WebRequest 'https://maven.neoforged.net/releases/net/neoforged/neoforge/26.2.0.93/neoforge-26.2.0.93-installer.jar' -OutFile run/stock/neoforge-installer.jar
Push-Location run/stock
java -jar neoforge-installer.jar --installServer .
New-Item -ItemType Directory -Force mods | Out-Null
Copy-Item ../../build/libs/ferrum-testmod-1.0.0.jar mods/
# Review Minecraft's EULA and set eula=true in eula.txt to run this server.
# For a local console-only check, set these in server.properties:
# server-ip=127.0.0.1
# server-port=0
# use-native-transport=false
# pause-when-empty-seconds=0
java --enable-native-access=ALL-UNNAMED -Xmx2G '@libraries/net/neoforged/neoforge/26.2.0.93/win_args.txt' --nogui
Pop-Location
```

Wait for `Done`. Run these console commands, pausing between them as indicated:

```text
forceload add 0 0
```

Let the server run for two seconds so the forced chunk becomes fully ticking. Then:

```text
tick freeze
setblock 0 100 0 ferrum_testmod:counter_block
tick step 5
```

Wait one second, then run:

```text
data get block 0 100 0 Counter
save-all flush
stop
```

Expect `5L` in a fresh block. Restart with the same mod/world, freeze promptly, and query
again: expect at least `5L`, never reset to zero. A previously placed block must first be
removed/replaced to reset its counter. Frozen ticks and unloaded/non-simulated chunks do not
advance it. Game tests provide the deterministic cadence check without console timing.

The exact built JAR was installed and booted headlessly on stock NeoForge on 2026-10-29.
The console check returned `5L`, saved/flushed, stopped cleanly, restarted and returned
`5L` again. Both launches reached `Done` and exited normally. Logs are retained locally in
`run/stock/boot.log` and `restart.log`; Gradle results are in `validation.log`.
The final launches had no ERROR lines or mod exception. They did emit environment/library
warnings: unavailable terminal features, Windows PerfOS counter registry type, and JOML's
deprecated Unsafe use. Native transport was disabled for this Windows console test.

Verified JAR SHA-256:

```text
E6C0205DE02CE21800C858BF863823D09504FD6B241DC73F701930B5F9FCE64A
```

## Required real-client checks

Install NeoForge 26.2.0.93 and this exact JAR on a normal 26.2 client and server. For client
testing, use the server's normal chosen port instead of `server-port=0`. `gradlew runClient`
is also available for a development client. Run the following with two clients:

1. Find Ferrum Counter Block in Functional Blocks, or use
   `/give @s ferrum_testmod:counter_block`. Place it and expect a copper-textured cube
   with its translated item name.
2. Right-click with empty hands. Expect chat containing a rising server counter and a
   nonzero client BE snapshot after roughly one second. Repeated clicks should show the
   snapshot updating in steps; this checks actual BE packets separately from the custom payload.
3. Have the second client join after the block has ticked. Its first empty-hand click
   should report the existing client snapshot, proving the chunk's initial update tag loaded.
4. Put a hopper containing seven iron ingots above the block. Expect insertion through
   the capability; `/data get block <x> <y> <z> Inventory` should show them. Save, restart,
   and check they remain. Break the block and expect the stored items plus the block drop
   in survival. No duplicate items should appear.
5. Remove force-loading and move both clients beyond simulation distance. Return and
   expect the counter to resume from its saved value, without advancing for every wall-clock
   tick spent unloaded. This needs a before/after sample plus known active-tick timing.
6. Attempt joining without the mod or with a mismatched required payload registration.
   Expect configuration rejection, not a successful play connection with missing blocks.

These client checks **have not been run**. Before opening a gameplay PR, the human author
must run them with a real client, attach a recording showing the block, two-client/late-join
counter messages, hopper inventory and restart, and open the PR themselves. Agents do not
open PRs. Capture: <attach screenshot or recording here>.

Bridge implementation and protection-plugin acceptance checks are specified in
[`docs/NEOFORGE_BRIDGE.md`](../../../docs/NEOFORGE_BRIDGE.md). No FerrumMC bridge runtime was
implemented or exercised as part of this mod build.

## Provenance and license

GPL-3.0-only; see [LICENSE](LICENSE). The wrapper and template setup were obtained from
[NeoForge's official 26.2 MDK](https://github.com/NeoForgeMDKs/MDK-26.2-ModDevGradle/tree/a0340c5454c01e43ccf83977601a4d90afee6e1f);
the template's license is retained as `TEMPLATE_LICENSE.txt`. Runtime Minecraft artifacts
are downloaded locally and must not be redistributed with this mod or FerrumMC.
