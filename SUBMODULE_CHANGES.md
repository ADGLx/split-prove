# Submodule Changes

Summary of split-prove-specific modifications to the vendored dependencies.

| Component | Current rev | Baseline | Status |
|---|---|---|---|
| [deps/midnight-ledger](deps/midnight-ledger) (submodule, branch `feature/split-prove-ledger-8.0.2`) | `3b279990` | `641d18e5` (tip of `feature/split-prove-no-sk`) | Summarised below |
| [deps/midnight-local-dev](deps/midnight-local-dev) (vendored — no `.git`, tracked inside the parent repo) | parent `HEAD` | `7e23340` (vendoring commit) | Summarised below |
| [deps/midnight-node](deps/midnight-node) (submodule, branch `feature/split-prove-node-0.22.3`) | `ce17c6a4` | `71fc6804` (3 commits before the first user commit `232f14d6`; upstream tip prior to your changes is `6f0ef437 bump node 0.22.3`) | Summarised below |
| [deps/midnight-indexer](deps/midnight-indexer) (submodule, branch `feature/split-prove-indexer-4.0.1`) | `3235a61` | `c90fb85` (v4.0.1 release tag) | Summarised below |

---

## midnight-ledger

`git diff 641d18e5...HEAD` (merge-base based): split-prove commits plus local working-tree changes sit on top of the `no-sk` baseline. The latest working-tree delta moves split admission from “trusted proof server gate” to “node verifies both proofs”.

### Latest uncommitted ledger delta: ledger-verified client proof

**Security model change**
- Final split zswap inputs now carry both the server-generated `spend-split` proof and the wallet-generated `clientDerivationProof` inside a typed envelope encoded in the opaque zswap `Proof(Vec<u8>)`.
- The rebuilt node verifies both proofs during normal zswap `well_formed()` checks and reconstructs the shared public statement from the split proof envelope.
- The proof server still pre-verifies `clientDerivationProof` for fast rejection, but it is no longer trusted for split admission. Bypassing the proof server no longer lets an attacker submit a valid `spend-split` proof with arbitrary `pk` / `nullifier` linkage.

**Zswap data model and verifier** ([deps/midnight-ledger/zswap/src/structure.rs](deps/midnight-ledger/zswap/src/structure.rs), [verify.rs](deps/midnight-ledger/zswap/src/verify.rs))
- `Input<P, D>` and `Offer<P, D>` keep their original serialized wire shape (`zswap-input[v2]`, `zswap-offer[v5]`) so stock wallet/local-dev shielded transfers remain compatible with the patched node.
- New `ZswapInputProof` / `SplitProofBundle` helpers encode/decode `spend-split`, `clientDerivationProof`, `public_key`, `coin_commitment`, and `coin_binding_tag` inside the proof bytes for split inputs.
- Split inputs verify `CLIENT_DERIVATION_VK` against `public_key`, `nullifier`, and `coin_binding_tag` before verifying `SPEND_SPLIT_VK`.
- Plain inputs still verify with the stock `SPEND_VK`; typed split envelopes take the split verifier path, and malformed split envelopes are rejected explicitly.

**Circuit/artifact changes** ([deps/midnight-ledger/zswap/zswap-split.compact](deps/midnight-ledger/zswap/zswap-split.compact), [deps/midnight-ledger/zswap/static](deps/midnight-ledger/zswap/static))
- `spendSplitUser` now computes/discloses `coinCommitment = H(coin, pk)`, asserts it equals the Merkle path leaf, and discloses the same `coinBindingTag` as the client proof.
- Regenerated `spend-split.{zkir,bzkir,prover,verifier}` and sha256 sidecars with the Compact compiler.
- Mirrored the regenerated `spend-split.zkir` into [deps/midnight-ledger/zkir-precompiles/zswap/spend-split.zkir](deps/midnight-ledger/zkir-precompiles/zswap/spend-split.zkir).
- Added [deps/midnight-ledger/zswap/static/client-derivation.verifier](deps/midnight-ledger/zswap/static/client-derivation.verifier), copied from the client-derivation circuit artifacts, so node/indexer binaries can verify the wallet proof without depending on proof-server paths.
- Artifacts are regenerated with the local Compact CLI, for example:
  ```bash
  compact compile --no-communications-commitment circuits/sk_proof.compact /tmp/sk-prove-compile
  compact compile --no-communications-commitment deps/midnight-ledger/zswap/zswap-split.compact /tmp/zswap-split-compile
  ```

**Construct/prove/proof-server flow**
- `Input::new_split()` returns a split proving context that carries the normal spend preimage, split public inputs, and required client proof without changing the zswap input struct layout.
- The split proving context produces `provedInputHex` with an encoded `ZswapInputProof::Split` envelope; the HTTP endpoint no longer hand-builds the envelope.
- `Input<ProofPreimage>::delta()` and `binding_randomness()` understand the split witness trailer (`nullifier`) appended after `rc`; the preview submit path uses these shared helpers so sealed transactions use the same Pedersen binding randomness the node recomputes.
- The synthetic split-spend proof-server test now deserializes `provedInputHex`, calls `Input<Proof>::well_formed(0)`, and asserts a raw split proof without the client proof, a tampered nullifier, and a malformed split envelope are rejected.

**Build impact**
- Rebuild the node, indexer, and proof-server images after verifier/artifact changes so they have the new ledger code and circuit blobs. The zswap input/offer wire tags remain compatible with local-dev's stock wallet SDK.
- Pure proof-server preview-client or test-side transaction assembly fixes, such as binding-randomness extraction, do not require rebuilding an already-running node or indexer; the node can already reject malformed sealed transactions correctly.

### Commits (newest first)

| SHA | Subject |
|---|---|
| `3b279990` | proof-server: add partial preview split-send support (transfer amount + shielded change output) |
| `d385990e` | Fix preview split-send recipient nonce (avoid `CommitmentAlreadyPresent`) |
| `c07e366d` | complete live split-send e2e with tx assembly and node inclusion check (`author_submitAndWatchExtrinsic`) |
| `99d8fdc0` | Fix split-prove Ledger 8 local dependency graph |
| `edb21000` | keep split zswap compatible with ledger 8.0 |
| `a72735ae` | wire `SPEND_SPLIT_VK` and `SIGN_SPLIT_VK` into `well_formed` dispatch (makes node accept split-prove tx) |
| `547987d5` | Implement split-prove server/client PoC (endpoint, preview client, e2e tests) |
| `482d905c` | add split-prove constructors for delegated proving without secret key |

### What changed, by area

**New zswap split constructors and circuits** ([deps/midnight-ledger/zswap/src/construct.rs](deps/midnight-ledger/zswap/src/construct.rs), +149)
- `AuthorizedClaim::new_split()` and `Input::new_split()` — accept pre-computed `(nullifier, pk, commitment, coinBindingTag)` instead of the raw secret key. The latest working-tree version keeps zswap input serialization stable; `Input::new_split()` returns a split context that carries `pk`, `coinBindingTag`, and `clientDerivationProof` until proving encodes them into the proof envelope.
- New circuit source: [`zswap/zswap-split.compact`](deps/midnight-ledger/zswap/zswap-split.compact) (+41).
- Compiled artifacts under [`zswap/static/`](deps/midnight-ledger/zswap/static/): `spend-split.{zkir,bzkir,prover,verifier}` and `sign-split.{zkir,bzkir,prover,verifier}` plus sha256 sidecars.
- Mirrored zkir bytecode under [`zkir-precompiles/zswap/`](deps/midnight-ledger/zkir-precompiles/zswap/).

**zkir support for committed inputs** ([deps/midnight-ledger/zkir/src/ir.rs](deps/midnight-ledger/zkir/src/ir.rs), [ir_vm.rs](deps/midnight-ledger/zkir/src/ir_vm.rs))
- `IrSource::prove_split()` with `committed_input_count`.
- `Preprocessed.committed_input_count` and `format_committed_instances()` override needed by the split flow.

**Ledger verification wiring** ([deps/midnight-ledger/zswap/src/verify.rs](deps/midnight-ledger/zswap/src/verify.rs), +71)
- `lazy_static` refs for `SPEND_SPLIT_VK` / `SIGN_SPLIT_VK` from the new `.verifier` blobs.
- Original committed behavior: `well_formed()` fallback path first tried `SPEND_VK`/`SIGN_VK`, then the split VKs.
- Latest working-tree behavior: split inputs are explicit via a typed proof envelope; the node verifies `CLIENT_DERIVATION_VK` and `SPEND_SPLIT_VK` against matching public inputs.

**Proof server** ([deps/midnight-ledger/proof-server/](deps/midnight-ledger/proof-server/))
- New endpoint `POST /v2/prove-split-spend` in [endpoints.rs](deps/midnight-ledger/proof-server/src/endpoints.rs) (+403). Reconstructs `QualifiedCoinInfo`, loads the Merkle tree, rejects handoffs whose commitment doesn't reproduce the root, pre-verifies `clientDerivationProof`, calls `Input::new_split`, proves `midnight/zswap/spend-split`, returns `proofHex` + `provedInputHex`. Latest working-tree behavior delegates envelope construction to the zswap split proving context so any caller can produce a node-acceptable split input.
- New file [preview_client.rs](deps/midnight-ledger/proof-server/src/preview_client.rs): preview wallet scanner, handoff builder, full tx assembly (recipient output + binding randomness + StandardTransaction + Dust handoff to the JS wallet bridge), and `author_submitAndWatchExtrinsic` submission.
- New driver binary [bin/preview_split_prove.rs](deps/midnight-ledger/proof-server/src/bin/preview_split_prove.rs) (+102).
- Integration tests [tests/integration_tests.rs](deps/midnight-ledger/proof-server/tests/integration_tests.rs): synthetic e2e (always runs) + opt-in live preview e2e (`MIDNIGHT_RUN_PREVIEW_E2E=1`).

**Misc**
- [proof-server/Cargo.toml](deps/midnight-ledger/proof-server/Cargo.toml), [zswap/Cargo.toml](deps/midnight-ledger/zswap/Cargo.toml): dep wiring.
- [zswap/src/error.rs](deps/midnight-ledger/zswap/src/error.rs), [zswap/src/prove.rs](deps/midnight-ledger/zswap/src/prove.rs), [zswap/src/structure.rs](deps/midnight-ledger/zswap/src/structure.rs): error variants, split-circuit key resolution, struct exposure.
- `Cargo.lock` (+163): rolls in new proof-server deps.

---

## midnight-local-dev

Vendored under `deps/midnight-local-dev` by commit `7e23340 Vendor midnight local dev under deps` (its working tree was copied into the parent repo, no separate `.git`).

`git diff 7e23340..HEAD -- deps/midnight-local-dev/`: **5 files, +211 / −19**.

### Commits

| SHA | Subject |
|---|---|
| `7c1e04a` | document and configure split-prove amounts (`MIDNIGHT_SPLIT_PROVE_SHIELDED_AMOUNT`, transfer amount in env) |
| `f9d76b4` | Updated READMEs |
| `1a41d28` | streamlined option 6 for the e2e on local node setup (`fundSplitProveE2ESetup`) |
| `ea52cbd` | rebuild indexer against local ledger so split-send blocks replay (also adds [Dockerfile.indexer](Dockerfile.indexer) and pins indexer image default) |

### What changed

- [src/funding.ts](deps/midnight-local-dev/src/funding.ts) (+123): new `fundSplitProveE2ESetup()` — funds accounts from `accounts.json` then sends a shielded NIGHT output to the hardcoded split-prove spender address, with up to 3 retries / 5s delay to recover from the proof-server's transient `BadInput("Failed direct assertion")` on first attempt. Honours `MIDNIGHT_SPLIT_PROVE_ACCOUNTS_FILE`, `MIDNIGHT_SPLIT_PROVE_SHIELDED_ADDRESS`, and `MIDNIGHT_SPLIT_PROVE_SHIELDED_AMOUNT`.
- [src/index.ts](deps/midnight-local-dev/src/index.ts) (+7): adds the `[6] Prepare split-prove e2e funding` menu entry.
- [standalone.yml](deps/midnight-local-dev/standalone.yml) (±1): default `MIDNIGHT_INDEXER_IMAGE` now points at the rebuilt `split-prove/indexer-standalone:local` so the stack replays split-send blocks instead of crashing.
- [.env.example](deps/midnight-local-dev/.env.example) (+4) and [README.md](deps/midnight-local-dev/README.md) (+94/−15): document the new env vars and option-6 flow.

---

## midnight-node

Three split-prove commits sit on top of `6f0ef437 bump node 0.22.3 (#1072)`. User-only diff (`git diff 6f0ef437..HEAD`): **7 files, +311 / −68** (most of that is `Cargo.lock` churn from switching to path deps).

### Commits

| SHA | Subject |
|---|---|
| `ce17c6a4` | Use local Ledger 8 crate graph for node build |
| `785e4ddd` | chore: simplify split-prove runtime image |
| `232f14d6` | feat: build split-prove node image on node 0.22.3 |

### What changed

**Repoint the Ledger 8 crate graph at the local checkout** ([Cargo.toml](deps/midnight-node/Cargo.toml), commit `ce17c6a4`)
- Was: workspace pulled `mn-ledger-8`, `ledger-storage-ledger-8`, `onchain-runtime-ledger-8`, `zswap-ledger-8` as pinned crates.io versions; `coin-structure` / `transient-crypto` / `zkir` / `midnight-serialize` were reused from the L7 entries; a single `[patch.crates-io] midnight-zswap = { path = "../midnight-ledger/zswap" }` covered the rest.
- Now: every ledger-8 crate (`base-crypto`, `coin-structure`, `midnight-serialize`, `transient-crypto`, `zkir`, plus the four pre-existing ones) is a `path = "../midnight-ledger/<crate>"` entry, and the `midnight-zswap` `[patch.crates-io]` is removed. This is what makes the rebuilt node link against the modified ledger (with the split-prove verifier keys + `well_formed` fallback) so it accepts split-send transactions.
- [ledger/Cargo.toml](deps/midnight-node/ledger/Cargo.toml) and [ledger/helpers/Cargo.toml](deps/midnight-node/ledger/helpers/Cargo.toml): add matching optional deps + `std` feature wiring.
- [ledger/src/lib.rs](deps/midnight-node/ledger/src/lib.rs) and [ledger/helpers/src/lib.rs](deps/midnight-node/ledger/helpers/src/lib.rs): the `ledger_8` module's `_local` aliases now point at the new `*-ledger-8` crates instead of inheriting the L7 ones.
- `midnight-storage-core` pinned with `=1.1.0` (was `1.1.0`) to keep the resolver from drifting.
- `Cargo.lock` (+277): consequence of the path-dep switch.

**Build a runnable split-prove node image** ([Dockerfile.split-prove](deps/midnight-node/Dockerfile.split-prove), commits `232f14d6` then `785e4ddd`)
- Multi-stage `rust:1.93` builder. Build context must be the parent `split-prove` repo root because the patched Cargo paths reference `../midnight-ledger`; the Dockerfile `COPY`s both `deps/midnight-node` and `deps/midnight-ledger` into `/build/deps/...`.
- Runtime stage is `public.ecr.aws/amazonlinux/amazonlinux:2023-minimal` with the standard observability tooling (`libfaketime`, `bytehound`) and the node's `entrypoint.sh` / `res/`.
- Build command: `docker build -f deps/midnight-node/Dockerfile.split-prove -t midnight-node:split-prove-0.22.3 .` from the parent repo root. Pass the resulting tag to local-dev as `MIDNIGHT_NODE_IMAGE`.

The follow-up commit `785e4ddd` trimmed the runtime stage (−13 / +7) — cosmetic simplification, no behavioural change.

## midnight-indexer

Single commit on top of the v4.0.1 release. Diff (`git diff c90fb85..HEAD`): **3 files, +767 / −594** (the bulk is `Cargo.lock` from the patch override).

### Commit

| SHA | Subject |
|---|---|
| `3235a61` | feat: build indexer against local split-prove ledger |

### What changed

**Repoint the v8 ledger crates at the local checkout** ([Cargo.toml](deps/midnight-indexer/Cargo.toml))
- Drop the `layout-v2` feature on `midnight-storage-core` (indexer doesn't reference it) and relax the version from `=1.1` to `1.0` so the local ledger's `storage-core 1.0.2` can satisfy it.
- Add a `[patch.crates-io]` block redirecting every v8 ledger crate (`midnight-base-crypto`, `midnight-coin-structure`, `midnight-ledger`, `midnight-ledger-static`, `midnight-onchain-runtime`, `midnight-onchain-state`, `midnight-onchain-vm`, `midnight-serialize`, `midnight-storage`, `midnight-storage-core`, `midnight-transient-crypto`, `midnight-zswap`) to `../midnight-ledger/<crate>`. The v7 ledger crates continue to resolve from crates.io — they're only touched for legacy-tx replay.

**Stub a new DB-trait method** ([indexer-common/src/infra/ledger_db/v1_1.rs](deps/midnight-indexer/indexer-common/src/infra/ledger_db/v1_1.rs))
- `storage-core 1.0.2` added `get_unreachable_keys()` to the `DB` trait. The indexer never GCs unreachable nodes in normal operation, so returning `Vec::new()` is correct for replay-only usage.

Without this commit, the stock `midnightntwrk/indexer-standalone:4.0.1` image links the packaged `midnight-zswap 8.0.0` verifier and exits with `Invalid proof — while verifying Zswap proof` while replaying any block containing a split-send tx. Build via the parent repo's [Dockerfile.indexer](Dockerfile.indexer): `docker build -f Dockerfile.indexer -t split-prove/indexer-standalone:local .`

## How to refresh this doc

```bash
# ledger
cd deps/midnight-ledger && git diff 641d18e5...HEAD --stat

# node
cd deps/midnight-node && git diff 6f0ef437..HEAD --stat

# indexer
cd ../midnight-indexer && git diff c90fb85..HEAD --stat

# local-dev (path-scoped from the parent repo)
cd ../.. && git log 7e23340..HEAD --oneline -- deps/midnight-local-dev/
git diff 7e23340..HEAD --stat -- deps/midnight-local-dev/
```
