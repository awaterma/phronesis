#!/usr/bin/env python3
"""Collect public discovery signals; never infer users from downloads or clones."""
import argparse
import datetime
import html
import json
import os
from pathlib import Path
import urllib.error
import urllib.parse
import urllib.request

REPOSITORY = "awaterma/phronesis"
CRATES = ("phronesis", "phronesis-mcp", "phronesis-governance", "phronesis-rhai", "phronesis-metrics")
TERMS = ('"awaterma/phronesis"', '"phr-mcp"', '"phronesis-mcp"')
LIMIT = 100


def search_results(data, kind):
    items = data["items"]
    if kind == "code":
        # REST code search does not honor the web UI's is:public qualifier.
        # Require an explicit public repository flag before retaining any result.
        items = [x for x in items if x.get("repository", {}).get("private") is False]
    return {"total": len(items) if kind == "code" else data["total_count"],
            "total_scope": "public results returned" if kind == "code" else "query matches",
            "incomplete": data.get("incomplete_results", False),
            "truncated": data["total_count"] > LIMIT,
            "items": [{"url": x["html_url"], "label": x.get("title", x.get("path", "reference"))}
                      for x in items if not x.get("repository", {}).get("private", False)]}


def fetch(url):
    headers = {"User-Agent": "phronesis-community-snapshot (github.com/awaterma/phronesis)"}
    if urllib.parse.urlparse(url).netloc == "api.github.com":
        headers["Accept"] = "application/vnd.github+json"
        token = os.environ.get("GH_TOKEN") or os.environ.get("GITHUB_TOKEN")
        if token:
            headers["Authorization"] = f"Bearer {token}"
    request = urllib.request.Request(url, headers=headers)
    with urllib.request.urlopen(request, timeout=30) as response:
        return json.load(response)


def collect(get=fetch):
    snapshot = {"schema_version": 1, "collected_at": datetime.datetime.now(datetime.timezone.utc).isoformat(),
                "repository": REPOSITORY, "sources": {}, "errors": []}

    def source(key, url, transform):
        try:
            snapshot["sources"][key] = {"url": url, "data": transform(get(url))}
        except (urllib.error.URLError, TimeoutError, ValueError, KeyError, TypeError) as error:
            # Never include response bodies, request headers, or credentials in artifacts.
            code = getattr(error, "code", None)
            if isinstance(error, urllib.error.HTTPError):
                error.close()
            snapshot["errors"].append({"source": key, "url": url, "error": type(error).__name__, "http_status": code})

    for crate in CRATES:
        source(f"crate:{crate}", f"https://crates.io/api/v1/crates/{crate}",
               lambda d: {k: d["crate"].get(k) for k in ("downloads", "recent_downloads", "max_version")})
        source(f"dependents:{crate}", f"https://crates.io/api/v1/crates/{crate}/reverse_dependencies?page=1&per_page={LIMIT}",
               lambda d: {"total": d["meta"]["total"], "limit": LIMIT, "truncated": d["meta"]["total"] > LIMIT,
                          "crates": [{"crate": v["crate"], "version": v["num"], "companion": v["crate"] in CRATES}
                                     for v in d["versions"]]})
    source("github:repository", f"https://api.github.com/repos/{REPOSITORY}",
           lambda d: {k: d[k] for k in ("stargazers_count", "forks_count", "subscribers_count")})
    source("github:forks", f"https://api.github.com/repos/{REPOSITORY}/forks?sort=newest&per_page={LIMIT}",
           lambda d: {"limit": LIMIT, "possibly_truncated": len(d) == LIMIT,
                      "items": [{k: r[k] for k in ("full_name", "html_url", "created_at")} for r in d if not r.get("private", True)]})
    for term in TERMS:
        for kind in ("issues", "code"):
            query = f"{term} -user:awaterma" + (" is:public" if kind == "issues" else "")
            url = "https://api.github.com/search/" + kind + "?" + urllib.parse.urlencode({"q": query, "per_page": LIMIT})
            source(f"github:{kind}:{term}", url,
                   lambda d, kind=kind: search_results(d, kind))
        url = "https://hn.algolia.com/api/v1/search_by_date?" + urllib.parse.urlencode({"query": term.strip('"'), "hitsPerPage": LIMIT})
        source(f"hn:{term}", url,
               lambda d: {"total": d["nbHits"], "truncated": d["nbHits"] > LIMIT,
                          "items": [{"url": "https://news.ycombinator.com/item?id=" + str(x["objectID"]),
                                     "label": x.get("title") or x.get("story_title") or "comment"} for x in d["hits"]]})
    snapshot["status"] = "partial" if snapshot["errors"] else "complete"
    return snapshot


def summary(snapshot):
    lines = ["# Phronesis public signals", "", f"Collected: {snapshot['collected_at']} — {snapshot['status']}", "",
             "Downloads, stars, forks and search hits are interest signals, **not confirmed users**.",
             "Companion crate downloads overlap; do not add them into a user count.", "",
             "Search results are unreviewed candidates: namesakes, mirrors and automated reports may appear.",
             "No private traffic, individual download identities, comment bodies or telemetry are collected.", ""]
    for key, entry in snapshot["sources"].items():
        data = entry["data"]
        lines.extend([f"## {html.escape(key)}", ""])
        if "items" in data:
            lines.append(f"Returned {len(data['items'])} candidates. " +
                         (f"Reported total ({data.get('total_scope', 'query matches')}): {data['total']}." if "total" in data else ""))
            for item in data["items"][:30]:
                label = html.escape(str(item.get("label", item.get("full_name", "reference"))))
                label = label.replace("[", "&#91;").replace("]", "&#93;").replace("\n", " ")
                url = item.get("url", item.get("html_url", ""))
                # Sources produce public GitHub/HN URLs; encode markdown delimiters.
                if url.startswith(("https://github.com/", "https://news.ycombinator.com/")):
                    lines.append(f"- [{label}]({url.replace(')', '%29').replace('(', '%28')})")
            if len(data["items"]) > 30:
                lines.append("See snapshot.json for the remaining returned candidates.")
        else:
            lines.extend(["```json", json.dumps(data, indent=2), "```"])
        if data.get("truncated") or data.get("possibly_truncated") or data.get("incomplete"):
            lines.append("**Partial source results:** pagination cap or API search limit reached.")
        lines.append("")
    if snapshot["errors"]:
        lines.extend(["## Unavailable sources", "", "These are unknown, not zero results.", ""])
        for error in snapshot["errors"]:
            lines.append(f"- {html.escape(error['source'])}: {error['error']} (HTTP {error['http_status']})")
    return "\n".join(lines) + "\n"


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--out-dir", type=Path, required=True)
    args = parser.parse_args()
    snapshot = collect()
    args.out_dir.mkdir(parents=True, exist_ok=True)
    (args.out_dir / "snapshot.json").write_text(json.dumps(snapshot, indent=2) + "\n")
    (args.out_dir / "summary.md").write_text(summary(snapshot))
    return 1 if snapshot["errors"] else 0


if __name__ == "__main__":
    raise SystemExit(main())
