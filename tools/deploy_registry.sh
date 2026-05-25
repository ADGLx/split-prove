#!/usr/bin/env bash
# Deploy the wallet registry contract (Solution A).
#
# Compiles `circuits/wallet_registry.compact`, then either:
#   • submits a deployment tx against a live patched node and captures the
#     deployed contract address (production / live-e2e mode), or
#   • in demo mode (`MIDNIGHT_REGISTRY_DEMO_MODE=1` or when no live deploy
#     environment is configured) writes a deterministic placeholder address.
#
# Either way, the result is persisted to
#   circuits/static/wallet-registry/contract_address.txt
# so admission code can read it on startup. In demo mode the proof-server
# installs a permissive registry-root checker that accepts any root — see
# `midnight_proof_server::install_registry_root_checker_for_demo`. The address
# file's main role for synthetic/preview e2e is to confirm the
# `wallet_registry_contract_address()` loader sees a value (smoke test of
# the boot wiring); the placeholder is never actually consulted because the
# permissive checker short-circuits the lookup.
#
# Required environment for live-deploy mode:
#   MIDNIGHT_NODE_WS_URL                  ws://127.0.0.1:9944
#   MIDNIGHT_INDEXER_URL                  http://127.0.0.1:8088/api/v4/graphql
#   MIDNIGHT_PREVIEW_REGISTRY_PREDEPLOYED_ADDRESS  hex address from a prior
#                                         contract deployment (the live
#                                         contract-deploy path is not yet
#                                         wired in preview_balance_submit_split_tx.mjs)
#
# Demo-mode toggle:
#   MIDNIGHT_REGISTRY_DEMO_MODE=1         force demo placeholder (skips
#                                         live submission attempts)

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
CIRCUITS_DIR="$REPO_ROOT/circuits"
STATIC_DIR="$CIRCUITS_DIR/static/wallet-registry"
ADDRESS_FILE="$STATIC_DIR/contract_address.txt"

NODE_WS_URL="${MIDNIGHT_NODE_WS_URL:-ws://127.0.0.1:9944}"
INDEXER_URL="${MIDNIGHT_INDEXER_URL:-http://127.0.0.1:8088/api/v4/graphql}"

# Demo placeholder — a deterministic value that's obviously synthetic when
# you look at it (`1f1e1d…` walks down from 0x1f for visual identification).
# The Rust loader accepts any 32-byte hex; the proof-server's permissive
# checker accepts any root, so this never gets cross-checked against a real
# contract state in demo mode.
PLACEHOLDER_ADDRESS="1f1e1d1c1b1a191817161514131211100f0e0d0c0b0a09080706050403020100"

log() { printf '[deploy_registry] %s\n' "$*" >&2; }

require_cmd() {
    command -v "$1" >/dev/null 2>&1 || {
        log "FATAL: required command '$1' not on PATH"
        exit 1
    }
}

require_cmd compact
require_cmd node

# Step 1 — compile circuits/wallet_registry.compact. Skipped if a recent
# `register.verifier` is already present (saves ~30s on repeated invocations
# during dev). Pass FORCE_RECOMPILE=1 to rebuild.
if [[ -z "${FORCE_RECOMPILE:-}" && -f "$STATIC_DIR/register.verifier" \
      && -f "$STATIC_DIR/register.prover" && -f "$STATIC_DIR/register.bzkir" ]]; then
    log "Reusing existing compile artifacts under $STATIC_DIR"
    log "  (set FORCE_RECOMPILE=1 to rebuild)"
else
    log "Compiling wallet_registry.compact"
    mkdir -p "$STATIC_DIR"
    # `compact compile` writes into compiler/contract/keys/zkir subdirs. We
    # need the keys + zkir at the top level so the embedded include_bytes!
    # paths in the Rust code still resolve.
    TMP_DIR="$(mktemp -d)"
    trap 'rm -rf "$TMP_DIR"' EXIT
    pushd "$CIRCUITS_DIR" >/dev/null
    compact compile -- \
        --no-communications-commitment \
        wallet_registry.compact \
        "$TMP_DIR"
    popd >/dev/null
    cp "$TMP_DIR/keys/register.prover"   "$STATIC_DIR/"
    cp "$TMP_DIR/keys/register.verifier" "$STATIC_DIR/"
    cp "$TMP_DIR/zkir/register.bzkir"    "$STATIC_DIR/"
    cp "$TMP_DIR/zkir/register.zkir"     "$STATIC_DIR/"
fi

PROVER="$STATIC_DIR/register.prover"
VERIFIER="$STATIC_DIR/register.verifier"
BZKIR="$STATIC_DIR/register.bzkir"

for f in "$PROVER" "$VERIFIER" "$BZKIR"; do
    [[ -f "$f" ]] || {
        log "FATAL: expected compile artifact missing: $f"
        exit 1
    }
done

# Step 2 — decide between live-deploy mode and demo placeholder mode.
demo_mode() {
    [[ "${MIDNIGHT_REGISTRY_DEMO_MODE:-}" == "1" ]] && return 0
    # If neither a predeployed address override nor a Midnight network is
    # configured, fall back to demo mode automatically.
    [[ -z "${MIDNIGHT_PREVIEW_REGISTRY_PREDEPLOYED_ADDRESS:-}" \
       && -z "${WALLET_REGISTRY_CONTRACT_ADDRESS:-}" ]]
}

if demo_mode; then
    log "Demo mode — no live deployment configured."
    log "Writing deterministic placeholder address to $ADDRESS_FILE."
    log "  (Set MIDNIGHT_PREVIEW_REGISTRY_PREDEPLOYED_ADDRESS=0x<64hex> for"
    log "   live-deploy mode, or MIDNIGHT_REGISTRY_DEMO_MODE=1 to silence"
    log "   this message.)"
    mkdir -p "$STATIC_DIR"
    printf '0x%s\n' "$PLACEHOLDER_ADDRESS" >"$ADDRESS_FILE"
    log "Registry placeholder address: 0x$PLACEHOLDER_ADDRESS"
    log "Wrote $ADDRESS_FILE — proof-server reads this on boot."
    exit 0
fi

# Step 3 — live-deploy mode. We piggyback on
# `tools/preview_balance_submit_split_tx.mjs --deploy-registry`, which
# expects a predeployed address override (the live-node contract-deploy tx
# construction itself is a documented gap — see `preview_balance_submit_split_tx.mjs`).
log "Live-deploy mode — submitting via $NODE_WS_URL"
DEPLOY_OUT="$(MIDNIGHT_NODE_WS_URL="$NODE_WS_URL" \
              MIDNIGHT_INDEXER_URL="$INDEXER_URL" \
              REGISTRY_STATIC_DIR="$STATIC_DIR" \
              node "$REPO_ROOT/tools/preview_balance_submit_split_tx.mjs" \
              --deploy-registry 2>&1)" || {
    log "FATAL: registry deploy tx submission failed"
    printf '%s\n' "$DEPLOY_OUT" >&2
    exit 1
}

# Step 4 — extract the contract address from the helper output:
#   registry_contract_address=0x...  (64 hex chars after 0x).
ADDRESS_HEX="$(printf '%s\n' "$DEPLOY_OUT" \
    | sed -n 's/^registry_contract_address=0x\([0-9a-fA-F]\{64\}\).*/\1/p' \
    | head -n1)"

if [[ -z "${ADDRESS_HEX:-}" ]]; then
    log "FATAL: could not parse registry contract address from deploy output:"
    printf '%s\n' "$DEPLOY_OUT" >&2
    exit 1
fi

# Step 5 — persist. `preview_balance_submit_split_tx.mjs` already writes
# the file but we double-check and re-emit the canonical 0x-prefixed form.
printf '0x%s\n' "$ADDRESS_HEX" >"$ADDRESS_FILE"
log "Registry contract deployed at 0x$ADDRESS_HEX"
log "Wrote $ADDRESS_FILE — proof-server and node admission read this on boot."
