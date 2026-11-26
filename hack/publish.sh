#!/usr/bin/env bash
# Publish the image to the registry with the tags X.Y.Z and sha-<short commit>
# (never a floating tag) by running the same build as hack/build.sh and pushing
# the result with apko publish. The index digest the tags point to is written to
# out/image.digest.
#   DRY_RUN=1 hack/publish.sh   print the tags and stop
#   IMAGE=<repo>                override ghcr.io/backend-bunny/hickory-filter
. "$(dirname "$0")/lib.sh"
cd "$ROOT"
IMAGE="${IMAGE:-ghcr.io/backend-bunny/hickory-filter}"
version="$(sed -n 's/^  version: \([^ ]*\).*/\1/p' melange.yaml | head -1)"
[ -n "$version" ] || die "no version in melange.yaml"
sha="${GITHUB_SHA:-$(git rev-parse HEAD)}"
tags="$IMAGE:$version $IMAGE:sha-${sha:0:7}"
if [ "${DRY_RUN:-0}" = 1 ]; then
  printf '%s\n' $tags
  exit 0
fi
PUBLISH_TAGS="$tags" exec "$ROOT/hack/build.sh"
