"""Fetch hash-pinned SDL3 and Steam Audio runtimes into the ignored crate runtime cache.

python crates/ara-audio/tools/fetch_runtime.py --platform windows-x64
Other Steam platforms: linux-x64, osx, android-armv8, ios. SDL on those targets is supplied
by ara-platform/packaging; the Rust bindings never build a second SDL copy.
"""
import argparse
import hashlib
import pathlib
import urllib.request
import zipfile

STEAM = ("https://github.com/ValveSoftware/steam-audio/releases/download/v4.8.1/steamaudio_4.8.1.zip",
         "4a0aa5ec1176f38f0b0993a37c2259d9e86f27e22d5e24f83ec4c3cb9a1d5449")
SDL = ("https://github.com/libsdl-org/SDL/releases/download/release-3.4.18/SDL3-3.4.18-win32-x64.zip",
       "75c2c0fc74e7d1206aaedc22893a35887332f4f85f5c8b74fddc10d177985f2c")


def download(root, name, entry):
    path = root / name
    actual = None
    if path.exists():
        with path.open("rb") as source:
            actual = hashlib.file_digest(source, "sha256").hexdigest()
    if actual != entry[1]:
        temporary = path.with_suffix(".partial")
        request = urllib.request.Request(entry[0], headers={"User-Agent": "Ara-Audio-runtime-fetch"})
        with urllib.request.urlopen(request, timeout=60) as source, temporary.open("wb") as target:
            while block := source.read(1024 * 1024):
                target.write(block)
        with temporary.open("rb") as source:
            actual = hashlib.file_digest(source, "sha256").hexdigest()
        if actual != entry[1]:
            raise RuntimeError(f"Hash mismatch for {name}: {actual}")
        temporary.replace(path)
    return path


def extract(archive, directory, accept):
    with zipfile.ZipFile(archive) as source:
        for name in source.namelist():
            if not accept(name) or name.endswith("/"):
                continue
            target = (directory / name).resolve()
            if not target.is_relative_to(directory.resolve()):
                raise RuntimeError(f"Archive entry escapes runtime cache: {name}")
            target.parent.mkdir(parents=True, exist_ok=True)
            with source.open(name) as data, target.open("wb") as output:
                while block := data.read(1024 * 1024):
                    output.write(block)


def main():
    parser = argparse.ArgumentParser(__doc__)
    parser.add_argument("--platform", default="windows-x64", choices=["windows-x64", "linux-x64", "osx", "android-armv8", "ios"])
    args = parser.parse_args()
    root = pathlib.Path(__file__).resolve().parents[1] / "runtime"
    root.mkdir(exist_ok=True)
    steam = download(root, "steam.zip", STEAM)
    prefix = f"steamaudio/lib/{args.platform}/"
    extract(steam, root / "steam", lambda n: n.startswith(prefix) or n in ("steamaudio/THIRDPARTY.md", "steamaudio/include/phonon.h", "steamaudio/include/phonon_version.h"))
    if args.platform == "windows-x64":
        extract(download(root, "sdl.zip", SDL), root / "sdl", lambda n: n in ("SDL3.dll", "LICENSE.txt", "README.md"))
    print(f"Pinned runtime ready: {root} ({args.platform})")


if __name__ == "__main__":
    main()
