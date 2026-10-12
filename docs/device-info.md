---
opcode: 0x001D
title: Device Information
description: Information about AirPods, such as model, firmware version, and serial number. This cannot be requested from the accessory; it is only sent by the accessory to the host upon connection.
---

## Device information

The accessory sends this packet on connection. It contains the device name, model, versions and serial numbers.

Each `null` indicates the start of a new string field.

The data is in this order:

- Name
- Model number
- Manufacturer (always "Apple Inc.")
- Serial number
- Version 1
- Version 2
- Hardware revision (?) (observed: `1.0.0`)
- Updater app version (?) (observed: `com.apple.accessory.updater.app.71`)
- Serial number (Left Bud)
- Serial number (Right Bud)
- Version (?) (observed: `8454371`)
- Trailing bytes (unknown format)

