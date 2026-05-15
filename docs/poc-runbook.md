# Split-Prove PoC Runbook

This PoC tests one boundary:

```text
wallet/client keeps the zswap secret key
proof server builds and proves a split spend without seeing that key
```

The wallet already knows how to derive keys, sync/index chain data, decrypt owned shielded outputs, filter spent outputs, and maintain the zswap Merkle state. The new wallet-side work for split proving is the **handoff** plus a real local proof: it computes the split-prove request fields and proves that the wallet knows the `sk` linking those fields.

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
    generate/reuse wallet attestation proof
    prove sk -> nullifier/coinBindingTag/commitmentSk
    compute nullifier
    read selected output commitmentHash
    package coin metadata + Merkle state/path
    POST /v2/prove-split-spend

proof server
  pre-verify wallet attestation and client derivation proofs
  reconstruct QualifiedCoinInfo
  recompute commitmentHash = H(coin, pk) and reject mismatches
  load Merkle tree/path
  reject handoffs whose commitment does not reproduce the tree root
  prove coinCommitment, Merkle membership, nullifier insertion, coinBindingTag, and value commitment
  call Input::new_split
  prove midnight/zswap/spend-split
  return proofHex + provedInputHex

local chain
  verify attestation, client-derivation, and spend-split proofs in node admission
  read through the indexer
  Dust-balance and submit through the wallet SDK/raw RPC bridge
```

The live e2e always spends the selected local-chain shielded output and creates
a shielded output for `MIDNIGHT_PREVIEW_RECIPIENT_SHIELDED_ADDRESS`. The token
movement is full split-send only: client derivation proof -> server split proof
-> split transaction assembly -> wallet SDK Dust balancing -> submission.

### Interpreting Proof Timings

The wallet-side per-spend client-derivation circuit is intentionally smaller than the server split-spend circuit. The current artifacts are:

| Circuit | prover key | Role |
|---|---:|---|
| `sk_prove` / client derivation | 2.82 MB | per spend, wallet-local |
| `wallet_attest` / wallet attestation | 2.82 MB | one time per `sk`, wallet-local |
| `spend-split` / server split spend | 5.74 MB | per spend, proof server |

The wallet attestation proves `pk = H_persistent(sk)` and `commitmentSk = H_transient(sk, r)` once. The per-spend client proof opens that same `commitmentSk`, proves the canonical `nullifier = H_persistent(coin, sk)`, and discloses `coinBindingTag = H_transient(domain, coin, pk)`. The server split proof computes the canonical `coinCommitment = H_persistent(coin, pk)`, checks the Merkle leaf, inserts the public nullifier, discloses the same `coinBindingTag`, and proves the value commitment. The e2e report treats the per-spend client proof and server proof durations as the primary split-prove comparison and separates out attestation, scan, handoff overhead, output proving, Dust balancing, and submit time because those are setup or full transaction workflow costs.

## Important Files

- `deps/midnight-ledger/proof-server/src/endpoints.rs`: `POST /v2/prove-split-spend`.
- `deps/midnight-ledger/proof-server/src/preview_client.rs`: local CLI helper that simulates wallet-side preview work and handoff creation.
- `deps/midnight-ledger/proof-server/src/bin/preview_split_prove.rs`: runnable preview e2e command.
- `deps/midnight-ledger/zswap/src/construct.rs`: `new_split` constructors.
- `deps/midnight-ledger/zswap/src/prove.rs`: split circuit key resolution.
- `deps/midnight-ledger/zswap/static/*split*`: local split proving artifacts.
- `tools/derive_midnight_zswap_seed.mjs`: temporary phrase-to-zswap-seed helper for the PoC.
- `tools/preview_balance_submit_split_tx.mjs`: wallet SDK Dust balancing and submit bridge.

## Compiling Circuits

Use the local Compact CLI directly when regenerating the split-prove artifacts:

```bash
compact compile --no-communications-commitment circuits/sk_proof.compact /tmp/sk-prove-compile
compact compile --no-communications-commitment circuits/wallet_attestation.compact /tmp/wallet-attest-compile
compact compile --no-communications-commitment deps/midnight-ledger/zswap/zswap-split.compact /tmp/zswap-split-compile
```

Copy the generated client derivation artifacts into `circuits/static/client-derivation/` and the wallet attestation artifacts into `circuits/static/wallet-attestation/`. Mirror the verifier keys needed by node admission into `deps/midnight-ledger/zswap/static/client-derivation.verifier` and `deps/midnight-ledger/zswap/static/wallet-attestation.verifier`. Copy the generated zswap `spendSplitUser` and `signSplitUser` artifacts into `deps/midnight-ledger/zswap/static/`, update the `.sha256` sidecars, and mirror `spendSplitUser.zkir` to `deps/midnight-ledger/zkir-precompiles/zswap/spend-split.zkir`.

## Environment

Create `.env`:

```bash
cp .env.example .env
```

Use the local-dev defaults from `.env.example`; the `MIDNIGHT_PREVIEW_*` prefix is historical and still targets the local undeployed network for this PoC:

```dotenv
MIDNIGHT_PREVIEW_NODE_WS=ws://127.0.0.1:9944
MIDNIGHT_PREVIEW_INDEXER_HTTP=http://127.0.0.1:8088/api/v4/graphql
MIDNIGHT_PREVIEW_INDEXER_WS=ws://127.0.0.1:8088/api/v4/graphql/ws
MIDNIGHT_PREVIEW_NETWORK_ID=undeployed

MIDNIGHT_PREVIEW_RECOVERY_PHRASE="<local-dev funded test phrase>"
MIDNIGHT_PREVIEW_ACCOUNT=0
MIDNIGHT_PREVIEW_ZSWAP_KEY_SCAN_LIMIT=1
MIDNIGHT_PREVIEW_ZSWAP_EVENT_LIMIT=50000
MIDNIGHT_PREVIEW_RECIPIENT_SHIELDED_ADDRESS=<receiver shielded address>
MIDNIGHT_PREVIEW_TRANSFER_AMOUNT=200000000
MIDNIGHT_PREVIEW_WALLET_SUBMIT_MODE=raw-rpc

MIDNIGHT_PREVIEW_ZSWAP_SEED_HEX=
```

`.env` is gitignored. Do not commit it. `MIDNIGHT_PREVIEW_ZSWAP_SEED_HEX` is
only enough for prove-only helper paths; the full split-send e2e requires the
recovery phrase so the helper can derive the matching Dust and unshielded keys.

## Run The Local Chain

The standalone Midnight local-dev network is vendored into this repo under
`deps/midnight-local-dev`. It stays as its own npm package.

Install its dependencies once:

```bash
make local-install
```

```bash
make local-nodes
```

The Makefile loads the root `.env` before starting local-dev. Put image
overrides there when needed:

```dotenv
MIDNIGHT_NODE_IMAGE=midnight-node:split-prove
MIDNIGHT_INDEXER_IMAGE=split-prove/indexer-standalone:local
MIDNIGHT_PROOF_SERVER_IMAGE=<split-proof-server-image>
```

The local chain uses `network_id=undeployed` and exposes `9944`, `8088`, and
`6300` on localhost.

### Rebuilt Indexer

The compose default for `MIDNIGHT_INDEXER_IMAGE` is
`split-prove/indexer-standalone:local`, which is the v4.0.1 indexer rebuilt
against `deps/midnight-ledger` so its Zswap verifier matches the rebuilt node.
Build it once with:

```bash
make rebuild-images
```

The indexer source lives at `deps/midnight-indexer` (submodule pointing at
`ADGLx/midnight-indexer`, branch `feature/split-prove-indexer-4.0.1`). Re-run
`make rebuild-images` after any change to that submodule or to
`deps/midnight-ledger`. To run against the stock 4.0.1 image instead (which
will crash on the split-send block), override:

```dotenv
MIDNIGHT_INDEXER_IMAGE=midnightntwrk/indexer-standalone:4.0.1
```

## Run The Preview PoC

Install dependencies once:

```bash
npm install
```

Run the e2e command:

```bash
make e2e
```

Expected output:

```text
split-sent preview output key_index=0 mt_index=<index> input_value=<raw NIGHT> transfer_value=<raw NIGHT> change_value=<raw NIGHT> token=<token-type> recipient=<shielded-address> status="proofBuilt" client_proof_ms=<ms> server_proof_ms=<ms> split_proof_total_ms=<ms> server_client_ratio=<ratio> proof_len=<bytes> tx_hash=<hash> tx_id=<id> tx_len=<hex chars> pre_submit_wasm_check=skipped inclusion=finalized block_hash=<hash>
```

The underlying preview driver can still be run directly. By default it starts a local proof server on a random port. To use an already running proof server:

```bash
cargo run --offline -p midnight-proof-server --bin preview-split-prove \
  --manifest-path deps/midnight-ledger/Cargo.toml \
  -- --proof-server-url http://127.0.0.1:6300
```

The command always assembles and submits the split-send transaction. It sends
`MIDNIGHT_PREVIEW_TRANSFER_AMOUNT` raw NIGHT units to
`MIDNIGHT_PREVIEW_RECIPIENT_SHIELDED_ADDRESS`, defaulting to 500 NIGHT, and
returns any remainder as shielded change to the spender wallet. Submission
defaults to `MIDNIGHT_PREVIEW_WALLET_SUBMIT_MODE=raw-rpc`, which uses the wallet
SDK to sync Dust, add fee-balancing `DustActions`, finalize the transaction, and
then submit the finalized transaction directly through node RPC. The selected
wallet must have enough spendable Dust. Use
`MIDNIGHT_PREVIEW_WALLET_SUBMIT_MODE=wallet` to submit through the wallet SDK
watcher instead.
The helper uses local wallet SDK packages when installed, or
`../one-am-wallet/node_modules`; override with `MIDNIGHT_PREVIEW_WALLET_NODE_MODULES`.
First-time Dust proving may need to download and verify Dust proving assets; the
helper retries `finalizeRecipe` twice by default. Tune with
`MIDNIGHT_PREVIEW_DUST_PROVE_ATTEMPTS` and
`MIDNIGHT_PREVIEW_DUST_PROVE_RETRY_DELAY_MS`.

When testing against the local Docker stack, the node and indexer images need
matching ledger/zswap code. The rebuilt indexer image
(`split-prove/indexer-standalone:local`, built from
`deps/midnight-indexer` against `deps/midnight-ledger`) is the compose default
and replays split-send blocks successfully. The stock
`midnightntwrk/indexer-standalone:4.0.1` image links its own packaged ledger
crates and will exit with
`malformed transaction: Invalid proof -- while verifying Zswap proof` while
replaying the block containing a split-send tx.

To verify transaction correctness without relying on the indexer's post-submit
replay, the e2e path uses these checks:

- Rust-side `Input<Proof>::well_formed` and `Output<Proof>::well_formed` are
  called before the tx is assembled, using the locally-built zswap verifier
  keys, so the Zswap proof is verified against the same code the rebuilt node
  runs (see `preview_client.rs`).
- The wallet helper uses `author_submitAndWatchExtrinsic` and waits for the
  node to report `finalized` by default before resolving. Tune with
  `MIDNIGHT_PREVIEW_RAW_RPC_WAIT_FOR=submitted|inBlock|finalized`. The node's
  inclusion check uses the same locally-built ledger code as the proof-server.
- An optional JS-side `ledger.wellFormed` check is available via
  `MIDNIGHT_PREVIEW_VALIDATE_LEDGER_WASM=1`. Off by default because the
  registry `@midnight-ntwrk/ledger-v8` wasm package links the packaged zswap
  verifier and will reject locally-modified proofs even when the local node
  accepts them — same root cause as the stock indexer crash.

The live e2e test asserts the local node reports inclusion with a block hash,
so it succeeds even when the stock indexer container crashes during replay.

## Endpoint

```text
POST /v2/prove-split-spend
```

Request shape:

```json
{
  "coinBindingTag": "<32-byte field hex>",
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
  "attestedCommitmentSk": "<32-byte field hex>",
  "attestationProof": "<serialized wallet attestation proof hex>",
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

Run the live full split-send e2e test. This is opt-in because it uses wallet
secrets from `.env`, the configured indexer/node, and a local proof server:

```bash
MIDNIGHT_RUN_PREVIEW_E2E=1 \
MIDNIGHT_PREVIEW_RECIPIENT_SHIELDED_ADDRESS=<receiver> \
cargo test --offline -p midnight-proof-server preview_wallet_proves_real_unspent_split_spend \
  --manifest-path deps/midnight-ledger/Cargo.toml \
  -- --nocapture
```
