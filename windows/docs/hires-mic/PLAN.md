# AirPods microphone on Windows: implementation history

This records the initial `windows-hires-mic` work and its hardware checks.
Some lifecycle behavior and installation paths have since changed. See
[Windows features](../../../docs/WINDOWS-FEATURES.md) and
[validation results](../../../docs/WINDOWS-VALIDATION.md) for the current build.

The feature exposes the AirPods' AAC-ELD uplink as a Windows microphone endpoint,
using the protocol described in
[LibrePods PR #655](https://github.com/librepods-org/librepods/pull/655).

## Driver choice

A virtual audio driver supplies the Windows capture endpoint. The plan initially
considered SYSVAD; the implementation used Microsoft's ACX `AudioCodec` sample,
which already had a ROOT-enumerated device and a capture circuit. ACX runs on
KMDF, also used by the NTPodsAAP Bluetooth driver. Both NTPods drivers require
test signing in the development setup.

## Architecture

```
AirPods ──AAP / L2CAP──▶ NTPodsAAP driver ──IOCTL──▶ app
                                                         │  decode AAC-ELD (FFmpeg / libavcodec)
                                                         │  → PCM
                                                         ▼
                                        NTPodsMic virtual audio driver
                                                         │
                                                         ▼
                                   Windows sees a "NTPods Microphone"
                                   (Teams / Zoom / Discord / OBS / …)
```

The enable command, uplink framing and AAC-ELD parameters came from PR #655.
The original plan placed decoding in the shared `app` crate with a
platform-specific audio sink. It later moved to `ntpodsd`.

## Original plan

1. **Virtual-mic driver base** — build + test-sign + install SYSVAD (or a trimmed
   fork) so Windows shows a "NTPods Microphone" capture endpoint. Prove it
   appears in Sound settings and apps. *(driver: `windows/drivers/mic/`)*
2. **PCM bridge** — an IOCTL/shared-ring for user mode to push PCM samples into
   the driver; feed a test tone / a WAV → verify it's audible on the virtual mic
   (record in Voice Recorder / Audacity).
3. **Protocol port** — from PR #655: the AAP command that enables the hi-res mic,
   the uplink packet framing + watchdog, and AAC-ELD decoding via FFmpeg
   (libavcodec) — in the shared crate, `platform::MicSink` for the OS sink.
4. **Integration** — talk into the AirPods → decoded audio reaches the virtual
   mic. Conversation-awareness pause during capture, AGC toggle, settings
   persistence (mirror #655).

## Initial risks

- **Audio driver complexity** — WaveRT buffering, formats, timing. SYSVAD is a
  large sample; capture buffering and timing needed validation.
- **AAC-ELD patents** — the decoder (FFmpeg) is patent-encumbered; distribution
  implications (PR #655 raises the same). Keep the decoder optional / documented.
- **Latency** — L2CAP → decode → driver ring; needs a small, steady buffer.
- **Test-signing** — same Test Mode requirement as the AAP driver.

## References

- Microsoft SYSVAD (virtual audio device sample), and the ACX audio samples.
- LibrePods PR #655 (Linux hi-res mic: AAC-ELD + PipeWire) — the protocol RE.
- [`windows/drivers/aap`](../../drivers/aap/) — Bluetooth profile driver.

## Recorded implementation checks

- **Phase 1** (`windows/drivers/mic/`, based on the MS ACX
  AudioCodec sample). Builds with WDK 28000, installs via `install.ps1`, and
  Windows shows a virtual **"Microphone (AudioCodec Device)"** (confirmed on
  hardware). This stage used the sample's dummy audio source.
- **Phase 1b** (`522f9e8`): trimmed to **capture-only** — dropped the
  render (speaker) circuit in `Device.cpp` (create/add/remove) so only a mic
  endpoint exists. The build passed.
  The original `install.ps1` used `devcon remove` on the prior device before
  installing the replacement.
- **Phase 2** (`87ed69f`, validated on hardware): `Common/MicPipe.{h,cpp}`
  — a spin-locked global PCM ring buffer + a control device `\\.\NTPodsMic`
  exposing `IOCTL_NTPODS_MIC_WRITE_PCM` (0x0022A000). `StreamEngine.cpp`
  `ProcessPacket` drains the ring instead of the WAV/tone dummy. Proven end to
  end: `lp-mic-test` (a user-mode tone feeder, `windows/tools/mic-test/`)
  pushed a 440 Hz sine and it was **recorded and audible** on "Microphone
  (AudioCodec Device)".
- **Phase 3a** (`c843d32`, validated on hardware): the AAP enable
  command works. The tray's "Hi-res microphone (test)" toggle sends `START_AUDIO`
  (`04 00 04 00 58 …`, from PR #655); the AirPods enter mic mode (A2DP playback
  dropped to right-only mono and required an A2DP reset, as in #655), and the
  receive loop confirmed the **0x58 uplink audio packets flow** ("receiving
  audio" card). Constants and `is_audio_packet` were added to `aap.rs`.
  See [microphone protocol research](../../../docs/WINDOWS-MIC-PROTOCOL-RESEARCH.md).
- **Phase 3b** (recorded speech on hardware). The tray decoded the 0x58 AUs
  (AAC-ELD) via an FFmpeg libavcodec shim (LGPL, `eld_shim.c`), resampled them
  and streamed to `\\.\NTPodsMic`. Mic frames are **480 samples @ 64 kHz**,
  although the 4-byte ASC indicates
  48 kHz. The +180/AU timestamp corresponds to a 24 kHz clock over 7.5 ms frames.
  The implementation resampled **64000 → 48000**, restricted capture to
  **48 kHz**, and used an 80 ms silence buffer for packet bursts. Pitch went
  ~105 → ~122 Hz (ref ~148), with zero click/gap artifacts reported.
  Audio was quiet and mono (single mic capsule).
- **Phase 4 integration**:
  - **A2DP auto-reset** — toggle the AirPods' AudioSink (0x110B) service to
    restore stereo after the mic stops (0x110D gave ERROR 1060; found via
    BluetoothEnumerateInstalledServices), with a persistent "Restoring stereo…"
    card through the reconnect.
  - **Auto-enable** — the driver's capture-activity counter
    (IOCTL_NTPODS_MIC_STATUS) lets the tray auto-start the hi-res stream when
    an app records and auto-stop (debounced) when it finishes. + a manual mode.
  - **Make-up gain** (×3, tanh soft-limit) so the mic isn't quiet.
  - **Minimal FFmpeg** (7.1, aac-only) — avcodec 69 MB → 0.7 MB.
  - **Name** "NTPods" (device-agnostic).
  - Single-instance guard.

> The pipeline described above as living in "the tray" was later moved into
> `ntpodsd`, which now owns the drivers and the decode; the UI is a thin IPC
> client. See [`../daemon-ipc/PLAN.md`](../daemon-ipc/PLAN.md).

At the end of this work, follow-ups included per-device endpoint names, Apple
Device-ID experiments and a shared decoder crate. Early recovery changes resent
`START_AUDIO` on silence, re-armed it after reconnects and opened ATT lazily to
avoid idle ATT stalls on the shared Bluetooth link. Current recovery behavior
has additional state and hardware checks; use the feature and validation docs
linked above when changing it.
