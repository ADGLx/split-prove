# Split-Prove Prototype

Proof of concept for Midnight zswap **split proving**: the wallet keeps the raw zswap secret key, while the proof server does the heavy proof work using only derived commitment values. The server never sees the raw key.

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

### Where `midnight-ledger` is consumed (the confusing part)

The ledger submodule is used by **both** the proof side and the verification side, which must stay in lockstep:

- **E2E tests / proof server** — Rust integration tests live inside the ledger submodule itself: [deps/midnight-ledger/proof-server/tests/integration_tests.rs](deps/midnight-ledger/proof-server/tests/integration_tests.rs) has the synthetic no-chain split test and the opt-in live full-tx test against the local-dev chain. Driver binary: `deps/midnight-ledger/proof-server/src/bin/preview_split_prove.rs` (name is historical — it targets local-dev).
- **Indexer Docker build** — [Dockerfile.indexer:18](Dockerfile.indexer#L18) copies `deps/midnight-ledger` into the build context; the indexer's `[patch.crates-io]` redirects ledger crates to this local checkout. Without it, the stock indexer crashes on a split-send block with `Invalid proof — while verifying Zswap proof`.
- **Node Docker build** — `deps/midnight-node` on its split-prove branch already pins the matching ledger; built once and passed to local-dev via `MIDNIGHT_NODE_IMAGE`.
- **Circuit compilation** — [Dockerfile.compactc](Dockerfile.compactc) and [build-circuits.sh:17-18](build-circuits.sh#L17) compile `zswap-split.compact` / `dust-split.compact` out of `deps/midnight-ledger/{zswap,ledger}/` into the zkir artifacts.

If you rebuild only one side, blocks get rejected. All three pinned branches must move together.

## End-to-End Flow

1. **Fund** — `midnight-local-dev` option 6 (`deps/midnight-local-dev/src/funding.ts`) sends a shielded NIGHT output to the split-prove wallet and polls the indexer until it appears.
2. **Derive (client)** — `preview-split-prove` reads the unspent output, derives `skCommitment` / `nullifier` / `commitmentHash` locally, and proves the **client-derivation circuit**.
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

1. `npm install` at the repo root, then `cp .env.example .env` (shipped values are throwaway and target the bundled `undeployed` chain).
2. Build the rebuilt node + indexer images (only when the ledger submodule moves):
   ```bash
   docker build -f deps/midnight-node/Dockerfile.split-prove -t midnight-node:split-prove-0.22.3 .
   docker build -f Dockerfile.indexer -t split-prove/indexer-standalone:local .
   ```
3. In another terminal, start the local chain and fund the wallet with **option 6**:
   ```bash
   cd deps/midnight-local-dev && npm install
   MIDNIGHT_NODE_IMAGE=midnight-node:split-prove-0.22.3 \
   MIDNIGHT_INDEXER_IMAGE=split-prove/indexer-standalone:local npm start
   ```

### What you'll see

Per-stage `tracing` events during the run, then a final banner (real numbers):

```
--- CLIENT (wallet, local) ---
  [1/6] scan       events replayed, coin selected             246 ms
  [2/6] derive     client-derivation proof built             1489 ms
         └─ of which local proving                           1477 ms
  [5/6] assemble+  recipient output, dust balance, submit  10850 ms

--- SERVER (proof-server, remote) ---
  [3/6] handoff    POST /v2/prove-split-spend                 431 ms total
         ├─ network (round-trip overhead)                     33 ms
         ├─ server: verify client derivation proof            13 ms
         └─ server: split-spend proving                      385 ms

--- NODE + INDEXER ---
  [6/6] submit     inclusion_status=inBlock  block_hash=0x6397…52f2

--- Local vs remote proving ---
  client local proving (derive):                             1477 ms
  server remote proving (split-spend):                        385 ms
  total wall-clock (stages 1–6):                            13016 ms

--- Role boundary check ---
  sk crossed the wire?  NO
  what crossed (ClientHandoff): skCommitment, nullifier, pk,
    commitmentHash, coinValue, coinType, coinNonce, mtIndex,
    contractAddress, clientDerivationProof
```

The three role headers map onto the six stages in [End-to-End Flow](#end-to-end-flow), making the split-prove boundary visible. The **role boundary check** statically enumerates the `ClientHandoff` fields — the point of split-prove is that no `sk` field appears here.

### Troubleshooting

- Funding fails with `Custom error: 1` + node logs show `Unrecognised discriminant`: node image out of sync with the ledger submodule. Rebuild images, `npm run clean` in `deps/midnight-local-dev`, restart.
- `make e2e` fails at submit with `Custom error: 185` + `PedersenCheckFailure`: binding-randomness / sealed-tx assembly in the preview client, not proof verification. No docker rebuild — just rerun.

See [POC_RUNBOOK.md](POC_RUNBOOK.md) for env vars, endpoint shapes, and tuning knobs.

## Split Proof Admission

The `spend-split` circuit verifies Merkle membership and structural consistency of `pk` / `nullifier` / `commitmentHash`, but — by design, since the whole point is to remove `sk` from the server — it does **not** check `nullifier = H(sk, coin)` or `pk = derive(sk)`. The `clientDerivationProof` is what attests to that link.

The final ledger-verified split input now carries both proofs:

- `clientDerivationProof`, proving the wallet knows `sk` and that `skCommitment`, `pk`, `commitmentHash`, and `nullifier` were derived from that key and coin.
- `spend-split`, proving the committed coin is in the Merkle tree, the declared nullifier is inserted, and the value commitment/spend rules are valid.

The zswap split proving path encodes `spend-split`, `clientDerivationProof`, and the shared public inputs into a typed envelope stored in the opaque zswap proof bytes. That keeps `zswap-input[v2]` / `zswap-offer[v5]` wire-compatible with the stock local-dev wallet SDK, so ordinary shielded funding transfers still decode on the patched node.

The node verifies both proofs at submit time and reconstructs the shared public inputs from the encoded split proof envelope. The proof server still pre-verifies `clientDerivationProof` for fast failure, but it is no longer in the trusted computing base for split admission.

This only holds when the node and indexer are rebuilt against this patched ledger. Stock preview nodes still reject split-send blocks because they do not have these ledger changes.
