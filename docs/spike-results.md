# Spike: ecMul cost in Compact lowering (Implementation step 0)

Measured 2026-05-15 with `compactc 0.31.0`. Methodology: compile single-operation
circuits with `--no-communications-commitment` and read the prover-key size as
a proxy for in-circuit constraint count (proportional under Halo2).

## Results

| Circuit | Operation modelled | Prover key | × vs SHA-256 |
|---|---|---:|---:|
| `bench_persistent_hash_sk` | 1× SHA-256(`sep[21] ‖ sk[32]`) — today's `pk = H(sk)` | 2,815 KB | 1.00 |
| `bench_pedersen_open` | 3× `hashToCurve` + 3× `ecMul` + 2× `ecAdd` — Pedersen variant of `C_sk` open | 1,344 KB | 0.48 |
| `bench_ec_mul_gen` | 1× `ecMulGenerator` (standard Jubjub generator, fixed-base) | 349 KB | 0.12 |
| `bench_ec_mul_var` | 1× `hashToCurve` + 1× `ecMul` (variable-base) | 355 KB | 0.13 |
| `bench_transient_hash` | 1× `transientHash` (Poseidon) over 3 `Field` inputs | 22 KB | 0.008 |
| `bench_poseidon_commit` | 1× `transientHash` over `(sk_lo, sk_hi, r)` — Poseidon variant of `C_sk` open | 22 KB | 0.008 |

## Observations

1. **Fixed-base vs variable-base `ecMul` is a wash** — 349 KB vs 355 KB. The
   plan's worry that Compact would only emit variable-base when generators are
   not the standard one is irrelevant in practice; both lowerings cost
   essentially the same. So custom protocol generators G_sk_lo / G_sk_hi /
   G_r will not pay a meaningful penalty.

2. **A `transientHash` (Poseidon) is ~130× cheaper than a SHA-256(sk)** in
   prover-key size. And ~60× cheaper than a single `ecMul`.

3. **Poseidon-based commit `C_sk = transientHash(sk_lo, sk_hi, r)` is ~60×
   cheaper than the Pedersen-based `C_sk = sk_lo·G_sk_lo + sk_hi·G_sk_hi + r·G_r`**
   in this Compact lowering (22 KB vs 1,344 KB).

## Decision

Switch the commitment primitive from **Pedersen** to **Poseidon
(`transientHash`)**. Both are sound; Poseidon is dramatically cheaper here and
removes the need for protocol-fixed Jubjub generators.

Soundness changes:
- Pedersen binding (DLOG hard on Jubjub + independent generators) →
  Poseidon binding (collision-resistance of the Compact `transientHash`).
- Pedersen hiding (information-theoretic in `r`) →
  Poseidon hiding (preimage resistance + fresh random `r`).

The two-limb injective encoding of the 32-byte `sk` is **unchanged**: we still
range-check `sk_lo, sk_hi < 2^128` and feed the same bit witnesses to both the
canonical SHA-256(sk) and the Poseidon commit. The scalar-reduction attack
note in the plan still applies — `sk' = sk + n·q` produces different limbs
⇒ different Poseidon image ⇒ commitment cross-check at admission fails.

## Projected total client-circuit cost

| Component | Earlier internal prototype | Pedersen commitment option | Current Poseidon commitment |
|---|---:|---:|---:|
| pk = SHA-256(sk) | 2,815 KB | — | — |
| nullifier = SHA-256(coin, sk) | ~2,500 KB | ~2,500 KB | ~2,500 KB |
| coinBindingTag (Poseidon) | 22 KB | 22 KB | 22 KB |
| C_sk open / commit | — | 1,344 KB | 22 KB |
| Disclosures + glue | ~50 KB | ~50 KB | ~50 KB |
| **Rough total** | **~5,400 KB** | **~3,900 KB (~30% drop)** | **~2,600 KB (~52% drop)** |

(The earlier measured client prover key was 5.20 MB — within rounding of the
rough sum above.)

## Confirmation against live e2e (2026-05-15)

`make e2e` after wiring `preview_client.rs` to register a wallet attestation
and attach it to every `/v2/prove-split-spend`:

| Stage | Time |
|---|---:|
| Wallet attestation (one-time per sk) | ~0.8-0.9 s |
| `clientDerivationProof` (per-spend, local) | **836 ms** |
| `spend-split` proof (server) | 1720 ms |
| Server verify client derivation | 4 ms |
| Server / client ratio | **2.06×** |

vs the earlier no-attestation prototype `clientDerivationProof = 1899 ms`, that's a
**−56.0%** per-spend client-side drop. The 2.06× asymmetry confirms the spike
projection (~25–35% prover-key drop translates to ~50% proving-time drop here
because the SHA-256 gadget dominates wall-clock more strongly than it does
prover-key size). Transaction reached `inclusion_status: inBlock`.

## Recursive wrapper update (latest live e2e)

The Poseidon `C_sk` decision still holds for the wallet-local circuit: the latest recursive-wrapper e2e measured `clientDerivationProof` local proving at 818 ms. The new cost center is the server-side recursive proving path, which includes both `spend-split` and the privacy wrapper. The same run measured 192,755 ms of remote server recursive proving, 193,573 ms split-prove proof-only total, and a 235.64x server/client proving ratio, with the transaction finalized on-chain.

This does not change the commitment decision in this spike. It changes the operational tradeoff of the current branch: wallet proving stays small, but recursive privacy moves a large amount of proving work to the proof server.

## Files

- `bench/bench_*.compact` — source circuits
- `bench/out_bench_*/keys/*.prover` — compiled artifacts

To reproduce:
```bash
for c in bench_persistent_hash_sk bench_transient_hash bench_ec_mul_gen \
         bench_ec_mul_var bench_pedersen_open bench_poseidon_commit ; do
  rm -rf bench/out_$c ; mkdir -p bench/out_$c
  ~/.compact/versions/0.31.0/aarch64-darwin/compactc \
    --no-communications-commitment bench/$c.compact bench/out_$c
done
ls -la bench/out_bench_*/keys/*.prover
```
