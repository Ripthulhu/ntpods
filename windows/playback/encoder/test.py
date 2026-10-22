import math
import hashlib
from pathlib import Path
import random
import struct
import subprocess
import tempfile
import zipfile

ROOT = Path(__file__).resolve().parent
BUILD = ROOT / "build" / "Release"
FLOAT_PREAMBLE = b"NPF32001" + struct.pack("<IHH", 48000, 2, 480)


def encode_float(samples, *arguments):
    result = subprocess.run([BUILD / "ntpods-eld-encoder.exe", "--pcm-f32", *arguments],
                            input=FLOAT_PREAMBLE + struct.pack("<" + "f" * len(samples), *samples),
                            capture_output=True, timeout=30, check=True)
    assert not result.stderr, result.stderr.decode(errors="replace")
    parse_stream(result.stdout)
    return result.stdout


def check_float_precision(pcm):
    integers = struct.unpack("<" + "h" * (len(pcm) // 2), pcm)
    for arguments in ((), ("--vbr",), ("--bitrate", "128000")):
        legacy = subprocess.run([BUILD / "ntpods-eld-encoder.exe", *arguments], input=pcm,
                                capture_output=True, timeout=30, check=True).stdout
        assert encode_float([value / 32768 for value in integers], *arguments) == legacy
    for frequency in (250, 1000, 4000, 8000):
        for level in (-100, -110):
            amplitude = 10 ** (level / 20)
            samples = [amplitude * math.sin(math.tau * frequency * index / 48000 + side * 0.73)
                       for index in range(48000) for side in (0, 1)]
            values = decode(encode_float(samples))
            section = values[0::2][12000:36000]
            measured = spectral_amplitude(section, frequency)
            power = sum(value * value for value in section) / len(section)
            assert 0.1 * amplitude < measured < 3 * amplitude, (frequency, level, measured)
            assert measured * measured / (2 * power) > 0.5, (frequency, level, power)
    assert not any(decode(subprocess.run(
        [BUILD / "ntpods-eld-encoder.exe"], input=bytes(48000 * 4),
        capture_output=True, timeout=30, check=True).stdout))
    for frequency in (250, 1000, 4000, 8000):
        samples = [value for index in range(48000)
                   for value in (1e-5 * math.sin(math.tau * frequency * index / 48000), 0)]
        section = decode(encode_float(samples))[0::2][12000:36000]
        assert spectral_amplitude(section, frequency) > 1e-6
    # ELD's content-dependent floor still removes this one-ear-only signal.
    samples = [value for index in range(48000)
               for value in (10 ** (-110 / 20) * math.sin(math.tau * 8000 * index / 48000), 0)]
    assert not any(decode(encode_float(samples)))
    for value in (math.nan, math.inf, -math.inf):
        bad = FLOAT_PREAMBLE + struct.pack("<960f", value, *([0] * 959))
        result = subprocess.run([BUILD / "ntpods-eld-encoder.exe", "--pcm-f32"],
                                input=bad, capture_output=True, timeout=10)
        assert result.returncode != 0 and b"finite samples" in result.stderr
    for bad in (b"", FLOAT_PREAMBLE[:-1], FLOAT_PREAMBLE.replace(b"NPF", b"BAD"),
                b"NPF32001" + struct.pack("<IHH", 48000, 2, 1024),
                FLOAT_PREAMBLE + bytes(3839), FLOAT_PREAMBLE + bytes(3841)):
        result = subprocess.run([BUILD / "ntpods-eld-encoder.exe", "--pcm-f32"],
                                input=bad, capture_output=True, timeout=10)
        assert result.returncode != 0
    # Conversion saturates finite out-of-range input before the Q31 cast.
    limits = [-2, -1, 1, 2, 3.4028234663852886e38, -3.4028234663852886e38]
    values = (limits * 160)
    clamped = [max(-1, min(1, value)) for value in values]
    assert encode_float(values) == encode_float(clamped)
    bundle = ROOT / "build" / "FDK-AAC-source.zip"
    assert hashlib.sha256(bundle.read_bytes()).hexdigest() == (
        ROOT / "build" / "FDK-AAC-source.sha256").read_text().strip().lower()
    with zipfile.ZipFile(bundle) as source:
        prefix = "fdk-aac-cac04476081a23c870fed05c36228cd02c5e7c3d/"
        header = source.read(prefix + "libSYS/include/machine_type.h")
        assert b"typedef LONG INT_PCM;" in header and b"#define SAMPLE_BITS 32" in header
        assert source.read(prefix + "ntpods-pcm-abi.c") == (ROOT / "pcm-abi.c").read_bytes()
        assert b"target_sources(fdk-aac PRIVATE ntpods-pcm-abi.c)" in source.read(prefix + "CMakeLists.txt")
        assert b"OUTPUT_NAME ntpods-fdk-q31" in source.read(prefix + "CMakeLists.txt")
        assert b"2026-10-20" in source.read(prefix + "NTPods-MODIFICATIONS.txt")
        original = ROOT / "vendor" / "fdk-aac-cac04476081a23c870fed05c36228cd02c5e7c3d.zip"
        with zipfile.ZipFile(original) as upstream:
            assert set(source.namelist()) == set(upstream.namelist()) | {
                prefix + "ntpods-pcm-abi.c", prefix + "NTPods-MODIFICATIONS.txt"}
            for name in upstream.namelist():
                if name not in {prefix + "libSYS/include/machine_type.h", prefix + "CMakeLists.txt"}:
                    assert source.read(name) == upstream.read(name), name


def parse_stream(data):
    assert data[:8] == b"NPELD001"
    frame_samples, asc_length = struct.unpack_from("<HH", data, 8)
    assert frame_samples == 480 and 1 <= asc_length <= 64
    asc = data[12:12 + asc_length]
    assert len(asc) == asc_length
    assert asc == bytes.fromhex("f8e65000")  # AOT 39, 48 kHz stereo, 480 samples, no SBR
    offset = 12 + asc_length
    sizes = []
    while offset < len(data):
        size, = struct.unpack_from("<H", data, offset)
        offset += 2
        assert 1 <= size <= 321
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


def decode(data):
    with tempfile.TemporaryDirectory(dir=ROOT / "build") as temporary:
        encoded = Path(temporary) / "audio.bin"
        decoded = Path(temporary) / "audio.f32"
        encoded.write_bytes(data)
        subprocess.run([BUILD / "verify-eld.exe", encoded, decoded], check=True, timeout=30)
        return struct.unpack("<" + "f" * (decoded.stat().st_size // 4), decoded.read_bytes())


def check_link_budgets(pcm, stress_pcm):
    integers = struct.unpack("<" + "h" * (len(pcm) // 2), pcm)
    for budget in range(128, 257):
        arguments = ["--vbr", "--link-budget", str(budget)]
        limited = subprocess.run([BUILD / "ntpods-eld-encoder.exe", *arguments],
                                 input=stress_pcm, capture_output=True, timeout=30, check=True)
        assert not limited.stderr, limited.stderr.decode(errors="replace")
        _, sizes = parse_stream(limited.stdout)
        assert len(sizes) >= 300 and max(sizes) <= budget * 10 // 8, (budget, max(sizes))
        if budget not in {128, 160, 161, 174, 191, 192, 256}:
            continue
        values = decode(limited.stdout)
        assert len(values) == len(sizes) * 960
        assert all(math.isfinite(value) and abs(value) <= 2 for value in values)
        tones = subprocess.run([BUILD / "ntpods-eld-encoder.exe", *arguments],
                               input=pcm, capture_output=True, timeout=30, check=True)
        assert encode_float([value / 32768 for value in integers], *arguments) == tones.stdout
        channels = decode(tones.stdout)
        for side, frequency in ((0, 440), (1, 880)):
            assert 0.1 < spectral_amplitude(channels[side::2][24000:120000], frequency) < 0.25
    reordered = subprocess.run([BUILD / "ntpods-eld-encoder.exe", "--link-budget", "174", "--vbr"],
                               input=pcm, capture_output=True, timeout=30, check=True)
    assert reordered.stdout == encode_float([value / 32768 for value in integers],
                                            "--vbr", "--link-budget", "174")
    for arguments in (["--link-budget", "174"], ["--bitrate", "192000", "--link-budget", "174"],
                      ["--vbr", "--link-budget"], ["--vbr", "--link-budget", "127"],
                      ["--vbr", "--link-budget", "257"], ["--vbr", "--link-budget", "174000"],
                      ["--vbr", "--link-budget", "174x"], ["--vbr", "--link-budget", "-174"],
                      ["--vbr", "--link-budget", "174.5"], ["--vbr", "--link-budget", "000"],
                      ["--vbr", "--link-budget", "174", "--link-budget", "174"]):
        rejected = subprocess.run([BUILD / "ntpods-eld-encoder.exe", *arguments],
                                  input=b"", capture_output=True, timeout=10)
        assert rejected.returncode != 0 and not rejected.stdout


def run():
    pcm = bytearray()
    for i in range(48000 * 3):
        left = round(6000 * math.sin(math.tau * 440 * i / 48000))
        right = round(6000 * math.sin(math.tau * 880 * i / 48000))
        pcm += struct.pack("<hh", left, right)
    process = subprocess.run([BUILD / "ntpods-eld-encoder.exe"], input=pcm,
                             capture_output=True, timeout=30, check=True)
    assert not process.stderr, process.stderr.decode(errors="replace")
    check_float_precision(pcm)
    asc, sizes = parse_stream(process.stdout)
    assert 300 <= len(sizes) <= 332
    variable = subprocess.run([BUILD / "ntpods-eld-encoder.exe", "--vbr"], input=pcm,
                              capture_output=True, timeout=30, check=True)
    assert not variable.stderr, variable.stderr.decode(errors="replace")
    _, variable_sizes = parse_stream(variable.stdout)
    assert len(variable_sizes) == len(sizes) and len(set(variable_sizes)) > 8
    for encoded_data, lengths in ((process.stdout, sizes), (variable.stdout, variable_sizes)):
        values = decode(encoded_data)
        assert len(values) == len(lengths) * 480 * 2
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
    transients = b"".join(struct.pack("<hh", 30000 if i % 480 == 0 else 0,
                                    -30000 if i % 960 == 0 else 0)
                         for i in range(48000))
    stress_pcm = noise + transients
    check_link_budgets(pcm, stress_pcm)
    stress = subprocess.run([BUILD / "ntpods-eld-encoder.exe"], input=stress_pcm,
                            capture_output=True, timeout=30, check=True)
    _, stress_sizes = parse_stream(stress.stdout)
    assert len(stress_sizes) >= 300 and max(stress_sizes) <= 321
    variable_stress = subprocess.run([BUILD / "ntpods-eld-encoder.exe", "--vbr"],
                                     input=bytes(48000 * 4) + stress_pcm,
                                     capture_output=True, timeout=30, check=True)
    _, variable_stress_sizes = parse_stream(variable_stress.stdout)
    assert 400 <= len(variable_stress_sizes) <= 432
    assert max(variable_stress_sizes[:98]) < 20
    assert len(set(variable_stress_sizes[102:298])) > 8
    assert max(variable_stress_sizes) <= 321
    variable_noise_rate = sum(variable_stress_sizes[102:298]) * 800 / 196
    variable_tone_rate = sum(variable_sizes[2:298]) * 800 / 296
    assert 128000 < variable_noise_rate <= 256000
    assert variable_tone_rate < variable_noise_rate
    decoded_variable = decode(variable_stress.stdout)
    assert len(decoded_variable) == len(variable_stress_sizes) * 960
    assert all(math.isfinite(value) and abs(value) <= 2 for value in decoded_variable)
    decode_rates = {128000, 129000, 192000, 255000, 256000}
    for bitrate in range(128000, 256001, 1000):
        selected = subprocess.run([BUILD / "ntpods-eld-encoder.exe", "--bitrate", str(bitrate)],
                                  input=stress_pcm, capture_output=True, timeout=30, check=True)
        selected_asc, selected_sizes = parse_stream(selected.stdout)
        assert selected_asc == asc
        assert len(selected_sizes) >= 300
        assert max(selected_sizes) <= math.ceil(bitrate / 800) + 1, (bitrate, max(selected_sizes))
        steady_sizes = selected_sizes[2:198]
        measured_bitrate = sum(steady_sizes) * 800 / len(steady_sizes)
        assert abs(measured_bitrate - bitrate) <= 800, (bitrate, measured_bitrate)
        if bitrate not in decode_rates:
            continue
        values = decode(selected.stdout)
        assert len(values) == len(selected_sizes) * 960
        assert all(math.isfinite(value) and abs(value) <= 2 for value in values)
    for arguments in (["--bitrate", "127000"], ["--bitrate", "257000"],
                      ["--bitrate", "128001"], ["--bitrate", "128000x"],
                      ["--bitrate", "-128000"], ["--bitrate"], ["--wrong", "256000"],
                      ["--vbr", "192000"], ["--vbr", "--bitrate", "192000"]):
        rejected = subprocess.run([BUILD / "ntpods-eld-encoder.exe", *arguments],
                                  input=b"", capture_output=True, timeout=10)
        assert rejected.returncode != 0 and not rejected.stdout
    for arguments in (["--pcm-f32", "--pcm-f32"], ["--vbr", "--vbr"],
                      ["--bitrate", "192000", "--bitrate", "192000"]):
        rejected = subprocess.run([BUILD / "ntpods-eld-encoder.exe", *arguments],
                                  input=b"", capture_output=True, timeout=10)
        assert rejected.returncode != 0 and not rejected.stdout
    print(f"PASS: ASC {asc.hex()}, {len(sizes)} AUs, peak {max(sizes)} bytes; "
          f"noise peak {max(stress_sizes)} bytes; 48 kHz stereo pitch, "
          "channel separation, all 129 bitrate budgets, noise/transients, Float32 precision and truncated input; "
          "all 129 VBR link budgets and cutoff/quality boundaries; "
          f"VBR tones {variable_tone_rate / 1000:.1f}, noise {variable_noise_rate / 1000:.1f} kbps, "
          f"peak {max(variable_stress_sizes)} bytes.")


if __name__ == "__main__":
    run()
