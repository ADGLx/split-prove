# Split-Prove

Proof-of-concept for Midnight zswap **delegated proving**: a wallet outsources the expensive spend proof to a proof server **without sending the raw zswap secret key (`sk`)**. The patched node verifies a three-proof bundle (`π_attest`, `π_sk`, `π_spend`) and byte-equates the shared public inputs before admitting the spend.

## Quick Start

One-time setup:

```bash
npm install
make local-install
cp .env.example .env
make rebuild-images
```

In another terminal, start the local stack:

```bash
make local-nodes
```

Fund the split-prove wallet via local-dev option 6, then run the demo:

```bash
make e2e
```

The demo scans for the funded shielded output, creates the wallet attestation and per-spend client proof, calls `/v2/prove-split-spend`, assembles the split transaction, Dust-balances it, submits to the rebuilt node, and waits for inclusion.

Both node and indexer must be rebuilt against the patched ledger. The helper refuses remote node/indexer URLs unless `MIDNIGHT_ALLOW_REMOTE_PATCHED_STACK=1` is set for a known patched pair.

## Docs

- [docs/split-proof-design.md](docs/split-proof-design.md) — proof boundary, ledger checks, common questions, discarded designs.
- [docs/poc-runbook.md](docs/poc-runbook.md) — environment variables, endpoint shape, operational details, scope.
- [docs/poc-comparison.md](docs/poc-comparison.md) — direct vs. registry vs. recursive-wrapper, side-by-side.
- [docs/submodule-changes.md](docs/submodule-changes.md) — vendored Midnight dependency changes.
- [docs/spike-results.md](docs/spike-results.md) — Compact proof-cost spike behind the Poseidon `C_sk` commitment.

## Repo Layout

- [circuits/](circuits/) — Compact circuits for wallet attestation and client derivation.
- [deps/midnight-ledger](deps/midnight-ledger) — patched ledger, zswap verifier, proof-server changes ([webisoftSoftware/midnight-ledger](https://github.com/webisoftSoftware/midnight-ledger/tree/feature/split-prove-ledger-8.0.2)).
- [deps/midnight-node](deps/midnight-node), [deps/midnight-indexer](deps/midnight-indexer) — rebuilt against the patched ledger ([ADGLx/midnight-node](https://github.com/ADGLx/midnight-node/tree/feature/split-prove-node-0.22.3), [ADGLx/midnight-indexer](https://github.com/ADGLx/midnight-indexer/tree/feature/split-prove-indexer-4.0.1)).
- [deps/midnight-local-dev](deps/midnight-local-dev) — local stack with the split-prove funding option.
- [tools/](tools/) — wallet SDK bridge helpers (seed derivation, Dust balancing, raw-RPC submit).
