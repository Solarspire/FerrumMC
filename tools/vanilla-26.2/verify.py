"""Compare engine extraction with the independently generated official 26.2 reports."""
import json
from pathlib import Path

ASSETS = Path(__file__).resolve().parents[2] / "assets"
def read(path):
    return json.loads((ASSETS / path).read_text(encoding="utf-8"))

reports = read("compat/26_2/registries.json")
for registry, file in [("item", "items"), ("entity_type", "entities"),
                       ("attribute", "attributes"), ("sound_event", "sounds"),
                       ("particle_type", "particles"), ("fluid", "fluids"),
                       ("game_event", "game_event"), ("data_component_type", "data_component"),
                       ("potion", "potion"), ("mob_effect", "effect")]:
    data = read(file + ".json")
    if isinstance(data, list):
        engine = {"minecraft:" + (entry if isinstance(entry, str) else entry["name"]).removeprefix("minecraft:"): i
                  for i, entry in enumerate(data)}
    else:
        engine = {"minecraft:" + name.removeprefix("minecraft:"): entry if isinstance(entry, int) else entry["id"]
                  for name, entry in data.items()}
    official = {name: entry["protocol_id"] for name, entry in reports["minecraft:" + registry]["entries"].items()}
    assert engine == official, f"{registry} differs from official registry IDs"
    print(f"PASS {registry}: {len(engine)} native IDs")

blocks = read("blocks.json")
vanilla_blocks = read("compat/26_2/blocks.json")
assert len(blocks["blocks"]) == len(vanilla_blocks)
for block in blocks["blocks"]:
    name = "minecraft:" + block["name"]
    vanilla = vanilla_blocks[name]
    assert block["id"] == reports["minecraft:block"]["entries"][name]["protocol_id"], name
    assert [state["id"] for state in block["states"]] == [state["id"] for state in vanilla["states"]], name
    assert block["default_state_id"] == next(state["id"] for state in vanilla["states"] if state.get("default")), name
print(f'PASS blocks: {len(blocks["blocks"])} blocks, {sum(len(b["states"]) for b in blocks["blocks"])} states')

packets = read("packets.json")
assert packets["version"] == 776
for phase, directions in read("compat/26_2/packets.json").items():
    for direction, entries in directions.items():
        expected = {name.removeprefix("minecraft:"): packet["protocol_id"] for name, packet in entries.items()}
        assert packets[direction]["config" if phase == "configuration" else phase] == expected
print("PASS packet IDs: every phase/direction matches official protocol 776")
