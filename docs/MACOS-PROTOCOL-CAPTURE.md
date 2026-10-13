# Comparing AirPods traffic with macOS

Capture a complete connection and change one thing at a time. The first goal
is to compare macOS's handshake, codec selection and microphone stop sequence
with [NTPods' current protocol](AIRPODS-PROTOCOL.md). Later runs can identify
notifications and settings that NTPods does not yet handle.

## Capture point and limits

The reference Tahoe setup passes a USB Bluetooth controller from Windows through
WSL to QEMU. Capture that WSL USB bus with `usbmon` and `dumpcap`, beginning
before controller handover. This retains controller initialization and L2CAP
channel setup, which the dissectors need to identify later packets.

The reference captures used x86-64 Tahoe 26.7.1 (25G241), with an Intel AX200
USB controller (`8087:0029`) through usbipd, WSL's virtual USB host and QEMU
xHCI. Its saved OpenCore configuration enabled IntelBTPatcher,
BlueToolFixup, AX200NativeComposite and VMHide, with `-btlfxboardid`; the Intel
firmware uploader was disabled. Record the versions loaded in the guest with
each capture. These compatibility changes and the Intel controller can affect
feature negotiation and timing. Repeat host-specific findings on physical Apple
hardware before applying them to all Macs.

Tahoe 25G241's HFP initialization can log `disabled by defaults write` after a
general codec-capability check fails. The message alone does not establish a
saved override. Its WBS and UWBS checks also depend on controller capabilities;
do not infer microphone support or change preferences from that wording alone.

USB/HCI capture does not measure over-the-air retransmissions, RF timing or
earbud-to-earbud traffic. USB submit and completion records are also not separate
Bluetooth messages. Use decoded and reassembled HCI/L2CAP traffic for protocol
counts, and keep the raw USB trace for checking reconstruction.

macOS unified logs provide route and timing context. They can omit or redact
payloads. Apple's [Bluetooth logging resources](https://developer.apple.com/feedback-assistant/profiles-and-logs/)
are an optional second capture source; do not treat an empty log as proof that
the earbuds sent nothing.

## Capture procedure

Use usbmon/dumpcap for a passed-through USB controller, or Apple's Bluetooth
logging tools on a physical Mac. The local Tahoe launcher and analysis helpers
are not distributed with NTPods.

1. Finish pending driver restarts. Quit NTPods normally and allow microphone
   recovery to finish before transferring its controller. Keep a wired keyboard
   and mouse available.
2. Record the source revision, macOS build, AirPods model/firmware, controller
   and loaded extension versions.
3. Start the USB capture before controller handover so it includes controller
   initialization and L2CAP channel setup. Start guest Bluetooth/audio logging
   at the same time. Bound the recording to 15 minutes or 256 MiB.
4. Attach the controller to macOS and connect one AirPods pair. Other pairs and
   unrelated USB devices should not share the captured bus.
5. Record a timestamped marker immediately before each action. Wait for its
   response before changing another setting; record any audible or visible
   effect separately.
6. Stop capture and collect guest logs while the VM is still running. Check
   capture statistics, truncation and parser errors before interpreting results.
7. Return the controller to Windows and verify it is available before restarting
   NTPods.

The [audio findings](MACOS-AUDIO-CAPTURE-2026-10-12.md) document the VM's
AAC-ELD playback and CVSD hands-free recording. It did not use the AAP microphone
route. The [case and sensor findings](MACOS-CASE-CAPTURE-2026-10-12.md) cover
battery fields, charger/lid transitions, descriptor discovery and unknown traffic.

## Runs

Use separate sessions for these groups. Repeat a surprising result before
changing NTPods. Keep audio at a comfortable level; these are protocol captures,
not acoustic latency measurements.

| Run | Actions and markers | What to compare |
| --- | --- | --- |
| Connection | `connect`, then 30 seconds idle | L2CAP/FastConnect channel setup, handshake, feature mask, notification subscription, device information, keys and initial settings |
| Controls | Switch supported listening modes; sweep Adaptive; toggle Conversation Awareness, Personalized Volume and Allow Off one at a time | Exact values, byte order, command/response pairing, unsolicited echoes, prerequisites and persistence after reconnect |
| Playback | `playback-start`, play a short local stereo file, `playback-stop` | Discovered endpoints, accepted codec, AAC versus vendor AAC-ELD, media clock, packet grouping, start and suspend sequence |
| Microphone | Start playback, `mic-start`, record briefly, `mic-stop`, keep playback running for 30 seconds | Input transport, first valid audio, playback codec changes, STOP command, drain, acknowledgments, remaining mic packets and stereo recovery |
| Wear changes | During a recording, mark one bud out/in, then both out/in; stop the recording | Ear-state reports, permission changes, stream sequence resets, decoder boundaries and restart order |
| Case and battery | Mark buds into open case, lid closed, charger attached/detached and lid reopened; wait 90 seconds after closure | AAP-to-BLE handover, case/charging bits, report intervals, fresh versus cached readings and sleep/disconnection timing |
| Sensors | Enable a sensor through an available macOS feature and mark controlled movements or measurement start/stop | Whether macOS requests a stream, its selector/rate and the packet format; absence of a macOS control leaves this run pending |

For the microphone run, select the AirPods as both input and output and record
the application name. Voice Memos and QuickTime may choose different routes.
A 48 kHz recording file does not establish an AAC-ELD Bluetooth uplink: inspect
the negotiated transport and media packets. Keep left/right playback observations
separate from decoder or channel-count metadata.

Start with Pro 3. Repeat the connection, supported controls and playback runs
with Pro 1. Compare each model with its own expected capability profile; missing
Adaptive or newer sensors on Pro 1 is not a protocol failure.

## Decode and compare

Decode with TShark/Wireshark and the
[pinned Apple dissectors](https://github.com/pabloaul/apple-wireshark/tree/2e9822247195fad2f79c729010308c33a29546ea).
Keep the trace, action markers, tool versions and parse errors together. Run Lua
dissection as an ordinary user; the local TShark setup disables Lua under root.

When capture starts with the controller already attached, Linux may not send
its USB descriptors again. Do not detach the guest controller just to obtain
them: that disrupted Tahoe's existing Bluetooth session in the local test.
Identify the controller's USB bus/address and, if needed, restrict Decode As
Bluetooth to that device. Record any override with the trace. Verify the address
for every session; it can change after reattachment.

Check that the trace contains Bluetooth traffic, includes the connection setup,
and has no truncated frames or Lua errors. A zero Bluetooth count needs capture
or dissection investigation before any protocol conclusion. Check dumpcap's output,
pcapng interface statistics and USB errors for drops.

Use the full trace with TShark's two-pass dissection. TShark
4.2.2 reassembles USB HCI transfers and enables ACL reassembly by default.
Check `usb.urb_id`, `usb.urb_type`, `usb.data_flag`, `usb.data_len`,
`hci_usb.msg.*` and `bthci_acl.reassembled_in` when validating USB
reconstruction. Outgoing data normally appears on submission; incoming data
appears on completion. Retain both records for completion status. Synthetic H4
fixtures alone do not verify live USB reconstruction. USB timestamps describe
the host/controller exchange, not RF delivery or sound reaching the ear.

Resolve connection handles, L2CAP PSM/CID mappings and direction before comparing
payloads. Reassemble ACL fragments and separate independent connections. Dynamic
CIDs, addresses, sequence numbers and session keys can differ between runs.
Keep their relationships while comparing semantic fields and timing. The pinned
FastConnect Lua plugin registers discovered CIDs globally, so capture one pair
per session and verify channel bindings before trusting its labels.

Separate standard Bluetooth traffic from Apple-specific messages. SDP discovers
services; L2CAP signaling opens and configures channels; AVDTP negotiates
playback; AVRCP carries media controls and metadata. HFP control traffic and
SCO/eSCO audio can identify the hands-free route. Their presence does not imply
an unimplemented Apple feature. Investigate unhandled Apple opcodes only after
identifying their enclosing protocol and connection.

Compare the trace with the linked sender/parser in the control or audio
reference. For each operation, record the sent request, peer report, elapsed
time and observed effect. For stream stop, find the last microphone packet and
the next usable playback packet; a transport acknowledgement alone is not
evidence that both earbuds recovered.

For unknown traffic, record:

| Field | Record |
| --- | --- |
| Context | Model, firmware, macOS build, app, scenario and marker |
| Location | Private capture name, frame numbers and relative time |
| Transport | Direction, connection/channel alias, PSM and opcode/subtype |
| Shape | Payload length, fixed fields, changing offsets and repetition rate |
| Interpretation | Candidate meaning, alternatives, confidence and the next distinguishing action |
| NTPods match | Parser/sender function, supported model and regression test, or `unhandled` |

Inspect unmatched traffic by opcode, direction, length and scenario. Preserve
unknown fields instead of silently assigning them names. A control may be sent
before a visible action, and a periodic report may merely coincide with it.

Raw traces can include microphone audio, link keys, identity keys and device
identifiers. Keep them outside Git, along with unredacted timelines and logs.
Share sanitized field descriptions and synthetic packet fixtures. Inspect the
actual export before publishing it; a packet summary can still contain secrets.

## Tool references

- [Linux usbmon](https://www.kernel.org/doc/html/latest/usb/usbmon.html)
- [dumpcap limits and capture options](https://www.wireshark.org/docs/man-pages/dumpcap.html)
- [Wireshark 4.2.2 USB HCI reassembly](https://gitlab.com/wireshark/wireshark/-/blob/v4.2.2/epan/dissectors/packet-hci_usb.c)
- [Apple protocol dissectors](https://github.com/pabloaul/apple-wireshark/tree/2e9822247195fad2f79c729010308c33a29546ea)
