# Split-Prove Prototype

Proof of concept for Midnight zswap **split proving**: the wallet keeps the raw zswap secret key, while the proof server builds the split-spend proof using only public handoff values and coin metadata. The server never sees the raw key. The client-derivation proof is intentionally limited to the relations that need `sk`; the server circuit owns coin commitment, Merkle membership, nullifier insertion, and value commitment proving.

The PoC can prove a real unspent shielded output, assemble a sealed split-send transaction, and submit it to the bundled local Midnight chain. It does **not** run against the public preview chain — preview's node and indexer don't have the split-prove ledger changes and reject split-send blocks (see the [end-to-end flow](#end-to-end-flow) for why both sides need the rebuilt stack).

## Repo Layout

- [src/](src/) — root Rust prototype (toy demo of the client/server boundary)
- [circuits/](circuits/) — Compact sources and compiled client-derivation / wallet-attestation artifacts
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
2. **Derive (client)** — `preview-split-prove` reads the unspent output, generates/reuses the wallet-attestation proof, derives `pk` / `nullifier` / `coinBindingTag` locally, and proves the **client-derivation circuit**. The attestation binds `pk` to `sk`; the per-spend proof binds the same `sk` to the canonical nullifier, coin-binding tag, and `C_sk`, so these proofs must be produced by the wallet.
3. **Handoff** — POST to `/v2/prove-split-spend` ([deps/midnight-ledger/proof-server/src/endpoints.rs](deps/midnight-ledger/proof-server/src/endpoints.rs)).
4. **Split proof (server)** — server pre-verifies the wallet-attestation and client-derivation proofs, calls `Input::new_split` ([deps/midnight-ledger/zswap/src/construct.rs](deps/midnight-ledger/zswap/src/construct.rs)) and proves `midnight/zswap/spend-split` using artifacts in `deps/midnight-ledger/zswap/static/`.
5. **Assemble (client)** — verifies the returned `Input<Proof>` locally, proves a recipient shielded output, and Dust-balances + finalizes via the wallet SDK bridge ([tools/preview_balance_submit_split_tx.mjs](tools/preview_balance_submit_split_tx.mjs)).
6. **Submit + replay** — finalized tx sent to the rebuilt node via `author_submitAndWatchExtrinsic`; the node verifies the wallet-attestation proof, client-derivation proof, and split-spend proof, then the rebuilt indexer replays the block and indexes the new output.

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
| Wallet role | Builds one spend proof preimage with `sk`-bearing sender evidence | Proves wallet attestation once, then proves `sk -> nullifier/coinBindingTag/C_sk` per spend |
| Server role | Proves `midnight/zswap/spend` from the full private witness | Proves `midnight/zswap/spend-split` from public handoff values and Merkle data |
| Secret-key exposure | Remote proof server can receive the spend witness | Raw `sk` never crosses the handoff boundary |
| Node verification | Verifies one stock Zswap spend proof | Verifies wallet-attestation, client-derivation, and split-spend proofs |
| Compatibility cost | Works with stock node/indexer | Requires rebuilt ledger, node, and indexer with split proof admission |

The `coinBindingTag` links both proofs to the same coin and public key. The canonical Zswap nullifier is still used, so a split spend and a stock spend of the same coin collide in the same ledger nullifier set.

### Proof Cost Note

The server owns the split-spend proof. The wallet-side client-derivation circuit only proves the `sk`-dependent relations and uses the canonical Zswap nullifier so split and non-split spends collide in the same ledger nullifier set.

The client circuit does not recompute `pk = H_persistent(sk)` per spend. Instead, a one-time *wallet attestation* circuit (`circuits/wallet_attestation.compact`) proves `pk = H(sk) ∧ C_sk = transientHash("midnight:sk-commit[v1]", sk, r)` once at wallet setup. Each per-spend client proof opens `C_sk` (Poseidon) instead of paying SHA-256(sk) again. The node admission verifier cross-checks the attestation's `(pk, C_sk)` public outputs against the per-spend proof's `(pk, C_sk)` to bind the chain. See [bench/SPIKE_RESULTS.md](bench/SPIKE_RESULTS.md) for the prover-key spike that picked Poseidon over Pedersen for `C_sk`.

| Circuit | Location | prover key | Notes |
|---|---|---:|---:|
| **`sk_prove`** | `circuits/static/client-derivation/` | **2,823,704 B (2.82 MB)** | per-spend wallet proof |
| `wallet_attest` (one-time per wallet) | `circuits/static/wallet-attestation/` | 2,817,222 B (2.82 MB) | setup proof |
| `spend-split` (server, unchanged) | `deps/midnight-ledger/zswap/static/` | 5.74 MB | — |

The per-spend wallet circuit proves (a) `nullifier = H_persistent("midnight:zswap-cn[v1]", coin, true, sk)` — byte-identical to stock so cross-path double-spend stays blocked, (b) `coinBindingTag = transientHash(domain, coin, pk)` — unchanged, and (c) `commitmentSk = transientHash("midnight:sk-commit[v1]", sk, r)` — the Poseidon open of the registered commitment. `pk` is taken as a private witness and disclosed; soundness comes from the attestation chain, not from re-deriving `pk` in-circuit. The server `spend-split` circuit is unchanged.

### Current Proof Comparison

The live e2e report separates proof-only work from baseline wallet transaction work.

| Build | client proof | server proof | server / client | client prover key |
|---|---:|---:|---:|---:|
| Earlier internal prototype (no wallet attestation) | 1899 ms | 1948 ms | **1.03×** | 5.20 MB |
| **Current design** | **836 ms** | **1720 ms** | **2.06×** | **2.82 MB** |
| delta | **−56.0%** | −11.7% | shift toward server | **−45.7%** |

The 2.06× server / client ratio is the asymmetric split the project was after — the wallet now does roughly a third of the proving work instead of half. The server-side proof is still the same per-spend `spend-split` circuit; the `commitment_sk` public-transcript cell belongs to the client-derivation proof and is cross-checked against the wallet-attestation proof during admission.

The wallet additionally pays a one-time `wallet-attestation` proof on first use per `sk` (about 0.8-0.9s in the current local run). In the e2e the run-isolated demo re-runs this each invocation so it shows up in the `[2/6] derive` stage (1704 ms total in the latest run, including 836 ms of per-spend client derivation proving); a real wallet generates the attestation once at registration and reuses it for every spend, so steady-state client cost is the 836 ms number.

The full demo still includes non-proof workflow costs such as scan, output proof, Dust balancing, transaction assembly, raw-RPC submission, and node inclusion. The latest run reported `inclusion_status=inBlock`, `block_hash=0xd92cea3b0fccce8e4cc09c6cd471f6865daaa4c072b0d920c1417fc8d44a5d31`, and no `sk` field crossed the handoff boundary.

See [POC_RUNBOOK.md](POC_RUNBOOK.md) for env vars, endpoint shapes, and tuning knobs.

## Split Proof Admission

The `spend-split` circuit verifies Merkle membership and structural consistency of `pk` / `nullifier` / `commitmentHash` / `coinBindingTag`, but — by design, since the whole point is to remove `sk` from the server — it does **not** check `nullifier = H_persistent(coin, sk)` or `pk = derive(sk)`. The wallet-attestation proof and `clientDerivationProof` attest to that link.

The split bundle carries *three* proofs and a fourth shared public input in a typed proof envelope; older internal bundle shapes fail closed against the current verifier.

The final ledger-verified split input carries:

- `attestationProof` (one-time per wallet), proving `pk = H_persistent("midnight:zswap-pk[v1]", sk) ∧ commitmentSk = transientHash("midnight:sk-commit[v1]", sk, r)`.
- `clientDerivationProof` (per spend), proving the wallet knows `(sk, r)` opening `commitmentSk`, that the canonical nullifier was derived from *that* `sk`, and that the coin-binding tag was derived from that `pk`.
- `spend-split` (server, unchanged), proving the committed coin is in the Merkle tree, the submitted `commitmentHash` equals `H(coin, pk)`, the declared nullifier is inserted, the same `coinBindingTag` is disclosed, and the value commitment/spend rules are valid.
- Shared public inputs `(pk, coin_commitment, coin_binding_tag, commitment_sk)` — the node admission verifier cross-checks that the attestation's `(pk, commitmentSk)` byte-equals the per-spend's `(pk, commitmentSk)`. Poseidon binding + SHA-256 collision-resistance + a 256→2-limb injective encoding of `sk` then force the per-spend `sk` to be the same 32-byte string the attestation bound to the canonical `pk`.

The zswap split proving path encodes all four (the three proofs + four `Fr`/byte arrays of shared public inputs) into a typed envelope stored in the opaque zswap proof bytes. That keeps the existing zswap input/offer wire tags compatible with the stock local-dev wallet SDK, so ordinary shielded funding transfers still decode on the patched node.

The node verifies all three proofs at submit time and reconstructs the shared public inputs from the encoded split proof envelope. The proof server's `/v2/prove-split-spend` endpoint also pre-verifies both `attestationProof` and `clientDerivationProof` for fast failure, but those pre-verifies are no longer in the trusted computing base for split admission.

This only holds when the node and indexer are rebuilt against this patched ledger. Stock preview nodes still reject split-send blocks because they do not have these ledger changes.

### Live e2e

The full stack — circuits, SDK (`src/attestation.rs`, `src/client.rs`, `src/server.rs`), ledger envelope (`deps/midnight-ledger/zswap/src/{structure,verify,construct}.rs`), proof-server endpoint (`deps/midnight-ledger/proof-server/src/endpoints.rs`), and the preview driver (`deps/midnight-ledger/proof-server/src/preview_client.rs`) — was run end-to-end via `make e2e` against the rebuilt node/indexer. The transaction reached `inclusion_status: inBlock` and the per-stage report matches the per-circuit prover-key spike measurements:

```
--- Proof-only comparison (split-prove work) ---
  client proof:  clientDerivationProof (local)           836 ms
  server proof:  spend-split proof (remote)             1720 ms
  split-prove proving total                             2556 ms
  ratio (server proof / client proof)                   2.06x

--- SERVER (proof-server, remote) ---
  [3/6] handoff    POST /v2/prove-split-spend           1770 ms total
         ├─ network (round-trip overhead)               3 ms
         ├─ server: verify client derivation proof      4 ms
         └─ server: split-spend proving                 1720 ms
```

`well_formed` skipped in this run is the wallet-SDK pre-submit check, not the node admission verifier; the node verifier ran on inclusion and accepted the split bundle. Soundness coverage from the unit suite:

| What's tested | Where | Verdict |
|---|---|---|
| Honest handoff IR-checks against `sk_prove` | `src/client.rs::client_derivation_preimage_checks_against_compact_ir` | passes |
| Tampered nullifier rejected | `src/client.rs::client_derivation_preimage_rejects_tampered_nullifier` | rejected |
| Honest attestation IR-checks | `src/attestation.rs::derive_outputs_matches_compact_ir` | passes |
| Tampered attestation pk / C_sk rejected | `src/attestation.rs::tampered_*_rejected` | rejected |
| Scalar-reduction `sk + n·q` does not open same C_sk | `src/attestation.rs::scalar_reduction_*`, `src/client.rs::v3_scalar_reduction_*` | rejected (Poseidon binding, not range check) |
| Per-spend proof with wrong blinding rejected | `src/client.rs::v3_per_spend_circuit_rejects_blinding_mismatch_with_handoff` | rejected |
| Per-spend proof with consistent blinding accepted | `src/client.rs::v3_per_spend_commitment_matches_attestation_when_blinding_consistent` | passes |
| Tampered `coinBindingTag` in admission | `deps/midnight-ledger/proof-server/tests/integration_tests.rs::synthetic_client_derivation_proof_is_verified_before_split_proving` | rejected (`MalformedSplitProofBundle`) |
| Split public-input storable tag round-trips | `zswap::structure::tag_enforcement_test_SplitPublicInputs` | passes |
| Live preview e2e against rebuilt chain | `make e2e` (above) | `inBlock` |
