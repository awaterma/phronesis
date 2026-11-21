#!/usr/bin/env bash
# Locked documentation-site build.
#
# Builds docs/ with Jekyll into docs/_site using the environment pinned in
# docs/Gemfile and docs/Gemfile.lock. When Docker is available the build
# runs in the Ruby image pinned BY DIGEST (docs/.build-image-digest, a
# multi-architecture manifest digest verified to be Ruby 3.3.14), with the
# gems cached in a named volume, so a local build matches CI exactly.
# Set DOCS_BUILD_NO_DOCKER=1 to force the local bundler path instead.
#
# Caches are preserved: only the freshly generated output directory is
# removed before a build. .jekyll-cache and the bundler volume are kept
# between builds — never delete them by hand.
#
# Usage: scripts/website/build.sh [DEST]
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
SITE_DIR="$REPO_ROOT/docs"
DIGEST_FILE="$REPO_ROOT/docs/.build-image-digest"
DIGEST="$(cat "$DIGEST_FILE" 2>/dev/null | tr -d '[:space:]')"
# Override only with another @sha256:... pin; never a floating tag.
IMAGE="${BUILDER_IMAGE:-ruby:3.3@${DIGEST}}"

if [ -z "$DIGEST" ]; then
  echo "error: docs/.build-image-digest is missing; cannot pin the build image" >&2
  exit 2
fi
case "$IMAGE" in
  *@sha256:*) ;;
  *) echo "error: BUILDER_IMAGE must be pinned by digest (name@sha256:...), got: $IMAGE" >&2; exit 2 ;;
esac

# Only the freshly generated output is removed; .jekyll-cache and the
# bundler volume are caches and are preserved. The rm happens on the HOST
# side: container-side deletion over a bind mount does not reliably
# propagate to macOS hosts, which would leave stale entries in the
# artifact for the exclusion audit to trip over.
rm -rf "$SITE_DIR/_site"

if command -v docker >/dev/null 2>&1 && [ "${DOCS_BUILD_NO_DOCKER:-0}" != "1" ]; then
  exec docker run --rm \
    -v "$REPO_ROOT":/repo \
    -v phronesis-docs-bundle:/usr/local/bundle \
    -e BUNDLE_FROZEN=true \
    -e JEKYLL_ENV=production \
    -w /repo/docs \
    "$IMAGE" \
    sh -c 'bundle install --quiet && bundle exec jekyll build --strict_front_matter -s /repo/docs -d /repo/docs/_site'
fi

cd "$SITE_DIR"
export BUNDLE_FROZEN=true
bundle install --quiet
JEKYLL_ENV=production bundle exec jekyll build --strict_front_matter
