#!/usr/bin/env bash
# Fetch FFmpeg 7.1 shared libraries for AAC-ELD decoding into vendor/ffmpeg.
# Runs in Git Bash on Windows and in WSL / Linux for cross-builds.
#
# Copy .lib (MSVC) and .dll.a (MinGW) import libraries for both targets.
# Pin a dated BtbN autobuild tag and SHA256; the rolling latest tag changes.
#
# Use the last autobuild of a month. BtbN retains about 14 daily builds and one
# monthly build for about two years; other daily tags are pruned. When updating,
# check the DLL SONAMEs and update URL and URL_SHA256 together with the source
# archives documented in docs/FFMPEG-SOURCES.md.
set -euo pipefail

URL="https://github.com/BtbN/FFmpeg-Builds/releases/download/autobuild-2026-07-31-14-10/ffmpeg-n7.1.5-12-g1fdbca85aa-win64-lgpl-shared-7.1.zip"
URL_SHA256="0f376f96fb38554ccefb1b2ae9c7c6a7b351f0e60a372b38262c320e8392c5d0"

here="$(cd "$(dirname "$0")" && pwd)"
dest="$here/vendor/ffmpeg"

# A previous partial download or edited DLL must not silently become the build input.
if [ -f "$dest/archive.sha256" ] && [ "$(cat "$dest/archive.sha256")" = "$URL_SHA256" ] &&
   [ -f "$dest/files.sha256" ] && (cd "$dest" && sha256sum --quiet -c files.sha256); then
  echo "verified cached FFmpeg in $dest"
  exit 0
fi

tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT
echo "fetching FFmpeg 7.1 shared libs…"
if ! curl -fsSL -o "$tmp/ff.zip" "$URL"; then
  echo "error: could not download $URL" >&2
  echo "If this is a 404, the pinned BtbN autobuild was pruned. Pick the LAST" >&2
  echo "autobuild of a month from https://github.com/BtbN/FFmpeg-Builds/releases" >&2
  echo "and update URL + URL_SHA256 above (those monthly tags are kept)." >&2
  exit 1
fi
echo "${URL_SHA256}  $tmp/ff.zip" | sha256sum -c -
unzip -q "$tmp/ff.zip" -d "$tmp/x"
root="$(find "$tmp/x" -maxdepth 1 -type d -name 'ffmpeg*' | head -1)"

mkdir -p "$dest/include" "$dest/lib" "$dest/bin" "$dest/licenses"
cp -r "$root"/include/libavcodec "$root"/include/libavutil "$root"/include/libswresample "$dest/include/"
# import libs — both MSVC (.lib) and MinGW (.dll.a)
cp "$root"/lib/avcodec.lib "$root"/lib/avutil.lib "$root"/lib/swresample.lib "$dest/lib/"
cp "$root"/lib/libavcodec.dll.a "$root"/lib/libavutil.dll.a "$root"/lib/libswresample.dll.a "$dest/lib/"
# runtime DLLs
cp "$root"/bin/avcodec-61.dll "$root"/bin/avutil-59.dll "$root"/bin/swresample-5.dll "$dest/bin/"
# Preserve the bundle's upstream license and build/source information.
find "$root" -maxdepth 1 -type f \( -iname '*license*' -o -iname '*readme*' \) -exec cp {} "$dest/licenses/" \;
printf '%s\n' "$URL_SHA256" > "$dest/archive.sha256"
(cd "$dest" && find include lib bin licenses -type f -print0 | sort -z | xargs -0 sha256sum > files.sha256)

echo "ffmpeg fetched into $dest"
