# Submodule Changes

Summary of split-prove-specific modifications to the vendored dependencies.

| Component | Current rev | Baseline | Status |
|---|---|---|---|
| [webisoftSoftware/midnight-ledger](https://github.com/webisoftSoftware/midnight-ledger/tree/feature/split-prove-ledger-8.0.2) ([local](../deps/midnight-ledger)) | `3b279990` | `641d18e5` (tip of `feature/split-prove-no-sk`) | Summarised below |
| [deps/midnight-local-dev](../deps/midnight-local-dev) (vendored — no `.git`, tracked inside the parent repo) | parent `HEAD` | `7e23340` (vendoring commit) | Summarised below |
| [ADGLx/midnight-node](https://github.com/ADGLx/midnight-node/tree/feature/split-prove-node-0.22.3) ([local](../deps/midnight-node)) | `ce17c6a4` | `71fc6804` (3 commits before the first user commit `232f14d6`; upstream tip prior to your changes is `6f0ef437 bump node 0.22.3`) | Summarised below |
| [ADGLx/midnight-indexer](https://github.com/ADGLx/midnight-indexer/tree/feature/split-prove-indexer-4.0.1) ([local](../deps/midnight-indexer)) | `3235a61` | `c90fb85` (v4.0.1 release tag) | Summarised below |

---

## midnight-ledger

`git diff 641d18e5...HEAD` (merge-base based): split-prove commits plus local working-tree changes sit on top of the `no-sk` baseline. The latest working-tree delta keeps split admission ledger-verified while moving the registry check to a host-installed root checker for the local POC.

### Latest ledger delta: local registry-root split proof

The current split-prove stack keeps the Solution A v4 proof shape. The wallet creates first-registration evidence locally, generates a synthetic registry witness for the POC, and sends only the per-spend client proof plus registry root into the split bundle.

**Security model change**
- Final split zswap inputs now carry **two** proofs and two shared public inputs inside a typed envelope encoded in the opaque zswap `Proof(Vec<u8>)`:
  - `spend_split_proof` (server) - Merkle membership of the spent coin, value commitment, nullifier insertion, and `coin_binding_tag`.
  - `client_derivation_proof` (per spend) - proves the canonical nullifier, `coin_binding_tag`, and `registry_root` from one wallet witness whose `reg_leaf` is a Merkle-path member.
  - Shared public inputs: `coin_binding_tag`, `registry_root`.
- The rebuilt node verifies both proofs during normal zswap `well_formed()` checks and then asks the process-wide registry-root checker whether `registry_root` is admissible. The local proof-server, node, and indexer install a permissive checker; binaries that forget to install a checker fail closed.
- The one-time wallet attestation proof remains wallet-side first-registration evidence. It is used to build the synthetic local witness and does not travel in the split proof bundle.
- The client derivation circuit proves the canonical Zswap nullifier (`midnight:zswap-cn[v1]`), so a stock spend and a split spend of the same coin still collide in the ledger nullifier set.

**Measured impact (live e2e, `inclusion_status=inBlock`)**
| Build | client proof | server proof | server / client | client prover key |
|---|---:|---:|---:|---:|
| Earlier internal prototype (no wallet attestation) | 1899 ms | 1948 ms | 1.03× | 5.20 MB |
| Current design | 836 ms | 1720 ms | 2.06x | 2.82 MB |
| delta | −56.0% | −11.7% | shift to server | −45.7% |

The wallet additionally pays a one-time ~759 ms `wallet_attest` proof as local first-registration evidence. The spike that picked Poseidon over a 3-base Pedersen open for `C_sk` (1344 KB prover key vs 22 KB on this Compact lowering) lives in [spike-results.md](spike-results.md) in the parent repo.

**Zswap data model and verifier** ([deps/midnight-ledger/zswap/src/structure.rs](../deps/midnight-ledger/zswap/src/structure.rs), [verify.rs](../deps/midnight-ledger/zswap/src/verify.rs))
- `Input<P, D>` and `Offer<P, D>` keep their original serialized wire shape so stock wallet/local-dev shielded transfers remain compatible with the patched node.
- `SplitPublicInputs` includes only `coin_binding_tag: Fr` and `registry_root: MerkleTreeDigest`.
- `SplitProofBundle` carries `client_derivation_proof: Proof` plus the server `spend_proof`. Older internal bundle shapes fail closed against the current verifier. Current wire layout:
  ```
  MAGIC ‖ u32 LE len ‖ spend_proof
        ‖ u32 LE len ‖ client_derivation_proof
        ‖ coin_binding_tag[32] ‖ registry_root[32]
  ```
- Split inputs verify `CLIENT_DERIVATION_VK` against `(nullifier, coin_binding_tag, registry_root)`, call the host-installed registry-root checker, then verify `SPEND_SPLIT_VK`.
- Plain inputs still verify with the stock `SPEND_VK`; malformed split envelopes are rejected explicitly via `MalformedSplitProofBundle`.

**Circuit/artifact changes** ([deps/midnight-ledger/zswap/zswap-split.compact](../deps/midnight-ledger/zswap/zswap-split.compact), [deps/midnight-ledger/zswap/static](../deps/midnight-ledger/zswap/static))
- `spendSplitUser` (server circuit) checks coin Merkle membership, inserts the nullifier, validates the value commitment, and discloses `coinBindingTag`.
- Mirrored `spend-split.zkir` into [deps/midnight-ledger/zkir-precompiles/zswap/spend-split.zkir](../deps/midnight-ledger/zkir-precompiles/zswap/spend-split.zkir).
- [deps/midnight-ledger/zswap/static/client-derivation.verifier](../deps/midnight-ledger/zswap/static/client-derivation.verifier) matches the current `sk_prove` circuit, whose public transcript has 3 cells: `nullifier`, `coin_binding_tag`, and `registry_root`. Source lives at [circuits/sk_proof.compact](../circuits/sk_proof.compact) in the parent repo; regenerate after any change.
- [circuits/wallet_attestation.compact](../circuits/wallet_attestation.compact) remains the wallet-side first-registration proof. It is not linked into node/indexer admission in the local POC.
- Artifacts are regenerated with the local Compact CLI, for example:
  ```bash
  compact compile --no-communications-commitment circuits/sk_proof.compact /tmp/sk-prove-compile
  compact compile --no-communications-commitment circuits/wallet_attestation.compact /tmp/wallet-attestation-compile
  compact compile --no-communications-commitment deps/midnight-ledger/zswap/zswap-split.compact /tmp/zswap-split-compile
  ```
  Then copy `sk_prove.verifier` to `deps/midnight-ledger/zswap/static/client-derivation.verifier`; wallet attestation artifacts are kept client/proof-server side for local witness generation.

**Construct/prove/proof-server flow**
- `Input::new_split()` takes `coin_binding_tag`, `registry_root`, and `client_derivation_proof`, and threads them into the emitted `SplitProofBundle`. The split proving context returns `provedInputHex` with an encoded `ZswapInputProof::Split` envelope; the HTTP endpoint does not hand-build the envelope.
- `Input<ProofPreimage>::delta()` and `binding_randomness()` are unchanged — the split witness trailer layout (`nullifier` appended after `rc`) is preserved, so local submit-path Pedersen binding randomness still matches what the node recomputes.
- `/v2/prove-split-spend` ([endpoints.rs](../deps/midnight-ledger/proof-server/src/endpoints.rs)) accepts `clientDerivationProof` and `registryRoot`; it verifies the 3-cell client proof transcript before server proving.
- [local_poc_client.rs](../deps/midnight-ledger/proof-server/src/local_poc_client.rs) generates wallet attestation evidence once per local POC run, creates a synthetic first-registration Merkle witness, attaches only `clientDerivationProof` and `registryRoot` to every `/v2/prove-split-spend` POST, and keeps `sk`, `r`, `salt`, and the registry path wallet-side.
- The synthetic split-spend integration test deserializes `provedInputHex`, calls `Input<Proof>::well_formed(0)`, and asserts that a raw split proof without the client proof, a tampered nullifier, a tampered `coinBindingTag`, a tampered `registryRoot`, and a malformed split envelope are all rejected (`MalformedSplitProofBundle`).

**Build impact**
- Rebuild the node, indexer, and proof-server images after verifier changes so they have the current envelope code and circuit blobs. The zswap input/offer outer wire tags remain compatible with local-dev's stock wallet SDK; only the opaque proof envelope changed.
- Regenerate `zswap/static/client-derivation.verifier` whenever [circuits/sk_proof.compact](../circuits/sk_proof.compact) changes. Wallet-attestation artifacts are local client/proof-server material.
- Pure proof-server local-client or test-side fixes (binding-randomness extraction, JSON field renames, role-boundary report wording) do not require rebuilding an already-running node or indexer; the node can already reject malformed sealed transactions correctly.

### Commits (newest first)

| SHA | Subject |
|---|---|
| `3b279990` | proof-server: add partial local split-send support (transfer amount + shielded change output) |
| `d385990e` | Fix local split-send recipient nonce (avoid `CommitmentAlreadyPresent`) |
| `c07e366d` | complete live split-send e2e with tx assembly and node inclusion check (`author_submitAndWatchExtrinsic`) |
| `99d8fdc0` | Fix split-prove Ledger 8 local dependency graph |
| `edb21000` | keep split zswap compatible with ledger 8.0 |
| `a72735ae` | wire `SPEND_SPLIT_VK` and `SIGN_SPLIT_VK` into `well_formed` dispatch (makes node accept split-prove tx) |
| `547987d5` | Implement split-prove server/client PoC (endpoint, local client, e2e tests) |
| `482d905c` | add split-prove constructors for delegated proving without secret key |

### What changed, by area

**New zswap split constructors and circuits** ([deps/midnight-ledger/zswap/src/construct.rs](../deps/midnight-ledger/zswap/src/construct.rs), +149)
- `Input::new_split()` accepts pre-computed `coinBindingTag`, `registryRoot`, and `clientDerivationProof` instead of the raw secret key. The latest working-tree version keeps zswap input serialization stable; `Input::new_split()` returns a split context until proving encodes the v4 bundle. `AuthorizedClaim::new_split()` / `sign-split` is only prototype placeholder wiring and is not the split-send authorization story.
- New circuit source: [`zswap/zswap-split.compact`](../deps/midnight-ledger/zswap/zswap-split.compact) (+41).
- Compiled artifacts under [`zswap/static/`](../deps/midnight-ledger/zswap/static/): `spend-split.{zkir,bzkir,prover,verifier}` plus sha256 sidecars. `sign-split.*` exists as prototype placeholder material only.
- Mirrored zkir bytecode under [`zkir-precompiles/zswap/`](../deps/midnight-ledger/zkir-precompiles/zswap/).

**zkir support for committed inputs** ([deps/midnight-ledger/zkir/src/ir.rs](../deps/midnight-ledger/zkir/src/ir.rs), [ir_vm.rs](../deps/midnight-ledger/zkir/src/ir_vm.rs))
- `IrSource::prove_split()` with `committed_input_count`.
- `Preprocessed.committed_input_count` and `format_committed_instances()` override needed by the split flow.

**Ledger verification wiring** ([deps/midnight-ledger/zswap/src/verify.rs](../deps/midnight-ledger/zswap/src/verify.rs), +71)
- `lazy_static` refs for `SPEND_SPLIT_VK` and `CLIENT_DERIVATION_VK` from the local `.verifier` blobs.
- Split inputs are explicit via a typed proof envelope; the node verifies client-derivation and spend-split proofs against matching `coinBindingTag` / `registryRoot` public inputs, then checks `registryRoot` through the installed host checker.

**Proof server** ([deps/midnight-ledger/proof-server/](../deps/midnight-ledger/proof-server/))
- New endpoint `POST /v2/prove-split-spend` in [endpoints.rs](../deps/midnight-ledger/proof-server/src/endpoints.rs) (+403). Reconstructs `QualifiedCoinInfo`, loads the Merkle tree, rejects handoffs whose commitment doesn't reproduce the root, pre-verifies `clientDerivationProof` against `registryRoot`, calls `Input::new_split`, proves `midnight/zswap/spend-split`, returns `proofHex` + `provedInputHex`. Envelope construction is delegated to the zswap split proving context so any caller can produce a node-acceptable split input.
- New file [local_poc_client.rs](../deps/midnight-ledger/proof-server/src/local_poc_client.rs): local wallet scanner, synthetic registry witness builder, handoff builder, full tx assembly (recipient output + binding randomness + StandardTransaction + Dust handoff to the JS wallet bridge), and raw-RPC submission.
- New driver binary [bin/local_poc_split_prove.rs](../deps/midnight-ledger/proof-server/src/bin/local_poc_split_prove.rs) (+102).
- Integration tests [tests/integration_tests.rs](../deps/midnight-ledger/proof-server/tests/integration_tests.rs): synthetic e2e (always runs) + opt-in local-node e2e (`MIDNIGHT_RUN_LOCAL_E2E=1`).

**Misc**
- [proof-server/Cargo.toml](../deps/midnight-ledger/proof-server/Cargo.toml), [zswap/Cargo.toml](../deps/midnight-ledger/zswap/Cargo.toml): dep wiring.
- [zswap/src/error.rs](../deps/midnight-ledger/zswap/src/error.rs), [zswap/src/prove.rs](../deps/midnight-ledger/zswap/src/prove.rs), [zswap/src/structure.rs](../deps/midnight-ledger/zswap/src/structure.rs): error variants, split-circuit key resolution, struct exposure.
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
| `ea52cbd` | rebuild indexer against local ledger so split-send blocks replay (also adds [Dockerfile.indexer](../Dockerfile.indexer) and pins indexer image default) |

### What changed

- [src/funding.ts](../deps/midnight-local-dev/src/funding.ts) (+123): new `fundSplitProveE2ESetup()` — funds accounts from `accounts.json` then sends a shielded NIGHT output to the hardcoded split-prove spender address, with up to 3 retries / 5s delay to recover from the proof-server's transient `BadInput("Failed direct assertion")` on first attempt. Honours `MIDNIGHT_SPLIT_PROVE_ACCOUNTS_FILE`, `MIDNIGHT_SPLIT_PROVE_SHIELDED_ADDRESS`, and `MIDNIGHT_SPLIT_PROVE_SHIELDED_AMOUNT`.
- [src/index.ts](../deps/midnight-local-dev/src/index.ts) (+7): adds the `[6] Prepare split-prove e2e funding` menu entry.
- [standalone.yml](../deps/midnight-local-dev/standalone.yml) (±1): default `MIDNIGHT_INDEXER_IMAGE` now points at the rebuilt `split-prove/indexer-standalone:local` so the stack replays split-send blocks instead of crashing.
- [.env.example](../deps/midnight-local-dev/.env.example) (+4) and [README.md](../deps/midnight-local-dev/README.md) (+94/−15): document the new env vars and option-6 flow.

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

**Repoint the Ledger 8 crate graph at the local checkout** ([Cargo.toml](../deps/midnight-node/Cargo.toml), commit `ce17c6a4`)
- Was: workspace pulled `mn-ledger-8`, `ledger-storage-ledger-8`, `onchain-runtime-ledger-8`, `zswap-ledger-8` as pinned crates.io versions; `coin-structure` / `transient-crypto` / `zkir` / `midnight-serialize` were reused from the L7 entries; a single `[patch.crates-io] midnight-zswap = { path = "../midnight-ledger/zswap" }` covered the rest.
- Now: every ledger-8 crate (`base-crypto`, `coin-structure`, `midnight-serialize`, `transient-crypto`, `zkir`, plus the four pre-existing ones) is a `path = "../midnight-ledger/<crate>"` entry, and the `midnight-zswap` `[patch.crates-io]` is removed. This is what makes the rebuilt node link against the modified ledger (with the split-prove verifier keys + `well_formed` fallback) so it accepts split-send transactions.
- [ledger/Cargo.toml](../deps/midnight-node/ledger/Cargo.toml) and [ledger/helpers/Cargo.toml](../deps/midnight-node/ledger/helpers/Cargo.toml): add matching optional deps + `std` feature wiring.
- [ledger/src/lib.rs](../deps/midnight-node/ledger/src/lib.rs) and [ledger/helpers/src/lib.rs](../deps/midnight-node/ledger/helpers/src/lib.rs): the `ledger_8` module's `_local` aliases now point at the new `*-ledger-8` crates instead of inheriting the L7 ones.
- `midnight-storage-core` pinned with `=1.1.0` (was `1.1.0`) to keep the resolver from drifting.
- `Cargo.lock` (+277): consequence of the path-dep switch.

**Build a runnable split-prove node image** ([Dockerfile.split-prove](../deps/midnight-node/Dockerfile.split-prove), commits `232f14d6` then `785e4ddd`)
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

**Repoint the v8 ledger crates at the local checkout** ([Cargo.toml](../deps/midnight-indexer/Cargo.toml))
- Drop the `layout-v2` feature on `midnight-storage-core` (indexer doesn't reference it) and relax the version from `=1.1` to `1.0` so the local ledger's `storage-core 1.0.2` can satisfy it.
- Add a `[patch.crates-io]` block redirecting every v8 ledger crate (`midnight-base-crypto`, `midnight-coin-structure`, `midnight-ledger`, `midnight-ledger-static`, `midnight-onchain-runtime`, `midnight-onchain-state`, `midnight-onchain-vm`, `midnight-serialize`, `midnight-storage`, `midnight-storage-core`, `midnight-transient-crypto`, `midnight-zswap`) to `../midnight-ledger/<crate>`. The v7 ledger crates continue to resolve from crates.io — they're only touched for legacy-tx replay.

**Stub a new DB-trait method** ([indexer-common/src/infra/ledger_db/v1_1.rs](../deps/midnight-indexer/indexer-common/src/infra/ledger_db/v1_1.rs))
- `storage-core 1.0.2` added `get_unreachable_keys()` to the `DB` trait. The indexer never GCs unreachable nodes in normal operation, so returning `Vec::new()` is correct for replay-only usage.

Without this commit, the stock `midnightntwrk/indexer-standalone:4.0.1` image links the packaged `midnight-zswap 8.0.0` verifier and exits with `Invalid proof — while verifying Zswap proof` while replaying any block containing a split-send tx. Build via the parent repo's [Dockerfile.indexer](../Dockerfile.indexer): `docker build -f Dockerfile.indexer -t split-prove/indexer-standalone:local .`

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
