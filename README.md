# FerrumMC

FerrumMC is a Rust-native hybrid Minecraft server: Pumpkin owns simulation and storage, while embedded PatchBukkit supplies a JVM for Bukkit/Paper plugins. It is a prototype, not a claim of complete Bukkit or mod compatibility.

FerrumMC runs a **native Java 26.2 engine / protocol 776** and accepts NeoForge 26.2 clients with the server's mods. Engine data, registries, packets and codecs use native identities. The embedded JVM runs real mod jars through Rust-owned world views; Java never independently simulates or saves another world. Existing worlds require reset after the native-ID migration.

## Build and run (Windows)

Install Rust stable 1.99.1 MSVC, VS 2022 Build Tools and Temurin JDK 25.0.2+10. The checked-in Gradle wrapper downloads Gradle 9.8.1; do not use the system Gradle.

```powershell
./scripts/build.ps1
./scripts/smoke.ps1
./scripts/run.ps1
```

Builds use debug mode. The runnable pair is `target/debug/ferrummc.exe` and the matching `target/debug/patchbukkit.dll`; never mix native plugins built against another engine/toolchain. The normal scratch server is `run/server`, using `ferrummc.toml`. Bukkit jars belong in `plugins/data/patchbukkit/patchbukkit-plugins` below the server directory. The isolated smoke server binds loopback only and disables authentication; do not expose its configuration publicly.

See [Phase 1 evidence, commands and blockers](docs/PHASE1.md), [source pins and local patches](UPSTREAM.md), the [engine/bridge audit](docs/research/01-pumpkin-patchbukkit-audit.md), and [NeoForge feasibility](docs/research/02-neoforge-bridge-feasibility.md). Historical audit findings describe their snapshots; Phase 1 records what was actually tested.

## Credits and licenses

- [Pumpkin](https://github.com/Pumpkin-MC/Pumpkin): authoritative Rust engine and the foundation of this workspace. Original copyright and contributor notices are retained. Root [LICENSE](LICENSE): GPL-3.0; permissively licensed plugin SDK crates keep their own licenses.
- [PatchBukkit](https://github.com/Pumpkin-MC/PatchBukkit): embedded JVM, headless Paper adapters and Bukkit plugin hosting. Pinned sources and [GPL-3.0 license](vendor/patchbukkit/LICENSE) are retained under `vendor/patchbukkit`.
- [Horizon](https://github.com/horizon-rust/horizon): behavior, differential-testing and performance reference. Its original code is MIT; no Horizon implementation is copied into Phase 1.
- [Arclight](https://github.com/IzzelAliz/Arclight): GPLv3 architecture reference for a future NeoForge/Bukkit integration; no Arclight implementation is copied into Phase 1.

Paper and Minecraft runtime artifacts are acquired and prepared locally in ignored caches. Do not redistribute prepared Minecraft/Paper server jars or decompiled Mojang sources. Retain upstream and dependency license/source obligations when distributing FerrumMC. Minecraft is a Mojang/Microsoft product; FerrumMC is not affiliated with or endorsed by them.
