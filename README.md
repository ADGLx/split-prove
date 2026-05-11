# Split-Prove Prototype

Separates ZK proving so the **client keeps the secret key** and the **server does the heavy proving**.

## Architecture

```
CLIENT (wallet, ~100µs)              SERVER (prover, ~2-10s)
─────────────────────────            ──────────────────────────
sk (secret key)                      Never sees sk
  ↓
nullifier = H(sk, coin)              Receives: {sk_commitment,
pk = H(sk)                             nullifier, pk, coin_info,
commitment = commit(pk, coin)           merkle_path}
sk_commitment = H(sk, random)            ↓
  ↓                                  Builds ProofPreimage with
POST /v2/prove {handoff}  ────────→    sk_commitment in inputs[0]
                                         ↓
                          ←────────  prove() → Proof
```

## Files

```
proto/split-prove/
├── README.md                        # This file
├── Cargo.toml                       # Uses local midnight-ledger paths
├── src/main.rs                      # End-to-end demo
└── deps/midnight-ledger/            # Forked midnight-ledger
    ├── zswap/
    │   ├── zswap.compact            # Original circuits (untouched)
    │   ├── zswap-split.compact      # NEW: split-prove circuits
    │   └── src/construct.rs         # NEW: new_split() constructors
    └── ledger/
        ├── dust.compact             # Original circuits (untouched)
        └── dust-split.compact       # NEW: split-prove dust circuit
```

## What Changed

### Compact circuits (`*-split.compact`)

The ONLY change is the `committed` keyword on `sk` parameters:

```diff
- export circuit spend(sk: Either<ZswapCoinSecretKey, ContractAddress>, ...)
+ export circuit spend_split(sk: committed Either<ZswapCoinSecretKey, ContractAddress>, ...)

- export circuit sign(secretKey: ZswapCoinSecretKey)
+ export circuit sign_split(secretKey: committed ZswapCoinSecretKey)

- export circuit spend(dust: DustOutput, sk: DustSecretKey, ...)
+ export circuit spend_split(dust: DustOutput, sk: committed DustSecretKey, ...)
```

All constraint logic is **identical** — the circuit still computes `nullifier = H(sk, coin)`
and `pk = H(sk)` internally. The `committed` keyword just changes how `sk` enters the
circuit: via a committed-instance column instead of a regular witness column.

### Rust constructors (`construct.rs`)

Added `new_split()` alongside existing constructors (non-breaking):
- `AuthorizedClaim::new_split()` — sign circuit
- `Input::new_split()` — spend circuit

These accept pre-computed `(nullifier, pk, commitment_hash, sk_commitment)` instead of raw `sk`.

## Compilation Steps

### 1. Compile the split circuits

```bash
# From the midnight-ledger directory
compactc zswap/zswap-split.compact --output zswap/static/spend-split
compactc zswap/zswap-split.compact --output zswap/static/sign-split
compactc ledger/dust-split.compact --output ledger/static/dust/spend-split
```

This produces: `spend-split.bzkir`, `sign-split.bzkir`, `spend-split.bzkir` (dust)

### 2. Generate proving/verifying keys

```bash
# Using the key generation example from midnight-zk-stdlib
cargo run --example keygen -- \
  --circuit zswap/static/spend-split.bzkir \
  --params params/bls_filecoin_2p15 \
  --output zswap/static/spend-split

# Repeat for sign-split and dust/spend-split
```

This produces: `.prover`, `.verifier` files for each circuit.

### 3. Deploy keys to server

Copy the `.bzkir`, `.prover`, `.verifier` files to the server's key directories
alongside the existing circuit keys.

### 4. Add /v2/prove endpoint

In `rust-wrapper/src/lib.rs`, add a new route that:
1. Deserializes `ClientHandoff` from the request body
2. Calls `Input::new_split()` / `AuthorizedClaim::new_split()` to build the ProofPreimage
3. Resolves the `spend-split` / `sign-split` circuit keys
4. Runs `prove_native()` → returns proof

### 5. Client SDK

Add `client_prepare(sk, coin)` function (~20 lines) that computes:
- nullifier, pk, commitment, sk_commitment
- Serializes into `ClientHandoff`
- POSTs to `/v2/prove`

## Security Properties

| Property | Guarantee |
|---|---|
| sk hidden from server | ✓ Server sees `sk_commitment = H(sk, r)`, not `sk` |
| Commitment binding | ✓ Client can't change sk after committing |
| Proof validity | ✓ Circuit still verifies nullifier/pk derived from real sk |
| No circuit weakening | ✓ Same constraints as original — only input method changes |

## Running the Prototype

```bash
# Build and run
cd proto/split-prove
cargo run

# Output shows:
# - Client computation: ~100µs
# - Server build: ~1ms
# - inputs[0] = sk_commitment (not raw sk) ✓
```
