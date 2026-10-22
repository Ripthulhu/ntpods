# AAC-ELD encoder

This worker encodes 48 kHz stereo PCM for AirPods AAC-ELD playback.
The normal build includes the worker and its separate FDK library in the MSI and ZIP.

## Build and check

Run from the repository root with Visual Studio 2022 C++ and CMake tools:

```powershell
./windows/playback/encoder/build.ps1
python windows/playback/encoder/test.py
```

The test checks stereo tones, quiet Float32 signals, S16 compatibility, bitrate
limits and malformed input through FDK's decoder. It does not play audio or
open Bluetooth. This round trip uses the encoder library's own decoder.

Build output and downloaded source stay in ignored `build` and `vendor` folders.
The executable and `ntpods-fdk-q31.dll` are in `build/Release`.

## Pipe format

Default input is signed 16-bit little-endian PCM, left then right, at 48,000 Hz.
Write complete blocks of 480 stereo frames (1,920 bytes).
With `--pcm-f32`, first write `NPF32001`, uint32 sample rate 48000, uint16
channels 2 and uint16 block length 480, all little-endian. Follow with complete
blocks of 480 stereo Float32 frames (3,840 bytes). The worker rejects a mismatched
preamble, non-finite samples and partial blocks. Closing stdin flushes the encoder.

Float32 samples convert directly to saturated 32-bit fixed-point PCM. S16 input
is scaled exactly into the same representation. The matching FDK DLL exports a
PCM-width check. Its distinct DLL name prevents either worker from loading the
other PCM ABI. Replace the worker and DLL together; the wider DLL also changes
FDK's decoder PCM ABI.

Output starts with this header:

| Field | Encoding |
| --- | --- |
| Magic and version | Eight ASCII bytes: `NPELD001` |
| Samples per channel per frame | Little-endian uint16: 480 |
| AudioSpecificConfig length | Little-endian uint16: 1–64 |
| AudioSpecificConfig | That many bytes |

Each following record contains a little-endian uint16 length and one raw AAC-ELD
access unit. Lengths are 1–321 bytes. EOF ends the stream; there is no zero-length
record. Errors go to stderr and return a nonzero exit code. The caller must check
the exit code before treating EOF as a normal stop.

The encoder uses AOT 39 and 480-sample frames, with SBR disabled.
`--bitrate <bits-per-second>` accepts 128000–256000 in 1000-bit steps; omitted, it
uses 256000. FDK's CBR frames can vary by one byte. Each access unit is limited to
`ceil(bitrate / 800) + 1` bytes, at most 321 bytes at 256 kbps.
`--vbr` selects FDK's quality mode 5 with a 256 kbps peak limit. Its internal
nominal stereo rate is 192 kbps, but it does not target a fixed average bitrate:
the output depends on the audio. It cannot be combined with `--bitrate`.
The same 321-byte access-unit limit applies. CBR remains the default.
The worker validates the requested bitrate, generated core format
and each encoded packet before sending it.
This is lossy audio; it does not implement Apple's wireless lossless mode.

For a guarded link-budget comparison, `--vbr --link-budget <kbps>` accepts
128–256. It caps each frame at `floor(kbps * 10 / 8)` bytes: 174 gives a
217-byte ceiling. The corresponding byte-aligned rate is passed to FDK's peak
bitrate control. Bandwidth is limited to 15,500 Hz at 128 kbps, 17,500 Hz at
129–160 kbps and 20,000 Hz above 160 kbps. FDK selects quality mode 4 below
192 kbps and mode 5 at or above it; their nominal stereo rates are 128 and
192 kbps. This controls frame size and bandwidth, not average bitrate, and is
not equivalent to Apple's converter. The worker checks the effective mode,
peak and bandwidth before emitting its header, then rejects any oversized AU.
The option cannot be combined with CBR. Omit it to retain normal rate control.
Offline checks cover every accepted budget, decoder round trips at the
bandwidth and quality boundaries, and S16/Float32 equivalence. Bluetooth
playback and coexistence benefits still require hardware checks.

## Source and licensing

FDK-AAC 2.0.3 is pinned to
[`cac04476081a23c870fed05c36228cd02c5e7c3d`](https://github.com/mstorsjo/fdk-aac/tree/cac04476081a23c870fed05c36228cd02c5e7c3d).
The build verifies the source archive's SHA-256:

```text
1b572fcb5e2a81aa0447a92ca95bfe4979f3edfc09918e0627b726bcd0165ff1
```

The worker's original code uses the [MIT license](LICENSE). FDK's license is
retained in [NOTICE-FDK.txt](NOTICE-FDK.txt). Packages include both notices and
the complete modified FDK source archive in `licenses/FDK-AAC-source.zip`.
The build verifies the original archive, changes the five PCM definitions in
`libSYS/include/machine_type.h` to use FDK's existing 32-bit paths, and includes
`ntpods-pcm-abi.c` and a dated modification notice in the source archive.
The source ZIP's hash receipt is checked during packaging. Wider input preserves
signals lost by S16 conversion, but its floor depends on the signal: the round
trip still loses a one-ear-only 8 kHz tone at -110 dBFS. It does not establish
24-bit acoustic precision or calibrated hearing-test output.
[FFmpeg identifies FDK's license as incompatible with GPL v2 and v3](https://ffmpeg.org/doxygen/trunk/md_LICENSE.html).
The GPL backend communicates through pipes and does not link FDK. Separate
processes do not by themselves establish GPL compatibility for a combined product;
see the [FSF's aggregation guidance](https://www.gnu.org/licenses/gpl-faq.en.html#MereAggregation).
FDK grants no patent license.

Codec settings follow the no-SBR profile in
[USBPods' Pro 3 playback implementation](https://github.com/han-um/USBPods-Pico2W-2-way-audio-implement/blob/d18d083bc52de3143d8d922c4abaae865cc0dc28/src/btstack/btstack_avdtp_source.c#L1678-L1724),
with the selected bitrate capped at 256 kbps.
See [playback support](../README.md) for tested models and remaining hardware
checks. An encoder round trip alone does not establish Bluetooth playback support.

The VBR option uses the rate-control API in the pinned FDK source:
`libAACenc/include/aacenc_lib.h` documents `AACENC_BITRATEMODE` and
`AACENC_PEAK_BITRATE`; `libAACenc/src/aacenc.cpp` defines mode 5's nominal rate.
The round-trip tests cover silence, stereo tones, noise and transients, plus the
per-frame size limit. Short Pro 3 VBR playback and pause/resume checks passed;
see the [hardware results](../../../docs/WINDOWS-VALIDATION.md#native-playback-driver-and-bitrate-controls)
for versions and remaining coverage.
