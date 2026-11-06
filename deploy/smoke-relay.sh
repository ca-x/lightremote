#!/usr/bin/env bash
# Accept an already-built executable, so running this never changes workspace locks.
set -euo pipefail
relay_bin=${1:-./target/debug/lightremote-relay}
export LIGHTREMOTE_RELAY_TOKEN=${LIGHTREMOTE_RELAY_TOKEN:-local-smoke-test-token}
export LIGHTREMOTE_BIND=${LIGHTREMOTE_BIND:-127.0.0.1:18787}
smoke_url="http://$LIGHTREMOTE_BIND"
smoke_tmp=$(mktemp -d)
cleanup() {
  if [[ -n ${relay_pid:-} ]]; then kill "$relay_pid" 2>/dev/null || true; wait "$relay_pid" 2>/dev/null || true; fi
  rm -rf "$smoke_tmp"
}
trap cleanup EXIT
"$relay_bin" >"$smoke_tmp/server.log" 2>&1 &
relay_pid=$!
for _ in {1..100}; do
  if curl --silent --fail "$smoke_url/healthz" >"$smoke_tmp/health.json"; then break; fi
  if ! kill -0 "$relay_pid" 2>/dev/null; then cat "$smoke_tmp/server.log" >&2; exit 1; fi
  sleep 0.05
done
python3 -c 'import json,sys; assert json.load(open(sys.argv[1])) == {"ok":True,"protocol":"lightremote-relay.v1"}' "$smoke_tmp/health.json"
[[ $(curl --silent -o /dev/null -w '%{http_code}' -X POST -H 'Content-Type: application/json' -d '{}' "$smoke_url/v1/rooms") == 401 ]]
curl --silent --fail -X POST -H "Authorization: Bearer $LIGHTREMOTE_RELAY_TOKEN" -H 'Content-Type: application/json' -d '{}' "$smoke_url/v1/rooms" >"$smoke_tmp/room.json"
python3 -c 'import json,sys; r=json.load(open(sys.argv[1])); assert len(r["host_token"]) == len(r["client_token"]) == 43; assert r["host_token"] != r["client_token"]; assert isinstance(r["expires_at"], int)' "$smoke_tmp/room.json"
echo 'Relay executable health/auth/create smoke passed.'
