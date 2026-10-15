# Apple Profile Fast Connect

Profile Fast Connect (PFC) bundles Bluetooth profile discovery and setup into
a short exchange. It assigns L2CAP channels directly, bypassing their normal
connection/configuration requests. NTPods currently uses Windows' ordinary
L2CAP channels; the [decoder](../windows/docs/pfc_decode.py) described here is
an offline analysis tool.

Evidence: three saved AirPods Pro 3 connections on firmware 9A348 and Tahoe
26.7.1 (25G241), plus that guest's `bluetoothd`. The captures contain profile
setup but no audio stream. The [Tahoe audio reference](MACOS-AUDIO-CAPTURE-2026-10-12.md)
records the executable hash and audio findings.

## Exchange

Each captured session follows this sequence:

| Transport | Direction | PFC message |
| --- | --- | --- |
| L2CAP Echo request, signaling CID `0001` | Buds → host | `01` Discovery; includes the buds' PFC CID |
| L2CAP Echo response | Host → buds | `02` Discovery response; includes the host's PFC CID |
| Buds' PFC CID | Host → buds | `03` Service descriptors |
| Host's PFC CID | Buds → host | `04` Descriptor response |
| Buds' PFC CID | Host → buds | `05` Service configuration |
| Host's PFC CID | Buds → host | `06` Setup complete, with per-profile status |

The profile exchanges include HFP, AVRCP, A2DP, AACP and GATT. Subsequent AACP,
ATT and AVRCP messages use the advertised CIDs without ordinary L2CAP channel
connection requests. All three sessions report zero profile status values.
Discovery to setup completion took 237, 280 and 446 ms at the captured USB/HCI
boundary. These are setup durations, not acoustic latency or a comparison
against Windows.

The representative case capture has discovery at frames 878/882, descriptors
at 914/927, configuration at 929 and completion at 941. The temporary PFC
channel is closed after the profile traffic begins. Channel IDs are assigned
per connection and must never be treated as fixed service identifiers.

## Framing

PFC messages start with `[message:u8, byte1:u8, version:LE16]`. Version is 5
in these captures. `byte1` is zero; its meaning remains unresolved. The saved
daemon reads these separately at `0x1006B2770`, then dispatches message types
3–6 through the table at `0x1006B29F4`.

An Echo request prefixes the PFC body with its length as LE16. An Echo
response starts directly with the PFC header. The observed request is
44 bytes including that prefix; the response is 42 bytes. These lengths
exclude the L2CAP signaling command header.

Offsets below are relative to the PFC header, after removing the request's
length prefix:

| Offset | Size | Discovery field |
| --- | --- | --- |
| 0 | 4 | Common message header |
| 4 | 2 | Sender's PFC CID, LE16 |
| 6 | 2 | MTU, LE16; 1,024 in the captured discovery |
| 8 | 4 | Profile mask, LE32 |
| 12 | 8 | Discovery flags retained as raw bytes by the decoder |
| 20 | 4 | Base features, LE32 |
| 24 | 4 | SDP database state, LE32 |
| 28 | 2 | Discovery data retained as raw bytes |
| 30 | 4 | Upper half of Apple's internal extended-feature word, LE32 |
| 34 | 4 | Lower half of that word, LE32 |
| 38 | 4 observed | Trailing discovery data |

Messages 3–6 contain sections after the common header:

- Section `01 00`: LE16 byte length, followed by general TLVs.
- Section `02 00`: profile records through the end of the message. Each record
  has a LE32 service mask, LE16 descriptor byte length, then descriptor TLVs.

The captured configuration message (`05`) starts directly with section 2;
section 1 is not required in every message.

TLVs start with a tag byte. Tags below `80` use a one-byte length; tags with
bit 7 set use a LE16 length. The key is `tag & 7F`. Values consume exactly the
declared length. Descriptor readers use little-endian integers, established
by reader construction at `0x1006AE2FD` and the helper at `0x1004F182E`.
Lengths apply to values, excluding the tag and length bytes. Empty profile
descriptors are valid; unknown tags must retain their boundaries.

Service masks observed are `00000001` HFP, `00000008` AVRCP, `00000010` A2DP,
`00080000` AACP and `00100000` GATT. Common descriptor keys include `01`
channel configuration, `03` version and `04` status. Channel configuration
is seven bytes: `[CID:LE16, PSM:LE16, MTU:LE16, mode:u8]`.

## A2DP descriptors

| Key | Value |
| --- | --- |
| `01` | Signaling channel configuration |
| `03` | Common version, LE16 |
| `10` | AVDTP version, LE16 |
| `11` | Endpoint count, endpoint descriptors and capability lists |
| `12` | Receiver delay, LE16 in tenths of a millisecond |
| `13` | Media channel configuration |
| `14` | Sender's selected SEID, one byte |
| `15` | Peer's selected SEID, one byte |
| `04` | Setup status, LE16 |

Key `11` contains a one-byte count, that many standard two-byte AVDTP SEP
descriptors, then one length-prefixed capability list per endpoint. Each list
uses a one-byte length and ordinary AVDTP category/length/value records.
The saved parser implements this at `0x100454A2C`–`0x100454FCC`.

The buds' descriptor contains `12 02 DC 05`: 1,500 tenths, or 150 ms.
Descriptor dispatch at `0x100454372` (table `0x100454A1C`) selects reader
`0x100454547`, which multiplies by 100 to convert to microseconds and calls
stream-latency setter `0x100482D10`. Ordinary AVDTP DelayReport uses the same setter at
`0x10044F7FA`–`0x10044F85F`. This field reports delay; it does not request a
smaller playback buffer.

In the representative connection, the host selects local SEID 6 and peer
SEID 3 with `15 01 03 14 01 06`. Both endpoints advertise Apple's AAC-ELD
profile. The selector at `0x1004550B4` and local endpoint lookup in
`0x100133BE9` establish which SEID belongs to which side. The completion
contains A2DP status `04 02 00 00`. The three traces contain no RTP or ordinary
AVDTP signaling, so playback and steady-state latency remain unverified.

## Starting playback

In the saved daemon, PFC and ordinary AVDTP setup use the same codec-configuration
builder at `0x100482288`, called from `0x10045514B` and `0x100448BBF` respectively.
`OI_AVDTP_FastConnectSetConfig` installs the selected configuration in the
endpoint's Open state (`3`) at `0x100133EA4`; Streaming is state `4`.

PFC completion and ordinary Open confirmation reach the same playback-start
state. Its handler at `0x1004488E1` calls `AVDTP_Start` at `0x10012B5E9`, which
sends ordinary signal `07` and waits for acceptance. The accepted response
advances the endpoint to Streaming. Both setup paths also use the same dynamic
latency eligibility checks before common audio setup at `0x100448FB2`.
This static path establishes no PFC-specific buffer setting or bypass of the
Start response; the captures do not verify the playback exchange on the wire.

## Windows boundary

[`BRB_L2CA_PING`](https://learn.microsoft.com/en-us/windows-hardware/drivers/ddi/bthddi/ns-bthddi-_brb_l2ca_ping)
can send Echo payloads and return their replies. WDK 10.0.26100 defines
`MAX_L2CAP_PING_DATA_LENGTH` as 44 bytes, which fits the captured discovery.
However, the discovery handler already creates channel state. A zero CID or
empty service selection has not been established as a harmless query.

[`BRB_L2CA_OPEN_CHANNEL`](https://learn.microsoft.com/en-us/windows-hardware/drivers/ddi/bthddi/ns-bthddi-_brb_l2ca_open_channel)
takes a PSM and negotiates a Windows-owned channel, returning an opaque handle.
The documented interface cannot adopt PFC's chosen local/remote CIDs or mark
a channel open without negotiation. The documented
[callback indications](https://learn.microsoft.com/en-us/windows-hardware/drivers/ddi/bthddi/ne-bthddi-_indication_code)
also provide no hook for unsolicited Echo requests, which initiate all three
saved sessions.

Full PFC therefore needs integration below the current profile-driver API,
including CID ownership, channel cleanup and packet routing. Injecting Echo
bytes alone would leave Windows without the channel state needed to use the
reply. A transport filter would also have to coordinate HCI controller credits
with BthPort. No runtime PFC probe or driver path is enabled in NTPods.

Normal pause/resume already reuses the selected endpoint and configuration.
PFC's effect on repeated `START` response time remains unverified. NTPods logs
request send time separately from response wait, which includes processing
incoming peer commands and is not a radio-only measurement.

## Relationship to Windows Swift Pair

[Swift Pair](https://learn.microsoft.com/en-us/windows-hardware/design/component-guidelines/bluetooth-swift-pair)
provides nearby discovery and a Windows pairing notification. It provides no PFC
channel control or reduction in an active stream's delay.

Swift Pair requires a Microsoft manufacturer advertisement (`0x0006`) while the
peripheral is in pairing mode. Its BR/EDR-only format includes the Classic address;
the dual-mode format instead requires Secure Connections on both LE and BR/EDR,
with Windows pairing over LE and deriving the Classic keys. The LE address must
remain stable during the Swift Pair session. The identifying information must be
in the advertisement itself, since Windows does not request a scan response.

AirPods proximity reports use Apple manufacturer data (`0x004C`, type `0x07`)
from a private LE address. The [battery decoder](WINDOWS-AIRPODS-CONTROL-PROTOCOL.md#battery-over-aap-and-ble)
resolves the address and decodes battery data using keys from an existing paired
AAP connection. Public fields in a complete `07 19`, subtype `01` record are:

| Bytes, relative to `07 19` | Field |
| --- | --- |
| 3–4 | Model, big-endian |
| 5, mask `0x44` | Either bit set means lid state is present |
| 8, mask `0x08` | Set means closed |

NTPods offers pairing on an explicit open report from a known earbud model.
These fields do not establish battery readings or device identity.

Before pairing, a matching model, name or signal strength does not establish
which Classic device sent an advertisement. Two Pro 3 captures also contain
public-address subtype `07` reports whose address matches the Classic peer, but
no verified field links those reports to an earlier private subtype `01` report.
Subtype `07` does not share the normal lid/battery layout and is excluded here.

Native Swift Pair requires the peripheral's Microsoft payload; PFC supplies no
such advertisement. Windows' [advertisement publisher](https://learn.microsoft.com/en-us/uwp/api/windows.devices.bluetooth.advertisement.bluetoothleadvertisementpublisher)
transmits local advertisements and cannot submit a received Apple advertisement
to Swift Pair. No route with unchanged AirPods firmware has been demonstrated.
A relay remains untested, including device identity, pairing-mode lifetime and
radio behavior.

### NTPods pairing

NTPods uses ordinary Windows pairing and profile setup, with no PFC, native Swift
Pair integration or advertisement relay. A
[Classic pairing-state selector](https://learn.microsoft.com/en-us/uwp/api/windows.devices.bluetooth.bluetoothdevice.getdeviceselectorfrompairingstate)
and `DeviceWatcher` enumerate Windows `AssociationEndpoint` devices. Names are
discovery hints and are never converted into addresses. After pairing, the backend
verifies the selected Classic device's paired status and AAP service before
connecting; it cannot fall back to an older pair.

| Entry point | Selection and timing |
| --- | --- |
| Header menu: Pair AirPods | Native dialog with a manual list, including renamed devices. Discovery lasts 30 seconds; Search again restarts it. Removed or newly paired devices leave the list. |
| Public open-lid report | Shows the card before the pairing gesture. Connect shows model-specific instructions and discovers Classic endpoints. A candidate must have AirPods in its name, pass the device-class filter and remain the sole match for two seconds; Windows may delay enumeration completion. Multiple matches wait for other cases to close. |
| Background Classic discovery | Runs for 15 seconds, pauses for 45 seconds, including in the tray. An AirPods endpoint opens the card at most once per device per app session; Connect pairs that endpoint. Discovery pauses while a card or dialog is open, during microphone use, and during shutdown. |
| Pairing request | One 60-second deadline covers card discovery and pairing. Temporary not-ready, connection and authentication failures retry the same endpoint every two seconds while gesture instructions remain visible. Manual pairing also times out after 60 seconds. |
| Card lifetime | Dismisses after 20 seconds, paused during interaction or pairing. Closing the card cancels its request. A closed-lid report cancels only the lid invitation for that model; reopening can offer pairing again. |

Known non-headphone device classes are excluded from automatic selection.
Renamed devices without AirPods in their name use the manual dialog. A public lid
report supplies only the model hint; Windows discovery supplies the endpoint.
Invitations are bounded by model and lid visit to avoid duplicate cards from two
buds. Public lid reports need no saved keys. Reports resolved to the selected
paired device retain battery and reconnection handling; existing pairs use
Connect. LE scanning pauses during microphone capture or uplink, and scanning
while connected remains opt-in.

`DeviceInformation.Pairing.Custom.PairAsync` accepts `ConfirmOnly` for the chosen
endpoint while the request is active. It accepts no PIN ceremonies, removes no
existing pairings, and detaches its handler on completion or cancellation. Basic
`DeviceInformationPairing.PairAsync` is
[unsupported in desktop apps](https://learn.microsoft.com/en-us/windows/apps/desktop/modernize/winrt-api-desktop-app-support#methods).
The [custom pairing API](https://learn.microsoft.com/en-us/windows/apps/develop/devices-sensors/pair-devices#custom-pairing)
lets the desktop app supply that confirmation handler.

## Decode a capture

Select one complete, reassembled PFC payload from TShark/Wireshark. Exclude the
HCI, L2CAP and signaling headers; retain the inner length for Echo requests.
Pass its hexadecimal bytes to the offline helper:

```powershell
# Synthetic A2DP descriptor carrying a 150 ms delay.
python windows/docs/pfc_decode.py profile '0400050002001000000004001202dc05'
python windows/tests/test-pfc-decode.py
```

Use `echo-request` or `echo-response` instead of `profile` for discovery.
With no hex argument, the helper reads one payload from standard input.
It preserves unknown field values and rejects truncated fields, inconsistent
lengths, unsupported section types and versions other than 5. It does not
reassemble transport fragments or infer per-connection channel mappings.
Decoded AACP descriptors can contain device identifiers; keep complete capture
output outside Git and use synthetic fixtures in tests.

The pinned [Apple Wireshark dissector](https://github.com/pabloaul/apple-wireshark/tree/2e9822247195fad2f79c729010308c33a29546ea/plugins/fastconnect)
helps locate traffic, but its message names differ from the verified wire
types, it truncates two bytes from Echo responses, and its TLV parser assumes
one-byte lengths. Verify the raw payload before interpreting its labels.
Background: [Apple's profile setup patent](https://patents.google.com/patent/US20190373430A1/en)
and [ToothPicker, appendix A.1.2](https://www.usenix.org/system/files/woot20-paper-heinze.pdf).
