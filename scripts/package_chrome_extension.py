from __future__ import annotations

import json
import re
import shutil
import zipfile
from html.parser import HTMLParser
from pathlib import Path
from urllib.parse import urlsplit

ROOT = Path(__file__).resolve().parents[1]
SOURCE = ROOT / "chrome-extension"
DIST = ROOT / "dist"
STAGING = DIST / "chrome-extension"
ARCHIVE = DIST / "twitch-streak-saver-chrome-extension.zip"
TEST_SUFFIX = ".test.js"


class HtmlResources(HTMLParser):
    def __init__(self) -> None:
        super().__init__()
        self.references: list[str] = []

    def handle_starttag(self, tag: str, attrs: list[tuple[str, str | None]]) -> None:
        for name, value in attrs:
            if name in {"src", "href", "poster"} and value:
                self.references.append(value)


def local_reference(base: Path, value: str) -> Path | None:
    parsed = urlsplit(value)
    if parsed.scheme or parsed.netloc or value.startswith("#"):
        return None

    resolved = (base / parsed.path).resolve()
    try:
        return resolved.relative_to(SOURCE.resolve())
    except ValueError as error:
        raise ValueError(f"extension resource escapes its source directory: {value}") from error


def manifest_references(manifest: dict[str, object]) -> set[Path]:
    references: set[Path] = set()

    def add(value: object) -> None:
        values = value if isinstance(value, list) else [value]
        for item in values:
            if isinstance(item, str):
                reference = local_reference(SOURCE, item)
                if reference is not None:
                    references.add(reference)

    background = manifest.get("background")
    if isinstance(background, dict):
        add(background.get("service_worker"))
        add(background.get("scripts"))

    content_scripts = manifest.get("content_scripts")
    if isinstance(content_scripts, list):
        for script in content_scripts:
            if isinstance(script, dict):
                add(script.get("js"))
                add(script.get("css"))

    for key in ("action", "browser_action", "page_action"):
        action = manifest.get(key)
        if isinstance(action, dict):
            add(action.get("default_popup"))
            add(action.get("default_icon"))

    for key in ("options_page", "devtools_page"):
        add(manifest.get(key))

    for key in ("options_ui", "side_panel"):
        page = manifest.get(key)
        if isinstance(page, dict):
            add(page.get("page"))
            add(page.get("default_path"))

    overrides = manifest.get("chrome_url_overrides")
    if isinstance(overrides, dict):
        for page in overrides.values():
            add(page)

    icons = manifest.get("icons")
    if isinstance(icons, dict):
        for icon in icons.values():
            add(icon)

    resources = manifest.get("web_accessible_resources")
    if isinstance(resources, list):
        for item in resources:
            if isinstance(item, dict):
                add(item.get("resources"))

    return references


def validate_resource_closure(manifest: dict[str, object], files: set[Path]) -> None:
    required = {Path("manifest.json"), Path("trusted-extension-id")}
    required.update(manifest_references(manifest))

    for path in sorted(files):
        source_file = SOURCE / path
        if path.suffix == ".js":
            source = source_file.read_text(encoding="utf-8")
            for call in re.finditer(r"\bimportScripts\s*\(([^)]*)\)", source, re.DOTALL):
                for value in re.findall(r"['\"]([^'\"]+)['\"]", call.group(1)):
                    reference = local_reference(source_file.parent, value)
                    if reference is not None:
                        required.add(reference)
        elif path.suffix == ".html":
            parser = HtmlResources()
            parser.feed(source_file.read_text(encoding="utf-8"))
            for value in parser.references:
                reference = local_reference(source_file.parent, value)
                if reference is not None:
                    required.add(reference)
        elif path.suffix == ".css":
            source = source_file.read_text(encoding="utf-8")
            for value in re.findall(r"url\(\s*['\"]?([^)'\"]+)", source):
                reference = local_reference(source_file.parent, value.strip())
                if reference is not None:
                    required.add(reference)

    missing = sorted(required - files)
    if missing:
        raise ValueError("extension package is missing referenced files: " + ", ".join(map(str, missing)))


def main() -> None:
    manifest = json.loads((SOURCE / "manifest.json").read_text(encoding="utf-8"))
    if manifest.get("manifest_version") != 3:
        raise ValueError("expected a Manifest V3 Chrome extension")

    files = {
        path.relative_to(SOURCE)
        for path in SOURCE.rglob("*")
        if path.is_file()
        and not path.is_symlink()
        and path.name != ".DS_Store"
        and not path.name.endswith(TEST_SUFFIX)
    }
    validate_resource_closure(manifest, files)

    DIST.mkdir(parents=True, exist_ok=True)
    shutil.rmtree(STAGING, ignore_errors=True)
    ARCHIVE.unlink(missing_ok=True)
    for relative in sorted(files):
        destination = STAGING / relative
        destination.parent.mkdir(parents=True, exist_ok=True)
        shutil.copy2(SOURCE / relative, destination)

    with zipfile.ZipFile(ARCHIVE, "w", compression=zipfile.ZIP_DEFLATED, compresslevel=9) as package:
        for relative in sorted(files):
            package.write(STAGING / relative, arcname=relative.as_posix())
        if package.testzip() is not None:
            raise ValueError("Chrome extension archive failed its integrity check")

    print(f"Packaged {len(files)} extension files: {ARCHIVE}")


if __name__ == "__main__":
    main()
