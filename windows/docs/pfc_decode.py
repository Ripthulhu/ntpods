#!/usr/bin/env python3
"""Decode one reassembled Apple Profile Fast Connect payload; never sends traffic."""
import argparse
import json
import sys


MESSAGES = {1: "discovery", 2: "discovery-response", 3: "descriptors",
            4: "descriptor-response", 5: "configuration", 6: "setup-complete"}
SERVICES = {1: "HFP", 8: "AVRCP", 0x10: "A2DP", 0x80000: "AACP", 0x100000: "GATT"}


class Reader:
    def __init__(self, data):
        self.data = data
        self.offset = 0

    def __bool__(self):
        return self.offset < len(self.data)

    def take(self, size):
        end = self.offset + size
        if end > len(self.data):
            raise ValueError(f"truncated field at offset {self.offset}: need {size} bytes")
        value = self.data[self.offset:end]
        self.offset = end
        return value

    def uint(self, size):
        return int.from_bytes(self.take(size), "little")


def endpoints(data):
    reader = Reader(data)
    count = reader.uint(1)
    descriptors = [reader.take(2) for _ in range(count)]
    result = []
    for first, second in descriptors:
        caps = Reader(reader.take(reader.uint(1)))
        capabilities = []
        while caps:
            category = caps.uint(1)
            capabilities.append({"category": category, "value": caps.take(caps.uint(1)).hex()})
        result.append({"seid": first >> 2, "in_use": bool(first & 2),
                       "media_type": second >> 4, "sink": bool(second & 8),
                       "capabilities": capabilities})
    if reader:
        raise ValueError("trailing endpoint bytes")
    return result


def tlvs(data, service=None):
    reader = Reader(data)
    result = []
    while reader:
        tag = reader.uint(1)
        value = reader.take(reader.uint(2 if tag & 0x80 else 1))
        key = tag & 0x7f
        field = {"tag": tag, "key": key, "value": value.hex()}
        if service in SERVICES:
            if key == 1 or (service == 0x10 and key == 0x13):
                if len(value) != 7:
                    raise ValueError("channel configuration must contain 7 bytes")
                channel = Reader(value)
                field["channel"] = {"cid": channel.uint(2), "psm": channel.uint(2),
                                    "mtu": channel.uint(2), "mode": channel.uint(1)}
            elif key in (3, 4) or (service == 0x10 and key in (0x10, 0x12)):
                if len(value) != 2:
                    raise ValueError(f"descriptor key {key:#x} must contain 2 bytes")
                field["u16"] = int.from_bytes(value, "little")
                if service == 0x10 and key == 0x12:
                    field["delay_ms"] = field["u16"] / 10
            elif service == 0x10 and key == 0x11:
                field["endpoints"] = endpoints(value)
            elif service == 0x10 and key in (0x14, 0x15):
                if len(value) != 1:
                    raise ValueError("selected SEID must contain one byte")
                field["local_seid" if key == 0x14 else "remote_seid"] = value[0]
        result.append(field)
    return result


def decode(data, kind):
    if kind not in ("echo-request", "echo-response", "profile"):
        raise ValueError("unknown payload kind")
    if kind == "echo-request":
        envelope = Reader(data)
        length = envelope.uint(2)
        data = envelope.take(length)
        if envelope:
            raise ValueError("Echo request length does not match payload")
    reader = Reader(data)
    message, flags, version = reader.uint(1), reader.uint(1), reader.uint(2)
    if version != 5:
        raise ValueError(f"unsupported PFC version {version}")
    expected = {"echo-request": (1,), "echo-response": (2,), "profile": (3, 4, 5, 6)}
    if message not in expected[kind]:
        raise ValueError(f"message {message} does not match {kind}")
    result = {"message": message, "name": MESSAGES[message],
              "header_byte_1": flags, "version": version}
    if kind != "profile":
        result["cid"] = reader.uint(2)
        result["mtu"] = reader.uint(2)
        result["profile_mask"] = reader.uint(4)
        result["discovery_bytes_12_19"] = reader.take(8).hex()
        result["base_features"] = reader.uint(4)
        result["sdp_database_state"] = reader.uint(4)
        result["discovery_bytes_28_29"] = reader.take(2).hex()
        # Apple stores the first LE32 word in the high half of its feature word.
        high, low = reader.uint(4), reader.uint(4)
        result["extended_features_internal"] = f"{(high << 32) | low:016x}"
        result["ulla_gate_bit36"] = bool(high & 0x10)
        result["tail"] = reader.take(len(data) - reader.offset).hex()
        return result
    result["sections"] = []
    while reader:
        section_type = reader.uint(2)
        section = {"type": section_type}
        if section_type == 1:
            section["fields"] = tlvs(reader.take(reader.uint(2)))
        elif section_type == 2:
            section["profiles"] = []
            while reader:
                service = reader.uint(4)
                fields = tlvs(reader.take(reader.uint(2)), service)
                section["profiles"].append({"service": service,
                                            "name": SERVICES.get(service, "unknown"),
                                            "fields": fields})
        else:
            raise ValueError(f"unsupported section type {section_type}")
        result["sections"].append(section)
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("kind", choices=("echo-request", "echo-response", "profile"))
    parser.add_argument("hex", nargs="?", help="payload hex; otherwise read stdin")
    args = parser.parse_args()
    try:
        data = bytes.fromhex(args.hex if args.hex is not None else sys.stdin.read())
        result = decode(data, args.kind)
    except ValueError as error:
        parser.error(str(error))
    print(json.dumps(result, indent=2))


if __name__ == "__main__":
    main()
