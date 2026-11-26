#!/usr/bin/env bash
# Smoke-test the built image (out/image.tar): user, annotations, no shell, a
# static binary, DNS answers for a blocked and an unblocked name against a
# local upstream, /health and /ready, with all capabilities dropped and a
# read-only root filesystem. Needs a container runtime (docker).
. "$(dirname "$0")/lib.sh"
need docker; need python3; need readelf
cd "$ROOT"
TAR="${IMAGE_TAR:-out/image.tar}"
[ -f "$TAR" ] || die "$TAR not found (run hack/build.sh)"
version="$(sed -n 's/^  version: \([^ ]*\).*/\1/p' melange.yaml)"
DNS_PORT="${DNS_PORT:-28053}"; HTTP_PORT="${HTTP_PORT:-28080}"; UP_PORT="${UP_PORT:-28054}"
work="$(mktemp -d)"; name="hf-smoke-$$"; pids=""
cleanup() { docker rm -f "$name" >/dev/null 2>&1 || true; for p in $pids; do kill "$p" 2>/dev/null || true; done; rm -rf "$work"; }
trap cleanup EXIT
fail=0; check() { if [ "$2" = ok ]; then echo "ok   $1"; else echo "FAIL $1 ($2)" >&2; fail=1; fi; }

ref="$(docker load -i "$TAR" | sed -n 's/^Loaded image: //p' | tail -1)"
[ -n "$ref" ] || die "docker load printed no image reference"
echo "image: $ref"

# user and annotations (labels carry the OCI annotations)
user="$(docker image inspect "$ref" --format '{{.Config.User}}')"
[ "$user" = 65532 ] || [ "$user" = "65532:65532" ] && check "runs as uid 65532" ok || check "runs as uid 65532" "$user"
for k in title description licenses source url; do
  v="$(docker image inspect "$ref" --format "{{index .Config.Labels \"org.opencontainers.image.$k\"}}")"
  [ -n "$v" ] && check "annotation org.opencontainers.image.$k" ok || check "annotation org.opencontainers.image.$k" missing
done
[ "$(docker image inspect "$ref" --format '{{index .Config.Labels "org.opencontainers.image.licenses"}}')" = "MIT OR Apache-2.0" ] \
  && check "license annotation matches" ok || check "license annotation matches" differs

# no shell
if docker run --rm --entrypoint /bin/sh "$ref" -c true >/dev/null 2>&1 || docker run --rm --entrypoint /bin/busybox "$ref" true >/dev/null 2>&1; then
  check "no shell in the image" "a shell ran"; else check "no shell in the image" ok; fi

# static binary, version
cid="$(docker create "$ref")"; docker cp "$cid:/usr/bin/hickory-filter" "$work/hf"
if readelf -l "$work/hf" | grep -q INTERP || readelf -d "$work/hf" 2>/dev/null | grep -q NEEDED; then check "static binary" "dynamic"; else check "static binary" ok; fi
out="$(docker run --rm --read-only --cap-drop ALL "$ref" --version)"
case "$out" in *"$version"*) check "--version is $version" ok ;; *) check "--version is $version" "$out" ;; esac
for f in LICENSE-MIT LICENSE-APACHE; do
  docker cp "$cid:/usr/share/doc/hickory-filter/$f" "$work/$f" >/dev/null 2>&1 \
    && check "ships $f" ok || check "ships $f" missing
done
docker cp "$cid:/usr/share/doc/hickory-filter/THIRD-PARTY-NOTICES.md" "$work/THIRD-PARTY-NOTICES.md" >/dev/null 2>&1 \
  && check "ships THIRD-PARTY-NOTICES.md" ok || check "ships THIRD-PARTY-NOTICES.md" missing
if [ -f "$ROOT/out/notices.json" ] && [ -f "$work/THIRD-PARTY-NOTICES.md" ]; then
  if python3 hack/notices.py check-image "$work/THIRD-PARTY-NOTICES.md" "$ROOT/out/notices.json" "$work/hf"; then
    check "notices list the same crates as notices.json and the binary" ok
  else check "notices list the same crates as notices.json and the binary" differ; fi
else check "notices.json present" missing; fi
docker rm "$cid" >/dev/null

# a local upstream that answers every A query with 192.0.2.1
cat > "$work/upstream.py" <<PY
import socket, struct, sys
s = socket.socket(socket.AF_INET, socket.SOCK_DGRAM); s.bind(("127.0.0.1", int(sys.argv[1])))
while True:
    q, a = s.recvfrom(512)
    i = 12
    while q[i]: i += q[i] + 1
    end = i + 5
    r = q[:2] + b"\x81\x80" + q[4:6] + b"\x00\x01\x00\x00\x00\x00" + q[12:end] + b"\xc0\x0c\x00\x01\x00\x01\x00\x00\x00\x3c\x00\x04\xc0\x00\x02\x01"
    s.sendto(r, a)
PY
python3 "$work/upstream.py" "$UP_PORT" & pids="$pids $!"
cat > "$work/config.toml" <<CFG
listen = ["127.0.0.1:$DNS_PORT"]
upstreams = ["dns://127.0.0.1:$UP_PORT"]
http = "127.0.0.1:$HTTP_PORT"
cache_size = 0
[filter]
refresh = "0"
rules = ["block domain blocked.example"]
CFG
chmod 755 "$work"; chmod 644 "$work/config.toml"
docker run -d --name "$name" --network host --read-only --cap-drop ALL --security-opt no-new-privileges \
  -v "$work/config.toml:/etc/hickory-filter/config.toml:ro" "$ref" >/dev/null

for _ in $(seq 1 50); do curl -sf "http://127.0.0.1:$HTTP_PORT/ready" >/dev/null && break; sleep 0.2; done
code() { curl -s -o /dev/null -w '%{http_code}' "http://127.0.0.1:$HTTP_PORT$1"; }
[ "$(code /health)" = 200 ] && check "/health is 200" ok || check "/health is 200" "$(code /health)"
[ "$(code /ready)" = 200 ] && check "/ready is 200" ok || check "/ready is 200" "$(code /ready)"
[ "$(code /metrics)" = 200 ] && check "/metrics is 200" ok || check "/metrics is 200" "$(code /metrics)"

ask() { python3 - "$DNS_PORT" "$1" <<'PY'
import socket, struct, sys
port, name = int(sys.argv[1]), sys.argv[2]
q = struct.pack(">HHHHHH", 7, 0x0100, 1, 0, 0, 0) + b"".join(bytes([len(p)]) + p.encode() for p in name.split(".")) + b"\0" + struct.pack(">HH", 1, 1)
s = socket.socket(socket.AF_INET, socket.SOCK_DGRAM); s.settimeout(5); s.sendto(q, ("127.0.0.1", port))
r = s.recv(512); print(".".join(str(b) for b in r[-4:]) if (r[3] & 15) == 0 and r[7] else "rcode%d" % (r[3] & 15))
PY
}
[ "$(ask blocked.example)" = 0.0.0.0 ] && check "blocked name is answered with the block response" ok || check "blocked name is answered with the block response" "$(ask blocked.example)"
[ "$(ask ok.example)" = 192.0.2.1 ] && check "unblocked name is forwarded to the upstream" ok || check "unblocked name is forwarded to the upstream" "$(ask ok.example)"
[ "$(docker inspect "$name" --format '{{.State.Running}}')" = true ] && check "still running" ok || check "still running" "$(docker logs "$name" 2>&1 | tail -3)"

[ $fail = 0 ] && echo "image smoke test passed"
exit $fail
