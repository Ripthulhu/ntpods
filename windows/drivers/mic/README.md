# NTPods ACX microphone driver

The microphone driver exposes a 48 kHz mono PCM16 capture endpoint on Windows 11
22H2 or newer. The daemon decodes AirPods AAC-ELD audio and feeds the driver;
recording apps use the Windows endpoint. See the
[microphone architecture](../../docs/hires-mic/PLAN.md) for the audio path.

Playback uses the separate `NTPodsAudio` driver on the AirPods AudioSink service.
Its shared render code lives in this directory's `Common/Render*` files; the
microphone device itself has only a capture circuit. See
[AirPods playback](../../playback/README.md).

The upstream [PortCls alternative](../mic-portcls) is retained for reference.
The current daemon and installer do not support it.

## Origin and license

This driver is based on Microsoft's
[ACX AudioCodec sample](https://github.com/microsoft/Windows-driver-samples/tree/main/audio/Acx/Samples),
which provides a ROOT-enumerated software device, a capture circuit and ACX on
KMDF. The sample's Microsoft Public License is retained in
[Microsoft-Public-License.txt](../../../licenses/Microsoft-Public-License.txt).
See [THIRD_PARTY.md](../../../THIRD_PARTY.md) for the unresolved licensing of
NTPods' additions.

NTPods replaces the sample audio source with PCM supplied over an IOCTL. Build
the matching driver, daemon and installer together using
[BUILD-WINDOWS.md](../../../docs/BUILD-WINDOWS.md). The complete installer creates
`ROOT\NTPodsMicACX` and provisions access before loading the driver. The old
single-driver installer refuses to run; [prebuilt](prebuilt/README.md) binaries
do not implement the current contract.

## Control bridge

The installer sets `AllowedUserSid` (REG_SZ) under
`HKLM\SYSTEM\CurrentControlSet\Services\NTPodsMicACX\Parameters`.
The `\\.\NTPodsMic` producer handle permits SYSTEM and that canonical local/domain
account SID. Missing or invalid configuration permits SYSTEM only. This ACL does
not restrict recording apps' ordinary access to the Windows microphone endpoint.
Only one producer handle can be open.

| Request | Input | Output |
| --- | --- | --- |
| `WRITE_PCM` (`0x0022A000`) | 2–8192 bytes, even length, 48 kHz mono PCM16 | None |
| `STATUS` (`0x00226004`) | None | 4-byte capture activity counter; at least 8 bytes also returns running capture-stream count |
| `FLUSH` (`0x0022A008`) | None | None |

Invalid sizes are rejected. The status counter advances while an app records,
allowing the daemon to detect capture demand. `FLUSH` clears queued speech while
keeping the endpoint present; it emits silence until PCM arrives again. Daemon
mute and stop transitions must stop writes and flush under the producer lock.

Opening or closing the producer, starting the first capture, and stopping the
last capture also clear the ring. Reads and writes use at most two bounded
copies under the spin lock; underrun silence is filled after releasing it.

The jitter buffer normally trims a backlog above 160 ms back to 100 ms.
For larger capture packets, the target holds at least two packets and the high
watermark at least three. These limits accommodate bursty uplink packets without
allowing clock drift to grow the delay indefinitely. Thanks to
[@Gab4545](https://github.com/Gab4545) for diagnosing the original buffer buildup.

## Capture timing

The capture pin permits one hardware stream (`MaxStreams = 1`). Windows' shared
audio engine distributes it to recording apps. Competing direct kernel-streaming
capture must fail instead of consuming samples intended for another stream.

The timer tracks delay correction in 100 ns units and converts it to PCM byte
position after subtracting it from elapsed time. Position queries cannot move
backwards after a delayed callback. Overdue completions are queued through the
timer rather than called recursively. Packet numbers and start timestamps are
published together. The first packet starts at RUN; a query before its completion
returns `STATUS_DEVICE_NOT_READY`. The internal packet counter is 64-bit.

RT buffers require exactly two nonempty packets aligned to PCM16 samples. Each
packet is at most 9,600 bytes (100 ms). The same maximum is advertised through
`DEVPKEY_KsAudio_PacketSize_Constraints2`; longer periods are rejected. The shared
constraint helper advertises a 4 ms minimum and a 5 ms RAW processing quantum.
These advertised values do not establish the period Windows chooses for a client.

Only event-driven hardware capture is supported. A single timer-driven buffer
returns `STATUS_NOT_SUPPORTED`: this engine does not implement ACX's required
page-aligned storage and incremental filling for that mode. An application's
timer-based WASAPI loop is distinct from the hardware buffer mode used by the
shared audio engine.

The circuit has no hardware volume node. Windows applies microphone levels in
software for shared-mode clients; exclusive-mode clients bypass that processing.

These rules follow Microsoft's [ACX streaming contract](https://learn.microsoft.com/en-us/windows-hardware/drivers/audio/acx-streaming),
[packet timestamp contract](https://learn.microsoft.com/en-us/windows-hardware/drivers/ddi/acxstreams/nc-acxstreams-evt_acx_stream_get_capture_packet),
and [pin stream limit](https://learn.microsoft.com/en-us/windows-hardware/drivers/ddi/acxpin/ns-acxpin-acx_pin_config).

## Shared playback bridge

`Common/RenderPipe.*` is compiled into the separate AudioSink playback driver.
It exposes one `\\.\NTPodsPlayback_<12-digit-address>` control device per pair,
with an exclusive backend handle and the installed-user/SYSTEM access policy.
Normal applications use the Windows output endpoint.

| Request | Input | Output |
| --- | --- | --- |
| `READ_PCM` (`0x00226400`) | None | Buffer: 4–1920 bytes, divisible by 4; returns 0 to capacity bytes of available 48 kHz stereo PCM16 |
| `STATUS` (`0x00226404`) | None | Exactly 48 bytes |
| `SET_PROVIDER` (`0x0022A408`) | Two DWORDs: version `1`, ready `0` or `1` | None |

The version-3 status layout is eight DWORDs (version, sample rate, channels, bits
per sample, running streams, queued frames, dropped frames, generation), followed
by a 64-bit Bluetooth address and two DWORDs (provider ready, device online).
The backend checks the address before routing audio.

Every queue reset advances its generation; the backend must discard pending PCM
and encoded audio when it changes. The ring holds at most 100 ms and drops oldest
frames on overflow. Reads complete immediately. Provider closure, the last render
pause, power-down and removal clear queued audio. A provider marked unready stops
accepting PCM without removing an already available endpoint during codec changes.

The authoritative layouts are in [`RenderPipe.h`](Common/RenderPipe.h), with
provider and buffering rules in `RenderProvider.h` and `RenderRing.h`.

## Checks

The full Windows build runs the bridge, render and packet-constraint tests.
A portable microphone check can also run from this driver directory:

```sh
c++ -std=c++17 -Wall -Wextra -Werror -fsanitize=address,undefined \
  tests/mic_bridge_test.cpp -o /tmp/ntpods-mic-bridge-test
/tmp/ntpods-mic-bridge-test
```

It exercises production request/SID validation, ring buffering, timing arithmetic,
delayed callbacks, resume offsets and malformed packet layouts without loading
the driver. It cannot measure Bluetooth clock drift or capture glitches.

Installed checks must cover producer ACLs, concurrent shared-mode recording,
level changes, pause/resume, producer exit, sleep/removal and Driver Verifier.
See [WINDOWS-VALIDATION.md](../../../docs/WINDOWS-VALIDATION.md) for hardware coverage.
