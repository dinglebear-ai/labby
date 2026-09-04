#!/bin/sh
set -eu

python3 scripts/check-depot-control-plane-contract.py
cargo test -p labby --lib --all-features dispatch::depot

if [ -d apps/gateway-admin/node_modules ]; then
  (cd apps/gateway-admin && npx tsc --noEmit && npm run test:unit && npm run build)
else
  echo 'gateway-admin dependencies are not installed; frontend qualification cannot run' >&2
  exit 1
fi

if rg -n 'LABBY_DEPOT_TOKEN' apps/gateway-admin/out; then
  echo 'server-only Depot credential variable leaked into static assets' >&2
  exit 1
fi

echo 'Local immutable qualification passed. Real-transport canary evidence is still required.'
