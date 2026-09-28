"""Import local palette data into the first-run theme template."""
import hashlib
import json
import os
import re
from pathlib import Path

root = Path(__file__).resolve().parent.parent
source_value = os.environ.get("TSHELL_THEME_SOURCE")
if source_value:
    source = Path(source_value)
else:
    candidates = sorted(root.glob("*/web/src/themeCatalog.ts"))
    if len(candidates) != 1:
        raise SystemExit(
            "Set TSHELL_THEME_SOURCE when the palette catalog source is not unique"
        )
    source = candidates[0]
if not source.is_absolute():
    source = root / source
if not source.is_file():
    raise SystemExit(f"Palette catalog source does not exist: {source}")
raw = source.read_bytes()
text = raw.decode("utf-8")
catalog = text.split("export const terminalThemeCatalog:", 1)[1].split(
    "export const defaultTerminalThemePreference", 1
)[0]
entries = []
ansi_keys = ["black", "red", "green", "yellow", "blue", "magenta", "cyan", "white",
             "brightBlack", "brightRed", "brightGreen", "brightYellow", "brightBlue",
             "brightMagenta", "brightCyan", "brightWhite"]
for match in re.finditer(r"^  '?([\w-]+)'?: \{\s+label: '([^']+)',\s+preview: \[([^]]+)\],\s+palette: \{(.*?)\n    \},\n  \},", catalog, re.M | re.S):
    theme_id, label, preview, colors = match.groups()
    colors = {k: int(v, 16) for k, v in re.findall(r"(\w+): '#([0-9a-fA-F]{6})'", colors)}
    assert set(colors) == set(ansi_keys + ["background", "foreground", "cursor", "selectionBackground"])
    display_label = {"tide-light": "Light", "tide-dark": "Dark"}.get(theme_id, label)
    entries.append(dict(id=theme_id, label=display_label,
                        preview=[int(v, 16) for v in re.findall(r"'#([0-9a-fA-F]{6})'", preview)],
                        **colors))
assert len(entries) == 17
assert re.search(r"defaultTerminalThemePreference:.*?= 'one-dark'", text)
order = re.search(r"const terminalThemeOptionIds:.*?= \[(.*?)\]", text, re.S)[1]
option_ids = re.findall(r"'([\w-]+)'", order)
option_ids += [e["id"] for e in entries if e["id"] not in option_ids]
entries.sort(key=lambda e: e["id"] == "one-dark")
hex_color = lambda value: f"#{value:06X}"
theme_file = {
    "version": 1,
    "themes": [
        {
            "id": e["id"],
            "name": e["label"],
            "foreground": hex_color(e["foreground"]),
            "background": hex_color(e["background"]),
            "cursor": hex_color(e["cursor"]),
            "selection": hex_color(e["selectionBackground"]),
            "ansi": [hex_color(e[k]) for k in ansi_keys],
        }
        for e in entries
    ],
}
target = root / "src/terminal_theme/defaults.json"
target.write_text(json.dumps(theme_file, indent=2) + "\n", encoding="utf-8")
fixture = {"source": "external/themeCatalog.ts", "sha256": hashlib.sha256(raw).hexdigest(),
           "default": "one-dark", "option_ids": option_ids, "themes": entries}
(root / "tests/fixtures/theme-catalog.json").write_text(
    json.dumps(fixture, indent=2) + "\n", encoding="utf-8")
print(f"Imported {len(entries)} terminal themes; default One Dark")
