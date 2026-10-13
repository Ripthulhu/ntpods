"""Check ear-state annotations from AAP hex exports."""
from pathlib import Path
import runpy
import subprocess
import sys


helper = Path(__file__).resolve().parents[1] / "docs/aap_extract.py"
annotate = runpy.run_path(str(helper))["annotate"]
prefix = bytes.fromhex("04 00 04 00 06 00")
states = ("in-ear", "out-of-ear", "in-case", "disconnected", "transitional")

for primary, primary_name in enumerate(states):
    for secondary, secondary_name in enumerate(states):
        packet = prefix + bytes((primary, secondary))
        expected = f"ear-detection  primary={primary_name} secondary={secondary_name}"
        assert annotate(packet) == expected, packet.hex()

packet = prefix + bytes((2, 1))
for length in range(len(packet)):
    assert "primary=" not in annotate(packet[:length]), length
assert annotate(bytes.fromhex("04 00 04 00 06 01 02 01")) == "opcode 0x106 (UNKNOWN)"
assert annotate(prefix + bytes((5, 255))) == "ear-detection  primary=0x5 secondary=0xff"
assert annotate(bytes.fromhex("04 00 05 00 06 00 02 01")) == "(non-AAP)"

result = subprocess.run(
    [sys.executable, str(helper)], input=packet.hex(" ") + "\n",
    text=True, capture_output=True, check=True,
)
assert result.stdout.strip() == (
    f"{'ear-detection  primary=in-case secondary=out-of-ear':<48}  {packet.hex(' ')}"
)
assert not result.stderr, result.stderr
print("AAP ear-state extraction checks passed.")
