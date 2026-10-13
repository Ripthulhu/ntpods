---
opcode: 0x001D
title: Device Information
description: Model, firmware and identity strings reported by the accessory on connection.
---

## Device information

The [Windows metadata parser](WINDOWS-AIRPODS-CONTROL-PROTOCOL.md#implemented-message-families)
starts the string list at byte 11 and normalizes recognized packed release
versions. The field names below remain research labels; unknown trailing bytes
are not decoded by NTPods.

The accessory sends this packet on connection. It contains the device name, model, versions and serial numbers.

Each NUL byte terminates a UTF-8 string.

The data is in this order:

- Name
- Model number
- Manufacturer (observed: "Apple Inc.")
- Serial number
- Version 1
- Version 2
- Hardware revision (?) (observed: `1.0.0`)
- Updater app version (?) (observed: `com.apple.accessory.updater.app.71`)
- Additional serial number 1 (side mapping unverified)
- Additional serial number 2 (side mapping unverified)
- Version (?) (observed: `8454371`)
- Trailing bytes (unknown format)

