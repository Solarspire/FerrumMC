# Official native 26.2 reference reports

These JSON files are Mojang data, retained under the same terms and attribution as the other vanilla inputs in [assets/NOTICE.md](../../NOTICE.md). No server jar or decompiled source is distributed here.

Source: official 26.2 server bundler, SHA-1 `823e2250d24b3ddac457a60c92a6a941943fcd6a`, downloaded from `https://piston-data.mojang.com/v1/objects/823e2250d24b3ddac457a60c92a6a941943fcd6a/server.jar` through Mojang's version manifest. Run in a normal cache outside the repository:

```powershell
java -DbundlerMainClass=net.minecraft.data.Main -jar server.jar --all --output generated
```

`blocks.json`, `packets.json` and `registries.json` are the reports with JSON whitespace removed. `data/` contains the synchronized-registry JSON inputs from `generated/data/minecraft`, also with whitespace removed. The engine extraction in `assets/` is native 26.2. `pumpkin-codegen -- registry registry_ids` generates synchronized network NBT and native registry identities for the NeoForge adapter. `python tools/vanilla-26.2/verify.py` compares the engine extraction against these independent official reports. Never edit generated Rust files directly.

`scripts/test-vanilla-codecs.ps1` independently downloads digest-pinned 26.2 jar into the user's normal cache and decodes smoke traffic with its original codecs and registry loaders.
