# Split-Prove Prototype

Proof of concept for Midnight zswap **split proving**: the wallet keeps the raw zswap secret key, while the proof server builds the split-spend proof using only public handoff values and coin metadata. The server never sees the raw key. The client-derivation proof is intentionally limited to the relations that need `sk`; the server circuit owns coin commitment, Merkle membership, nullifier insertion, and value commitment proving.

The PoC can prove a real unspent shielded output, assemble a sealed split-send transaction, and submit it to the bundled local Midnight chain. It does **not** run against the public preview chain — preview's node and indexer don't have the split-prove ledger changes and reject split-send blocks (see the [end-to-end flow](#end-to-end-flow) for why both sides need the rebuilt stack).

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

### Where `midnight-ledger` is consumed

Used by **both** proving and verification — they must stay in lockstep:

- **E2E tests / proof server** — integration tests live in the submodule: [deps/midnight-ledger/proof-server/tests/integration_tests.rs](deps/midnight-ledger/proof-server/tests/integration_tests.rs) (synthetic + live full-tx).
- **Indexer Docker build** — [Dockerfile.indexer](Dockerfile.indexer) copies this submodule and `[patch.crates-io]`s the ledger crates to it; without it the stock indexer crashes on split-send blocks.
- **Node Docker build** — `deps/midnight-node`'s split-prove branch already pins the matching ledger; pass the built image via `MIDNIGHT_NODE_IMAGE`.
- **Circuit compilation** — regenerated with the local Compact CLI; see [POC_RUNBOOK.md](POC_RUNBOOK.md) for the exact commands.

Rebuild only one side and blocks get rejected.

## End-to-End Flow

1. **Fund** — `midnight-local-dev` option 6 (`deps/midnight-local-dev/src/funding.ts`) sends a shielded NIGHT output to the split-prove wallet and polls the indexer until it appears.
2. **Derive (client)** — `preview-split-prove` reads the unspent output, derives `pk` / `nullifier` / `coinBindingTag` locally, and proves the **client-derivation circuit**. This proof binds `sk` to `pk`, `nullifier`, and the coin binding tag, so it must be produced by the wallet.
3. **Handoff** — POST to `/v2/prove-split-spend` ([deps/midnight-ledger/proof-server/src/endpoints.rs](deps/midnight-ledger/proof-server/src/endpoints.rs)).
4. **Split proof (server)** — server pre-verifies the client-derivation proof, calls `Input::new_split` ([deps/midnight-ledger/zswap/src/construct.rs](deps/midnight-ledger/zswap/src/construct.rs)) and proves `midnight/zswap/spend-split` using artifacts in `deps/midnight-ledger/zswap/static/`.
5. **Assemble (client)** — verifies the returned `Input<Proof>` locally, proves a recipient shielded output, and Dust-balances + finalizes via the wallet SDK bridge ([tools/preview_balance_submit_split_tx.mjs](tools/preview_balance_submit_split_tx.mjs)).
6. **Submit + replay** — finalized tx sent to the rebuilt node via `author_submitAndWatchExtrinsic`; the node verifies both the client-derivation proof and the split-spend proof, then the rebuilt indexer replays the block and indexes the new output.

## Running the E2E

```bash
make e2e
```

That's the headline demo — runs the live split-send against the local chain with per-stage tracing and a final report.

### One-time setup

1. Install JS dependencies and create the local env file:
   ```bash
   npm install
   make local-install
   cp .env.example .env
   ```
2. Build the rebuilt node + indexer images (only when the ledger submodule moves):
   ```bash
   make rebuild-images
   ```
3. In another terminal, start the local chain and fund the wallet with **option 6**:
   ```bash
   make local-nodes
   ```
   The Makefile loads `.env` before running local-dev. Use `make local-env` to
   check the resolved image tags.

## Original vs Split Proving

In stock Zswap, one spend proof covers the whole spend relation. If proving is outsourced, the proof server receives a `midnight/zswap/spend` preimage whose private witness includes the sender evidence for the coin, so the server must be local or trusted.

Split-prove divides that work into two linked proofs:

| Area | Original Zswap Spend | Split-Prove PoC |
|---|---|---|
| Wallet role | Builds one spend proof preimage with `sk`-bearing sender evidence | Proves only `sk -> pk/nullifier/coinBindingTag` locally |
| Server role | Proves `midnight/zswap/spend` from the full private witness | Proves `midnight/zswap/spend-split` from public handoff values and Merkle data |
| Secret-key exposure | Remote proof server can receive the spend witness | Raw `sk` never crosses the handoff boundary |
| Node verification | Verifies one stock Zswap spend proof | Verifies the client-derivation proof and the split-spend proof |
| Compatibility cost | Works with stock node/indexer | Requires rebuilt ledger, node, and indexer with split proof admission |

The `coinBindingTag` links both proofs to the same coin and public key. The canonical Zswap nullifier is still used, so a split spend and a stock spend of the same coin collide in the same ledger nullifier set.

### Proof Cost Note

The server owns the split-spend proof. The wallet-side client-derivation circuit only proves the `sk`-dependent relations and uses the canonical Zswap nullifier so split and non-split spends collide in the same ledger nullifier set.

| Circuit | Location | prover key |
|---|---|---:|
| `sk_prove` | `circuits/static/client-derivation/` | 5.20 MB |
| `spend-split` | `deps/midnight-ledger/zswap/static/` | 5.74 MB |

The wallet proves `pk = H_persistent(sk)`, `nullifier = H_persistent(coin, sk)`, and `coinBindingTag = H_transient(domain, coin, pk)`. The server proves `coinCommitment = H_persistent(coin, pk)`, checks the Merkle leaf, discloses the same `coinBindingTag`, inserts the public nullifier, and proves the value commitment. `pk`, `nullifier`, and `coinCommitment` stay canonical so stock-wallet funding outputs and stock spend nullifiers remain compatible with split proving.

### Recent Live E2E Result

Run: `make e2e` against the bundled local chain (12 workers, debug build). Rerun this after rebuilding the proof-server/node/indexer images whenever the circuit artifacts change.

| Stage | Role | Wall-clock |
|---|---|---:|
| [1/6] scan | client | 300 ms |
| [2/6] derive (of which local proving) | client | 949 ms (938 ms) |
| [3/6] handoff (network + verify + server prove) | client → server | 1982 ms (15 + 19 + 1917) |
| [5/6] assemble + Dust balance + submit | client → node | 8835 ms |
| [6/6] independent on-chain verify | node | 59 ms |
| **Total wall-clock** | | **12068 ms** |

- Client local proving **938 ms** vs server split-spend **1917 ms** → server/client ratio **2.04×**.
- Split-prove proof-only total: **2855 ms**. Use that, not the full demo wall-clock, when comparing local wallet proof work to remote split proof work.
- The 8835 ms `assemble+` block is wallet-SDK Dust balancing and raw-RPC submit — baseline wallet transaction work, not split-prove proving overhead.
- `inclusion_status=inBlock`, `block_hash=0xcbc5af250f...8c2b1659`. No `sk` field appears in the `ClientHandoff` that crosses the wire.

See [POC_RUNBOOK.md](POC_RUNBOOK.md) for env vars, endpoint shapes, and tuning knobs.

## Split Proof Admission

The `spend-split` circuit verifies Merkle membership and structural consistency of `pk` / `nullifier` / `commitmentHash` / `coinBindingTag`, but — by design, since the whole point is to remove `sk` from the server — it does **not** check `nullifier = H_persistent(coin, sk)` or `pk = derive(sk)`. The `clientDerivationProof` is what attests to that link.

The final ledger-verified split input now carries both proofs:

- `clientDerivationProof`, proving the wallet knows `sk` and that `pk`, `nullifier`, and `coinBindingTag` were derived from that key and coin.
- `spend-split`, proving the committed coin is in the Merkle tree, the submitted `commitmentHash` equals `H(coin, pk)`, the declared nullifier is inserted, the same `coinBindingTag` is disclosed, and the value commitment/spend rules are valid.

The zswap split proving path encodes `spend-split`, `clientDerivationProof`, and the shared public inputs into a typed envelope stored in the opaque zswap proof bytes. That keeps `zswap-input[v2]` / `zswap-offer[v5]` wire-compatible with the stock local-dev wallet SDK, so ordinary shielded funding transfers still decode on the patched node.

The node verifies both proofs at submit time and reconstructs the shared public inputs from the encoded split proof envelope. The proof server still pre-verifies `clientDerivationProof` for fast failure, but it is no longer in the trusted computing base for split admission.

This only holds when the node and indexer are rebuilt against this patched ledger. Stock preview nodes still reject split-send blocks because they do not have these ledger changes.
