"""Check shipped translations and .NET format placeholders."""
from pathlib import Path
from string import Formatter
import xml.etree.ElementTree as ET


def resources(path):
    values = {}
    for item in ET.parse(path).getroot().findall("data"):
        key = item.attrib["name"].replace(".", "_")
        assert key not in values, f"{path}: duplicate key {key}"
        text = item.findtext("value")
        assert text and text.strip(), f"{path}: empty value for {key}"
        values[key] = text
    return values


def fields(text):
    return sorted((key, spec) for _, key, spec, _ in Formatter().parse(text) if key is not None)


root = Path(__file__).resolve().parents[1] / "winui/NTPods.WinUI/Strings"
english = resources(root / "en-US/Resources.resw")
for language in ("pt-PT", "fr-FR", "es-ES"):
    translated = resources(root / language / "Resources.resw")
    missing = english.keys() - translated.keys()
    assert not missing, f"{language}: missing {sorted(missing)}"
    for key, value in english.items():
        assert fields(value) == fields(translated[key]), f"{language}: placeholders differ for {key}"
print(f"Localization checks passed for {len(english)} keys in all four languages.")
