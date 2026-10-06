#!/usr/bin/env python3
"""Verify and rewrite the repository's release identity."""

from __future__ import annotations

import argparse
import os
import re
import sys
import tempfile
from dataclasses import dataclass
from datetime import datetime, timezone
from pathlib import Path
from typing import Literal, Mapping, Sequence

ReleaseKind = Literal["major", "minor", "patch"]
SEMVER_SOURCE = r"(?:0|[1-9][0-9]*)\.(?:0|[1-9][0-9]*)\.(?:0|[1-9][0-9]*)"
REVISION_SOURCE = r"[1-9][0-9]*"
TAG_PATTERN = re.compile(rf"v(?P<version>{SEMVER_SOURCE})\Z")
RELEASE_LABELS: dict[str, ReleaseKind] = {
    "release:major": "major",
    "release:minor": "minor",
    "release:patch": "patch",
}
MAINTAINER = "ymkthr <ymkthr@users.noreply.github.com>"
WEEKDAYS = ("Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun")
MONTHS = (
    "Jan",
    "Feb",
    "Mar",
    "Apr",
    "May",
    "Jun",
    "Jul",
    "Aug",
    "Sep",
    "Oct",
    "Nov",
    "Dec",
)


class ReleaseError(ValueError):
    """A release input or repository invariant is invalid."""


@dataclass(frozen=True, order=True)
class SemVer:
    major: int
    minor: int
    patch: int

    @classmethod
    def parse(cls, value: str) -> "SemVer":
        if re.fullmatch(SEMVER_SOURCE, value) is None:
            raise ReleaseError(f"invalid canonical semantic version: {value!r}")
        major, minor, patch = (int(part) for part in value.split("."))
        return cls(major, minor, patch)

    @classmethod
    def from_tag(cls, tag: str) -> "SemVer":
        match = TAG_PATTERN.fullmatch(tag)
        if match is None:
            raise ReleaseError(f"invalid canonical release tag: {tag!r}")
        return cls.parse(match.group("version"))

    def bump(self, kind: ReleaseKind) -> "SemVer":
        if kind == "major":
            return SemVer(self.major + 1, 0, 0)
        if kind == "minor":
            return SemVer(self.major, self.minor + 1, 0)
        if kind == "patch":
            return SemVer(self.major, self.minor, self.patch + 1)
        raise ReleaseError(f"invalid release kind: {kind!r}")

    def __str__(self) -> str:
        return f"{self.major}.{self.minor}.{self.patch}"


@dataclass(frozen=True)
class ReleaseRequest:
    pr_number: int
    merge_sha: str
    kind: ReleaseKind

    def __post_init__(self) -> None:
        if self.pr_number < 1:
            raise ReleaseError("PR number must be positive")
        if re.fullmatch(r"[0-9a-f]{40}", self.merge_sha) is None:
            raise ReleaseError("merge SHA must be 40 lowercase hexadecimal characters")
        if self.kind not in ("major", "minor", "patch"):
            raise ReleaseError(f"invalid release kind: {self.kind!r}")


@dataclass(frozen=True)
class ReleasePlan:
    request: ReleaseRequest
    previous_version: SemVer
    next_version: SemVer
    released_at: datetime
    rewritten_files: tuple[str, ...]


@dataclass(frozen=True)
class Field:
    path: str
    name: str
    kind: Literal["version", "revision"]
    pattern: re.Pattern[str]


@dataclass(frozen=True)
class Identity:
    version: SemVer
    revision: int


def _pattern(source: str, flags: int = re.MULTILINE) -> re.Pattern[str]:
    return re.compile(source, flags)


FIELDS: tuple[Field, ...] = (
    Field(
        "daemon/Cargo.toml",
        "daemon package version",
        "version",
        _pattern(rf'^version = "(?P<value>{SEMVER_SOURCE})"$'),
    ),
    Field(
        "daemon/Cargo.lock",
        "voice-jad lock version",
        "version",
        _pattern(
            rf'^\[\[package\]\]\n(?:(?!^\[\[package\]\]$).)*?^name = "voice-jad"\n'
            rf'(?:(?!^\[\[package\]\]$).)*?^version = "(?P<value>{SEMVER_SOURCE})"$',
            re.MULTILINE | re.DOTALL,
        ),
    ),
    Field(
        "fcitx5/CMakeLists.txt",
        "CMake project version",
        "version",
        _pattern(
            rf"^project\(fcitx5-voice-ja VERSION (?P<value>{SEMVER_SOURCE}) LANGUAGES CXX\)$"
        ),
    ),
    Field(
        "packaging/arch/PKGBUILD",
        "Arch pkgver",
        "version",
        _pattern(rf"^pkgver=(?P<value>{SEMVER_SOURCE})$"),
    ),
    Field(
        "packaging/arch/PKGBUILD",
        "Arch pkgrel",
        "revision",
        _pattern(rf"^pkgrel=(?P<value>{REVISION_SOURCE})$"),
    ),
    Field(
        "packaging/aur/PKGBUILD",
        "AUR pkgver",
        "version",
        _pattern(rf"^pkgver=(?P<value>{SEMVER_SOURCE})$"),
    ),
    Field(
        "packaging/aur/PKGBUILD",
        "AUR pkgrel",
        "revision",
        _pattern(rf"^pkgrel=(?P<value>{REVISION_SOURCE})$"),
    ),
    Field(
        "packaging/aur/.SRCINFO",
        "AUR metadata pkgver",
        "version",
        _pattern(rf"^[ \t]*pkgver = (?P<value>{SEMVER_SOURCE})$"),
    ),
    Field(
        "packaging/aur/.SRCINFO",
        "AUR metadata pkgrel",
        "revision",
        _pattern(rf"^[ \t]*pkgrel = (?P<value>{REVISION_SOURCE})$"),
    ),
    Field(
        "packaging/aur/.SRCINFO",
        "AUR source archive version",
        "version",
        _pattern(
            rf"^[ \t]*source = fcitx5-voice-ja-(?P<value>{SEMVER_SOURCE})\.tar\.gz::"
            rf"https://github\.com/ymkthr/fcitx5-voice-ja/archive/refs/tags/v{SEMVER_SOURCE}\.tar\.gz$"
        ),
    ),
    Field(
        "packaging/aur/.SRCINFO",
        "AUR source tag version",
        "version",
        _pattern(
            rf"^[ \t]*source = fcitx5-voice-ja-{SEMVER_SOURCE}\.tar\.gz::"
            rf"https://github\.com/ymkthr/fcitx5-voice-ja/archive/refs/tags/v(?P<value>{SEMVER_SOURCE})\.tar\.gz$"
        ),
    ),
    Field(
        "packaging/rpm/fcitx5-voice-ja.spec",
        "RPM Version",
        "version",
        _pattern(rf"^Version:[ \t]+(?P<value>{SEMVER_SOURCE})$"),
    ),
    Field(
        "packaging/rpm/fcitx5-voice-ja.spec",
        "RPM Release",
        "revision",
        _pattern(rf"^Release:[ \t]+(?P<value>{REVISION_SOURCE})%\{{\?dist\}}$"),
    ),
)

DEBIAN_CHANGELOG = "packaging/deb/debian/changelog"
RPM_SPEC = "packaging/rpm/fcitx5-voice-ja.spec"
RELEASE_FILES: tuple[str, ...] = tuple(
    dict.fromkeys(field.path for field in FIELDS)
) + (DEBIAN_CHANGELOG,)


@dataclass(frozen=True)
class _Head:
    version: SemVer
    revision: int


def parse_labels(labels: Sequence[str]) -> ReleaseKind | None:
    recognized: list[ReleaseKind] = []
    unknown: list[str] = []
    for label in labels:
        if label in RELEASE_LABELS:
            recognized.append(RELEASE_LABELS[label])
        elif label.startswith("release:"):
            unknown.append(label)
    if unknown:
        raise ReleaseError("unknown release label(s): " + ", ".join(unknown))
    if len(recognized) > 1:
        raise ReleaseError("exactly zero or one release label is allowed")
    return recognized[0] if recognized else None


def parse_timestamp(value: str) -> datetime:
    source = value[:-1] + "+00:00" if value.endswith("Z") else value
    try:
        parsed = datetime.fromisoformat(source)
    except ValueError as error:
        raise ReleaseError(f"invalid release timestamp: {value!r}") from error
    if parsed.tzinfo is None:
        raise ReleaseError("release timestamp must include a UTC offset")
    if parsed.microsecond:
        raise ReleaseError("release timestamp must have whole-second precision")
    return parsed.astimezone(timezone.utc)


def _one_match(field: Field, text: str) -> re.Match[str]:
    matches = list(field.pattern.finditer(text))
    if len(matches) != 1:
        raise ReleaseError(
            f"{field.path}: expected exactly one {field.name}, found {len(matches)}"
        )
    return matches[0]


def _parse_debian_head(text: str) -> _Head:
    match = re.match(
        rf"\Afcitx5-voice-ja \((?P<version>{SEMVER_SOURCE})-"
        rf"(?P<revision>{REVISION_SOURCE})\) unstable; urgency=medium\n",
        text,
    )
    if match is None:
        raise ReleaseError(
            f"{DEBIAN_CHANGELOG}: malformed or missing newest changelog stanza"
        )
    return _Head(
        SemVer.parse(match.group("version")), int(match.group("revision"))
    )


def _rpm_changelog_marker(text: str) -> re.Match[str]:
    markers = list(re.finditer(r"^%changelog$", text, re.MULTILINE))
    if len(markers) != 1:
        raise ReleaseError(f"{RPM_SPEC}: expected exactly one %changelog marker")
    return markers[0]


def _parse_rpm_changelog_head(text: str) -> _Head:
    marker = _rpm_changelog_marker(text)
    entry = text[marker.end() + 1 :]
    match = re.match(
        rf"\A\* [^\n]+ - (?P<version>{SEMVER_SOURCE})-"
        rf"(?P<revision>{REVISION_SOURCE})\n",
        entry,
    )
    if match is None:
        raise ReleaseError(f"{RPM_SPEC}: malformed or missing newest changelog entry")
    return _Head(
        SemVer.parse(match.group("version")), int(match.group("revision"))
    )


def _read_files(root: Path) -> dict[str, str]:
    texts: dict[str, str] = {}
    for relative in RELEASE_FILES:
        path = root / relative
        try:
            texts[relative] = path.read_text(encoding="utf-8")
        except OSError as error:
            raise ReleaseError(f"cannot read {relative}: {error}") from error
    return texts


def _verify_texts(
    texts: Mapping[str, str],
    *,
    highest_tags: Sequence[str] | None = None,
    expected_version: SemVer | None = None,
    previous_version: SemVer | None = None,
    kind: ReleaseKind | None = None,
) -> Identity:
    versions: list[tuple[str, SemVer]] = []
    revisions: list[tuple[str, int]] = []
    for field in FIELDS:
        match = _one_match(field, texts[field.path])
        value = match.group("value")
        if field.kind == "version":
            versions.append((field.name, SemVer.parse(value)))
        else:
            revisions.append((field.name, int(value)))

    debian = _parse_debian_head(texts[DEBIAN_CHANGELOG])
    rpm = _parse_rpm_changelog_head(texts[RPM_SPEC])
    versions.extend(
        (("Debian changelog head", debian.version), ("RPM changelog head", rpm.version))
    )
    revisions.extend(
        (("Debian changelog revision", debian.revision), ("RPM changelog revision", rpm.revision))
    )

    version_values = {value for _, value in versions}
    if len(version_values) != 1:
        detail = ", ".join(f"{name}={value}" for name, value in versions)
        raise ReleaseError(f"version fields disagree: {detail}")
    revision_values = {value for _, value in revisions}
    if len(revision_values) != 1:
        detail = ", ".join(f"{name}={value}" for name, value in revisions)
        raise ReleaseError(f"package revisions disagree: {detail}")

    identity = Identity(next(iter(version_values)), next(iter(revision_values)))
    if expected_version is not None and identity.version != expected_version:
        raise ReleaseError(
            f"repository version {identity.version} does not match expected version {expected_version}"
        )
    if (previous_version is None) != (kind is None):
        raise ReleaseError("previous version and release kind must be checked together")
    if previous_version is not None and kind is not None:
        expected_next = previous_version.bump(kind)
        if identity.version != expected_next:
            raise ReleaseError(
                f"{kind} bump from {previous_version} is {expected_next}, not {identity.version}"
            )
    if highest_tags is not None:
        canonical = [
            SemVer.from_tag(tag)
            for tag in highest_tags
            if TAG_PATTERN.fullmatch(tag) is not None
        ]
        if not canonical:
            raise ReleaseError("no canonical vX.Y.Z tag exists")
        highest = max(canonical)
        if identity.version != highest:
            raise ReleaseError(
                f"repository version {identity.version} does not match highest canonical tag v{highest}"
            )
    return identity


def verify_tree(
    root: Path,
    *,
    highest_tags: Sequence[str] | None = None,
    expected_version: SemVer | None = None,
    previous_version: SemVer | None = None,
    kind: ReleaseKind | None = None,
) -> Identity:
    return _verify_texts(
        _read_files(root),
        highest_tags=highest_tags,
        expected_version=expected_version,
        previous_version=previous_version,
        kind=kind,
    )


def _replace_registered_fields(
    texts: dict[str, str], version: SemVer, revision: int
) -> None:
    fields_by_path: dict[str, list[Field]] = {}
    for field in FIELDS:
        fields_by_path.setdefault(field.path, []).append(field)

    for path, fields in fields_by_path.items():
        text = texts[path]
        edits: list[tuple[int, int, str, str]] = []
        for field in fields:
            match = _one_match(field, text)
            start, end = match.span("value")
            replacement = str(version) if field.kind == "version" else str(revision)
            edits.append((start, end, replacement, field.name))
        edits.sort(reverse=True)
        previous_start = len(text) + 1
        for start, end, replacement, name in edits:
            if end > previous_start:
                raise ReleaseError(f"{path}: overlapping registered field {name}")
            text = text[:start] + replacement + text[end:]
            previous_start = start
        texts[path] = text


def _debian_date(released_at: datetime) -> str:
    return (
        f"{WEEKDAYS[released_at.weekday()]}, {released_at.day:02d} "
        f"{MONTHS[released_at.month - 1]} {released_at.year:04d} "
        f"{released_at.hour:02d}:{released_at.minute:02d}:{released_at.second:02d} +0000"
    )


def _rpm_date(released_at: datetime) -> str:
    return (
        f"{WEEKDAYS[released_at.weekday()]} {MONTHS[released_at.month - 1]} "
        f"{released_at.day:02d} {released_at.year:04d}"
    )


def _prepend_changelogs(
    texts: dict[str, str], version: SemVer, pr_number: int, released_at: datetime
) -> None:
    _parse_debian_head(texts[DEBIAN_CHANGELOG])
    debian_entry = (
        f"fcitx5-voice-ja ({version}-1) unstable; urgency=medium\n\n"
        f"  * Release {version} from PR #{pr_number}.\n\n"
        f" -- {MAINTAINER}  {_debian_date(released_at)}\n\n"
    )
    texts[DEBIAN_CHANGELOG] = debian_entry + texts[DEBIAN_CHANGELOG]

    rpm_text = texts[RPM_SPEC]
    marker = _rpm_changelog_marker(rpm_text)
    insertion = (
        f"\n* {_rpm_date(released_at)} {MAINTAINER} - {version}-1\n"
        f"- Release {version} from PR #{pr_number}.\n"
    )
    texts[RPM_SPEC] = rpm_text[: marker.end()] + insertion + rpm_text[marker.end() :]


def _write_files(root: Path, texts: Mapping[str, str], paths: Sequence[str]) -> None:
    for relative in paths:
        destination = root / relative
        mode = destination.stat().st_mode & 0o777
        temporary_name: str | None = None
        try:
            with tempfile.NamedTemporaryFile(
                mode="w",
                encoding="utf-8",
                newline="",
                dir=destination.parent,
                prefix=f".{destination.name}.",
                delete=False,
            ) as temporary:
                temporary.write(texts[relative])
                temporary.flush()
                os.fsync(temporary.fileno())
                temporary_name = temporary.name
            os.chmod(temporary_name, mode)
            os.replace(temporary_name, destination)
        except OSError as error:
            if temporary_name is not None:
                try:
                    os.unlink(temporary_name)
                except FileNotFoundError:
                    pass
            raise ReleaseError(f"cannot write {relative}: {error}") from error


def bump_tree(
    root: Path,
    request: ReleaseRequest,
    released_at: datetime,
    *,
    highest_tags: Sequence[str] | None = None,
) -> ReleasePlan:
    released_at = released_at.astimezone(timezone.utc)
    original = _read_files(root)
    current = _verify_texts(original, highest_tags=highest_tags)
    next_version = current.version.bump(request.kind)

    rewritten = dict(original)
    _replace_registered_fields(rewritten, next_version, 1)
    _prepend_changelogs(rewritten, next_version, request.pr_number, released_at)
    after = _verify_texts(
        rewritten,
        expected_version=next_version,
        previous_version=current.version,
        kind=request.kind,
    )
    if after.revision != 1:
        raise ReleaseError("rewritten package revision is not 1")

    changed = tuple(path for path in RELEASE_FILES if rewritten[path] != original[path])
    if set(changed) != set(RELEASE_FILES):
        missing = sorted(set(RELEASE_FILES) - set(changed))
        raise ReleaseError("release bump did not rewrite: " + ", ".join(missing))
    _write_files(root, rewritten, changed)
    written = verify_tree(
        root,
        expected_version=next_version,
        previous_version=current.version,
        kind=request.kind,
    )
    if written.revision != 1:
        raise ReleaseError("written package revision is not 1")

    return ReleasePlan(
        request=request,
        previous_version=current.version,
        next_version=next_version,
        released_at=released_at,
        rewritten_files=changed,
    )


def _add_tag_arguments(parser: argparse.ArgumentParser) -> None:
    parser.add_argument(
        "--require-highest-tag",
        action="store_true",
        help="require the tree version to equal the highest canonical --tag",
    )
    parser.add_argument("--tag", action="append", default=[], help="repository tag")


def _highest_tag_argument(args: argparse.Namespace) -> Sequence[str] | None:
    if args.tag and not args.require_highest_tag:
        raise ReleaseError("--tag requires --require-highest-tag")
    return args.tag if args.require_highest_tag else None


def _print_identity(identity: Identity) -> None:
    print(f"version={identity.version}")
    print(f"revision={identity.revision}")


def _labels_command(args: argparse.Namespace) -> None:
    labels = list(args.labels) + list(args.label)
    if not labels:
        labels = [line.rstrip("\r\n") for line in sys.stdin]
    result = parse_labels(labels)
    print(result or "none")


def _verify_command(args: argparse.Namespace) -> None:
    previous = SemVer.parse(args.previous_version) if args.previous_version else None
    expected = SemVer.parse(args.expected_version) if args.expected_version else None
    if bool(args.previous_version) != bool(args.kind):
        raise ReleaseError("--previous-version and --kind must be supplied together")
    identity = verify_tree(
        Path(args.root),
        highest_tags=_highest_tag_argument(args),
        expected_version=expected,
        previous_version=previous,
        kind=args.kind,
    )
    _print_identity(identity)


def _bump_command(args: argparse.Namespace) -> None:
    request = ReleaseRequest(args.pr_number, args.merge_sha, args.kind)
    plan = bump_tree(
        Path(args.root),
        request,
        parse_timestamp(args.released_at),
        highest_tags=_highest_tag_argument(args),
    )
    print(f"previous_version={plan.previous_version}")
    print(f"next_version={plan.next_version}")
    print(f"tag=v{plan.next_version}")
    print(f"pr_number={plan.request.pr_number}")
    print(f"merge_sha={plan.request.merge_sha}")
    print(f"kind={plan.request.kind}")
    print(f"released_at={plan.released_at.isoformat().replace('+00:00', 'Z')}")
    print("rewritten_files=" + ",".join(plan.rewritten_files))


def _parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(description=__doc__)
    subparsers = parser.add_subparsers(dest="command", required=True)

    labels = subparsers.add_parser("labels", help="classify release labels")
    labels.add_argument("labels", nargs="*", help="label names; stdin is used when omitted")
    labels.add_argument("--label", action="append", default=[], help="a label name")
    labels.set_defaults(run=_labels_command)

    verify = subparsers.add_parser("verify", help="verify release fields")
    verify.add_argument("--root", default=".", help="repository root")
    verify.add_argument("--expected-version")
    verify.add_argument("--previous-version")
    verify.add_argument("--kind", choices=("major", "minor", "patch"))
    _add_tag_arguments(verify)
    verify.set_defaults(run=_verify_command)

    bump = subparsers.add_parser("bump", help="rewrite release fields")
    bump.add_argument("--root", default=".", help="repository root")
    bump.add_argument("--kind", required=True, choices=("major", "minor", "patch"))
    bump.add_argument("--pr-number", required=True, type=int)
    bump.add_argument("--merge-sha", required=True)
    bump.add_argument("--released-at", required=True)
    _add_tag_arguments(bump)
    bump.set_defaults(run=_bump_command)
    return parser


def main(argv: Sequence[str] | None = None) -> int:
    parser = _parser()
    args = parser.parse_args(argv)
    try:
        args.run(args)
    except ReleaseError as error:
        parser.exit(2, f"error: {error}\n")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
