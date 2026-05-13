# Split-Prove Prototype

Proof of concept for Midnight zswap **split proving**: the wallet keeps the raw zswap secret key, while the proof server does the heavy proof work using only derived commitment values. The server never sees the raw key.

The PoC can prove a real unspent shielded output, assemble a sealed split-send transaction, and submit it to a local (or preview) Midnight chain.

## Repo Layout

- [src/](src/) — root Rust prototype (toy demo of the client/server boundary)
- [circuits/](circuits/) — compiled client-derivation zkir artifacts
- [tools/](tools/) — JS/TS helpers (wallet seed derivation, Dust balancing, raw-RPC submit)
- [deps/](deps/) — git submodules (see below)
- [Cargo.toml](Cargo.toml) — workspace; path-deps into `deps/midnight-ledger`
- [Dockerfile.indexer](Dockerfile.indexer), [Dockerfile.compactc](Dockerfile.compactc), [build-circuits.sh](build-circuits.sh)

## Submodules

| Submodule | Branch | Purpose |
|---|---|---|
| [deps/midnight-ledger](deps/midnight-ledger) | `feature/split-prove-ledger-8.0.2` | Modified ledger + zswap + proof-server source. **Single source of truth** for split-prove changes. |
| [deps/midnight-indexer](deps/midnight-indexer) | `feature/split-prove-indexer-4.0.1` | Stock v4.0.1 indexer, rebuilt against the patched ledger so it can replay split-send blocks. |
| [deps/midnight-node](deps/midnight-node) | `feature/split-prove-node-0.22.3` | Stock v0.22.3 node, rebuilt against the patched ledger so it accepts split-send blocks. |
| [deps/midnight-local-dev](deps/midnight-local-dev) | — | Docker-compose orchestrator for node + indexer + proof-server. Option 6 funds the e2e wallet. |

### Where `midnight-ledger` is consumed (the confusing part)

The ledger submodule is used by **both** the proof side and the verification side, which must stay in lockstep:

- **E2E tests / proof server** — Rust integration tests live inside the ledger submodule itself: [deps/midnight-ledger/proof-server/tests/integration_tests.rs:547](deps/midnight-ledger/proof-server/tests/integration_tests.rs#L547) (synthetic) and [:582](deps/midnight-ledger/proof-server/tests/integration_tests.rs#L582) (live preview). Driver binary: `deps/midnight-ledger/proof-server/src/bin/preview_split_prove.rs`.
- **Indexer Docker build** — [Dockerfile.indexer:18](Dockerfile.indexer#L18) copies `deps/midnight-ledger` into the build context; the indexer's `[patch.crates-io]` redirects ledger crates to this local checkout. Without it, the stock indexer crashes on a split-send block with `Invalid proof — while verifying Zswap proof`.
- **Node Docker build** — `deps/midnight-node` on its split-prove branch already pins the matching ledger; built once and passed to local-dev via `MIDNIGHT_NODE_IMAGE`.
- **Circuit compilation** — [Dockerfile.compactc](Dockerfile.compactc) and [build-circuits.sh:17-18](build-circuits.sh#L17) compile `zswap-split.compact` / `dust-split.compact` out of `deps/midnight-ledger/{zswap,ledger}/` into the zkir artifacts.

If you rebuild only one side, blocks get rejected. All three pinned branches must move together.

## End-to-End Flow

1. **Fund** — `midnight-local-dev` option 6 (`deps/midnight-local-dev/src/funding.ts`) sends a shielded NIGHT output to the split-prove wallet and polls the indexer until it appears.
2. **Derive (client)** — `preview-split-prove` reads the unspent output, derives `skCommitment` / `nullifier` / `commitmentHash` locally, and proves the **client-derivation circuit**.
3. **Handoff** — POST to `/v2/prove-split-spend` ([deps/midnight-ledger/proof-server/src/endpoints.rs](deps/midnight-ledger/proof-server/src/endpoints.rs)).
4. **Split proof (server)** — server verifies the client-derivation proof, calls `Input::new_split` ([deps/midnight-ledger/zswap/src/construct.rs](deps/midnight-ledger/zswap/src/construct.rs)) and proves `midnight/zswap/spend-split` using artifacts in `deps/midnight-ledger/zswap/static/`.
5. **Assemble (client)** — verifies the returned `Input<Proof>` locally, proves a recipient shielded output, and Dust-balances + finalizes via the wallet SDK bridge ([tools/preview_balance_submit_split_tx.mjs](tools/preview_balance_submit_split_tx.mjs)).
6. **Submit + replay** — finalized tx sent to the rebuilt node via `author_submitAndWatchExtrinsic`; the rebuilt indexer replays the block and indexes the new output.

## Running the E2E

### Prerequisites

```bash
npm install
cp .env.example .env   # set MIDNIGHT_PREVIEW_RECOVERY_PHRASE and MIDNIGHT_PREVIEW_RECIPIENT_SHIELDED_ADDRESS
```

Build the rebuilt indexer image once (compose default is `split-prove/indexer-standalone:local`):

```bash
docker build -f Dockerfile.indexer -t split-prove/indexer-standalone:local .
```

### Start the local chain

```bash
cd deps/midnight-local-dev
npm install
MIDNIGHT_NODE_IMAGE=<rebuilt-node-image> npm start
```

Exposes node `127.0.0.1:9944`, indexer `:8088`, proof server `:6300`. In the CLI, pick **option 6** to fund the split-prove wallet.

### Run the live e2e (full split-send tx against the local chain)

```bash
MIDNIGHT_RUN_PREVIEW_E2E=1 \
MIDNIGHT_PREVIEW_RECIPIENT_SHIELDED_ADDRESS=<receiver> \
cargo test --offline -p midnight-proof-server preview_wallet_proves_real_unspent_split_spend \
  --manifest-path deps/midnight-ledger/Cargo.toml \
  -- --nocapture
```

Or run the driver binary directly (handy when iterating):

```bash
cargo run --offline -p midnight-proof-server --bin preview-split-prove \
  --manifest-path deps/midnight-ledger/Cargo.toml \
  -- --proof-server-url http://127.0.0.1:6300
```

### Run the synthetic e2e (no chain, no wallet)

```bash
cargo test --offline -p midnight-proof-server synthetic_client_derivation_proof_is_verified_before_split_proving \
  --manifest-path deps/midnight-ledger/Cargo.toml \
  -- --nocapture
```

See [POC_RUNBOOK.md](POC_RUNBOOK.md) for environment variables, endpoint shapes, and tuning knobs.

## Known Shortcoming: Trusted Admission Gate

The `spend-split` circuit verifies Merkle membership and structural consistency of `pk` / `nullifier` / `commitmentHash`, but — by design, since the whole point is to remove `sk` from the server — it does **not** check `nullifier = H(sk, coin)` or `pk = derive(sk)`. The `clientDerivationProof` is what attests to that link.

In this PoC the proof server verifies `clientDerivationProof` off-chain before generating the split spend proof, but the final ledger-verified artifact does **not** recursively verify or aggregate the client proof. An attacker who bypasses the proof server and proves `spend-split` directly with arbitrary `pk` / `nullifier` against any public commitment can have the ledger accept it — effectively stealing the coin's value to a `pk` they control. The real owner's later spend would still succeed (their nullifier differs), but the value is already gone.

This means the proof server is currently in the trusted computing base. Production fixes (any of):

- Recursively verify `clientDerivationProof` inside `spend-split`.
- Aggregate the client and spend proofs into one ledger-submitted artifact.
- Bind the client proof's public outputs into `spend-split` public inputs so the ledger verifier checks both at submit time.

Until one of these lands, "split proving" here means "split proving gated by a trusted admission server".
