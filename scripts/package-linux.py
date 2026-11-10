#!/usr/bin/env python3
"""Build native Linux .deb and tar.gz artifacts without installing or publishing them."""
import hashlib
import json
from pathlib import Path
import platform
import shutil
import subprocess
import tarfile
import tempfile

ROOT = Path(__file__).resolve().parents[1]
APP_ID = "com.austinpoor.ganymede"


def run(*args, cwd=ROOT):
    return subprocess.check_output(args, cwd=cwd, text=True).strip()


def stage(binary, destination, source=ROOT):
    """Create a prefix-relative tree usable by both archive and Debian consumers."""
    files = {
        "bin/ganymede": binary,
        f"share/applications/{APP_ID}.desktop": source / f"packaging/linux/{APP_ID}.desktop",
        f"share/icons/hicolor/1024x1024/apps/{APP_ID}.png": source / "ganymede-gui/branding/icon-source.png",
        "share/doc/ganymede/LICENSE-MIT": source / "LICENSE-MIT",
        "share/doc/ganymede/LICENSE-APACHE": source / "LICENSE-APACHE",
        "share/doc/ganymede/README.md": source / "docs/development/linux.md",
    }
    for relative, original in files.items():
        target = destination / relative
        target.parent.mkdir(parents=True, exist_ok=True)
        shutil.copyfile(original, target)
        target.chmod(0o755 if relative.startswith("bin/") else 0o644)


def package(binary, version, arch, output, source=ROOT):
    if arch not in ("amd64", "arm64"):
        raise ValueError(f"Unsupported Linux architecture: {arch}")
    output.mkdir(parents=True, exist_ok=True)
    name = f"ganymede-{version}-linux-{arch}"
    with tempfile.TemporaryDirectory(prefix="ganymede-package-") as directory:
        work = Path(directory)
        bundle = work / name
        stage(binary, bundle, source)
        run("desktop-file-validate", str(bundle / f"share/applications/{APP_ID}.desktop"))
        archive = output / f"{name}.tar.gz"
        with tarfile.open(archive, "w:gz") as tar:
            def normalize(info):
                info.uid = info.gid = 0
                info.uname = info.gname = "root"
                return info
            tar.add(bundle, arcname=name, filter=normalize)
        debroot = work / "debian/ganymede"
        shutil.copytree(bundle, debroot / "usr")
        # dpkg-shlibdeps requires source metadata even for a binary-only package.
        (work / "debian").mkdir(exist_ok=True)
        (work / "debian/control").write_text(
            "Source: ganymede\nSection: science\nPriority: optional\n"
            "Maintainer: Ganymede contributors <noreply@github.com>\n\n"
            "Package: ganymede\nArchitecture: any\nDescription: Native Jupyter desktop client\n"
        )
        (debroot / "DEBIAN").mkdir()
        dependencies = run("dpkg-shlibdeps", "-O", "-edebian/ganymede/usr/bin/ganymede", cwd=work)
        dependencies = next(line.split("=", 1)[1] for line in dependencies.splitlines()
                            if line.startswith("shlibs:Depends="))
        # These are loaded dynamically or called by the desktop backend.
        dependencies += ", libvulkan1, libegl1, libfontconfig1, libwayland-client0, xdg-utils, xdg-desktop-portal"
        control = debroot / "DEBIAN/control"
        installed_size = sum(p.stat().st_size for p in bundle.rglob("*") if p.is_file()) // 1024 + 1
        control.write_text(
            f"Package: ganymede\nVersion: {version}\nArchitecture: {arch}\n"
            "Section: science\nPriority: optional\n"
            "Maintainer: Ganymede contributors <noreply@github.com>\n"
            "Homepage: https://github.com/a-poor/ganymede\n"
            f"Installed-Size: {installed_size}\nDepends: {dependencies}\n"
            "Recommends: gnome-keyring\nSuggests: python3, python3-venv\n"
            "Description: Native Jupyter desktop client\n"
            " Edit notebooks and connect to local or remote Jupyter servers.\n"
        )
        deb = output / f"ganymede_{version}_{arch}.deb"
        run("dpkg-deb", "--root-owner-group", "--build", str(debroot), str(deb))
        checksums = output / f"{name}.sha256"
        lines = []
        for path in (archive, deb):
            with path.open("rb") as stream:
                lines.append(f"{hashlib.file_digest(stream, 'sha256').hexdigest()}  {path.name}\n")
        checksums.write_text("".join(lines))
        return archive, deb, checksums


def main():
    if platform.system() != "Linux":
        raise SystemExit("Linux is required to package Ganymede.")
    arch = run("dpkg", "--print-architecture")
    run("bash", "scripts/fetch-assets.sh")
    subprocess.run(["cargo", "build", "--locked", "--release", "-p", "ganymede-gui",
                    "--no-default-features"], cwd=ROOT, check=True)
    metadata = json.loads(run("cargo", "metadata", "--locked", "--no-deps", "--format-version", "1"))
    version = next(p["version"] for p in metadata["packages"] if p["name"] == "ganymede-gui")
    target = Path(metadata["target_directory"])
    for artifact in package(target / "release/ganymede-gui", version, arch, target / "dist"):
        print(artifact)


if __name__ == "__main__":
    main()
