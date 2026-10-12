# Vendored FFmpeg (fetched, not committed)

The daemon links a tiny slice of **FFmpeg 7.1** (`avcodec` / `avutil` /
`swresample`) to decode the AirPods' hi-res **AAC-ELD** microphone stream.

These libraries are **fetched at build time**, not committed — the FFmpeg headers
alone are ~28k lines, which is pure noise in the repo/PR. Only this note and the
license live here.

- **Fetch:** run `../fetch-ffmpeg.sh` (CI does it automatically before building the
  daemon). It downloads a pinned FFmpeg 7.1 LGPL **shared** build from
  [BtbN/FFmpeg-Builds](https://github.com/BtbN/FFmpeg-Builds), verifies its
  SHA256, and drops the headers + import libs (both MSVC `.lib` and MinGW
  `.dll.a`) + DLLs into `include/`, `lib/`, `bin/` here.
- **License:** this bundle is **LGPL v3+** (`avcodec_license()` and its bundled
  license confirm `--enable-version3`) — see `LICENSE-ffmpeg.txt` and the
  repository's GPL-3.0 text incorporated by LGPL-3.0.
  The `-lgpl-shared` build ships the runtime DLLs (`avcodec-61.dll`,
  `avutil-59.dll`, `swresample-5.dll`) alongside the app.
- **Updating:** bump `URL` / `URL_SHA256` in `fetch-ffmpeg.sh` when moving versions
  (keep the `av*-NN.dll` SONAMEs in sync with what the app loads).
