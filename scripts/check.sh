#!/usr/bin/env bash
# Run from any directory; dependency installation belongs to the caller.
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/.."
check_rust() {
  cargo fmt --all -- --check
  # Omitting --workspace intentionally selects Cargo.toml's default-members.
  cargo clippy --locked --all-targets --no-default-features -- -D warnings
  cargo test --locked --no-default-features
  cargo clippy --locked --all-targets -- -D warnings
  cargo test --locked
}
check_web() {
  (cd web && npm run typecheck && npm test && npm run build)
}
check_cloudflare() {
  (cd cloudflare && npm run check && npm test)
}
case "${1:-all}" in
  rust) check_rust ;;
  web) check_web ;;
  cloudflare) check_cloudflare ;;
  all) check_rust; check_web; check_cloudflare ;;
  *) echo "usage: $0 [rust|web|cloudflare|all]" >&2; exit 2 ;;
esac
