#!/usr/bin/env python3

from __future__ import annotations

import shutil
import sys
import tempfile
import unittest
from pathlib import Path

PACKAGING = Path(__file__).resolve().parent
ROOT = PACKAGING.parent
sys.path.insert(0, str(PACKAGING))

import release


class SemVerTests(unittest.TestCase):
    def test_bumps_all_release_kinds(self) -> None:
        version = release.SemVer.parse("2.7.9")
        self.assertEqual(str(version.bump("major")), "3.0.0")
        self.assertEqual(str(version.bump("minor")), "2.8.0")
        self.assertEqual(str(version.bump("patch")), "2.7.10")

    def test_rejects_noncanonical_versions(self) -> None:
        malformed = (
            "",
            "1",
            "1.2",
            "1.2.3.4",
            "01.2.3",
            "1.02.3",
            "1.2.03",
            "v1.2.3",
            "1.2.3-alpha",
            "+1.2.3",
        )
        for value in malformed:
            with self.subTest(value=value), self.assertRaises(release.ReleaseError):
                release.SemVer.parse(value)


class LabelTests(unittest.TestCase):
    def test_accepts_zero_or_one_release_label(self) -> None:
        self.assertIsNone(release.parse_labels(["documentation"]))
        self.assertEqual(
            release.parse_labels(["documentation", "release:minor"]), "minor"
        )

    def test_rejects_multiple_release_labels(self) -> None:
        with self.assertRaises(release.ReleaseError):
            release.parse_labels(["release:patch", "release:minor"])
        with self.assertRaises(release.ReleaseError):
            release.parse_labels(["release:patch", "release:patch"])

    def test_rejects_unknown_release_label(self) -> None:
        with self.assertRaises(release.ReleaseError):
            release.parse_labels(["release:hotfix"])


class ReleaseTreeTests(unittest.TestCase):
    def setUp(self) -> None:
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.tree = Path(self.temporary.name)
        self._copy_release_tree(self.tree)

    @staticmethod
    def _copy_release_tree(destination: Path) -> None:
        for relative in release.RELEASE_FILES:
            target = destination / relative
            target.parent.mkdir(parents=True, exist_ok=True)
            shutil.copy2(ROOT / relative, target)

    def _request(self, kind: release.ReleaseKind) -> release.ReleaseRequest:
        return release.ReleaseRequest(317, "a" * 40, kind)

    def test_live_tree_is_coherent(self) -> None:
        identity = release.verify_tree(ROOT)
        self.assertGreaterEqual(identity.revision, 1)
        self.assertEqual(str(release.SemVer.parse(str(identity.version))), str(identity.version))

    def test_each_bump_kind_rewrites_a_coherent_tree(self) -> None:
        current = release.verify_tree(self.tree).version
        for kind in ("major", "minor", "patch"):
            with self.subTest(kind=kind):
                with tempfile.TemporaryDirectory() as directory:
                    tree = Path(directory)
                    self._copy_release_tree(tree)
                    plan = release.bump_tree(
                        tree,
                        self._request(kind),
                        release.parse_timestamp("2026-10-07T01:02:03Z"),
                    )
                    self.assertEqual(plan.previous_version, current)
                    self.assertEqual(plan.next_version, current.bump(kind))
                    self.assertEqual(set(plan.rewritten_files), set(release.RELEASE_FILES))
                    identity = release.verify_tree(tree)
                    self.assertEqual(identity.version, current.bump(kind))
                    self.assertEqual(identity.revision, 1)

    def test_detects_disagreeing_exact_field(self) -> None:
        current = release.verify_tree(self.tree).version
        cmake = self.tree / "fcitx5/CMakeLists.txt"
        text = cmake.read_text(encoding="utf-8")
        cmake.write_text(
            text.replace(
                f"VERSION {current} LANGUAGES",
                f"VERSION {current.bump('major')} LANGUAGES",
                1,
            ),
            encoding="utf-8",
        )
        with self.assertRaisesRegex(release.ReleaseError, "version fields disagree"):
            release.verify_tree(self.tree)

    def test_detects_disagreeing_package_revision(self) -> None:
        revision = release.verify_tree(self.tree).revision
        pkgbuild = self.tree / "packaging/arch/PKGBUILD"
        text = pkgbuild.read_text(encoding="utf-8")
        pkgbuild.write_text(
            text.replace(f"pkgrel={revision}\n", f"pkgrel={revision + 1}\n", 1),
            encoding="utf-8",
        )
        with self.assertRaisesRegex(release.ReleaseError, "package revisions disagree"):
            release.verify_tree(self.tree)

    def test_detects_duplicate_exact_field(self) -> None:
        current = release.verify_tree(self.tree).version
        pkgbuild = self.tree / "packaging/arch/PKGBUILD"
        text = pkgbuild.read_text(encoding="utf-8")
        pkgbuild.write_text(
            text.replace(
                f"pkgver={current}\n", f"pkgver={current}\npkgver={current}\n", 1
            ),
            encoding="utf-8",
        )
        with self.assertRaisesRegex(release.ReleaseError, "expected exactly one Arch pkgver"):
            release.verify_tree(self.tree)

    def test_highest_canonical_tag_must_match_the_tree(self) -> None:
        current = release.verify_tree(self.tree).version
        release.verify_tree(
            self.tree,
            highest_tags=("not-a-release", f"v{current}", "v01.2.3"),
        )
        with self.assertRaisesRegex(release.ReleaseError, "highest canonical tag"):
            release.verify_tree(
                self.tree,
                highest_tags=(f"v{current}", f"v{current.bump('patch')}"),
            )

    def test_bump_preserves_existing_changelogs_and_unrelated_fields(self) -> None:
        old_debian = (self.tree / release.DEBIAN_CHANGELOG).read_text(encoding="utf-8")
        old_rpm = (self.tree / release.RPM_SPEC).read_text(encoding="utf-8")
        old_rpm_history = old_rpm.split("%changelog\n", 1)[1]
        old_manifest_description = (
            self.tree / "daemon/Cargo.toml"
        ).read_text(encoding="utf-8").split("description = ", 1)[1]

        plan = release.bump_tree(
            self.tree,
            self._request("patch"),
            release.parse_timestamp("2026-10-07T01:02:03Z"),
        )

        new_debian = (self.tree / release.DEBIAN_CHANGELOG).read_text(encoding="utf-8")
        new_rpm = (self.tree / release.RPM_SPEC).read_text(encoding="utf-8")
        new_manifest = (self.tree / "daemon/Cargo.toml").read_text(encoding="utf-8")
        self.assertTrue(new_debian.endswith(old_debian))
        self.assertTrue(new_rpm.endswith(old_rpm_history))
        self.assertEqual(
            new_manifest.split("description = ", 1)[1], old_manifest_description
        )
        self.assertIn(f"Release {plan.next_version} from PR #317.", new_debian)
        self.assertIn(f"Release {plan.next_version} from PR #317.", new_rpm)
        self.assertIn("Wed, 07 Oct 2026 01:02:03 +0000", new_debian)
        self.assertIn("* Wed Oct 07 2026 ", new_rpm)

    def test_equal_instants_produce_identical_rewrites(self) -> None:
        with tempfile.TemporaryDirectory() as other_directory:
            other = Path(other_directory)
            self._copy_release_tree(other)
            release.bump_tree(
                self.tree,
                self._request("minor"),
                release.parse_timestamp("2026-10-07T10:02:03+09:00"),
            )
            release.bump_tree(
                other,
                self._request("minor"),
                release.parse_timestamp("2026-10-07T01:02:03Z"),
            )
            for relative in release.RELEASE_FILES:
                with self.subTest(path=relative):
                    self.assertEqual(
                        (self.tree / relative).read_bytes(),
                        (other / relative).read_bytes(),
                    )


if __name__ == "__main__":
    unittest.main()
