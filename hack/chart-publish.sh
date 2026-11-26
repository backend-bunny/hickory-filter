#!/usr/bin/env bash
# Package the chart from a temp copy with image.digest filled in, and push it
# to OCI. The committed values.yaml keeps `digest: ""`.
#   IMAGE_DIGEST=sha256:... hack/chart-publish.sh   package, push, write out/chart.digest
#   DRY_RUN=1 ...                                   package only (digest optional)
. "$(dirname "$0")/lib.sh"
need helm
REGISTRY="${CHART_REGISTRY:-oci://ghcr.io/backend-bunny/charts}"
DRY_RUN="${DRY_RUN:-0}"
digest="${IMAGE_DIGEST:-}"
if [ -z "$digest" ]; then
  [ "$DRY_RUN" = 1 ] || die "IMAGE_DIGEST is required (the index digest just published)"
  digest="sha256:$(printf '0%.0s' $(seq 64))"
fi
[[ "$digest" =~ ^sha256:[0-9a-f]{64}$ ]] || die "IMAGE_DIGEST is not a sha256 digest: $digest"

work="$(mktemp -d)"; trap 'rm -rf "$work"' EXIT
cp -r "$ROOT/charts/hickory-filter" "$work/hickory-filter"
sed -i "s|^  digest: \"\"|  digest: \"$digest\"|" "$work/hickory-filter/values.yaml"
grep -q "digest: \"$digest\"" "$work/hickory-filter/values.yaml" || die "could not set image.digest"
helm lint "$work/hickory-filter" --strict >/dev/null
helm package "$work/hickory-filter" --destination "$work" >/dev/null
pkg="$(ls "$work"/hickory-filter-*.tgz)"
mkdir -p "$ROOT/out"
if [ "$DRY_RUN" = 1 ]; then
  cp "$pkg" "$ROOT/out/"
  echo "dry run: packaged out/$(basename "$pkg"); image: $(helm template r "$ROOT/out/$(basename "$pkg")" | sed -n 's/^ *image: "\(.*\)"/\1/p')"
  exit 0
fi
push="$(helm push "$pkg" "$REGISTRY" 2>&1)"; echo "$push"
sed -n 's/^Digest: //p' <<<"$push" | tail -1 > "$ROOT/out/chart.digest"
[ -s "$ROOT/out/chart.digest" ] || die "no chart digest in the helm push output"
