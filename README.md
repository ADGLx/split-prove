# Split-Prove Prototype

This repo is a proof of concept for Midnight zswap split proving:

```text
wallet/client keeps the zswap secret key
proof server does the heavy proof generation
proof server never receives the raw secret key
```

The current PoC can prove a real unspent shielded output from the Midnight preview chain. It does not submit a transaction yet.

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
    compute skCommitment
    compute nullifier
    compute commitmentHash
    package coin metadata + Merkle state/path
    POST /v2/prove-split-spend

proof server
  reconstruct QualifiedCoinInfo
  load Merkle tree/path
  call Input::new_split
  prove midnight/zswap/spend-split
  return proofHex

preview chain
  read-only through the indexer for this PoC
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
  skCommitment = H(sk, blinding)               nullifier
  coin metadata                                commitmentHash
  Merkle witness/state                         coin metadata
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
- Preview-chain PoC CLI that derives a wallet zswap key, finds an unspent owned shielded output, builds the split handoff, calls the proof server, and receives a real proof.

## Important Files

- `POC_RUNBOOK.md`: concise setup and run instructions.
- `src/client.rs`: root prototype handoff logic.
- `src/server.rs`: root prototype preimage builders.
- `tools/derive_midnight_zswap_seed.mjs`: temporary preview wallet seed helper.
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
proved preview output key_index=0 mt_index=1622 value=500 token=<token-type> status=proofBuilt proof_len=<bytes>
```

To call an already running proof server:

```bash
cargo run --offline -p midnight-proof-server --bin preview-split-prove \
  --manifest-path deps/midnight-ledger/Cargo.toml \
  -- --proof-server-url http://127.0.0.1:6300
```

## Endpoint Shape

```json
{
  "skCommitment": "<32-byte field hex>",
  "nullifier": "<32-byte hex>",
  "commitmentHash": "<32-byte hex>",
  "coinValue": 500,
  "coinType": "<32-byte token type hex>",
  "coinNonce": "<32-byte hex>",
  "mtIndex": 1622,
  "contractAddress": null,
  "zswapState": "<serialized zswap state hex>",
  "prove": true
}
```

Successful response:

```json
{
  "status": "proofBuilt",
  "keyLocation": "midnight/zswap/spend-split",
  "merklePathSource": "zswapState",
  "proofHex": "<serialized proof hex>",
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

Important caveat for the PoC: the server endpoint currently receives precomputed handoff values. Before treating this as production-safe, the split circuit and verifier path need careful review to ensure those values are constrained exactly as intended against the committed secret.

## Remaining Work

- Move the PoC preview client logic into the real wallet/client integration point.
- Decide whether the server API should accept full `zswapState` or a smaller Merkle witness.
- Assemble a full transaction from the returned proof.
- Submit that transaction to preview via `wss://rpc.preview.midnight.network`.
- Harden request validation and proof-server operational behavior.
