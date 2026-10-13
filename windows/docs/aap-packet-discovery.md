# Capturing AirPods packets

AAP carries AirPods control traffic over classic-Bluetooth L2CAP PSM `0x1001`.
Captures from Apple devices help verify packet definitions and identify unknown
commands. Keep raw captures private: they can contain device identifiers, audio
and traffic from other Bluetooth devices.

## Capture one action at a time

Start before connecting the AirPods, perform one action, and note its timestamp.
Repeat it to distinguish the response from unrelated status traffic. Record the
model, firmware, host OS and Bluetooth adapter with each capture.

Useful actions include changing listening modes, removing one bud, putting buds
in the case, opening or closing the lid, and connecting or removing the charger.
Test lid and charger changes separately. Heart-rate and motion streams need their
own start/stop captures; an acknowledgement alone does not establish a live stream.

## macOS and iOS

On macOS, use PacketLogger from Apple's
[Additional Tools for Xcode](https://developer.apple.com/download/all/).
Start a live capture, perform the action and save the `.pklg` file. The
[macOS capture guide](../../docs/MACOS-PROTOCOL-CAPTURE.md) covers this project's
capture setup and analysis tools.

For iOS, use the Bluetooth profile and instructions from Apple's
[Profiles and Logs](https://developer.apple.com/feedback-assistant/profiles-and-logs/)
page. Follow the instructions for the device's OS version to collect the trace
and sysdiagnose. For a heart-rate capture, use an activity that reads the AirPods
sensor and capture both its start and stop. Remove the logging profile afterward.

## Inspect the packets

Filter L2CAP traffic in Wireshark and identify the channels negotiated for AAP.
These prefixes help locate traffic; use the
[protocol reference](../../docs/AIRPODS-PROTOCOL.md) for parsing and supported fields.

| Prefix | Traffic |
| --- | --- |
| `04 00 04 00 04` | Component battery reports |
| `04 00 04 00 06` | Ear detection |
| `04 00 04 00 09 00` | Control commands and reported values |
| `04 00 04 00 17 00` | RTBuddy descriptors, configuration and sensor data |
| `04 00 04 00 4B` | Conversation Awareness events |
| `04 00 04 00 58 00` | Microphone control and uplink audio |

From the repository root, this helper annotates AAP-looking byte runs in a text
export:

```sh
tshark -r airpods-macos.pklg -Y btl2cap -T fields -e frame.time_relative -e data \
  | python3 windows/docs/aap_extract.py
```

The helper accepts plain-text hex exports too. It does not reassemble fragments,
and its labels are only hints. For example, RTBuddy also carries motion and other
sensors, and charging status `0x05` does not establish the lid position. Wireshark
may expose a decoded protocol field instead of `data`; inspect the packet tree if
the command produces no payloads.

## Document a finding

Add confirmed packet layouts to [AAP definitions](../../docs/AAP%20Definitions.md)
and implementation details to the [protocol reference](../../docs/AIRPODS-PROTOCOL.md).
Keep unknown fields and firmware-specific observations identified as such.
For case reports, compare the
[charging definitions](../../docs/AAP%20Definitions.md#case-and-charging-transitions)
before assigning a new meaning to a status byte.

Include a short, sanitized packet sample and the action that produced it in the
change. Keep full captures, personal addresses and recordings out of the repository.
