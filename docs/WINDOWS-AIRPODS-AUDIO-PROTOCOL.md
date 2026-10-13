# AirPods audio protocol

This describes the playback and microphone paths implemented in NTPods through
0.2.19, checked against source on 2026-10-13. It is a reference for comparing a
macOS capture with NTPods. Implemented behavior, local hardware results and
unresolved fields are identified separately.

The earlier [microphone research](WINDOWS-MIC-PROTOCOL-RESEARCH.md) preserves
pinned upstream sources and the history of the clock and recovery assumptions.
[Windows validation](WINDOWS-VALIDATION.md#native-playback-driver-and-bitrate-controls)
records hardware results; [model support](WINDOWS-MODEL-SUPPORT.md) owns the
capability list.

## Channels and ownership

| Path | L2CAP PSM | What crosses it |
| --- | --- | --- |
| AAP/AACP control and microphone | `0x1001` | Settings, notifications, microphone start/stop and compressed uplink |
| AVDTP signaling | `0x0019`, first channel | Endpoint discovery, codec configuration, stream state |
| AVDTP media | `0x0019`, separate channel | RTP and compressed stereo playback |
| ATT | `0x001F`, separate channel | Other device features; not the playback or microphone transport below |

The AAP kernel driver opens these channels through the Windows Bluetooth stack.
Playback signaling and media are authenticated and encrypted, with separate
handles from AAP and ATT. Media uses a negotiated 200 ms L2CAP flush timeout;
signaling requests an infinite flush timeout. This is a stale-packet limit, not
an end-to-end latency measurement. The accepted media MTU determines packet
size; the host's receive MTU request is at most 8,192 bytes.

The AudioSink driver supplies 48 kHz stereo signed 16-bit PCM to the backend.
The backend encodes and packetizes it. The microphone takes the reverse path
through a separate 48 kHz mono signed 16-bit PCM endpoint. These PCM formats and
Windows engine periods do not describe the Bluetooth wire format.

Sources: [L2CAP transport](../windows/drivers/aap/L2cap.c),
[playback channels](../windows/drivers/aap/Playback.c),
[bridge contract](../windows/drivers/aap/PlaybackContract.h),
[playback overview](../windows/playback/README.md).

## Playback negotiation

NTPods opens signaling, sends `DISCOVER`, then queries unused audio sink SEIDs
with `GET_CAPABILITIES`. The selected sink SEID comes from the reply; the local
source SEID is 1. NTPods does not assume the earbuds use a fixed sink SEID.

The sequence is `SET_CONFIGURATION` → `OPEN` → open media channel → `START`
when Windows playback starts. `SUSPEND` pauses the stream when playback stops.
Cleanup sends `CLOSE` for an open stream or `ABORT` for partial configuration,
then closes both channels. Codec or bitrate changes open a new stream on the
same Windows output.

Single-packet signaling has the following header:

| Byte | Meaning |
| --- | --- |
| 0 | High nibble: transaction label; bits 3–2: packet type, required to be 0; bits 1–0: command 0, general reject 1, accept 2, reject 3 |
| 1 | Signal ID in low six bits; reserved upper bits must be 0 |
| 2 onward | Signal payload; SEIDs occupy the upper six bits of their byte |

Implemented signal IDs are `01` Discover, `02` Get Capabilities, `03` Set
Configuration, `04` Get Configuration, `05` Reconfigure, `06` Open, `07` Start,
`08` Close, `09` Suspend, `0A` Abort and `0C` Get All Capabilities. Requests wait
up to two seconds for a matching label and signal. Fragmented signaling is
rejected. Peer commands are handled while awaiting replies and while streaming;
a peer Suspend or Close ends the current playback run. A peer Start already
accepted in the open state must not cause a duplicate Start request.

Capabilities use `[category:u8, length:u8, value:length]` records. Category 1 is
empty Media Transport. Category 7 begins with media type `00` and codec type
`02` for AAC or `FF` for vendor codecs. Truncated records and duplicate known
categories are rejected before configuration.

When AAC-ELD is selected, NTPods uses it only when a sink advertises the
implemented profile. Otherwise it tries AAC after examining the sinks. An invalid
manual ELD bitrate, malformed response or transport error is an error, not a reason to
silently change codec. Best effort selects the highest implemented rate within
the advertised limit and packet budget; it does not adapt to radio conditions.

Sources and parser tests: [session.rs](../windows/daemon/src/playback/session.rs),
[protocol.rs](../windows/daemon/src/playback/protocol.rs),
[AAC capabilities](../windows/daemon/src/playback/aac_protocol.rs),
[codec policy](../windows/daemon/src/playback/codec.rs).

## AAC playback

| Item | NTPods value |
| --- | --- |
| Encoder | FFmpeg AAC-LC; 48 kHz, stereo, 1,024 samples/channel per AU |
| AudioSpecificConfig | `11 90`; encoder output may append `56 E5 00`, explicitly declaring no SBR |
| Bitrate range | 96–512 kbps in 1 kbps steps, limited by peer capability |
| Codec capability data | Six bytes: object type, frequency/channel bits, bitrate; 48 kHz stereo requires byte 2 mask `84` |
| Object type choice | `80` when offered, otherwise `40`; MPEG-4-only encoder tools are disabled |
| Bitrate field | Lower 23 bits of bytes 3–5, big-endian; zero peer limit means unspecified |
| RTP | Version 2, payload type 96, sequence starts at 0, SSRC 1 |
| RTP clock | 90 kHz; timestamp advances 1,920 per AU, wrapping at 32 bits |
| Frame duration | 1,024 / 48,000 seconds, about 21.33 ms |

The payload is a LATM AudioMuxElement. NTPods repeats StreamMuxConfig for each
AU: `useSameStreamMux=0`, `audioMuxVersion=0`, one program/layer/subframe,
`allStreamsSameTimeFraming=1`, ASC `11 90`, `frameLengthType=0`, fullness `FF`,
no other data and no CRC. The payload length uses repeated `FF` bytes followed
by the remainder, then the AU bits. The final byte is zero padded.

An element larger than the media MTU is split after the 12-byte RTP header.
Fragments share the timestamp; sequence increments for every packet and the
marker bit is set only on the final fragment. One AU is bounded to 1,536 bytes.
NTPods uses the RTP clock and fragmentation described by
[RFC 6416](https://www.rfc-editor.org/rfc/rfc6416.html).

Sources and fixtures: [aac_protocol.rs](../windows/daemon/src/playback/aac_protocol.rs),
[AAC encoder](../windows/daemon/src/playback/aac_shim.c),
[encoder tests](../windows/daemon/tests/aac_shim_test.c).

## Apple AAC-ELD playback

The selected category-7 value is `00 FF` followed by this 14-byte vendor data at
256 kbps:

```text
4C 00 00 00  01 80  00 80 00 84 00 83 E8 00
```

`4C 00 00 00` is vendor `0x004C` and `01 80` is codec `0x8001`, both
little-endian. The parser requires data byte 7 bit `80`, byte 9 mask `84`, and
byte 11 bit `80`. The low 23 bits of data bytes 11–13 carry the big-endian
bitrate. NTPods copies the remaining profile bytes from the Pro 3-tested
[USBPods implementation](https://github.com/han-um/USBPods-Pico2W-2-way-audio-implement/blob/d18d083bc52de3143d8d922c4abaae865cc0dc28/src/btstack/btstack_avdtp_source.c);
it does not assign meanings to all their reserved bits.

| Item | NTPods value |
| --- | --- |
| Encoder | FDK AOT 39 / ER AAC ELD, 48 kHz stereo, 480 samples/channel per AU |
| AudioSpecificConfig | `F8 E6 50 00`, no SBR |
| Bitrate | CBR 128–256 kbps in 1 kbps steps; optional experimental VBR up to 256 kbps |
| RTP header | `80 60`, big-endian sequence and timestamp, SSRC `11 22 33 44` |
| RTP clock | 48 kHz sample count; timestamp advances 960 per two-AU packet |
| AU geometry | 480 samples = 10 ms; two AUs = 20 ms per media packet |
| Bounds | Exactly two AUs; each nonempty and at most 321 bytes; complete packet must fit media MTU |

The optional variable profile uses FDK quality mode 5 with a 256 kbps peak limit.
It retains the same ASC, packet framing and 321-byte AU limit. Mode 5 has a
nominal stereo rate of 192 kbps, but does not target that average: actual bitrate
depends on the signal. Negotiation requires the full 256 kbps packet budget;
otherwise the profile reports an error. Encoder resets preserve the selection.
Existing preferences default to CBR, and normal AAC settings are unchanged.
The macOS encoder is a different implementation, so matching its nominal rates
does not establish matching fidelity or latency. Hardware VBR tests are pending.

Each AU immediately follows this four-byte header:

```text
0xB0 | ((au_sequence >> 8) & 0x0F), au_sequence & 0xFF,
0x10 | ((au_length   >> 8) & 0x0F), au_length   & 0xFF
```

The AU sequence starts at 1 and wraps at 12 bits. RTP sequence starts at 0 and
wraps at 16 bits. There is no LATM wrapper or AAC fragmentation on this path.
The `B` and `1` high nibbles follow USBPods. NTPods retains a fixed SSRC;
whether the earbuds require those exact values remains unverified.

The maximum bitrate allowed by the MTU is
`min(floor((MTU - 20) / 2) - 1, 320) * 800` bits/s, with subtraction clamped
at zero. The extra byte allows CBR rounding. At an MTU of 672, two 321-byte AUs
fit in 662 bytes. If Best effort needs a lower rate after opening media, NTPods
sends `RECONFIGURE` before Start. Manual settings that do not fit fail.

On Suspend/restart, the ELD packetizer discards partial AU pairs and resets its
AU sequence and timestamp; RTP packet sequence continues within that session.
The encoder is recreated. NTPods clears queued PCM and primes playback with a
50 ms scheduling lead. Its frame clock counts samples from one origin, avoiding
rounding drift from repeated millisecond delays. These host choices still need
comparison with macOS packet pacing and buffering.

Sources and fixtures: [protocol.rs](../windows/daemon/src/playback/protocol.rs),
[packetizer clocks](../windows/daemon/src/playback/codec.rs),
[stream loop](../windows/daemon/src/playback/mod.rs),
[encoder settings](../windows/playback/encoder/encoder.c),
[encoder tests](../windows/playback/encoder/test.py).

## High-quality microphone uplink

The ordinary AAP session performs its handshake, feature declaration and
notification subscription before microphone use; those constants are in
[aap.rs](../windows/daemon/src/aap.rs). The microphone commands are:

```text
START: 04 00 04 00 58 00 00 00 09 00 00 01 82 00 00 00 04 96 00
STOP:  04 00 04 00 58 00 00 00 02 00 03 01
```

NTPods recognizes microphone messages by prefix `04 00 04 00 58 00`. Uplink
audio additionally requires bytes 6–7 to be `01 00` and at least 22 header bytes.
The remaining header fields are not decoded. Starting at offset 22, records are:

| Relative offset | Value |
| --- | --- |
| 0–3 | Unsigned 32-bit little-endian device timestamp |
| 4 | AU byte length, 1–255 |
| 5 onward | One raw AAC-ELD AU; the next record follows immediately |

NTPods validates the whole SDU before delivering any AU. A truncated record,
zero length or partial tail discards the entire SDU. The parser accepts varying
AU counts; four AUs per SDU was an observed upstream pattern, not a fixed rule.
There is no decoded microphone-start or microphone-stop acknowledgement.
Successful command transmission proves only transport acceptance. Valid decoded
PCM confirms readiness; silence after Stop does not prove stereo playback works.

The decoder uses ASC `F8 E6 30 00`: AOT 39, mono, 48 kHz coding configuration,
480 samples per decoded AU. FFmpeg output must be mono float at 48 kHz with 480
finite samples and no corruption flags. NTPods applies gain 3 with a `tanh`
limiter and resamples using the inherited **64 kHz presentation-clock
assumption** to the 48 kHz Windows input. This gives about 360 output samples
per AU, or 7.5 ms, after resampler delay. The ASC alone does not establish that
clock. Local speech pitch/speed checks support it; a calibrated long recording
and macOS trace are still needed.

Wire timestamps currently feed aggregate delta statistics only. Their unit,
reset conditions, legal repeats and wrap semantics are unresolved; NTPods does
not use them to discard, reorder or conceal audio. A two-second packet gap,
invalid SDU or decode failure clears decoder/resampler history and queued PCM.
A fresh decoder primes 3,840 silent output samples (80 ms). Valid silent PCM
counts as progress; amplitude does not determine stream health.

Sources and tests: [demux](../windows/daemon/src/aap.rs),
[decoder wrapper](../windows/daemon/src/eld.rs),
[decoder/resampler](../windows/daemon/src/eld_shim.c),
[format validation tests](../windows/daemon/tests/eld_shim_test.c),
[receive integration](../windows/daemon/src/main.rs).

## Microphone lifecycle and playback recovery

The lifecycle is `Idle → Starting → Capturing → Stopping → Recovering → Idle`
with an Error state for failed starts, PCM stalls or failed recovery.
Automatic mode follows capture-client count and an optional stop delay; Keep
active retains the stream without clients; Manual follows explicit intent.
Disabling the feature, losing permission or both buds being confirmed out closes
the PCM gate immediately. The Windows input stays present and supplies silence.
One worn bud permits capture; unknown wear reports retain confirmed wear state.

Starting and Capturing require decoded PCM progress within five seconds. The
first deadline begins after Start leaves the Bluetooth queue. A failure sends
Stop and performs bounded cleanup; it cannot restart indefinitely while the
same capture request remains active.

Stop waits for 500 ms without uplink, allowing a two-second drain window. If
audio continues, NTPods retries Stop once with one further two-second window.
After an attempted start, even quiet uplink requires one playback recovery.
With NTPods native playback, recovery suspends AVDTP, recreates the encoder,
clears queued PCM and restarts the stream if playback was active. The request
has an eight-second bound. It keeps the AudioSink driver and endpoint present.
The older Windows/Alternative A2DP path uses a targeted audio-service reset.

After recovery, the lifecycle again allows queued SDUs to drain. Failed recovery
or continued uplink triggers one targeted reconnect and requires an explicit
microphone retry. Device identity and request generation prevent a delayed
recovery from acting on a newly selected pair.

Removing both buds while recording stops the stream. Automatic and Keep active
may restart after reinsertion and completed recovery; Manual intent is cleared
on removal. Resetting only the decoder did not fix the observed reinsertion
static. The stop/recovery/new-start sequence did, on the earlier Alternative
A2DP setup. Reinsertion remains unverified with native playback.

Conversation Awareness currently restores host volume ducking when capture
starts and skips host ducking during capture. NTPods does not temporarily write
device control `0x28`. Whether macOS changes that setting, and how it restores
an intervening user change, are open comparison questions.

Sources and transition tests: [mic_lifecycle.rs](../windows/daemon/src/mic_lifecycle.rs),
[main.rs](../windows/daemon/src/main.rs), [recovery routing](../windows/daemon/src/bt.rs),
[native reset](../windows/daemon/src/playback/mod.rs),
[PCM bridge tests](../windows/drivers/mic/tests/mic_bridge_test.cpp),
[playback transport tests](../windows/drivers/aap/tests/playback_transport_test.cpp).

## Hardware evidence and macOS comparison

Pro 3 on firmware 9A348 and Pro 1 on 6F21 passed AAC 256 kbps and AAC-ELD
128/192/256 kbps stereo checks on Intel AX200. Both completed 70 seconds of ELD
at 256 kbps, crossing the 12-bit AU sequence wrap. Pro 3 also passed short
simultaneous stereo playback/microphone captures and post-stop stereo checks
with both codecs. Recorded native recovery was about 1.8 seconds. Pro 1's
high-quality microphone remains disabled.

Other listed playback models and newer microphone-eligible models are
source-supported candidates, not locally verified hardware. ELD advertisement
does not establish microphone support. Long native sessions, reinsertion,
sleep/resume and repeated failure recovery remain open in the validation record.

For each macOS trace, keep model, firmware, macOS version, adapter and application
with the observation. Answer these questions before changing the implementation:

1. Does macOS use AAP `0x58` for microphone capture on this adapter, or HFP/SCO?
   An application reporting a high sample rate cannot establish the transport.
2. Which AVDTP codec/profile bytes does it select before, during and after
   capture? Does it use plain ELD, ELD with SBR, AAC-LC or another transport?
3. How many ELD AUs appear per packet? What are the AU flags, counter resets,
   SSRC, RTP increments, MTU and packet spacing across start and suspend?
4. Do microphone Start/Stop differ, and is there an acknowledgement or another
   control message before capture becomes usable? What changes in header bytes
   8–21 and device timestamps during pauses, bud handoff and reinsertion?
5. Does microphone Stop cause AVDTP Suspend/Start, Close/Open, codec
   reconfiguration, a route message or a link disconnect? Does macOS keep the
   microphone armed between recording clients?
6. Does macOS change Conversation Awareness, microphone-side selection or gain?
   Compare a baseline where the user leaves those settings unchanged.

The [Tahoe capture](MACOS-AUDIO-CAPTURE-2026-10-12.md) observed AAC-ELD playback
and ordinary CVSD HFP recording. It did not exercise the AAP high-quality
microphone route or establish a replacement for its stop/recovery sequence.
Compare complete event sequences, not isolated matching bytes. Keep recordings,
raw captures, Bluetooth addresses and pairing keys outside the repository;
commit only reviewed field descriptions and synthetic parser fixtures.
