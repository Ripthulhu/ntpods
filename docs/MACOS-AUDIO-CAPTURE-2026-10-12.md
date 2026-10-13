# Tahoe audio protocol findings

Evidence: captured on 2026-10-12 with AirPods Pro 3 firmware 9A348,
Tahoe 26.7.1 (25G241), and the patched Intel AX200 VM
described in the [capture guide](MACOS-PROTOCOL-CAPTURE.md). This run used CVSD
hands-free capture and Apple AAC-ELD playback. It did not exercise the AACP
microphone route used by NTPods.

## Capture scope

The first session contains pairing and profile setup; the second contains the
microphone check. They have 15,329 and
12,322 frames respectively, with no capture drops or truncated frames. The
second trace starts mid-connection, so decoding it alone misses dynamic channel
bindings. Analysis merged both traces chronologically and forced Bluetooth
decoding for the verified USB controller. Its first 15,329 frames belong to
the pairing session; subtract 15,329 from later merged frame numbers to locate
them in the original microphone trace.

Complete CaseInfo `0x23` and certificate `0x27` packets needed empty-tail guards
in the pinned Lua dissector. HFP and AVDTP decoding was unaffected; see
[capture limits](MACOS-CASE-CAPTURE-2026-10-12.md#capture-quality-and-limits).

## Microphone and return to playback

Both the Settings input meter and Voice Memos negotiated HFP codec 1 (CVSD).
Successful eSCO connection events report CVSD air mode, a 12-slot interval
(7.5 ms), and 60-byte receive/transmit packets. USB isochronous transfers ran in
both directions. TShark did not decode their audio payloads in this setup;
codec identification comes from HFP negotiation and the HCI connection event.

| Event | UTC | Original microphone frame |
| --- | --- | --- |
| Voice Memos codec selection | 21:27:40.124 | 8238 |
| eSCO connection complete | 21:27:40.603 | 8345 |
| Host requests eSCO disconnect | 21:28:02.797 | 11268 |
| eSCO disconnect complete, local reason `0x16` | 21:28:03.133 | 11294 |
| Later playback Start request | 21:28:15.719 | 11508 |
| Start accepted | 21:28:16.010 | 11513 |
| First subsequent RTP packet | 21:28:16.099 | 11547 |

No AACP `0x58` start, stop or uplink appears. The ACL connection and configured
AAC-ELD playback channel remain present across microphone use. Later playback
starts on that channel without codec reconfiguration. Playback was started
separately, so the gap before Start is not a recovery-delay measurement. This
run also does not verify simultaneous stereo playback during capture or acoustic
channel separation afterward.

## AAC-ELD playback comparison

Pairing frame 5177 requests vendor `0x004C`, codec `0x8001`; frame 5194 accepts
it. All vendor profile bytes match NTPods' template except the bitrate field,
which NTPods' parser reads as 320 kbps with its high bit set. The guest configures
its encoder separately, so that field is not the measured stream bitrate.

| Setting | Tahoe evidence | NTPods at capture time |
| --- | --- | --- |
| Rate and channels | Encoder logs: 48 kHz stereo | 48 kHz stereo |
| Encoder bitrate | Logs: 192 kbps target, VBR, 256 kbps maximum | CBR, selectable 128–256 kbps |
| Access-unit bounds | Logs: 32–320 bytes | At most 321 bytes |
| Packet grouping | Logs: three frames per 30 ms | Two 480-sample frames per 20 ms |
| Transport size | Logs: MTU 1,023, RTP frame length 988 | Bound to negotiated media MTU |
| RTP timestamps after recording | Increments 27–33, usually 30, at roughly 30 ms intervals | Increment 960 sample ticks per 20 ms |
| Reported delay | Earbud DelayReport 150 ms; guest adds 30 ms packet grouping | No equivalent measured comparison in this run |

Guest encoder logs report `delayMode=1` and `mCodecDelay=240` samples. These are
encoder properties, not a measured end-to-end delay. The trace did not establish
the encoder's AudioSpecificConfig, SBR setting or a minimum playback buffer.
The timestamp and packet-grouping differences warrant further decoding; this
capture does not establish that NTPods' hardware-tested packet format is wrong.

Raw traces, merged captures and unredacted logs remain outside Git.

## Intel controller and microphone eligibility

At capture time, the VM loaded IntelBTPatcher 2.5.1, BlueToolFixup 2.7.2 and
Lilu 1.7.2.
Setup records identify IntelBTPatcher as the
[lshbluesky 2.5.1 release](https://github.com/lshbluesky/IntelBluetoothFirmware/releases/tag/v2.5.1),
whose source includes the upstream Tahoe changes and later BLE fixes. This is
release provenance, not an independent hash check of the running kext.
The captured IORegistry showed the AX200NativeComposite personality active.
IntelBluetoothFirmware was disabled in the saved config and was not loaded;
the passed-through controller had running firmware. The captured Bluetooth
NVRAM values matched the [BlueToolFixup instructions](https://github.com/acidanthera/BrcmPatchRAM/blob/2.7.2/README.md#bluetoolfixupkext),
including 14 zero bytes for `bluetoothInternalControllerInfo`.

System Information reported `THIRD_PARTY_DONGLE`. Captured controller logs classify
the external adapter with chipset value zero. Inspection of this build's x86-64
`bluetoothd` shows that its WBS, UWBS and stereo capability checks reject that
value. The UWBS Mac branch identifies supported Broadcom chipsets starting with
BCM4364B3. This explains why the wider-band HFP codecs were unavailable in this
run; it does not establish the eligibility rules for the separate studio-quality
recording feature. The log text `disabled by defaults write` also occurs when
these capability checks fail, without a saved disabling preference.

Hackintosh reports describe a separate
[AirPods playback failure on Intel controllers](https://www.reddit.com/r/hackintosh/comments/1sex75t/release_finally_fixed_the_infamous_airpods/),
including an AX200/Pro 3 test. The proposed
[BlueToolFixup FastConnect patch](https://github.com/acidanthera/BrcmPatchRAM/pull/38)
bypasses a failed Bluetooth clock check during A2DP startup. It does not enable
microphone codecs. This trace demonstrates AAC-ELD playback without it.
[Earlier microphone reports](https://www.reddit.com/r/hackintosh/comments/mab9hp/native_intelbluetoothfirmwarekext_engaging/)
describe firmware-dependent HFP failures and warm-boot workarounds; those tests
predate Tahoe's studio-quality feature.

[Apple's firmware notes](https://support.apple.com/en-us/106340) include Pro 3,
macOS 26 and Voice Memos for studio-quality recording. They do not establish
support for an AX200 VM. Apple's
[high-quality recording API](https://developer.apple.com/documentation/avfaudio/avaudiosession/categoryoptions-swift.struct/bluetoothhighqualityrecording)
documents an EU restriction, but that API lists iOS, iPadOS and Mac Catalyst.
Its restriction alone cannot explain native macOS Voice Memos behavior.

Studio-recording eligibility and its required controller commands remain
unresolved. The HFP capability checks alone do not justify an override.
