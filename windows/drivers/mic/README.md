# NTPodsMic — virtual audio (microphone) driver (ACX)

> This hardened build supports the ACX microphone on Windows 11 22H2 or newer.
> The upstream PortCls alternative remains in [`../mic-portcls`](../mic-portcls),
> but does not implement the hardened control contract and is not supported by
> this build's installer or daemon.

The virtual-microphone driver for the [hi-res mic feature](../../docs/hires-mic/PLAN.md):
Windows sees a **NTPods** microphone that the daemon feeds with the AirPods'
decoded AAC-ELD audio, so any app (Teams, Zoom, Discord, OBS…) can use it.

## Origin / license

This is based on the **Microsoft ACX `AudioCodec` sample** from
[microsoft/Windows-driver-samples](https://github.com/microsoft/Windows-driver-samples/tree/main/audio/Acx/Samples)
(**Microsoft Public License, MS-PL**, retained in
[`Microsoft-Public-License.txt`](../../../licenses/Microsoft-Public-License.txt)), chosen because:

- it's a **ROOT-enumerated** (software, no-hardware) audio device — i.e. virtual;
- it already has a **capture circuit** (`Common/CaptureCircuit.cpp`) — a mic;
- it uses **ACX** on top of **KMDF** — the same framework family as our
  `NTPodsAAP` driver (unlike SYSVAD's older PortCls).

It was trimmed to capture-only (the render circuit is gone, so no phantom speaker
grabs the default output) and its audio source swapped for a PCM feed pushed from
user mode over an IOCTL.

## Status

This checkout must be built with the matching daemon and installer. Its driver
changes need the Windows checks below before deployment:

- The complete installer provisions the intended user's SID, signs the test
  package, and creates `ROOT\NTPodsMicACX`. The old single-driver `install.ps1`
  refuses to run because it bypassed that setup.
- [x] **Capture-only** — the render (speaker) circuit was removed from `Device.cpp`.
- [x] **PCM bridge** — `Common/MicPipe.{h,cpp}`: a spin-locked ring buffer and a
  control device `\\.\NTPodsMic` with `IOCTL_NTPODS_MIC_WRITE_PCM`
  (`0x0022A000`). `StreamEngine::ProcessPacket` drains the ring instead of the
  sample's WAV/tone dummy; an underrun reads as silence.
- [x] **Bounded latency** — the uplink and the capture timer run on independent
  clocks, so the ring used to creep up to full and stay there, adding its whole
  capacity as a fixed delay. It now trims back to ~100 ms whenever the
  backlog passes ~160 ms (never below two capture packets), so mic latency stays
  put instead of drifting. 100 ms covers the uplink's jitter: the AirPods send
  30 ms per packet, but gaps of up to ~100 ms between packets are normal, and a
  32 ms target crackled on every late one. Thanks to [@Gab4545](https://github.com/Gab4545) for
  diagnosing it.
- [x] **Capture-activity counter** — `IOCTL_NTPODS_MIC_STATUS` advances while an
  app records, which is how the daemon auto-starts and auto-stops the AAP uplink.
- [x] The capture circuit is restricted to **48 kHz mono 16-bit** (what the decoded
  AirPods stream is resampled to).

The upstream binaries in [`prebuilt/`](prebuilt) do **not** contain this checkout's
control ACL, exclusive-producer, flush, or capture-count changes. Do not package
them with the hardened daemon; build and sign the matching source instead.

## Build

From the repository root, run `./windows/build.ps1` with the toolchain pinned in
[BUILD-WINDOWS.md](../../../docs/BUILD-WINDOWS.md). It builds both drivers, the
daemon and UI, then creates the complete experimental MSI. Builds do not install
anything. Install the generated MSI, or use `install.ps1` from its complete
distribution on a prepared test system. The installer configures both driver
ACLs before starting them; the scripts in this driver source directory do not.

See [the microphone plan](../../docs/hires-mic/PLAN.md) for protocol notes.

## Control bridge contract

The installer provisions `AllowedUserSid` (REG_SZ) under
`HKLM\SYSTEM\CurrentControlSet\Services\NTPodsMicACX\Parameters` before creating
the device. The producer handle allows SYSTEM and that canonical local/domain
account SID only. Missing or invalid configuration allows SYSTEM only. This ACL
does not change normal applications' access to the Windows microphone endpoint.
Only one producer handle can be open.

- `WRITE_PCM` (`0x0022A000`): 2–8192 input bytes, even length, no output. PCM is
  always 48 kHz mono PCM16. Oversized or misaligned writes are rejected.
- `STATUS` (`0x00226004`): no input. A 4-byte output returns the capture activity
  counter. An output of at least 8 bytes additionally returns the number of
  running ACX capture streams as a second DWORD.
- `FLUSH` (`0x0022A008`): no buffers. Clears queued PCM immediately; the endpoint
  stays present and emits silence until fed again. Daemon mute/off transitions
  must stop writes and issue this operation under their producer lock.

Opening or closing the producer clears buffered speech, as do the first capture
start and last capture stop. The ring performs at most two bounded bulk copies
per read/write; silence filling happens outside the spin lock. Existing jitter
limits remain approximately 100 ms target / 160 ms high watermark, with larger
capture packet sizes taken into account. Actual latency and long-call drift
still require hardware measurement.

Portable checks (no driver is loaded):

```sh
c++ -std=c++17 -Wall -Wextra -Werror -fsanitize=address,undefined \
  tests/mic_bridge_test.cpp -o /tmp/ntpods-mic-bridge-test
/tmp/ntpods-mic-bridge-test
```

These exercise the same request validator, SID validator and ring implementation
compiled into the driver. Driver Verifier, producer ACLs, capture transitions,
and multi-client audio behavior also need the isolated Windows test setup.

## Capture timing and sharing

The capture pin exposes one hardware stream (`MaxStreams = 1`). Windows' shared
audio engine distributes that stream to ordinary recording apps. A competing
direct kernel-streaming capture must fail instead of consuming samples meant
for another capture. Concurrent shared-mode Sound Recorder and Discord still
need to be checked with this driver build.

The timer keeps its delay correction in 100 ns units and converts it to PCM byte
position only after subtracting it from elapsed time. Position queries cannot
move backwards after a delayed callback. Overdue completions are queued through
the timer rather than called recursively. Packet numbers and their start
timestamps are published together; the first packet starts at the RUN timestamp,
and a query before the first completion returns `STATUS_DEVICE_NOT_READY`.
The internal packet counter is 64-bit so the schedule survives the public
32-bit counter wrapping during long sessions. RT buffers require exactly two
nonempty packets with lengths that do not split a PCM16 sample.
Each packet is limited to 9,600 bytes (100 ms). The same maximum is advertised
through `DEVPKEY_KsAudio_PacketSize_Constraints2` before the endpoint is exposed.
Longer periods are rejected instead of losing speech to the ring's latency cap.

The circuit exposes no hardware volume node: Windows applies microphone levels
in software. The previous sample volume node only remembered its value and never
changed the samples. Shared-mode level changes still need an installed recording
check; Windows software endpoint volume does not apply to exclusive-mode clients.

Only event-driven hardware capture is supported. A single timer-driven buffer
returns `STATUS_NOT_SUPPORTED`: ACX requires page-aligned, page-divisible storage
at offset zero and incremental filling for that mode, which this packet engine
does not implement. Direct/exclusive timer-driven clients are outside this
build's supported scope. Ordinary shared-mode app compatibility still requires
the Windows checks above; an application's timer-based WASAPI loop is distinct
from the hardware buffer mode used by the shared audio engine.

The portable checks cover the production timing arithmetic, a simulated
30-minute timeline, delayed callbacks, resume offsets, malformed packet layouts,
rejection of 200 ms packets, and lossless buffering at the maximum supported
period with a steady producer. They do not measure the Bluetooth/PCM clock difference or prove that
real capture remains glitch-free. Timer stalls, pause/resume, concurrent apps,
producer exit and Driver Verifier remain hardware validation gates.

These rules follow Microsoft's [ACX streaming contract](https://learn.microsoft.com/en-us/windows-hardware/drivers/audio/acx-streaming),
[packet timestamp contract](https://learn.microsoft.com/en-us/windows-hardware/drivers/ddi/acxstreams/nc-acxstreams-evt_acx_stream_get_capture_packet),
and [pin stream limit](https://learn.microsoft.com/en-us/windows-hardware/drivers/ddi/acxpin/ns-acxpin-acx_pin_config).
