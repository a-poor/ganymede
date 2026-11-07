#!/usr/bin/env python3
"""Build a verified macOS release and upload it as a GitHub draft."""
import argparse
import fcntl
import hashlib
import json
import os
from pathlib import Path
import plistlib
import re
import shutil
import subprocess
import sys
import tempfile

ROOT = Path(__file__).resolve().parent.parent
REPO = "a-poor/ganymede"
BUNDLE_ID = "com.austinpoor.ganymede"


def run(*args, capture=False):
    result = subprocess.run(args, cwd=ROOT, check=True, text=True,
                            stdout=subprocess.PIPE if capture else None)
    return result.stdout.strip() if capture else None


def require(condition, message):
    if not condition:
        raise ValueError(message)


def release_tag(version):
    require(re.fullmatch(r"\d+\.\d+\.\d+", version),
            "This workflow supports stable X.Y.Z releases only.")
    return "v" + version


def validate_existing(release, commit):
    require(release["isDraft"], "This version is already published; bump the version first.")
    require(release["targetCommitish"] == commit,
            "Existing draft targets a different commit; do not overwrite it.")


def validate_bundle(bundle, version):
    with (bundle / "Contents/Info.plist").open("rb") as source:
        info = plistlib.load(source)
    require(info.get("CFBundleIdentifier") == BUNDLE_ID, "Unexpected bundle identifier.")
    require(info.get("CFBundleShortVersionString") == version
            and info.get("CFBundleVersion") == version, "Bundle version does not match Cargo.")


def verify_app(bundle, version):
    validate_bundle(bundle, version)
    require(run("lipo", "-archs", str(bundle / "Contents/MacOS/Ganymede"), capture=True) == "arm64",
            "Expected an Apple Silicon binary.")
    run("codesign", "--verify", "--deep", "--strict", str(bundle))
    run("xcrun", "stapler", "validate", str(bundle))
    run("spctl", "--assess", "--type", "execute", "--verbose=2", str(bundle))


def find_release(tag):
    # A successful list distinguishes a missing release from an auth/network error.
    releases = json.loads(run("gh", "release", "list", "--repo", REPO,
                              "--limit", "1000", "--json", "tagName", capture=True))
    if not any(item["tagName"] == tag for item in releases):
        require(len(releases) < 1000, "Release list limit reached; check the version manually.")
        return None
    return json.loads(run("gh", "release", "view", tag, "--repo", REPO,
                          "--json", "isDraft,targetCommitish", capture=True))


def preflight(notes):
    require(sys.platform == "darwin", "macOS is required.")
    require(run("uname", "-m", capture=True) == "arm64", "Run on an Apple Silicon Mac.")
    require(notes.is_file() and notes.read_text().strip(), "Provide a nonempty release notes file.")
    for tool in ["cargo", "git", "gh", "xcrun", "codesign", "spctl", "ditto", "lipo"]:
        require(shutil.which(tool), f"Missing required tool: {tool}")
    for name in ["GANYMEDE_SIGNING_IDENTITY", "GANYMEDE_NOTARY_PROFILE"]:
        require(os.environ.get(name), f"Set {name}; credentials must stay in Keychain.")
    # Avoid a caller silently enabling features through custom compiler flags.
    require(not os.environ.get("RUSTFLAGS") and not os.environ.get("CARGO_ENCODED_RUSTFLAGS"),
            "Unset custom Rust flags before releasing.")
    require(not run("git", "status", "--porcelain", capture=True),
            "Commit all changes (including the version and lockfile) before releasing.")
    metadata = json.loads(run("cargo", "metadata", "--locked", "--no-deps",
                              "--format-version", "1", capture=True))
    versions = {p["version"] for p in metadata["packages"] if p["name"] in
                {"ganymede-core", "ganymede-gui"}}
    require(len(versions) == 1, "Workspace package versions differ.")
    version = versions.pop()
    tag = release_tag(version)
    commit = run("git", "rev-parse", "HEAD", capture=True)
    remote = run("git", "remote", "get-url", "origin", capture=True)
    require(remote in {f"https://github.com/{REPO}.git", f"https://github.com/{REPO}",
                       f"git@github.com:{REPO}.git"}, "Unexpected origin repository.")
    refs = run("git", "ls-remote", "origin", "refs/heads/main", f"refs/tags/{tag}",
               f"refs/tags/{tag}^{{}}", capture=True)
    refs = dict(reversed(line.split()) for line in refs.splitlines())
    require(refs.get("refs/heads/main") == commit, "Push this commit to main before releasing.")
    tag_commit = refs.get(f"refs/tags/{tag}^{{}}", refs.get(f"refs/tags/{tag}"))
    require(tag_commit is None or tag_commit == commit, "Release tag points to a different commit.")
    existing = find_release(tag)
    if existing:
        validate_existing(existing, commit)
    return version, tag, commit


def stage_artifacts(version):
    dist = ROOT / "target/dist"
    verify_app(dist / "Ganymede.app", version)
    # Verify what a user extracts, not only the source bundle.
    with tempfile.TemporaryDirectory(prefix="ganymede-release-verify-") as temp:
        run("ditto", "-x", "-k", str(dist / "Ganymede-macOS-notarized.zip"), temp)
        verify_app(Path(temp) / "Ganymede.app", version)
    dmg = dist / "Ganymede-macOS-notarized.dmg"
    run("hdiutil", "verify", str(dmg))
    run("codesign", "--verify", "--strict", str(dmg))
    run("xcrun", "stapler", "validate", str(dmg))
    run("spctl", "--assess", "--type", "open", "--context", "context:primary-signature",
        "--verbose=2", str(dmg))
    destination = dist / ("v" + version)
    destination.mkdir(exist_ok=True)
    artifacts = []
    checksums = []
    for ext in ["dmg", "zip"]:
        path = destination / f"Ganymede-{version}-macos-arm64.{ext}"
        shutil.copyfile(dist / f"Ganymede-macOS-notarized.{ext}", path)
        checksums.append(f"{hashlib.sha256(path.read_bytes()).hexdigest()}  {path.name}\n")
        artifacts.append(path)
    sums = destination / "SHA256SUMS.txt"
    sums.write_text("".join(checksums))
    return artifacts + [sums]


def upload_draft(tag, commit, notes, artifacts):
    existing = find_release(tag)
    if existing:
        validate_existing(existing, commit)
        run("gh", "release", "edit", tag, "--repo", REPO, "--notes-file", str(notes))
    else:
        run("gh", "release", "create", tag, "--repo", REPO, "--target", commit,
            "--title", f"Ganymede {tag}", "--notes-file", str(notes), "--draft")
    # Retry is safe only for the draft targeting this exact commit.
    validate_existing(find_release(tag), commit)
    run("gh", "release", "upload", tag, *map(str, artifacts), "--repo", REPO, "--clobber")
    remote = json.loads(run("gh", "release", "view", tag, "--repo", REPO,
                           "--json", "isDraft,targetCommitish,assets,url", capture=True))
    validate_existing(remote, commit)
    uploaded = {a["name"]: a for a in remote["assets"]}
    for path in artifacts:
        asset = uploaded.get(path.name, {})
        require(asset.get("state") == "uploaded" and asset.get("size") == path.stat().st_size,
                f"Upload incomplete: {path.name}")
        require(asset.get("digest") == "sha256:" + hashlib.sha256(path.read_bytes()).hexdigest(),
                f"Uploaded checksum mismatch: {path.name}")
    print(f"Draft ready for review: {remote['url']}")
    print("Nothing has been published. Review the notes and assets in GitHub before publishing.")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--notes", required=True, type=Path, help="Release notes Markdown file")
    parser.add_argument("--check", action="store_true", help="Preflight only; no build or upload")
    args = parser.parse_args()
    notes = args.notes.resolve()
    version, tag, commit = preflight(notes)
    print(f"Preflight passed: {tag} at {commit}", flush=True)
    if args.check:
        return
    lock_path = ROOT / "target/release-macos.lock"
    lock_path.parent.mkdir(exist_ok=True)
    with lock_path.open("w") as lock:
        try:
            fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
        except BlockingIOError:
            raise ValueError("Another release is using this checkout.") from None
        run("cargo", "fmt", "--all", "--", "--check")
        run("bash", "scripts/fetch-assets.sh")
        run("cargo", "clippy", "--locked", "--workspace", "--all-targets", "--", "-D", "warnings")
        run("cargo", "test", "--locked", "--workspace")
        run("cargo", "check", "--locked", "-p", "ganymede-gui", "--features", "unfinished")
        run("bash", "scripts/package-macos.sh")
        run("bash", "scripts/notarize-macos.sh")
        artifacts = stage_artifacts(version)
        require(not run("git", "status", "--porcelain", capture=True), "Source changed during release.")
        require(run("git", "rev-parse", "HEAD", capture=True) == commit, "HEAD changed during release.")
        upload_draft(tag, commit, notes, artifacts)


if __name__ == "__main__":
    try:
        main()
    except (ValueError, OSError, subprocess.CalledProcessError) as error:
        print(f"Release stopped: {error}", file=sys.stderr)
        sys.exit(1)
