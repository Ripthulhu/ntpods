"""Install the two measured 48 kHz SADIE II profiles for the Windows listening trial."""

import argparse
import hashlib
import io
import os
from pathlib import Path
import tempfile
import urllib.request
import zipfile


RECORD = "https://zenodo.org/api/records/12092466/files"
PROFILES = {
    "H3": (12425209, "8f0b198245e872458313492879af77d0d3ef937eaedb141a9adbe1e69c26dbda",
           11654654, "55a4c119a89f1eb35799d0f23a1ef2cf4676042beff9386d84c74a767806d24e"),
    "H20": (9299084, "b92225f994b1e6ab425c0a58e1e18ce7ebf22dcd8512d04448d2d4afd1c4ea3f",
            8753949, "0377272defbf45f1f098c2fc0256fc4a9aa022545033ac9c80ea661403f61181"),
}


def install(directory, subject, info):
    size, digest, sofa_size, sofa_digest = info
    dest = directory / f"{subject}.sofa"
    if dest.is_file() and dest.stat().st_size == sofa_size:
        if hashlib.sha256(dest.read_bytes()).hexdigest() == sofa_digest:
            return
    installed = Path(os.environ["LOCALAPPDATA"]) / "AirPods Spatial Audio" / "hrtf" / dest.name
    if installed != dest and installed.is_file() and installed.stat().st_size == sofa_size:
        data = installed.read_bytes()
        if hashlib.sha256(data).hexdigest() == sofa_digest:
            write_profile(directory, dest, data)
            return
    name = f"{subject}_HRIR_SOFA"
    with urllib.request.urlopen(f"{RECORD}/{name}.zip/content", timeout=60) as response:
        data = response.read(size + 1)
    if len(data) != size or hashlib.sha256(data).hexdigest() != digest:
        raise ValueError(f"{subject} download does not match the published archive used for this preview")
    with zipfile.ZipFile(io.BytesIO(data)) as archive:
        member = archive.getinfo(f"{name}/{subject}_48K_24bit_256tap_FIR_SOFA.sofa")
        if member.file_size != sofa_size:
            raise ValueError(f"Unexpected {subject} profile size")
        sofa = archive.read(member)
    if hashlib.sha256(sofa).hexdigest() != sofa_digest:
        raise ValueError(f"Unexpected {subject} profile digest")
    write_profile(directory, dest, sofa)


def write_profile(directory, dest, sofa):
    with tempfile.NamedTemporaryFile(dir=directory, delete=False) as staging:
        temporary = Path(staging.name)
        try:
            staging.write(sofa)
            staging.close()
            os.replace(temporary, dest)
        finally:
            temporary.unlink(missing_ok=True)


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path)
    args = parser.parse_args()
    directory = args.output or Path(os.environ["LOCALAPPDATA"]) / "AirPods Spatial Audio" / "hrtf"
    directory.mkdir(parents=True, exist_ok=True)
    for subject, info in PROFILES.items():
        install(directory, subject, info)
        print(f"Verified SADIE II {subject}")
