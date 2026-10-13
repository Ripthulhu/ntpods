# Comparing AirPods traffic with macOS

Capture a complete connection and change one thing at a time. The first goal
is to compare macOS's handshake, codec selection and microphone stop sequence
with [NTPods' current protocol](AIRPODS-PROTOCOL.md). Later runs can identify
notifications and settings that NTPods does not yet handle.

## Capture point and limits

The local Tahoe setup passes a USB Bluetooth controller from Windows through
WSL to QEMU. Capture that WSL USB bus with `usbmon` and `dumpcap`, beginning
before controller handover. This retains controller initialization and L2CAP
channel setup, which the dissectors need to identify later packets.

The prepared guest is x86-64 Tahoe 26.7.1 (25G241), using an Intel AX200
USB controller (`8087:0029`) through usbipd, WSL's virtual USB host and QEMU
xHCI. Its saved working OpenCore configuration enables IntelBTPatcher,
BlueToolFixup, AX200NativeComposite and VMHide, with `-btlfxboardid`; the Intel
firmware uploader is disabled. Record the versions loaded in the guest with
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

## Local Tahoe capture kit

The existing `AirPods-Tahoe` folder contains the VM launchers, capture controls,
guest log collector and a pinned Apple Wireshark dissector. Raw sessions live
in its `Captures` folder, outside this repository.
The launcher names below refer to that local lab folder. On another host,
collect an equivalent full USB/HCI trace and action markers with usbmon or
Apple's Bluetooth logging tools, then follow the same runs and decoding checks.

1. Finish any pending Windows driver restart. Quit NTPods normally and confirm
   its microphone and playback recovery have finished. Keep a wired keyboard
   and a mouse that do not depend on the controller being passed through.
2. Open **Start Tahoe.cmd** and wait for macOS and its SSH connection. Record
   the macOS build, model/firmware, controller, VM/extension versions and the
   current NTPods source revision for the comparison.
3. Open **Start Bluetooth Capture.cmd** before controller handover. It starts
   the bounded USB trace and matching guest log session. Use **Capture Status.cmd**
   to confirm it is running.
4. Open **GiveBluetoothToTahoe.cmd**, approve Windows' administrator prompt and
   connect the chosen AirPods pair in macOS. Leave other pairs in their cases.
5. Use **Mark Capture.cmd** immediately before each action. Wait for the device
   response before changing another setting. Record the visible or audible
   result as well as the marker.
6. Open **Stop Bluetooth Capture.cmd** while the VM is still running. It stops
   the USB trace and copies the guest logs. Check the status and collected files.
7. Open **ReturnBluetoothToWindows.cmd**, then **Stop Tahoe.cmd**. Confirm the
   controller is back in Windows before reopening NTPods.

The capture stops at 15 minutes or 256 MiB of USB traffic. Use Stop even after
the limit so the guest logs are collected. At startup, the helper rejects
unrelated devices on the monitored WSL USB bus. Do not import other devices
onto that bus during the run.
If guest log collection fails, the logs remain in `~/AirPods-Captures` in Tahoe.

The first live controller run on 2026-10-12 collected 7,355 USB frames,
including 3,505 decoded Bluetooth frames, with no dropped or truncated frames
and no Lua errors. Controller initialization, LE reports and SDP decoded.
Connection attempts timed out, followed by an authentication-failure disconnect;
the saved Pro 3 pairing needs renewal. This validates the capture point, but
does not yet establish an AAP or audio result. The earlier controller-free
smoke test contained only 24 USB frames.

The later reconnect capture collected 16,513 frames
with no drops or Lua errors. Three ACL connections succeeded, but repeated SDP
channel requests received no response. The earbuds terminated those links after
12.9, 21.3 and 28.2 seconds with reason `0x13` (Remote User Terminated Connection).
Guest logs reported `BT_ERROR_L2CAP_SIGNALMAN_TIMEOUT`. The disconnect reason
does not identify the cause.

That run contained no authentication or encryption exchange, completed SDP
channel, AACP handshake or audio negotiation. QuickTime had no AirPods input,
so recording never started. It establishes a service-discovery failure in this
Intel VM and leaves the macOS microphone transport untested.

After fresh pairing and a Bluetooth toggle, the next run completed HFP, AACP
and AVDTP setup. Restarting CoreAudio restored the missing input. Settings and
Voice Memos then selected CVSD over eSCO; playback used Apple AAC-ELD before
and after capture. No AACP `0x58` microphone stream appeared. See the
[2026-10-12 audio capture results](MACOS-AUDIO-CAPTURE-2026-10-12.md) for the
wire evidence, encoder settings and limits of this Intel VM comparison.

The [case and charging captures](MACOS-CASE-CAPTURE-2026-10-12.md) checked AAP
and BLE battery reports against NTPods, including charger and lid transitions.
They also catalogue traffic the app does not yet interpret.

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

The buttons call `support/capture-session.py`. From the kit folder in WSL,
the equivalent commands are:

```sh
sudo python3 support/capture-session.py start
sudo python3 support/capture-session.py status
sudo python3 support/capture-session.py mark "mic-start"
sudo python3 support/capture-session.py stop
python3 support/analyze-capture.py latest
```

Use a short, single-line marker. Start and Stop control both USB capture and
guest logging; neither command transfers Bluetooth. The analysis command also
accepts a session's `bluetooth.pcapng` or `manifest.json` path instead of `latest`.

Run analysis as the ordinary WSL user after stopping. It uses TShark and
`support/tahoe-wireshark.lua` to produce
`summary.json`, `timeline.tsv` and `analysis-errors.txt`. Root execution disables
Lua in this setup. Keep the summary, markers and original trace together.

When capture starts with the controller already attached, Linux may not send
its USB descriptors again. Do not detach the guest controller just to obtain
them: that disrupted Tahoe's existing Bluetooth session in the local test.
For a verified AX200 bus/address, use the analyzer's `--usb-device BUS:ADDRESS`
option. It restricts forced Bluetooth decoding to that device, checks the bus
against the manifest and records the override in the summary. Verify the
address for each session; USB addresses can change after reattachment.

Check that the trace contains Bluetooth traffic, includes the connection setup,
and has no truncated frames or Lua errors. A zero Bluetooth count needs capture
or dissection investigation before any protocol conclusion. Check `dumpcap.log`,
pcapng interface statistics and USB errors for drops; the summary does not
extract a dropped-packet count.

The analyzer passes the full trace to TShark with two-pass dissection. TShark
4.2.2 reassembles USB HCI transfers and enables ACL reassembly by default.
Check `usb.urb_id`, `usb.urb_type`, `usb.data_flag`, `usb.data_len`,
`hci_usb.msg.*` and `bthci_acl.reassembled_in` when validating the first real
Bluetooth trace. Outgoing data normally appears on submission; incoming data
appears on completion. Retain both records for completion status. Existing
synthetic tests cover H4 dissection, not reconstruction of a live USB Bluetooth
session. USB timestamps describe the host/controller exchange, not RF delivery
or sound reaching the ear.

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
