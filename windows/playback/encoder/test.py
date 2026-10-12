import math
from pathlib import Path
import random
import struct
import subprocess
import tempfile

ROOT = Path(__file__).resolve().parent
BUILD = ROOT / "build" / "Release"


def parse_stream(data):
    assert data[:8] == b"NPELD001"
    frame_samples, asc_length = struct.unpack_from("<HH", data, 8)
    assert frame_samples == 480 and 1 <= asc_length <= 64
    asc = data[12:12 + asc_length]
    assert len(asc) == asc_length
    bits = "".join(f"{byte:08b}" for byte in asc)
    assert int(bits[:5], 2) == 31 and int(bits[5:11], 2) + 32 == 39
    assert int(bits[11:15], 2) == 3  # 48 kHz
    assert int(bits[15:19], 2) == 2  # stereo
    assert bits[19] == "1"  # 480-sample granule
    assert bits[23:25] == "10"  # SBR present, same sample rate
    offset = 12 + asc_length
    sizes = []
    while offset < len(data):
        size, = struct.unpack_from("<H", data, offset)
        offset += 2
        assert 1 <= size <= 320
        assert len(data) - offset >= size
        offset += size
        sizes.append(size)
    assert offset == len(data)
    return asc, sizes


def spectral_amplitude(samples, frequency):
    step = math.tau * frequency / 48000
    real = sum(value * math.cos(step * i) for i, value in enumerate(samples))
    imag = sum(value * math.sin(step * i) for i, value in enumerate(samples))
    return 2 * math.hypot(real, imag) / len(samples)


def run():
    pcm = bytearray()
    for i in range(48000 * 3):
        left = round(6000 * math.sin(math.tau * 440 * i / 48000))
        right = round(6000 * math.sin(math.tau * 880 * i / 48000))
        pcm += struct.pack("<hh", left, right)
    process = subprocess.run([BUILD / "ntpods-eld-encoder.exe"], input=pcm,
                             capture_output=True, timeout=30, check=True)
    assert not process.stderr, process.stderr.decode(errors="replace")
    asc, sizes = parse_stream(process.stdout)
    assert 300 <= len(sizes) <= 332
    with tempfile.TemporaryDirectory(dir=ROOT / "build") as temporary:
        encoded = Path(temporary) / "tone.bin"
        decoded = Path(temporary) / "tone.f32"
        encoded.write_bytes(process.stdout)
        subprocess.run([BUILD / "verify-eld.exe", encoded, decoded], check=True, timeout=30)
        values = struct.unpack("<" + "f" * (decoded.stat().st_size // 4), decoded.read_bytes())
    assert len(values) == len(sizes) * 480 * 2
    left, right = values[0::2], values[1::2]
    for channel, own_frequency, other_frequency in ((left, 440, 880), (right, 880, 440)):
        section = channel[24000:120000]
        assert all(math.isfinite(value) and abs(value) < 0.5 for value in section)
        amplitude = spectral_amplitude(section, own_frequency)
        leakage = spectral_amplitude(section, other_frequency)
        assert 0.1 < amplitude < 0.25, (own_frequency, amplitude)
        assert leakage < amplitude * 0.02, (own_frequency, leakage)
        crossings = sum(a <= 0 < b for a, b in zip(section, section[1:]))
        frequency = crossings * 48000 / len(section)
        assert abs(frequency - own_frequency) < 2, frequency
    for malformed in (b"\x00", bytes(1919), bytes(1921)):
        failed = subprocess.run([BUILD / "ntpods-eld-encoder.exe"], input=malformed,
                                capture_output=True, timeout=10)
        assert failed.returncode != 0
        assert b"complete 480-frame" in failed.stderr
    empty = subprocess.run([BUILD / "ntpods-eld-encoder.exe"], input=b"",
                           capture_output=True, timeout=10, check=True)
    parse_stream(empty.stdout)
    random_source = random.Random(20261012)
    noise = b"".join(struct.pack("<h", random_source.randrange(-24000, 24000))
                     for _ in range(48000 * 2 * 2))
    stress = subprocess.run([BUILD / "ntpods-eld-encoder.exe"], input=noise,
                            capture_output=True, timeout=30, check=True)
    _, stress_sizes = parse_stream(stress.stdout)
    assert len(stress_sizes) >= 200 and max(stress_sizes) <= 320
    for bitrate in (128000, 129000, 192000, 255000, 256000):
        selected = subprocess.run([BUILD / "ntpods-eld-encoder.exe", "--bitrate", str(bitrate)],
                                  input=noise, capture_output=True, timeout=30, check=True)
        selected_asc, selected_sizes = parse_stream(selected.stdout)
        # SBR crossover frequency changes with bitrate; the core format stays fixed.
        assert selected_asc[:3] == asc[:3]
        assert len(selected_sizes) >= 200 and max(selected_sizes) <= bitrate // 800
        with tempfile.TemporaryDirectory(dir=ROOT / "build") as temporary:
            encoded = Path(temporary) / "rate.bin"
            decoded = Path(temporary) / "rate.f32"
            encoded.write_bytes(selected.stdout)
            subprocess.run([BUILD / "verify-eld.exe", encoded, decoded], check=True, timeout=30)
            values = struct.unpack("<" + "f" * (decoded.stat().st_size // 4), decoded.read_bytes())
            assert len(values) == len(selected_sizes) * 960
            assert all(math.isfinite(value) and abs(value) <= 2 for value in values)
    for arguments in (["--bitrate", "127000"], ["--bitrate", "257000"],
                      ["--bitrate", "128001"], ["--bitrate", "128000x"],
                      ["--bitrate", "-128000"], ["--bitrate"], ["--wrong", "256000"]):
        rejected = subprocess.run([BUILD / "ntpods-eld-encoder.exe", *arguments],
                                  input=b"", capture_output=True, timeout=10)
        assert rejected.returncode != 0 and not rejected.stdout
    print(f"PASS: ASC {asc.hex()}, {len(sizes)} AUs, peak {max(sizes)} bytes; "
          f"noise peak {max(stress_sizes)} bytes; 48 kHz stereo pitch, "
          "channel separation and truncated input.")


if __name__ == "__main__":
    run()
