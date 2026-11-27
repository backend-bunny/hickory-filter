#!/usr/bin/env bash
# Lint the chart, validate its rendered manifests against the Kubernetes
# schemas, and compare golden renders. `--update` rewrites the golden files.
# Golden files hold VERSION where the chart version appears, so the release
# PR's version bump doesn't need to regenerate them.
. "$(dirname "$0")/lib.sh"
need helm; need kubeconform
CHART="$ROOT/charts/hickory-filter"
T="$CHART/tests"
update=0; [ "${1:-}" = "--update" ] && update=1
ver="$(sed -n 's/^version: \([^ ]*\).*/\1/p' "$CHART/Chart.yaml")"
render() { helm template rel "$CHART" --namespace ns "$@" | sed "s/${ver//./\\.}/VERSION/g"; }
fail=0

echo "== lint"
helm lint "$CHART" --strict >/dev/null
helm lint "$CHART" --strict -f "$CHART/ci/all-on-values.yaml" >/dev/null

echo "== schema validation (GrafanaDashboard has no published schema and is the only kind skipped)"
for values in "" "-f $CHART/ci/all-on-values.yaml"; do
  # shellcheck disable=SC2086
  helm template rel "$CHART" --namespace ns $values \
    | kubeconform -strict -summary -skip GrafanaDashboard -schema-location default \
        -schema-location 'https://raw.githubusercontent.com/datreeio/CRDs-catalog/main/{{.Group}}/{{.ResourceKind}}_{{.ResourceAPIVersion}}.json' -
done

echo "== golden renders"
mkdir -p "$T/golden"
for f in "$T"/cases/*.yaml; do
  n="$(basename "$f" .yaml)"
  out="$(render -f "$f")"
  if [ $update = 1 ]; then printf '%s\n' "$out" > "$T/golden/$n.yaml"
  elif ! diff -u "$T/golden/$n.yaml" <(printf '%s\n' "$out"); then echo "golden mismatch: $n" >&2; fail=1; fi
done
n=all-on; out="$(render -f "$CHART/ci/all-on-values.yaml")"
if [ $update = 1 ]; then printf '%s\n' "$out" > "$T/golden/$n.yaml"
elif ! diff -u "$T/golden/$n.yaml" <(printf '%s\n' "$out"); then echo "golden mismatch: $n" >&2; fail=1; fi

echo "== default pod meets the restricted Pod Security Standard"
def="$(helm template rel "$CHART")"
for want in 'runAsNonRoot: true' 'runAsUser: 65532' 'runAsGroup: 65532' 'allowPrivilegeEscalation: false' \
            'readOnlyRootFilesystem: true' 'type: RuntimeDefault' 'drop: \["ALL"\]' 'path: /health' 'path: /ready'; do
  grep -q -- "$want" <<<"$def" || { echo "default render lacks: $want" >&2; fail=1; }
done
! grep -qE 'privileged|hostNetwork|hostPort|hostPath|add:' <<<"$def" || { echo "default render has a restricted-profile violation" >&2; fail=1; }

echo "== forward is the default; recursive mode renders no upstreams and no egress rules"
base="$(helm template rel "$CHART")"
grep -q '^    upstreams = ' <<<"$base" || { echo "the default render has no upstreams" >&2; fail=1; }
! grep -q 'mode = "recursive"' <<<"$base" || { echo "the default render is recursive" >&2; fail=1; }
rec="$(helm template rel "$CHART" -f "$T/cases/recursive-all-on.yaml")"
grep -q 'mode = "recursive"' <<<"$rec" || { echo "the recursive render lacks the mode" >&2; fail=1; }
! grep -q 'upstreams' <<<"$rec" || { echo "the recursive render still has upstreams" >&2; fail=1; }
! grep -qi 'egress' <<<"$rec" || { echo "the NetworkPolicy restricts egress in recursive mode" >&2; fail=1; }
grep -q 'policyTypes: \[Ingress\]' <<<"$rec" || { echo "the recursive render has no NetworkPolicy" >&2; fail=1; }

echo "== README values exist in values.schema.json"
python3 - "$CHART" <<'PY' || fail=1
import json, re, sys
chart = sys.argv[1]
schema = json.load(open(chart + "/values.schema.json"))
defs = schema.get("definitions", {})
def has(node, path):
    if "$ref" in node: node = defs[node["$ref"].split("/")[-1]]
    if not path: return True
    props = node.get("properties", {})
    if path[0] in props: return has(props[path[0]], path[1:])
    return node.get("type") == "object" and "properties" not in node  # free-form object
bad = []
for line in open(chart + "/README.md"):
    m = re.match(r"\| `([A-Za-z.]+)` \|", line)
    if m and not has(schema, m.group(1).split(".")): bad.append(m.group(1))
if bad: print("README names values missing from the schema:", bad); sys.exit(1)
PY

echo "== rendering must fail"
for f in "$T"/fail/*.yaml; do
  want="$(sed -n 's/^# expect: //p' "$f")"
  if err="$(helm template rel "$CHART" -f "$f" 2>&1)"; then echo "$(basename "$f"): rendered but should fail" >&2; fail=1
  elif ! grep -qF "$want" <<<"$err"; then echo "$(basename "$f"): error lacks '$want': $err" >&2; fail=1; fi
done
[ $fail = 0 ] && echo "chart ok"
exit $fail
