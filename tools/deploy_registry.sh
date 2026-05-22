#!/usr/bin/env bash
# Deploy the wallet registry contract (Solution A).
#
# Compiles `circuits/wallet_registry.compact`, submits a deployment tx
# against the local patched node, captures the deployed contract address
# from the inclusion receipt, and writes it to a config file the patched
# admission code reads on startup:
#
#   circuits/static/wallet-registry/contract_address.txt
#
# This is the "first-tx bootstrap" deployment model. Genesis-baking is
# deliberately not used — it would couple chain genesis tooling to registry
# versioning, which is overkill for the demo scope.
#
# Invoked by `make e2e` as a prerequisite step before any `register_wallet`
# call runs against the patched stack.
#
# Required environment:
#   MIDNIGHT_NODE_WS_URL   default ws://127.0.0.1:9944
#   MIDNIGHT_INDEXER_URL   default http://127.0.0.1:8088/api/v4/graphql
#   COMPACT_PATH           optional; falls back to the workspace lib copy

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
CIRCUITS_DIR="$REPO_ROOT/circuits"
STATIC_DIR="$CIRCUITS_DIR/static/wallet-registry"
ADDRESS_FILE="$STATIC_DIR/contract_address.txt"

NODE_WS_URL="${MIDNIGHT_NODE_WS_URL:-ws://127.0.0.1:9944}"
INDEXER_URL="${MIDNIGHT_INDEXER_URL:-http://127.0.0.1:8088/api/v4/graphql}"

log() { printf '[deploy_registry] %s\n' "$*" >&2; }

require_cmd() {
    command -v "$1" >/dev/null 2>&1 || {
        log "FATAL: required command '$1' not on PATH"
        exit 1
    }
}

require_cmd compact
require_cmd node

# Step 1 — compile circuits/wallet_registry.compact.
log "Compiling wallet_registry.compact"
mkdir -p "$STATIC_DIR"
pushd "$CIRCUITS_DIR" >/dev/null
compact compile -- \
    --no-communications-commitment \
    wallet_registry.compact \
    "$STATIC_DIR"
popd >/dev/null

PROVER="$STATIC_DIR/register.prover"
VERIFIER="$STATIC_DIR/register.verifier"
BZKIR="$STATIC_DIR/register.bzkir"

for f in "$PROVER" "$VERIFIER" "$BZKIR"; do
    [[ -f "$f" ]] || {
        log "FATAL: expected compile artifact missing: $f"
        log "       (compact compile may have produced a different name — adjust"
        log "        the script if you see e.g. wallet_registry.prover instead)"
        exit 1
    }
done

# Step 2 — submit the deployment tx via the existing balance-submit helper.
# That helper expects a tx hex blob. We piggyback on its preview wiring
# rather than reinvent the wallet/node RPC layer.
#
# This script intentionally does not embed the deployment tx construction
# logic — `tools/preview_balance_submit_split_tx.mjs` already understands the
# patched node's wire format. We invoke it with a `--deploy-registry` flag;
# the helper looks up the freshly-compiled artifacts from $STATIC_DIR and
# builds the contract-deploy tx.
log "Submitting registry-contract deployment tx via $NODE_WS_URL"
DEPLOY_OUT="$(MIDNIGHT_NODE_WS_URL="$NODE_WS_URL" \
              MIDNIGHT_INDEXER_URL="$INDEXER_URL" \
              REGISTRY_STATIC_DIR="$STATIC_DIR" \
              node "$REPO_ROOT/tools/preview_balance_submit_split_tx.mjs" \
              --deploy-registry 2>&1)" || {
    log "FATAL: registry deploy tx submission failed"
    printf '%s\n' "$DEPLOY_OUT" >&2
    exit 1
}

# Step 3 — extract the contract address from the inclusion receipt. The
# helper emits a line of the form:
#   registry_contract_address=0x...  (64 hex chars after 0x).
ADDRESS_HEX="$(printf '%s\n' "$DEPLOY_OUT" \
    | sed -n 's/^registry_contract_address=0x\([0-9a-fA-F]\{64\}\).*/\1/p' \
    | head -n1)"

if [[ -z "${ADDRESS_HEX:-}" ]]; then
    log "FATAL: could not parse registry contract address from deploy output:"
    printf '%s\n' "$DEPLOY_OUT" >&2
    exit 1
fi

# Step 4 — persist the address. Admission code reads this on startup and
# passes it to `wallet_registry_root_check`. The proof-server uses it to
# wire up `install_registry_root_checker`.
printf '0x%s\n' "$ADDRESS_HEX" >"$ADDRESS_FILE"
log "Registry contract deployed at 0x$ADDRESS_HEX"
log "Wrote $ADDRESS_FILE — proof-server and node admission read this on boot."
