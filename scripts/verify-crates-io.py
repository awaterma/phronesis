#!/usr/bin/env python3
"""Fail if any publishable workspace crate is missing from crates.io.

The release pipeline has reported success while publishing nothing on four
consecutive releases, by three separate mechanisms:

1. A version tag was taken as proof of publication for the whole version
   group, so a partial publish looked complete (v0.20.1, v0.21.0, v0.22.1 —
   `phronesis-mcp` left behind every time).
2. release-plz computed the release from a commit sha that existed only in
   the runner, failed mid-run, and left the tag pointing at the right commit
   anyway.
3. `release_always = false` made release-plz skip a Release PR it had not
   itself authored — "skipping release: current commit is not from a release
   PR" — and exit green.

The common thread is not any one mechanism: it is that **nothing checked the
registry**. `.phronesis/wiki/decisions/2026-07-18-release-tag-masks-partial-publish.md`
already prescribes "verify releases against crates.io, not job status", but a
prescription a human has to remember is not a safeguard.

This turns that step into a build failure. It runs on every push to `main`,
where the invariant is simple: the version in the workspace manifest is the
version that should be on crates.io. If `main` is ahead of the registry, a
release was supposed to happen and did not — whatever the release job said.
"""

from __future__ import annotations

import json
import os
import subprocess
import sys
import time
import urllib.error
import urllib.request

# crates.io rejects requests without a descriptive User-Agent.
USER_AGENT = "phronesis-release-verify (https://github.com/awaterma/phronesis)"

# A publish is not visible in the API the instant it returns, so poll rather
# than fail on the first miss. Generous, because the cost of a false failure
# here is a confusing red build on a release that actually worked.
ATTEMPTS = int(os.environ.get("VERIFY_ATTEMPTS", "10"))
SLEEP_SECONDS = int(os.environ.get("VERIFY_SLEEP_SECONDS", "30"))


def publishable_packages() -> list[tuple[str, str]]:
    """(name, version) for each workspace member that is meant to be published.

    `publish = []` in a manifest means "never publish"; anything else (usually
    absent, decoded as None) means it goes to crates.io.
    """
    raw = subprocess.run(
        ["cargo", "metadata", "--no-deps", "--format-version", "1"],
        capture_output=True,
        text=True,
        check=True,
    ).stdout
    packages = json.loads(raw)["packages"]
    return sorted(
        (p["name"], p["version"]) for p in packages if p.get("publish") != []
    )


def is_published(name: str, version: str) -> bool:
    """True when this exact version exists on crates.io.

    Checks the exact version rather than `max_version`: a partial publish can
    leave the registry holding a *newer* version of one crate than another, so
    "the latest is recent enough" is not the question being asked.
    """
    url = f"https://crates.io/api/v1/crates/{name}/{version}"
    request = urllib.request.Request(url, headers={"User-Agent": USER_AGENT})
    try:
        with urllib.request.urlopen(request, timeout=30) as response:
            return response.status == 200
    except urllib.error.HTTPError as error:
        if error.code == 404:
            return False
        # Anything else (429, 5xx) is a question we could not ask, not a
        # negative answer. Treat it as "not yet" so the retry loop covers it,
        # rather than failing the build on a transient registry hiccup.
        print(f"  {name} {version}: registry returned {error.code}, retrying")
        return False
    except urllib.error.URLError as error:
        print(f"  {name} {version}: registry unreachable ({error.reason}), retrying")
        return False


def main() -> int:
    expected = publishable_packages()
    if not expected:
        print("No publishable workspace packages found — refusing to pass vacuously.")
        return 1

    print("Expecting on crates.io:")
    for name, version in expected:
        print(f"  {name} {version}")

    missing = list(expected)
    for attempt in range(1, ATTEMPTS + 1):
        missing = [(n, v) for n, v in missing if not is_published(n, v)]
        if not missing:
            print(f"\nAll {len(expected)} crates are on crates.io at the expected version.")
            return 0
        if attempt < ATTEMPTS:
            names = ", ".join(f"{n} {v}" for n, v in missing)
            print(f"\nAttempt {attempt}/{ATTEMPTS}: still missing {names}")
            print(f"Waiting {SLEEP_SECONDS}s for the registry to catch up...")
            time.sleep(SLEEP_SECONDS)

    print("\nRELEASE INCOMPLETE — the workspace is ahead of crates.io:")
    for name, version in missing:
        print(f"  {name} {version} is NOT published")
    print(
        "\nA green release job does not mean the crates were published; this has\n"
        "now happened four times. Recovery is in\n"
        ".phronesis/wiki/decisions/2026-07-18-release-tag-masks-partial-publish.md\n"
        "— check whether a version tag is masking the registry check, and\n"
        "whether release-plz skipped because the commit was not from a Release\n"
        "PR it authored."
    )
    return 1


if __name__ == "__main__":
    sys.exit(main())
