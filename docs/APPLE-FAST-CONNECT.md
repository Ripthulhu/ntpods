# Apple Profile Fast Connect

Profile Fast Connect (PFC) bundles Bluetooth profile discovery and setup into
a short exchange. It assigns L2CAP channels directly, bypassing their normal
connection/configuration requests. NTPods currently uses Windows' ordinary
L2CAP channels; the [decoder](../windows/docs/pfc_decode.py) described here is
an offline analysis tool.

The byte layouts below were checked against three saved connections from
AirPods Pro 3 firmware 9A348 and Tahoe 26.7.1 (25G241), plus that guest's
`bluetoothd`. The executable hash and related audio findings are in the
[Tahoe audio reference](MACOS-AUDIO-CAPTURE-2026-10-12.md).
These captures contain profile setup but no subsequent audio stream.

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

An Echo **request** prefixes the PFC body with its length as LE16. An Echo
**response** starts directly with the PFC header. The observed request is
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
The reader at `0x100454547` multiplies by 100 and updates the same microsecond
delay value used by ordinary AVDTP DelayReport. This field reports delay;
it does not request a smaller playback buffer.

In the representative connection, the host selects local SEID 6 and peer
SEID 3 with `15 01 03 14 01 06`. Both endpoints advertise Apple's AAC-ELD
profile. The selector at `0x1004550B4` and local endpoint lookup in
`0x100133BE9` establish which SEID belongs to which side. The completion
contains A2DP status `04 02 00 00`. There is no RTP or ordinary AVDTP
signaling in these three traces, so they do not verify actual playback from
the PFC configuration or its steady-state latency.

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
Start response. The three captured connections contain no playback packets to
verify the subsequent exchange on the wire.

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
request send time separately from response wait to distinguish driver delays
from later reply handling during the next playback check. Response wait also
includes processing incoming peer commands; it is not a radio-only measurement.

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
