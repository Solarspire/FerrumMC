# Vanilla 26.2 compatibility inputs

These JSON files are Mojang data, retained under the same terms and attribution as the other vanilla inputs in [assets/NOTICE.md](../../NOTICE.md). No server jar or decompiled source is distributed here.

Source: official 26.2 server bundler, SHA-1 `823e2250d24b3ddac457a60c92a6a941943fcd6a`, downloaded from `https://piston-data.mojang.com/v1/objects/823e2250d24b3ddac457a60c92a6a941943fcd6a/server.jar` through Mojang's version manifest. Run in a normal cache outside the repository:

```powershell
java -DbundlerMainClass=net.minecraft.data.Main -jar server.jar --all --output generated
```

`blocks.json`, `packets.json` and `registries.json` are the reports with JSON whitespace removed. `data/` contains the synchronized-registry JSON inputs from `generated/data/minecraft`, also with whitespace removed. `pumpkin-codegen -- compatibility registry` generates the mapping arrays and network NBT; never edit those generated Rust files directly. Engine assets remain 26.3.

`scripts/test-vanilla-codecs.ps1` independently downloads digest-pinned 26.2 and 26.3 jars into the user's normal cache and decodes smoke traffic with their original codecs and registry loaders.
