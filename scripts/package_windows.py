from __future__ import annotations

import os
import shutil
import subprocess
import sys
import tempfile
import zipfile
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
DIST = ROOT / "dist"
BUILD_DIR = Path(os.environ.get("CARGO_TARGET_DIR", ROOT / ".build"))


def main() -> None:
    if sys.platform == "darwin":
        target = "x86_64-pc-windows-gnu"
        toolchain = "gnu"
        linker = shutil.which("x86_64-w64-mingw32-gcc")
        if linker is None:
            raise SystemExit("Install mingw-w64 to package Windows from macOS.")
        rustflags = "-C target-feature=+crt-static -C link-arg=-static"
    elif sys.platform == "win32":
        target = "x86_64-pc-windows-msvc"
        toolchain = "msvc"
        linker = None
        rustflags = "-C target-feature=+crt-static"
    else:
        raise SystemExit("Windows packaging supports macOS (GNU cross-build) or Windows (native MSVC build).")

    env = os.environ.copy()
    env["CARGO_TARGET_DIR"] = str(BUILD_DIR)
    env["RUSTFLAGS"] = rustflags
    if linker is not None:
        env["CARGO_TARGET_X86_64_PC_WINDOWS_GNU_LINKER"] = linker

    subprocess.run(["rustup", "target", "add", target], cwd=ROOT, env=env, check=True)
    distill = shutil.which("distill")
    cargo = [distill, "--cwd", str(ROOT), "cargo"] if distill else ["cargo"]
    subprocess.run(
        cargo + ["build", "--locked", "--release", "--target", target, "--bins"],
        cwd=ROOT,
        env=env,
        check=True,
    )
    subprocess.run(
        [sys.executable, "scripts/package_chrome_extension.py"],
        cwd=ROOT,
        check=True,
    )

    package_name = f"twitch-streak-saver-windows-x86_64-{toolchain}"
    package_dir = DIST / package_name
    archive = DIST / f"{package_name}.zip"
    binaries = BUILD_DIR / target / "release"

    DIST.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(dir=DIST, prefix=".windows-package-") as temporary:
        staging = Path(temporary)
        staged_package = staging / package_name
        staged_extension = staged_package / "chrome-extension"
        staged_extension.mkdir(parents=True)

        for name in ("twitch-streak-saver.exe", "twitch_streak_saver_host.exe"):
            binary = binaries / name
            if not binary.is_file():
                raise FileNotFoundError(f"Windows build did not produce {binary}")
            shutil.copy2(binary, staged_package / name)

        shutil.copytree(DIST / "chrome-extension", staged_extension, dirs_exist_ok=True)
        staged_archive = staging / archive.name
        with zipfile.ZipFile(staged_archive, "w", compression=zipfile.ZIP_DEFLATED, compresslevel=9) as package:
            for path in sorted(staged_package.rglob("*")):
                if path.is_file():
                    package.write(path, arcname=path.relative_to(staging).as_posix())
            expected_entries = {
                f"{package_name}/{name}"
                for name in ("twitch-streak-saver.exe", "twitch_streak_saver_host.exe")
            }
            extension_source = DIST / "chrome-extension"
            expected_entries.update(
                f"{package_name}/chrome-extension/{path.relative_to(extension_source).as_posix()}"
                for path in extension_source.rglob("*")
                if path.is_file()
            )
            entries = set(package.namelist())
            roots = {entry.split("/", 1)[0] for entry in entries}
            if roots != {package_name} or entries != expected_entries:
                raise ValueError("Windows ZIP contents do not match the staged app and extension files")
        with zipfile.ZipFile(staged_archive, "r") as packaged:
            bad_entry = packaged.testzip()
            if bad_entry is not None:
                raise ValueError(f"Windows package archive failed its integrity check: {bad_entry}")

        shutil.rmtree(package_dir, ignore_errors=True)
        os.replace(staged_package, package_dir)
        os.replace(staged_archive, archive)

    print(f"Packaged {target}: {archive}")


if __name__ == "__main__":
    main()
