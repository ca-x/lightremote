#!/usr/bin/env bash
# Run the same external contract against an existing Rust binary and local Wrangler.
set -euo pipefail
relay_bin=${1:-./target/debug/roc-remote-relay}
repo_dir=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
relay_bin=$(realpath "$relay_bin")
test_tmp=$(mktemp -d)
cleanup() {
  for child in ${rust_pid:-} ${cf_pid:-}; do kill "$child" 2>/dev/null || true; done
  for child in ${rust_pid:-} ${cf_pid:-}; do wait "$child" 2>/dev/null || true; done
  rm -rf "$test_tmp"
}
trap cleanup EXIT
export ROC_REMOTE_RELAY_TOKEN=local-external-contract-test-token
python3 - "$repo_dir" "$test_tmp" <<'PY'
import pathlib, socket, sys
repo, tmp = map(pathlib.Path, sys.argv[1:])
# Choose two available ports; listeners start immediately after this probe.
with socket.socket() as first, socket.socket() as second:
    first.bind(('127.0.0.1', 0)); second.bind(('127.0.0.1', 0))
    ports = [first.getsockname()[1], second.getsockname()[1]]
(tmp/'ports').write_text('\n'.join(map(str, ports)))
config = (repo/'cloudflare/wrangler.toml').read_text().replace('main = "src/index.ts"', f'main = "{repo}/cloudflare/src/index.ts"')
config = config.replace('ROC_REMOTE_ROOM_TTL_SECONDS = \"3600\"', 'ROC_REMOTE_ROOM_TTL_SECONDS = \"4\"').replace('ROC_REMOTE_WAIT_SECONDS = \"30\"', 'ROC_REMOTE_WAIT_SECONDS = \"1\"')
(tmp/'wrangler.toml').write_text(config)
(tmp/'.dev.vars').write_text('ROC_REMOTE_RELAY_TOKEN=local-external-contract-test-token\n')
PY
mapfile -t relay_ports <"$test_tmp/ports"
ROC_REMOTE_ROOM_TTL_SECONDS=4 ROC_REMOTE_WAIT_SECONDS=1 ROC_REMOTE_BIND="127.0.0.1:${relay_ports[0]}" "$relay_bin" >"$test_tmp/rust.log" 2>&1 &
rust_pid=$!
WRANGLER_SEND_METRICS=false "$repo_dir/cloudflare/node_modules/.bin/wrangler" dev --config "$test_tmp/wrangler.toml" --local --ip 127.0.0.1 --port "${relay_ports[1]}" >"$test_tmp/cloudflare.log" 2>&1 &
cf_pid=$!
for index in 0 1; do
  relay_url="http://127.0.0.1:${relay_ports[$index]}"
  for _ in {1..200}; do
    if curl --silent --fail "$relay_url/healthz" >/dev/null; then break; fi
    if ! kill -0 "$rust_pid" 2>/dev/null || ! kill -0 "$cf_pid" 2>/dev/null; then cat "$test_tmp/rust.log" "$test_tmp/cloudflare.log" >&2; exit 1; fi
    sleep 0.05
  done
  if [[ $index == 0 ]]; then echo 'Rust relay:'; else echo 'Cloudflare local runtime:'; fi
  python3 "$repo_dir/deploy/relay-conformance.py" "$relay_url" --wait-seconds 1
done
