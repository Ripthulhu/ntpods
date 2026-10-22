"""Check PFC framing with synthetic payloads, including malformed lengths."""
import json
from pathlib import Path
import runpy
import subprocess
import sys


helper = Path(__file__).resolve().parents[1] / "docs/pfc_decode.py"
decode = runpy.run_path(str(helper))["decode"]


def rejects(packet, kind="profile"):
    try:
        decode(packet, kind)
    except ValueError:
        return
    raise AssertionError(f"accepted malformed {kind}: {packet.hex()}")


def profile(fields, service=0x10, message=4):
    return (bytes((message, 0, 5, 0, 2, 0)) + service.to_bytes(4, "little")
            + len(fields).to_bytes(2, "little") + fields)


# Standard AVDTP SEP descriptors precede all length-prefixed capability lists.
fields = bytes.fromhex(
    "03 02 66 00 10 02 67 00 "
    "01 07 40 00 19 00 ff 03 00 "
    "13 07 41 00 19 00 ff 03 00 "
    "11 0d 02 04 08 08 08 04 01 00 08 00 02 01 00 "
    "12 02 dc 05"
)
packet = profile(fields)
result = decode(packet, "profile")
parsed = result["sections"][0]["profiles"][0]["fields"]
assert result["name"] == "descriptor-response"
assert parsed[2]["channel"] == {"cid": 64, "psm": 25, "mtu": 1023, "mode": 0}
assert parsed[3]["channel"]["cid"] == 65
assert parsed[4]["endpoints"][0] == {
    "seid": 1, "in_use": False, "media_type": 0, "sink": True,
    "capabilities": [{"category": 1, "value": ""}, {"category": 8, "value": ""}],
}
assert parsed[4]["endpoints"][1]["seid"] == 2
assert parsed[5]["delay_ms"] == 150
extended = decode(profile(b"\x92\x02\x00\xdc\x05"), "profile")
assert extended["sections"][0]["profiles"][0]["fields"][0]["delay_ms"] == 150
selection = decode(profile(bytes.fromhex("15 01 03 14 01 06"), message=5), "profile")
selected = selection["sections"][0]["profiles"][0]["fields"]
assert selected[0]["remote_seid"] == 3 and selected[1]["local_seid"] == 6

for message, name in ((3, "descriptors"), (5, "configuration"), (6, "setup-complete")):
    assert decode(profile(b"", message=message), "profile")["name"] == name
unknown = decode(profile(b"\xfe\x00\x01" + b"x" * 256, service=0x200000), "profile")
assert unknown["sections"][0]["profiles"][0]["fields"][0]["value"] == "78" * 256
unknown_keys = decode(profile(bytes.fromhex("01 01 ff 03 00"), service=0x200000), "profile")
assert unknown_keys["sections"][0]["profiles"][0]["fields"] == [
    {"tag": 1, "key": 1, "value": "ff"}, {"tag": 3, "key": 3, "value": ""},
]
general = bytes.fromhex("06 00 05 00 01 00 04 00 01 02 ab cd")
assert decode(general, "profile")["sections"][0]["fields"][0]["value"] == "abcd"
combined = general + profile(b"", service=1)[4:]
assert len(decode(combined, "profile")["sections"]) == 2

body = (bytes.fromhex("01 00 05 00 40 00 00 04") + bytes(12)
        + bytes.fromhex("01 00 00 00") + bytes(6)
        + bytes.fromhex("00 00 10 00 01 00 00 00 de ad be ef"))
request = len(body).to_bytes(2, "little") + body
echo = decode(request, "echo-request")
assert echo["extended_features_internal"] == "0010000000000001"
assert echo["ulla_gate_bit36"] is False
assert echo["cid"] == 64 and echo["mtu"] == 1024
assert echo["profile_mask"] == 0 and echo["sdp_database_state"] == 0
assert echo["tail"] == "deadbeef"
response = b"\x02" + body[1:]
assert decode(response, "echo-response")["tail"] == "deadbeef"
# Captured Pro 3 and Intel Mac words; ULLA uses bit 36, not low-word bit 4.
for high, low, expected in ((0x087c7c1b, 0, True), (0x004d040d, 0, False), (0, 0x10, False)):
    candidate = response[:30] + high.to_bytes(4, "little") + low.to_bytes(4, "little") + response[38:]
    parsed_response = decode(candidate, "echo-response")
    assert parsed_response["ulla_gate_bit36"] is expected
    request_body = b"\x01" + candidate[1:]
    assert decode(len(request_body).to_bytes(2, "little") + request_body, "echo-request")["ulla_gate_bit36"] is expected
rejects(request, "echo-response")
rejects(request + b"\x00", "echo-request")
rejects(response, "echo-request")
rejects(request[:4] + b"\x06\x00" + request[6:], "echo-request")
for end in range(len(request)):
    rejects(request[:end], "echo-request")
for end in range(38):
    rejects(response[:end], "echo-response")
# A prefix ending at a section boundary is a valid shorter message.
for end in range(len(packet)):
    if end not in (4, 6):
        rejects(packet[:end])
for malformed in (b"\x12\x02\xdc", b"\x92\x02", b"\x12\x01\xdc",
                  b"\x01\x06" + bytes(6), b"\x11\x01\x01",
                  b"\x11\x07\x01\x04\x08\x03\x07\x02\x00",
                  b"\x11\x02\x00\x00", b"\x14\x00"):
    rejects(profile(malformed))
rejects(bytes.fromhex("03 00 05 00 03 00"))
rejects(bytes.fromhex("03 00 06 00"))
rejects(bytes.fromhex("02 00 05 00"))

cli = subprocess.run([sys.executable, str(helper), "profile"], input=packet.hex(),
                     text=True, capture_output=True, check=True)
assert json.loads(cli.stdout) == result and not cli.stderr
bad_cli = subprocess.run([sys.executable, str(helper), "profile", "03gg"],
                         text=True, capture_output=True)
assert bad_cli.returncode != 0 and "error:" in bad_cli.stderr and not bad_cli.stdout
print("PFC decoding checks passed.")
