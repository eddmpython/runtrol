"""Gate: one license policy, and every surface that states it says the same thing.

The license is not decoration here. The product is copyleft so that a fork cannot be closed, and four
packages are deliberately permissive so that other programs can link against them. That split is stated
in a Cargo table, three client manifests, an npm manifest, two lockfiles, four READMEs, `NOTICE`, and
`CONTRIBUTING.md`. No build fails when one of them drifts, and within an hour of the move to AGPL four
of them had: a lockfile still said MIT, a manifest comment pointed at a section that had been deleted,
and the contributor grant named its grantee by pointing at a file that named nobody.

Two of the checks here are less obvious than the rest.

`LICENSE` must contain the license and nothing else. Scanners identify a license by comparing the whole
file, so a third-party agreement appended to it makes the repository read as `NOASSERTION`, which is how
this repository read before the move. The agreement for the CA root data the Core embeds therefore lives
in `NOTICE`, and `NOTICE` has to travel wherever a binary does.

`CONTRIBUTING.md` must name the copyright holder. The inbound grant is what keeps one party able to
relicense the work later, and a grant whose grantee is identified only by a pointer is a grant to nobody
once that pointer moves.

Exit codes:
    0 every surface agrees
    2 a surface drifted
"""

from __future__ import annotations

import json
import sys
import tomllib
from dataclasses import dataclass, field, replace
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]

PRODUCT = "AGPL-3.0-only"
PERMISSIVE = "Apache-2.0"
HOLDER = "eddmpython"
SOURCE_URL = "https://github.com/eddmpython/runtrol"
PUBLISHED = ("crates/runtrol-runtime-protocol", "crates/runtrol-runtime-client")
NPM_PACKAGE = "clients/typescript"
PYTHON_PACKAGE = "clients/python"
READMES = ("README.md", "README_EN.md", "README_JA.md", "README_ZH.md")

AGPL_TITLE = "GNU AFFERO GENERAL PUBLIC LICENSE"
APACHE_TITLE = "Apache License"
EMBEDDED_DATA_AGREEMENT = "Community Data License Agreement"
FOREIGN_AGREEMENTS = (EMBEDDED_DATA_AGREEMENT, APACHE_TITLE, "MIT License")


@dataclass(frozen=True)
class Surfaces:
    """Every place that states who licenses what, already read."""

    workspaceLicense: str | None
    members: dict[str, str | None] = field(default_factory=dict)
    published: tuple[str, ...] = ()
    licenseText: str = ""
    noticeText: str = ""
    contributing: str = ""
    npmLicense: str | None = None
    pythonLicense: str | None = None
    lockLicenses: dict[str, str | None] = field(default_factory=dict)
    publishedLicenseFiles: dict[str, str] = field(default_factory=dict)
    readmes: dict[str, str] = field(default_factory=dict)


def problems(surfaces: Surfaces) -> list[str]:
    """Return every disagreement between the surfaces."""
    found: list[str] = []

    if surfaces.workspaceLicense != PRODUCT:
        found.append(f"the workspace declares {surfaces.workspaceLicense!r} instead of {PRODUCT}")
    if sorted(surfaces.published) != sorted(PUBLISHED):
        found.append(f"the published members are {sorted(surfaces.published)}, not {sorted(PUBLISHED)}")
    for member, declared in sorted(surfaces.members.items()):
        if member in surfaces.published or member == PYTHON_PACKAGE:
            if declared != PERMISSIVE:
                found.append(f"{member} is a public client and must declare {PERMISSIVE}, not {declared!r}")
        elif declared is not None:
            found.append(f"{member} is not published yet overrides the workspace license with {declared!r}")

    if AGPL_TITLE not in surfaces.licenseText:
        found.append("LICENSE does not carry the product license text")
    for agreement in FOREIGN_AGREEMENTS:
        if agreement in surfaces.licenseText:
            found.append(f"LICENSE also carries {agreement}, so scanners cannot identify the license")

    for wanted, why in (
        ("Affero General Public License", "the product license"),
        (PERMISSIVE, "the permissive exception"),
        (HOLDER, "the copyright holder"),
        (SOURCE_URL, "the corresponding source offer"),
        (EMBEDDED_DATA_AGREEMENT, "the agreement for the CA root data the Core embeds"),
    ):
        if wanted not in surfaces.noticeText:
            found.append(f"NOTICE does not state {why}")

    for wanted, why in (
        (HOLDER, "name the holder the grant runs to"),
        ("NOTICE", "point at where that holder is named"),
        (PRODUCT, "name the product license"),
        (PERMISSIVE, "name the permissive exception"),
    ):
        if wanted not in surfaces.contributing:
            found.append(f"CONTRIBUTING.md does not {why}")

    if surfaces.npmLicense != PERMISSIVE:
        found.append(f"the npm client declares {surfaces.npmLicense!r} instead of {PERMISSIVE}")
    if surfaces.pythonLicense != PERMISSIVE:
        found.append(f"the Python client declares {surfaces.pythonLicense!r} instead of {PERMISSIVE}")
    for lock, declared in sorted(surfaces.lockLicenses.items()):
        if declared != PERMISSIVE:
            found.append(f"{lock} still records {declared!r} for the npm client")

    for member, text in sorted(surfaces.publishedLicenseFiles.items()):
        if APACHE_TITLE not in text:
            found.append(f"{member}/LICENSE does not carry the permissive license text")

    for name, text in sorted(surfaces.readmes.items()):
        for wanted in (PRODUCT, PERMISSIVE):
            if wanted not in text:
                found.append(f"{name} does not name {wanted}")

    return found


def readSurfaces() -> Surfaces:
    """Read every surface from the working tree."""
    workspace = tomllib.loads((ROOT / "Cargo.toml").read_text(encoding="utf-8"))
    members: dict[str, str | None] = {}
    published: list[str] = []
    for member in workspace["workspace"]["members"]:
        package = tomllib.loads((ROOT / member / "Cargo.toml").read_text(encoding="utf-8"))["package"]
        declared = package.get("license")
        members[member] = None if isinstance(declared, dict) else declared
        if package.get("publish", True) is not False:
            published.append(member)

    lockLicenses: dict[str, str | None] = {}
    for lock, key in (
        (f"{NPM_PACKAGE}/package-lock.json", ""),
        ("extensions/runtrol-vscode/package-lock.json", f"../../{NPM_PACKAGE}"),
    ):
        packages = json.loads((ROOT / lock).read_text(encoding="utf-8"))["packages"]
        lockLicenses[lock] = packages.get(key, {}).get("license")

    npm = json.loads((ROOT / NPM_PACKAGE / "package.json").read_text(encoding="utf-8"))
    python = tomllib.loads((ROOT / PYTHON_PACKAGE / "pyproject.toml").read_text(encoding="utf-8"))
    return Surfaces(
        workspaceLicense=workspace["workspace"]["package"].get("license"),
        members=members,
        published=tuple(published),
        licenseText=(ROOT / "LICENSE").read_text(encoding="utf-8"),
        noticeText=(ROOT / "NOTICE").read_text(encoding="utf-8"),
        contributing=(ROOT / "CONTRIBUTING.md").read_text(encoding="utf-8"),
        npmLicense=npm.get("license"),
        pythonLicense=python["project"].get("license"),
        lockLicenses=lockLicenses,
        publishedLicenseFiles={
            member: (ROOT / member / "LICENSE").read_text(encoding="utf-8")
            for member in (*PUBLISHED, PYTHON_PACKAGE)
        },
        readmes={name: (ROOT / name).read_text(encoding="utf-8") for name in READMES},
    )


def main() -> int:
    """Report every surface that no longer agrees with the license policy."""
    found = problems(readSurfaces())
    if found:
        print("[licenseSsot] FAIL. the license policy is stated inconsistently:", file=sys.stderr)
        for one in found:
            print(f"  - {one}", file=sys.stderr)
        return 2
    print(f"[licenseSsot] OK. {PRODUCT} product, {PERMISSIVE} for {len(PUBLISHED) + 2} published packages.")
    return 0


def selftest() -> int:
    """Prove each kind of drift makes the gate red."""
    green = readSurfaces()
    if problems(green):
        print("[licenseSsot] selftest: the working tree was rejected while green.", file=sys.stderr)
        return 2

    mutations: list[tuple[str, Surfaces]] = [
        ("the workspace relicenses", replace(green, workspaceLicense="MIT")),
        ("a published crate falls back to copyleft", replace(green, members={**green.members, PUBLISHED[0]: None})),
        ("a private crate overrides the workspace", replace(green, members={**green.members, "crates/runtrol-core": "MIT"})),
        ("a member starts publishing", replace(green, published=(*green.published, "crates/runtrol-core"))),
        ("LICENSE regains an appended agreement", replace(green, licenseText=f"{green.licenseText}\n{EMBEDDED_DATA_AGREEMENT}\n")),
        ("LICENSE loses the license text", replace(green, licenseText="")),
        ("NOTICE loses the holder", replace(green, noticeText=green.noticeText.replace(HOLDER, "someone"))),
        ("NOTICE loses the source offer", replace(green, noticeText=green.noticeText.replace(SOURCE_URL, ""))),
        ("NOTICE loses the embedded data agreement", replace(green, noticeText=green.noticeText.replace(EMBEDDED_DATA_AGREEMENT, ""))),
        ("the grant stops naming its grantee", replace(green, contributing=green.contributing.replace(HOLDER, "the maintainer"))),
        ("the npm client relicenses", replace(green, npmLicense="MIT")),
        ("the Python client relicenses", replace(green, pythonLicense="MIT")),
        ("a lockfile keeps the old license", replace(green, lockLicenses={**green.lockLicenses, f"{NPM_PACKAGE}/package-lock.json": "MIT"})),
        ("a published crate ships the wrong license file", replace(green, publishedLicenseFiles={**green.publishedLicenseFiles, PUBLISHED[0]: "MIT License"})),
        ("one language README drifts", replace(green, readmes={**green.readmes, "README_JA.md": "no license section"})),
    ]
    escaped = [what for what, mutated in mutations if not problems(mutated)]
    for what in escaped:
        print(f"[licenseSsot] selftest: {what} escaped the gate.", file=sys.stderr)
    if escaped:
        return 2
    print(f"[licenseSsot --selftest] OK. {len(mutations)} kinds of license drift make the gate red.")
    return 0


if __name__ == "__main__":
    if "--selftest" in sys.argv:
        raise SystemExit(selftest())
    raise SystemExit(main())
