# Microphone protocol sources and open questions

The [audio protocol](WINDOWS-AIRPODS-AUDIO-PROTOCOL.md) documents NTPods' current
packets, decoder and recovery behavior. This reference records the upstream
evidence behind that implementation and questions still requiring measurements.

## Pinned sources

These revisions were reviewed in October 2026.

| Source | Revision | Contribution |
| --- | --- | --- |
| [LibrePods Linux PR 655](https://github.com/librepods-org/librepods/pull/655) | [99b0e580](https://github.com/LuanAdemi/librepods/tree/99b0e580a93f2a26647f89816895ca1e5f70a3ec) | Control sequence, demux, FFmpeg decoder and Linux recovery |
| [Android rewrite](https://github.com/librepods-org/librepods/tree/34076c85f0fc2eaf32ad1d5cad1bb7315df9c89c) | `34076c85` | Matching commands, timestamp field and in-app recording |
| [Android PR 723](https://github.com/librepods-org/librepods/pull/723) | [5afad4dc](https://github.com/IvanChanPing/librepods/tree/5afad4dc4de5016a57c1e521b27d044d2e738047) | Bounded decoder worker; author reported compile/unit checks only |
| [Linux PR 774](https://github.com/librepods-org/librepods/pull/774) | [96344dae](https://github.com/injkgz/librepods/tree/96344dae683465e4b5ca036b62d1d08c79be9aff) | FDK-AAC/PipeWire implementation; author reported Pro 3 start/stop coexistence |
| [USBPods two-way fork](https://github.com/han-um/USBPods-Pico2W-2-way-audio-implement/tree/d18d083bc52de3143d8d922c4abaae865cc0dc28) | `d18d083b` | AACP uplink and AAC-ELD playback on Pico 2 W |
| [NTPods upstream](https://github.com/arctumn/ntpods/tree/188e45df20259b616ce155be812086259d1a2488) | `188e45df` | Baseline Windows transport |
| [FFmpeg AAC decoder](https://github.com/FFmpeg/FFmpeg/blob/n7.1/libavcodec/aac/aacdec.c) | `n7.1` | ELD format, frame size and flush behavior |

## Coding rate and presentation clock

The Linux, Android and Pico sources agree on PSM `0x1001`, AACP opcode `0x58`,
the start/stop packets, a 22-byte uplink header and AU records containing
`timestamp:u32le, length:u8, data:length`. They use ASC `F8 E6 30 00`:
AOT 39 / ER AAC ELD, mono, 48 kHz coding configuration, 480 samples per AU.

The [Linux decoder](https://github.com/LuanAdemi/librepods/blob/99b0e580a93f2a26647f89816895ca1e5f70a3ec/linux-rust/src/audio/eld.rs)
presents those samples at 64 kHz. The other Linux decoder and Pico follow the
same assumption. NTPods resamples to a 48 kHz Windows input, producing roughly
360 samples per AU after resampler delay: 7.5 ms per AU, or 30 ms for four AUs.

Source agreement and local speech pitch/speed checks support this conversion.
They do not establish exact clock calibration or long-run drift. The ASC
establishes the coding format, not the presentation clock. FFmpeg's
`decode_eld_specific_config` and `aac_decode_er_frame` support validating
480 mono float samples at 48 kHz before resampling.

The wire timestamp's units, reset conditions, legal repeats and wrap behavior
remain unresolved. The Linux and Pico demuxes skip it. Android's
[MicrophoneFrame](https://github.com/librepods-org/librepods/blob/34076c85f0fc2eaf32ad1d5cad1bb7315df9c89c/android/app/src/main/kotlin/me/kavishdevar/librepods/data/audio/MicrophoneFrame.kt)
exposes it, but the recording service passes only the AU to the decoder.
PR 723's [decoder](https://github.com/IvanChanPing/librepods/blob/5afad4dc4de5016a57c1e521b27d044d2e738047/android/app/src/main/java/me/kavishdevar/librepods/audio/AacEldAudio.kt)
generates timestamps in 7,500 µs steps.

NTPods records aggregate timestamp deltas, repeats and backward steps without
saving AU data. It does not use this field to reorder, drop or conceal audio.
The proposed 180-tick / 24 kHz interpretation lacks a pinned capture.

## Stereo coexistence and recovery

Recovery depends on the host audio path:

| Source | Stop/recovery behavior |
| --- | --- |
| PR 655 | Schedules an A2DP reset after start and stop; restarts after two seconds without SDUs |
| Android rewrite | Sends Stop, disconnects A2DP, waits 500 ms and reconnects |
| PR 774 | Author reported Pro 3 playback retained across toggles without a service reset |
| USBPods | Owns the Bluetooth stack and AAC-ELD playback; uses the same mic commands and a two-second watchdog |
| NTPods native playback | Bounded AVDTP suspend/restart with fresh encoder state; endpoint stays present |

These implementations provide no alternate Stop packet or acknowledgment that
confirms stereo recovery. Quiet uplink proves only that microphone packets
stopped. PR 774's report and the Pico
[coexistence report](https://github.com/librepods-org/librepods/pull/655#issuecomment-4980170555)
show that a full Bluetooth-service reset is not universal.

On the older Windows/Alternative A2DP path, a long recording ended with quiet
uplink but only one audible playback channel. An audio-service reset restored
both. Decoder/resampler resets alone also failed to resolve reinsertion static;
stopping while both buds were out, recovering playback and starting a fresh
stream restored speech. Check audible left/right playback after capture;
endpoint presence and decode success cannot confirm it. Current native results
and remaining coverage are in [Windows validation](WINDOWS-VALIDATION.md).

## Conversation Awareness during capture

PR 655 optionally disables device Conversation Awareness through control
`0x28=2`, then restores it after capture. NTPods restores host volume ducking and
ignores host ducking events during capture; it does not change that device
setting. No capture establishes that this difference caused reinsertion static.

Any temporary device override must own its restoration: override only a known
enabled state, preserve explicit user changes, and restore only on the same
connection. Do not persist a temporary disabled value. Process or power loss
cannot guarantee restoration, so a blind off/on pair is insufficient.

## Decoder and buffering constraints

[NTPods upstream PR 7](https://github.com/arctumn/ntpods/pull/7) reported Pro 3 SDU
arrival gaps of roughly 26 ms median, 56 ms p95, 78 ms p99 and 91 ms maximum.
A 32 ms cushion underran and a 64 ms watermark discarded bursts. This motivated
the 100 ms target and 160 ms watermark. Measure the current producer before
reducing either.

NTPods validates complete SDUs and fixed decoder output, rejects corrupt or
nonfinite samples, bounds resampler output, and clears old audio after invalid
input or a two-second packet gap. A five-second decoded-PCM watchdog stops a
stalled capture through bounded recovery. Valid silent PCM counts as progress.
These checks cannot detect every acoustically wrong but valid signal.

Preserve the [FFmpeg send/receive contract](https://ffmpeg.org/doxygen/7.1/group__lavc__encdec.html):
an input can produce zero or multiple outputs, and output must be drained to
`EAGAIN`. The [resampler API](https://ffmpeg.org/doxygen/7.1/group__lswr.html)
provides capacity, delay and compensation queries. Clock compensation needs a
measured error before it can improve this path.

Long-run calibration needs wall time, decoded/delivered sample counts,
underrun/overflow counts and an acoustic reference. Keep arrival delay,
decoder delay, driver buffering and recovery time separate. Repeat both-bud
reinsertion, single-bud handoff, case closure, sleep/resume and capture failures
on each supported microphone profile.
