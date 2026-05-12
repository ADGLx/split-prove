# Split-Prove PoC Runbook

This PoC tests one boundary:

```text
wallet/client keeps the zswap secret key
proof server builds and proves a split spend without seeing that key
```

The wallet already knows how to derive keys, sync/index chain data, decrypt owned shielded outputs, filter spent outputs, and maintain the zswap Merkle state. The new wallet-side work for split proving is the **handoff**: compute the split-prove request fields and send them to the proof server.

## Current Flow

```text
wallet/client
  existing wallet work:
    derive zswap key
    fetch preview/indexer data
    decrypt owned shielded outputs
    filter spent outputs
    build Merkle state/path

  split-prove handoff:
    prove sk -> skCommitment/nullifier/commitmentHash/pk
    compute skCommitment
    compute nullifier
    compute commitmentHash
    package coin metadata + Merkle state/path
    POST /v2/prove-split-spend

proof server
  reconstruct QualifiedCoinInfo
  load Merkle tree/path
  reject handoffs whose commitment does not reproduce the tree root
  call Input::new_split
  prove midnight/zswap/spend-split
  return proofHex + provedInputHex

preview chain
  read through the indexer
  optionally Dust-balance and submit through the wallet SDK
```

The submit mode spends the selected preview-chain shielded output and creates a
replacement output back to the same wallet key. Full submit mode is now:
client derivation proof -> server split proof -> split transaction assembly ->
wallet SDK Dust balancing -> wallet SDK submission.

## Important Files

- `deps/midnight-ledger/proof-server/src/endpoints.rs`: `POST /v2/prove-split-spend`.
- `deps/midnight-ledger/proof-server/src/preview_client.rs`: local CLI helper that simulates wallet-side preview work and handoff creation.
- `deps/midnight-ledger/proof-server/src/bin/preview_split_prove.rs`: runnable preview e2e command.
- `deps/midnight-ledger/zswap/src/construct.rs`: `new_split` constructors.
- `deps/midnight-ledger/zswap/src/prove.rs`: split circuit key resolution.
- `deps/midnight-ledger/zswap/static/*split*`: local split proving artifacts.
- `tools/derive_midnight_zswap_seed.mjs`: temporary phrase-to-zswap-seed helper for the PoC.
- `tools/preview_balance_submit_split_tx.mjs`: wallet SDK Dust balancing and submit bridge.

## Environment

Create `.env`:

```bash
cp .env.example .env
```

Set either a recovery phrase or a direct zswap seed:

```dotenv
MIDNIGHT_PREVIEW_INDEXER_WS=wss://indexer.preview.midnight.network/api/v4/graphql/ws
MIDNIGHT_PREVIEW_NODE_WS=wss://rpc.preview.midnight.network
MIDNIGHT_PREVIEW_NETWORK_ID=preview

MIDNIGHT_PREVIEW_RECOVERY_PHRASE="word1 word2 ... word24"
MIDNIGHT_PREVIEW_ACCOUNT=0
MIDNIGHT_PREVIEW_ZSWAP_KEY_SCAN_LIMIT=1
MIDNIGHT_PREVIEW_ZSWAP_EVENT_LIMIT=50000
MIDNIGHT_PREVIEW_SUBMIT_MODE=wallet

MIDNIGHT_PREVIEW_ZSWAP_SEED_HEX=
```

`.env` is gitignored. Do not commit it. `MIDNIGHT_PREVIEW_ZSWAP_SEED_HEX` is
only enough for prove-only mode; wallet submit mode requires the recovery phrase
so the helper can derive the matching Dust and unshielded keys.

## Run The Preview PoC

Install dependencies once:

```bash
npm install
```

Run the e2e command:

```bash
cargo run --offline -p midnight-proof-server --bin preview-split-prove \
  --manifest-path deps/midnight-ledger/Cargo.toml
```

Expected output:

```text
proved preview output key_index=0 mt_index=1622 value=500 token=<token-type> status=proofBuilt proof_len=<bytes> tx_hash=<hash> tx_len=<hex chars> submitted=false
```

By default the command starts a local proof server on a random port. To use an already running proof server:

```bash
cargo run --offline -p midnight-proof-server --bin preview-split-prove \
  --manifest-path deps/midnight-ledger/Cargo.toml \
  -- --proof-server-url http://127.0.0.1:6300
```

Submit the assembled transaction to the actual preview chain through the wallet SDK:

```bash
MIDNIGHT_PREVIEW_SUBMIT_TX=true \
cargo run --offline -p midnight-proof-server --bin preview-split-prove \
  --manifest-path deps/midnight-ledger/Cargo.toml
```

Submission defaults to `MIDNIGHT_PREVIEW_SUBMIT_MODE=raw-rpc`, which uses the
wallet SDK to sync Dust, add fee-balancing `DustActions`, finalize the
transaction, and then submit the finalized transaction directly through node RPC.
The selected wallet must have enough spendable Dust. Use
`MIDNIGHT_PREVIEW_SUBMIT_MODE=wallet` to submit through the wallet SDK watcher
instead.
The helper uses local wallet SDK packages when installed, or
`../one-am-wallet/node_modules`; override with `MIDNIGHT_PREVIEW_WALLET_NODE_MODULES`.
First-time Dust proving may need to download and verify Dust proving assets; the
helper retries `finalizeRecipe` twice by default. Tune with
`MIDNIGHT_PREVIEW_DUST_PROVE_ATTEMPTS` and
`MIDNIGHT_PREVIEW_DUST_PROVE_RETRY_DELAY_MS`.

## Endpoint

```text
POST /v2/prove-split-spend
```

Request shape:

```json
{
  "skCommitment": "<32-byte field hex>",
  "nullifier": "<32-byte hex>",
  "pk": "<32-byte public key hex>",
  "commitmentHash": "<32-byte hex>",
  "coinValue": 500,
  "coinType": "<32-byte token type hex>",
  "coinNonce": "<32-byte hex>",
  "mtIndex": 1622,
  "contractAddress": null,
  "zswapState": "<serialized zswap state hex>",
  "clientDerivationProof": "<serialized client derivation proof hex>",
  "prove": true
}
```

Response shape:

```json
{
  "status": "proofBuilt",
  "keyLocation": "midnight/zswap/spend-split",
  "merklePathSource": "zswapState",
  "inputPreimageHex": "<serialized Input<ProofPreimage> hex>",
  "proofHex": "<serialized proof hex>",
  "provedInputHex": "<serialized Input<Proof> hex>",
  "proofError": null
}
```

When `"prove": true`, the endpoint requires `zswapState` or `zswapStateFile`. The older simulated single-leaf fallback is still useful for preimage-shape debugging with `"prove": false`, but it is intentionally rejected for proof-building requests.

## What Is Still Missing

- Move the PoC preview client logic into the real wallet/client integration point.
- Replace full `zswapState` handoff with the smallest acceptable Merkle witness if that is the desired production API.
- Move the wallet SDK Dust balancing bridge into the real wallet/client integration point.
- Harden the endpoint and request validation for production.

## Quick Checks

Compile the CLI:

```bash
cargo check --offline -p midnight-proof-server --bin preview-split-prove \
  --manifest-path deps/midnight-ledger/Cargo.toml
```

Run the synthetic e2e test. This generates a client derivation proof, posts it over the
HTTP endpoint, verifies it on the server, and builds the split spend proof:

```bash
cargo test --offline -p midnight-proof-server synthetic_client_derivation_proof_is_verified_before_split_proving \
  --manifest-path deps/midnight-ledger/Cargo.toml \
  -- --nocapture
```

Run the live preview-wallet e2e test. This is opt-in because it uses wallet
secrets from `.env`, the hosted preview indexer websocket, and a local proof server:

```bash
MIDNIGHT_RUN_PREVIEW_E2E=1 \
cargo test --offline -p midnight-proof-server preview_wallet_proves_real_unspent_split_spend \
  --manifest-path deps/midnight-ledger/Cargo.toml \
  -- --nocapture
```

Run the live submit-to-preview-chain e2e. This spends the selected output:

```bash
MIDNIGHT_RUN_PREVIEW_E2E=1 MIDNIGHT_PREVIEW_SUBMIT_TX=true \
cargo test --offline -p midnight-proof-server preview_wallet_proves_real_unspent_split_spend \
  --manifest-path deps/midnight-ledger/Cargo.toml \
  -- --nocapture
```
