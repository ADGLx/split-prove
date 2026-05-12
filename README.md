# Split-Prove Prototype

This repo is a proof of concept for Midnight zswap split proving:

```text
wallet/client keeps the zswap secret key
proof server does the heavy proof generation
proof server never receives the raw secret key
```

The current PoC can prove a real unspent shielded output from the Midnight preview chain, assemble a sealed transaction, and optionally submit that transaction to the preview chain.

## Current Flow

```text
wallet/client
  existing wallet work:
    derive zswap keys
    sync/index chain data
    decrypt owned shielded outputs
    filter spent outputs
    maintain Merkle state/path

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
  indexer read for wallet state
  optional Dust balancing and submission through the wallet SDK
```

The only wallet work that is new for split proving is the handoff construction. Key derivation, chain sync, owned-output decryption, spent filtering, and Merkle tracking are normal wallet responsibilities.

## Architecture

```text
CLIENT / WALLET                              PROOF SERVER
───────────────────────────────              ───────────────────────────────
sk stays local                               never receives raw sk
owned shielded coin selected
Merkle state/path available

derive handoff:
  nullifier = H(sk, coin)                    receives:
  commitmentHash = commit(pk, coin)            skCommitment
  skCommitment = H(sk, blinding)               clientDerivationProof
  coin metadata                                nullifier
  Merkle witness/state                         pk
                                                commitmentHash
                                                coin metadata
                                                Merkle witness/state

POST /v2/prove-split-spend  ───────────────▶  Input::new_split(...)
                                                ↓
                                              prove spend-split

proofHex                    ◀───────────────  serialized proof
```

The root prototype also has a smaller local demo of the same boundary:

```text
src/client.rs  -> computes ClientHandoff
src/server.rs  -> builds split ProofPreimage
```

## What Is Implemented

- Root Rust prototype for client/server split proving.
- Modified Midnight proof server endpoint:

```text
POST /v2/prove-split-spend
```

- Split zswap constructors:
  - `Input::new_split`
  - `AuthorizedClaim::new_split`

- Local split circuit/proving artifacts under `deps/midnight-ledger/zswap/static`.
- Preview-chain PoC CLI that derives wallet keys, finds an unspent owned shielded output, builds the split handoff, calls the proof server, receives a real proof, assembles the split transaction, and can hand it to the wallet SDK for Dust balancing and submission.
- Runtime hardening checks that reject split spends when the supplied commitment does not match the Merkle root/path, and require real `zswapState`/`zswapStateFile` for proof-building requests instead of the simulated single-leaf fallback.
- Client derivation circuit compiled with `compact compile +0.31.0 --no-communications-commitment`, with proving artifacts under `circuits/static/client-derivation`.
- Proof-server verification of `clientDerivationProof` before building a real split spend proof.

## Important Files

- `POC_RUNBOOK.md`: concise setup and run instructions.
- `src/client.rs`: root prototype handoff logic.
- `src/server.rs`: root prototype preimage builders.
- `tools/derive_midnight_zswap_seed.mjs`: temporary preview wallet seed helper.
- `tools/preview_balance_submit_split_tx.mjs`: wallet SDK bridge for Dust balancing and preview submission.
- `.env.example`: preview environment template.
- `deps/midnight-ledger/proof-server/src/endpoints.rs`: real proof-server endpoint.
- `deps/midnight-ledger/proof-server/src/preview_client.rs`: preview CLI helper split into wallet scan, handoff build, and proof-server POST.
- `deps/midnight-ledger/zswap/src/construct.rs`: split constructors.
- `deps/midnight-ledger/zswap/src/prove.rs`: split proving artifact resolution.

## Setup

Install Node dependencies for the temporary wallet seed helper:

```bash
npm install
```

Create local env:

```bash
cp .env.example .env
```

Set either:

```dotenv
MIDNIGHT_PREVIEW_RECOVERY_PHRASE="word1 word2 ... word24"
```

or:

```dotenv
MIDNIGHT_PREVIEW_ZSWAP_SEED_HEX=<32-byte-hex-seed>
```

`.env` is gitignored.

## Local Chain

The standalone Midnight local-dev network is vendored as a self-contained npm
package under `deps/midnight-local-dev`. Install and run it from that directory:

```bash
cd deps/midnight-local-dev
npm install
MIDNIGHT_NODE_IMAGE=<rebuilt-node-image> npm start
```

When testing with a rebuilt split-proof proof-server image as well, pass both
image overrides:

```bash
MIDNIGHT_NODE_IMAGE=<rebuilt-node-image> \
MIDNIGHT_PROOF_SERVER_IMAGE=<split-proof-server-image> \
npm start
```

The local network exposes the node, indexer, and proof server at the usual
undeployed endpoints: `127.0.0.1:9944`, `127.0.0.1:8088`, and
`127.0.0.1:6300`.

## Run

Run the root toy demo:

```bash
cargo run --offline --bin split-prove-demo
```

Run the preview-chain e2e PoC:

```bash
cargo run --offline -p midnight-proof-server --bin preview-split-prove \
  --manifest-path deps/midnight-ledger/Cargo.toml
```

Expected output:

```text
proved preview output key_index=0 mt_index=1622 value=500 token=<token-type> status=proofBuilt proof_len=<bytes> tx_hash=<hash> tx_len=<hex chars> submitted=false
```

To call an already running proof server:

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

The full e2e submit path is: local client derivation proof, server split proof,
local split transaction assembly, wallet SDK Dust fee balancing, then raw RPC
submission of the finalized transaction. It requires `MIDNIGHT_PREVIEW_RECOVERY_PHRASE`; direct
`MIDNIGHT_PREVIEW_ZSWAP_SEED_HEX` is still supported for prove-only runs.

`MIDNIGHT_PREVIEW_SUBMIT_MODE=raw-rpc` is the default when
`MIDNIGHT_PREVIEW_SUBMIT_TX=true`; it still uses the wallet SDK to sync Dust,
add fee-balancing `DustActions`, and finalize the transaction before wrapping it
as a node extrinsic. Use `MIDNIGHT_PREVIEW_SUBMIT_MODE=wallet` to submit through
the wallet SDK watcher instead.
The helper uses local wallet SDK packages when installed, or
`../one-am-wallet/node_modules`; override with `MIDNIGHT_PREVIEW_WALLET_NODE_MODULES`.
First-time Dust proving may need to download and verify Dust proving assets; the
helper retries `finalizeRecipe` twice by default. Tune with
`MIDNIGHT_PREVIEW_DUST_PROVE_ATTEMPTS` and
`MIDNIGHT_PREVIEW_DUST_PROVE_RETRY_DELAY_MS`.

This spends the selected preview-chain shielded output and creates a replacement
output back to the same wallet key.

Run the synthetic HTTP e2e test:

```bash
cargo test --offline -p midnight-proof-server synthetic_client_derivation_proof_is_verified_before_split_proving \
  --manifest-path deps/midnight-ledger/Cargo.toml \
  -- --nocapture
```

Run the live preview-wallet e2e test:

```bash
MIDNIGHT_RUN_PREVIEW_E2E=1 \
cargo test --offline -p midnight-proof-server preview_wallet_proves_real_unspent_split_spend \
  --manifest-path deps/midnight-ledger/Cargo.toml \
  -- --nocapture
```

Run the live submit-to-preview-chain e2e:

```bash
MIDNIGHT_RUN_PREVIEW_E2E=1 MIDNIGHT_PREVIEW_SUBMIT_TX=true \
cargo test --offline -p midnight-proof-server preview_wallet_proves_real_unspent_split_spend \
  --manifest-path deps/midnight-ledger/Cargo.toml \
  -- --nocapture
```

## Endpoint Shape

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

Successful response:

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

## Security Properties

| Property | Principle |
|---|---|
| Secret key stays client-side | The raw zswap coin secret key is used by the wallet/client and is not sent to the proof server. |
| Server receives a handoff, not custody | The server receives derived proof inputs such as `skCommitment`, `nullifier`, `commitmentHash`, coin metadata, and Merkle data. |
| Split proof uses dedicated circuits/artifacts | The proof is built for `midnight/zswap/spend-split`, not the original raw-secret-key spend path. |
| Proof server does heavy work only | The server builds the split proof preimage, resolves proving data, and generates the proof. |
| Wallet remains responsible for wallet state | Key derivation, output decryption, spent filtering, and Merkle tracking remain wallet/client responsibilities. |

Important caveat for the PoC: the server now verifies a client-side derivation proof before proof-building requests, and validates that the supplied commitment sits at the claimed Merkle position. The remaining production question is how to bind or aggregate that client proof into the final ledger-verified artifact instead of treating it as a proof-server admission check.

## Remaining Work

- Move the PoC preview client logic into the real wallet/client integration point.
- Bind the server spend proof to the verified client-proof public outputs, or aggregate/recursively verify the client proof.
- Decide whether the server API should accept full `zswapState` or a smaller Merkle witness.
- Move the wallet SDK Dust balancing bridge into the real wallet/client integration point.
- Harden request validation and proof-server operational behavior.
