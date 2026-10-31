# Native 26.2 inputs

FerrumMC uses the retained 26.2 Extractor dump for engine assets and the official datagen reports in `assets/compat/26_2` as an independent identity oracle. The official server bundler SHA-1 is `823e2250d24b3ddac457a60c92a6a941943fcd6a`; its embedded version declares protocol 776 and world data version 4903.

Run `python tools/vanilla-26.2/verify.py` to compare every native block/state, packet and built-in registry ID with the official reports. Run `cargo run --locked -p pumpkin-codegen` after changing inputs. Generated files must never be hand edited.

`Extract.java` supplements the retained Extractor inputs with native fuel and compost tables, brewing results, decorated-pot patterns, tool transform maps, and component persistence/swap-animation flags. It runs the unmodified official 26.2 classes, binds vanilla item tags, initializes components, and evaluates `PotionBrewing.hasMix/mix` for vanilla containers, potions and reagents. Compile and run with JDK 25 and a classpath containing the inner server jar and its bundled libraries:

```powershell
javac -proc:none -cp $classpath -d .build-cache tools/vanilla-26.2/Extract.java
java -cp "$classpath;.build-cache" Extract assets
python tools/vanilla-26.2/verify.py
cargo run --locked -p pumpkin-codegen
```

Keep downloaded server jars and decompiled vanilla sources in a cache outside the repository. The extracted inputs retain Mojang attribution under `assets/NOTICE.md`. No runtime jars or decompiled vanilla source belong in Git.

Existing FerrumMC worlds require reset when switching to native IDs. No owner-world migration is performed here.
