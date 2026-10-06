#!/usr/bin/env python3

from __future__ import annotations

import sys
import unittest
from pathlib import Path
from unittest import mock

OBS_DIRECTORY = Path(__file__).resolve().parent
sys.path.insert(0, str(OBS_DIRECTORY))

import publish


def result_xml(
    overrides: dict[tuple[str, str], tuple[str, str, bool]] | None = None,
    *,
    omit: tuple[str, str] | None = None,
) -> str:
    values = {
        target: ("published", "succeeded", False)
        for target in publish.REQUIRED_TARGETS
    }
    values.update(overrides or {})
    rows = ["<resultlist>"]
    for repository, arch in sorted(values):
        if (repository, arch) == omit:
            continue
        repository_code, package_code, dirty = values[(repository, arch)]
        rows.append(
            f'<result project="project" repository="{repository}" arch="{arch}" '
            f'code="{repository_code}" state="{repository_code}" '
            f'dirty="{"true" if dirty else "false"}">'
            f'<status package="{publish.PACKAGE}" code="{package_code}"/>'
            "</result>"
        )
    rows.append("</resultlist>")
    return "".join(rows)


def binary_outputs(version: str) -> dict[publish.ObsTarget, str]:
    return {
        publish.ObsTarget(
            "Fedora_43", "x86_64"
        ): f"fcitx5-voice-ja-{version}-1.1.x86_64.rpm\n",
        publish.ObsTarget(
            "Fedora_44", "x86_64"
        ): f"fcitx5-voice-ja-{version}-1.113.x86_64.rpm\n",
        publish.ObsTarget(
            "Debian_13", "x86_64"
        ): f"fcitx5-voice-ja_{version}-1_amd64.deb\n",
    }


class ResultTests(unittest.TestCase):
    def test_all_required_published_targets_succeed(self) -> None:
        results = publish.parse_results_xml(result_xml())
        self.assertEqual(publish.assess_results(results), publish.ResultState.SUCCEEDED)
        matrix = publish.format_target_matrix(results)
        for repository, arch in publish.REQUIRED_TARGETS:
            self.assertIn(f"{repository}  {arch}", matrix)

    def test_building_or_dirty_target_waits(self) -> None:
        target = ("Fedora_44", "x86_64")
        building = publish.parse_results_xml(
            result_xml({target: ("building", "building", False)})
        )
        dirty = publish.parse_results_xml(
            result_xml({target: ("published", "succeeded", True)})
        )
        self.assertEqual(publish.assess_results(building), publish.ResultState.WAITING)
        self.assertEqual(publish.assess_results(dirty), publish.ResultState.WAITING)

    def test_failed_repository_or_package_fails(self) -> None:
        target = ("Debian_13", "x86_64")
        for codes in (("published", "failed", False), ("broken", "unknown", False)):
            with self.subTest(codes=codes):
                results = publish.parse_results_xml(result_xml({target: codes}))
                self.assertEqual(
                    publish.assess_results(results), publish.ResultState.FAILED
                )

    def test_missing_required_target_fails(self) -> None:
        results = publish.parse_results_xml(
            result_xml(omit=("Fedora_43", "x86_64"))
        )
        with self.assertRaisesRegex(publish.PublishError, "missing Fedora_43/x86_64"):
            publish.assess_results(results)

class BinaryTests(unittest.TestCase):
    def test_exact_requested_package_binaries_succeed(self) -> None:
        version = publish.SemVer.parse("1.5.0")
        self.assertEqual(
            publish.assess_binaries(binary_outputs("1.5.0"), version),
            publish.ResultState.SUCCEEDED,
        )

    def test_successful_results_with_stale_package_binaries_wait(self) -> None:
        version = publish.SemVer.parse("1.5.0")
        self.assertEqual(
            publish.assess_binaries(binary_outputs("1.4.9"), version),
            publish.ResultState.WAITING,
        )

    def test_similar_rpm_revision_does_not_match(self) -> None:
        version = publish.SemVer.parse("1.5.0")
        binaries = binary_outputs("1.5.0")
        binaries[publish.ObsTarget("Fedora_43", "x86_64")] = (
            "fcitx5-voice-ja-1.5.0-10.fc43.x86_64.rpm\n"
        )
        self.assertEqual(
            publish.assess_binaries(binaries, version),
            publish.ResultState.WAITING,
        )

class PublicationOrderTests(unittest.TestCase):
    def test_waiting_release_succeeds_when_a_newer_version_supersedes_it(self) -> None:
        config = Path("/tmp/oscrc")
        with (
            mock.patch.object(
                publish,
                "_server_version",
                return_value=publish.SemVer.parse("1.5.1"),
            ),
            mock.patch.object(publish, "_run_osc") as run_osc,
        ):
            state = publish.wait_for_results(
                config,
                publish.SemVer.parse("1.5.0"),
                deadline=0,
                poll_interval=1,
            )

        self.assertEqual(state, publish.ResultState.SUPERSEDED)
        run_osc.assert_not_called()


class UpdateDecisionTests(unittest.TestCase):
    def setUp(self) -> None:
        self.previous = publish.SemVer.parse("1.4.9")
        self.incoming = publish.SemVer.parse("1.5.0")

    def test_updates_only_from_the_recorded_predecessor(self) -> None:
        self.assertEqual(
            publish.decide_update(self.previous, self.incoming, self.previous),
            publish.UpdateDecision.UPDATE,
        )

    def test_same_version_is_reconciled_without_a_duplicate_release(self) -> None:
        self.assertEqual(
            publish.decide_update(self.incoming, self.incoming, self.previous),
            publish.UpdateDecision.RECONCILE,
        )

    def test_older_remote_waits_for_its_predecessor(self) -> None:
        self.assertEqual(
            publish.decide_update(
                publish.SemVer.parse("1.4.8"), self.incoming, self.previous
            ),
            publish.UpdateDecision.WAIT_FOR_PREDECESSOR,
        )

    def test_newer_remote_is_already_superseded(self) -> None:
        self.assertEqual(
            publish.decide_update(
                publish.SemVer.parse("1.5.1"), self.incoming, self.previous
            ),
            publish.UpdateDecision.SUPERSEDED,
        )

    def test_refuses_an_unrelated_intermediate_version(self) -> None:
        with self.assertRaisesRegex(publish.PublishError, "neither predecessor"):
            publish.decide_update(
                publish.SemVer.parse("1.4.10"),
                publish.SemVer.parse("2.0.0"),
                self.previous,
            )


if __name__ == "__main__":
    unittest.main()
