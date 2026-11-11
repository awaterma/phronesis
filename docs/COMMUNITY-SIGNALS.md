# Community signals

[Share your experience](https://github.com/awaterma/phronesis/discussions/162) or
browse [Discussions](https://github.com/awaterma/phronesis/discussions). This is
voluntary: a description is enough; private project names or source are not needed.

## Weekly snapshots

The **Community signals** GitHub Actions workflow runs Mondays at 15:17 UTC
(07:17 Pacific standard time / 08:17 daylight time) and can be run manually from
[Actions](https://github.com/awaterma/phronesis/actions/workflows/community-signals.yml).
The schedule becomes active when the workflow is merged into the default branch.
GitHub may delay scheduled runs and disables schedules in inactive public repos
after 60 days; this is not a guaranteed delivery service.

Each run publishes a readable job summary and a uniquely named artifact containing
`snapshot.json` and `summary.md`, retained for 90 days. Download snapshots before
expiry for longer history. Compare snapshots by `collected_at`, source key, counts,
and candidate URLs; no historical comparison is fabricated on the first run.

Collected sources:

- Downloads, recent downloads, and current versions for all five crates.
- Published reverse dependencies, with companion crates labeled separately.
- Public GitHub star/fork/watch counts and fork URLs.
- Public GitHub code and issue/PR searches for `awaterma/phronesis`, `phr-mcp`, and
  `phronesis-mcp`, excluding the maintainer's repositories.
- Hacker News story/comment candidates via its Algolia search API for those terms.

The collector uses Python's standard library and the workflow's read-only
`GITHUB_TOKEN`. No additional credential is needed. Private repository traffic, application
telemetry, download identities, and message bodies are not collected. Code-search
results are retained only when GitHub explicitly marks their repository public;
the displayed code-search total counts public results returned, not all matches. It neither posts
comments nor contacts people. Source URLs and timestamps accompany the data.

Transient HTTP errors receive up to two bounded retries (respecting numeric
Retry-After values up to 60 seconds). Source failures produce a **partial** artifact and a failed workflow so missing
information is visible. Authentication restrictions, API outages and rate limits
are unknowns, not zero adoption. Code search may be unavailable with a particular
GitHub token; the other snapshots are still retained. Results are capped at 100
per query/source and explicitly marked when truncated or incomplete; the readable
summary shows at most 30 links per source, with the rest in JSON.

## Interpreting the results

Downloads include repeated installations, CI and dependency fetching. Companion
crate counts overlap and must not be added into a unique-user estimate. A fork or
star shows interest; neither proves deployment. Published reverse dependencies
miss private projects, unpublished applications, Git dependencies and CLI users.
Search results can be namesakes, mirrors, automated scanners or stale references.
A security scanner's mention is a review lead, not a confirmed vulnerability.
Review a result before calling it independent adoption or substantive discussion.

For wider web and Reddit coverage, periodically search the exact repository URL,
`"phr-mcp"`, and `"phronesis-mcp"` using those sites or a search engine. These are
manual checks: the workflow does not scrape Reddit or claim full-web coverage.

## Run locally

```sh
python3 -m unittest discover -s scripts -p test_community_snapshot.py
# GH_TOKEN is optional for public counts, but required for GitHub code search.
python3 scripts/community-snapshot.py --out-dir /tmp/phronesis-community-signals
```

References: [GitHub scheduled workflows](https://docs.github.com/en/actions/reference/workflows-and-actions/events-that-trigger-workflows#schedule),
[GitHub search limits](https://docs.github.com/en/rest/search/search),
[Hacker News search API](https://hn.algolia.com/api).
