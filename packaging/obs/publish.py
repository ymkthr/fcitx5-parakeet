#!/usr/bin/env python3
"""Reconcile prepared release sources with the OBS package."""

from __future__ import annotations

import argparse
import configparser
import enum
import os
import re
import shutil
import stat
import subprocess
import sys
import tempfile
import time
import xml.etree.ElementTree as ET
from dataclasses import dataclass
from pathlib import Path
from typing import Mapping, Sequence

API_URL = "https://api.opensuse.org"
PROJECT = "home:ymkthr:fcitx5-voice-ja"
PACKAGE = "fcitx5-voice-ja"
MANAGED_FILES = frozenset(
    {
        "_service",
        "fcitx5-voice-ja.spec",
        "fcitx5-voice-ja.dsc",
        "debian.tar.xz",
        "vendor.tar.xz",
    }
)
SEMVER_SOURCE = r"(?:0|[1-9][0-9]*)\.(?:0|[1-9][0-9]*)\.(?:0|[1-9][0-9]*)"
BINARY_FILENAME_PATTERNS = {
    ("Fedora_43", "x86_64"): rf"{PACKAGE}-{{version}}-1(?:\.[0-9]+)?\.x86_64\.rpm",
    ("Fedora_44", "x86_64"): rf"{PACKAGE}-{{version}}-1(?:\.[0-9]+)?\.x86_64\.rpm",
    ("Debian_13", "x86_64"): rf"{PACKAGE}_{{version}}-1_amd64\.deb",
}
REQUIRED_TARGETS = frozenset(BINARY_FILENAME_PATTERNS)
FAILED_CODES = frozenset(
    {
        "failed",
        "broken",
        "unresolvable",
        "excluded",
        "disabled",
        "locked",
    }
)
AUTH_MARKERS = (
    "http error 401",
    "http error 403",
    "unauthorized",
    "forbidden",
    "authentication",
    "credentials",
)
CONFLICT_MARKERS = (
    "409 conflict",
    "conflict in project",
    "out of date",
    "outdated working copy",
    "working copy is too old",
)


class PublishError(RuntimeError):
    """The OBS update cannot proceed safely."""


class RevisionConflict(PublishError):
    """The OBS package changed after checkout."""


@dataclass(frozen=True, order=True)
class SemVer:
    major: int
    minor: int
    patch: int

    @classmethod
    def parse(cls, value: str) -> "SemVer":
        if re.fullmatch(SEMVER_SOURCE, value) is None:
            raise PublishError(f"invalid canonical semantic version: {value!r}")
        return cls(*(int(part) for part in value.split(".")))

    def __str__(self) -> str:
        return f"{self.major}.{self.minor}.{self.patch}"


@dataclass(frozen=True, order=True)
class ObsTarget:
    repository: str
    arch: str


@dataclass(frozen=True)
class TargetResult:
    repository_code: str
    package_code: str
    dirty: bool


class ResultState(enum.Enum):
    SUCCEEDED = "succeeded"
    SUPERSEDED = "superseded"
    WAITING = "waiting"
    FAILED = "failed"


class UpdateDecision(enum.Enum):
    UPDATE = "update"
    RECONCILE = "reconcile"
    WAIT_FOR_PREDECESSOR = "wait-for-predecessor"
    SUPERSEDED = "superseded"


def parse_spec_version(text: str) -> SemVer:
    matches = re.findall(rf"^Version:\s+({SEMVER_SOURCE})$", text, re.MULTILINE)
    if len(matches) != 1:
        raise PublishError(f"expected exactly one canonical Version field, found {len(matches)}")
    return SemVer.parse(matches[0])


def decide_update(
    remote: SemVer, incoming: SemVer, previous: SemVer
) -> UpdateDecision:
    if incoming <= previous:
        raise PublishError(
            f"incoming version {incoming} must be newer than predecessor {previous}"
        )
    if remote > incoming:
        return UpdateDecision.SUPERSEDED
    if remote == incoming:
        return UpdateDecision.RECONCILE
    if remote == previous:
        return UpdateDecision.UPDATE
    if remote < previous:
        return UpdateDecision.WAIT_FOR_PREDECESSOR
    raise PublishError(
        f"OBS version {remote} is neither predecessor {previous} nor release {incoming}"
    )


def inspect_sources(directory: Path) -> Mapping[str, Path]:
    try:
        if not stat.S_ISDIR(directory.lstat().st_mode):
            raise PublishError("prepared source path is not a directory")
        entries = list(os.scandir(directory))
    except OSError as error:
        raise PublishError(f"cannot inspect prepared sources: {error}") from error
    names = {entry.name for entry in entries}
    if names != MANAGED_FILES:
        missing = sorted(MANAGED_FILES - names)
        unexpected = sorted(names - MANAGED_FILES)
        parts = []
        if missing:
            parts.append("missing " + ", ".join(missing))
        if unexpected:
            parts.append("unexpected " + ", ".join(unexpected))
        raise PublishError("invalid prepared source set: " + "; ".join(parts))

    sources: dict[str, Path] = {}
    for entry in entries:
        try:
            mode = entry.stat(follow_symlinks=False).st_mode
        except OSError as error:
            raise PublishError(f"cannot inspect prepared source {entry.name}: {error}") from error
        if not stat.S_ISREG(mode):
            raise PublishError(f"prepared source is not a regular file: {entry.name}")
        sources[entry.name] = Path(entry.path)
    return sources


def _open_regular_file(path: Path):
    flags = os.O_RDONLY
    if hasattr(os, "O_NOFOLLOW"):
        flags |= os.O_NOFOLLOW
    try:
        descriptor = os.open(path, flags)
    except OSError as error:
        raise PublishError(f"cannot open regular file {path.name}: {error}") from error
    metadata = os.fstat(descriptor)
    if not stat.S_ISREG(metadata.st_mode):
        os.close(descriptor)
        raise PublishError(f"source changed to a non-regular file: {path.name}")
    return os.fdopen(descriptor, "rb")


def _read_regular_file(path: Path) -> bytes:
    with _open_regular_file(path) as source:
        return source.read()


def _replace_file(source: Path, destination: Path) -> None:
    try:
        with _open_regular_file(source) as input_file:
            if destination.exists() or destination.is_symlink():
                destination.unlink()
            descriptor = os.open(
                destination, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o644
            )
            with os.fdopen(descriptor, "wb") as output:
                shutil.copyfileobj(input_file, output, length=1024 * 1024)
                output.flush()
                os.fsync(output.fileno())
    except OSError as error:
        raise PublishError(f"cannot replace OBS source {destination.name}: {error}") from error


def parse_results_xml(xml: str, package: str = PACKAGE) -> dict[ObsTarget, TargetResult]:
    try:
        root = ET.fromstring(xml)
    except ET.ParseError as error:
        raise PublishError(f"invalid OBS result XML: {error}") from error
    if root.tag != "resultlist":
        raise PublishError(f"unexpected OBS result root: {root.tag}")

    results: dict[ObsTarget, TargetResult] = {}
    for result in root.findall("result"):
        repository = result.get("repository")
        arch = result.get("arch")
        repository_code = result.get("code")
        if not repository or not arch or not repository_code:
            raise PublishError("OBS result is missing repository, arch, or code")
        target = ObsTarget(repository, arch)
        if target in results:
            raise PublishError(f"duplicate OBS result for {repository}/{arch}")
        statuses = [node for node in result.findall("status") if node.get("package") == package]
        if len(statuses) != 1:
            raise PublishError(
                f"OBS result for {repository}/{arch} has {len(statuses)} statuses for {package}"
            )
        package_code = statuses[0].get("code")
        if not package_code:
            raise PublishError(f"OBS result for {repository}/{arch} has no package code")
        results[target] = TargetResult(
            repository_code=repository_code,
            package_code=package_code,
            dirty=result.get("dirty") == "true",
        )
    return results


def assess_results(results: Mapping[ObsTarget, TargetResult]) -> ResultState:
    expected = {ObsTarget(repository, arch) for repository, arch in REQUIRED_TARGETS}
    actual = set(results)
    if actual != expected:
        missing = sorted(expected - actual)
        unexpected = sorted(actual - expected)
        parts = []
        if missing:
            parts.append(
                "missing " + ", ".join(f"{item.repository}/{item.arch}" for item in missing)
            )
        if unexpected:
            parts.append(
                "unexpected "
                + ", ".join(f"{item.repository}/{item.arch}" for item in unexpected)
            )
        raise PublishError("OBS target set differs: " + "; ".join(parts))

    if any(
        result.repository_code.lower() in FAILED_CODES
        or result.package_code.lower() in FAILED_CODES
        for result in results.values()
    ):
        return ResultState.FAILED
    if all(
        result.repository_code == "published"
        and result.package_code == "succeeded"
        and not result.dirty
        for result in results.values()
    ):
        return ResultState.SUCCEEDED
    return ResultState.WAITING


def format_target_matrix(results: Mapping[ObsTarget, TargetResult]) -> str:
    rows = ["repository  arch  repository-code  package-code  dirty"]
    for target in sorted(results):
        result = results[target]
        rows.append(
            f"{target.repository}  {target.arch}  {result.repository_code}  "
            f"{result.package_code}  {'yes' if result.dirty else 'no'}"
        )
    return "\n".join(rows)


def assess_binaries(
    binaries: Mapping[ObsTarget, str], version: SemVer
) -> ResultState:
    for repository, arch in REQUIRED_TARGETS:
        target = ObsTarget(repository, arch)
        pattern = BINARY_FILENAME_PATTERNS[(repository, arch)].format(
            version=re.escape(str(version))
        )
        if not any(re.fullmatch(pattern, name) for name in binaries[target].splitlines()):
            return ResultState.WAITING
    return ResultState.SUCCEEDED


def _write_config(path: Path, username: str, password: str) -> None:
    if not username or not password:
        raise PublishError("OBS_USERNAME and OBS_PASSWORD must be non-empty")
    if any(character in username + password for character in ("\0", "\n", "\r")):
        raise PublishError("OBS credentials must not contain NUL or newline characters")
    config = configparser.ConfigParser(interpolation=None)
    config["general"] = {
        "apiurl": API_URL,
        "checkout_no_colon": "0",
    }
    config[API_URL] = {
        "user": username,
        "pass": password,
        "credentials_mgr_class": "osc.credentials.PlaintextConfigFileCredentialsManager",
    }
    descriptor = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
    with os.fdopen(descriptor, "w", encoding="utf-8", newline="\n") as output:
        config.write(output)
        output.flush()
        os.fsync(output.fileno())
    os.chmod(path, 0o600)


def _osc_environment() -> dict[str, str]:
    environment = dict(os.environ)
    environment.pop("OBS_USERNAME", None)
    environment.pop("OBS_PASSWORD", None)
    environment.pop("OSC_USERNAME", None)
    environment.pop("OSC_PASSWORD", None)
    return environment


def _run_osc(
    config: Path,
    arguments: Sequence[str],
    *,
    cwd: Path,
    conflict_is_retryable: bool = False,
) -> str:
    command = [
        "osc",
        "--config",
        str(config),
        "--no-keyring",
        "--apiurl",
        API_URL,
        *arguments,
    ]
    try:
        result = subprocess.run(
            command,
            cwd=cwd,
            env=_osc_environment(),
            text=True,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            check=False,
        )
    except OSError as error:
        raise PublishError(f"cannot run osc: {error}") from error
    if result.returncode == 0:
        return result.stdout

    detail = "\n".join(part.strip() for part in (result.stdout, result.stderr) if part.strip())
    normalized = detail.lower()
    if conflict_is_retryable and any(marker in normalized for marker in CONFLICT_MARKERS):
        raise RevisionConflict("OBS package revision changed during commit")
    if any(marker in normalized for marker in AUTH_MARKERS):
        raise PublishError("OBS authentication or authorization failed")
    raise PublishError(f"osc {' '.join(arguments[:1])} failed: {detail or 'no diagnostic'}")


def _list_binaries(config: Path) -> dict[ObsTarget, str]:
    return {
        ObsTarget(repository, arch): _run_osc(
            config,
            ["list", "-b", PROJECT, PACKAGE, repository, arch],
            cwd=config.parent,
        )
        for repository, arch in REQUIRED_TARGETS
    }


def _server_version(config: Path) -> SemVer:
    spec = _run_osc(
        config,
        ["cat", PROJECT, PACKAGE, "fcitx5-voice-ja.spec"],
        cwd=config.parent,
    )
    return parse_spec_version(spec)


def _checkout(config: Path, parent: Path) -> Path:
    _run_osc(config, ["checkout", PROJECT, PACKAGE], cwd=parent)
    checkout = parent / PROJECT / PACKAGE
    if not checkout.is_dir():
        raise PublishError(f"osc did not create expected checkout {checkout}")
    return checkout


def _remote_version(checkout: Path) -> SemVer:
    spec = checkout / "fcitx5-voice-ja.spec"
    try:
        metadata = spec.lstat()
        if not stat.S_ISREG(metadata.st_mode):
            raise PublishError("remote OBS spec is not a regular file")
        return parse_spec_version(spec.read_text(encoding="utf-8"))
    except OSError as error:
        raise PublishError(f"cannot read remote OBS spec: {error}") from error


def _reconcile_checkout(
    config: Path,
    checkout: Path,
    sources: Mapping[str, Path],
    incoming: SemVer,
    previous: SemVer,
    *,
    dry_run: bool,
) -> tuple[UpdateDecision, bool]:
    remote = _remote_version(checkout)
    decision = decide_update(remote, incoming, previous)
    if dry_run or decision in {
        UpdateDecision.WAIT_FOR_PREDECESSOR,
        UpdateDecision.SUPERSEDED,
    }:
        return decision, False

    for name in sorted(MANAGED_FILES):
        _replace_file(sources[name], checkout / name)
    _run_osc(config, ["addremove"], cwd=checkout)
    status_output = _run_osc(config, ["status"], cwd=checkout)
    if not status_output.strip():
        return decision, False
    _run_osc(
        config,
        ["commit", "-m", f"Release {incoming}"],
        cwd=checkout,
        conflict_is_retryable=True,
    )
    return decision, True


def wait_for_results(
    config: Path,
    version: SemVer,
    *,
    deadline: float,
    poll_interval: float,
    failure_is_terminal: bool = False,
) -> ResultState:
    while True:
        remote = _server_version(config)
        if remote > version:
            print(
                f"OBS version {version} was superseded by {remote}",
                flush=True,
            )
            return ResultState.SUPERSEDED
        xml = _run_osc(
            config,
            ["results", "--no-multibuild", "--xml", PROJECT, PACKAGE],
            cwd=config.parent,
        )
        results = parse_results_xml(xml)
        matrix = format_target_matrix(results)
        print(matrix, flush=True)
        state = assess_results(results)
        if state is ResultState.SUCCEEDED:
            state = assess_binaries(_list_binaries(config), version)
            if state is ResultState.SUCCEEDED:
                return state
            print(
                f"OBS package binaries do not yet contain version {version}-1",
                flush=True,
            )
        elif state is ResultState.FAILED:
            if failure_is_terminal:
                return state
            raise PublishError("OBS reported a failed target\n" + matrix)
        remaining = deadline - time.monotonic()
        if remaining <= 0:
            raise PublishError(
                f"timed out waiting for OBS targets at version {version}-1\n"
                + matrix
            )
        time.sleep(min(poll_interval, remaining))


def publish(
    source_directory: Path,
    incoming: SemVer,
    previous: SemVer,
    username: str,
    password: str,
    *,
    dry_run: bool = False,
    timeout: float = 7200,
    poll_interval: float = 30,
    conflict_retries: int = 3,
) -> None:
    if timeout <= 0 or poll_interval <= 0 or conflict_retries < 1:
        raise PublishError("timeout, poll interval, and conflict retries must be positive")
    sources = inspect_sources(source_directory)
    prepared_version = parse_spec_version(
        _read_regular_file(sources["fcitx5-voice-ja.spec"]).decode("utf-8")
    )
    if prepared_version != incoming:
        raise PublishError(
            f"prepared spec version {prepared_version} does not match release {incoming}"
        )
    decide_update(previous, incoming, previous)

    deadline = time.monotonic() + timeout
    with tempfile.TemporaryDirectory(prefix="obs-release-") as temporary_name:
        temporary = Path(temporary_name)
        config = temporary / "oscrc"
        _write_config(config, username, password)
        try:
            conflicts = 0
            predecessor_terminal = False
            while True:
                decision = decide_update(
                    _server_version(config),
                    incoming,
                    previous,
                )
                if dry_run or decision is UpdateDecision.SUPERSEDED:
                    print(
                        f"OBS source decision={decision.value} committed=no",
                        flush=True,
                    )
                    return
                if decision is UpdateDecision.WAIT_FOR_PREDECESSOR:
                    remaining = deadline - time.monotonic()
                    if remaining <= 0:
                        raise PublishError(
                            f"timed out waiting for predecessor version {previous}"
                        )
                    time.sleep(min(poll_interval, remaining))
                    continue
                if decision is UpdateDecision.UPDATE and not predecessor_terminal:
                    predecessor_state = wait_for_results(
                        config,
                        previous,
                        deadline=deadline,
                        poll_interval=poll_interval,
                        failure_is_terminal=True,
                    )
                    print(
                        f"OBS predecessor version {previous} is "
                        f"{predecessor_state.value}",
                        flush=True,
                    )
                    predecessor_terminal = True
                    continue

                with tempfile.TemporaryDirectory(
                    prefix="checkout-", dir=temporary
                ) as checkout_name:
                    checkout = _checkout(config, Path(checkout_name))
                    try:
                        decision, committed = _reconcile_checkout(
                            config,
                            checkout,
                            sources,
                            incoming,
                            previous,
                            dry_run=False,
                        )
                    except RevisionConflict:
                        conflicts += 1
                        if conflicts >= conflict_retries:
                            raise PublishError(
                                f"OBS revision changed during {conflicts} commit attempts"
                            )
                        continue

                print(
                    f"OBS source decision={decision.value} committed={'yes' if committed else 'no'}",
                    flush=True,
                )
                if decision is UpdateDecision.SUPERSEDED:
                    return
                if decision is UpdateDecision.WAIT_FOR_PREDECESSOR:
                    continue
                break
            wait_for_results(
                config,
                incoming,
                deadline=deadline,
                poll_interval=poll_interval,
            )
        finally:
            config.unlink(missing_ok=True)


def _semver_argument(value: str) -> SemVer:
    try:
        return SemVer.parse(value)
    except PublishError as error:
        raise argparse.ArgumentTypeError(str(error)) from error


def _parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("sources", type=Path, help="directory containing five prepared files")
    parser.add_argument("--version", required=True, type=_semver_argument)
    parser.add_argument("--previous-version", required=True, type=_semver_argument)
    parser.add_argument("--dry-run", action="store_true")
    parser.add_argument("--timeout", type=float, default=7200)
    parser.add_argument("--poll-interval", type=float, default=30)
    parser.add_argument("--conflict-retries", type=int, default=3)
    return parser


def main(argv: Sequence[str] | None = None) -> int:
    args = _parser().parse_args(argv)
    try:
        publish(
            args.sources,
            args.version,
            args.previous_version,
            os.environ.get("OBS_USERNAME", ""),
            os.environ.get("OBS_PASSWORD", ""),
            dry_run=args.dry_run,
            timeout=args.timeout,
            poll_interval=args.poll_interval,
            conflict_retries=args.conflict_retries,
        )
    except (PublishError, UnicodeDecodeError) as error:
        print(f"error: {error}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
