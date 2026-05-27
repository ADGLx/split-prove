# Split Proof Design

Split-Prove divides a zswap spend into wallet-local proof work and server proof work. The server can prove the expensive spend circuit without receiving the wallet's raw zswap secret key.

## Proof Boundary

### Wallet setup

The wallet can generate `pi_attest` once per key as first-registration evidence. It proves:

- the wallet knows `sk`;
- `C_sk` commits to that `sk` using wallet-only randomness `r`;
- `reg_leaf` commits to `C_sk` with a wallet-only `salt`.

In the local-node POC this evidence is kept wallet-side; the client builds a synthetic first-registration Merkle witness instead of deploying a registry contract.

### Per spend, client side

The wallet generates `pi_sk` for each spend. It proves:

- the wallet knows an `sk` whose `reg_leaf` is in the registry tree rooted at `registryRoot`;
- the canonical zswap nullifier was derived from that `sk` and the coin;
- the coin-binding tag was derived from the coin and `pk`.

The client proof uses the normal zswap nullifier relation, so a stock spend and split spend of the same coin collide in the same nullifier set.

### Per spend, proof server side

The proof server generates `pi_spend`. It proves:

- the coin commitment was built from the coin and `pk`;
- the coin commitment is in the Merkle tree;
- the declared nullifier is inserted;
- the value commitment and spend rules are valid.

The server receives public values, coin metadata, Merkle data, and proofs. It does not receive `sk`, `r`, `salt`, or the registry Merkle path.

### Transaction submission

The patched node verifies the v4 split bundle: `pi_sk`, `pi_spend`, and the shared public inputs:

- `coinBindingTag`;
- `registryRoot`.

The transaction is accepted only if both proofs verify and `registryRoot` is the current root of the configured wallet-registry contract. Synthetic local roots require the explicit `MIDNIGHT_SPLIT_REGISTRY_DEV_ACCEPT_ALL=1` opt-in.

## Why The Wallet Proof Is Smaller

The client does less repeated work because the per-spend proof uses Poseidon/transient-hash relations for the registration leaf and Merkle path rather than recomputing expensive persistent-hash relations wherever possible.

The current proof-side design commits to an injective limb encoding of the 32-byte secret key instead of treating `sk` as one field/scalar. That avoids scalar-reduction ambiguity.

## Common Questions

### Does split proving preserve normal double-spend semantics?

Yes. The client proof derives the canonical zswap nullifier, so a stock spend and a split spend of the same coin collide in the same nullifier set.

### What prevents the server from changing wallet or coin-binding data?

The ledger verifies `pi_sk` against `nullifier`, `coinBindingTag`, and `registryRoot`, and verifies the server proof against the same split public inputs. Tampering with those values breaks verification.

### Why add a wallet attestation proof?

It provides first-registration evidence for the wallet's registry leaf. In the local POC the proof is generated and retained off-chain while the split bundle carries only the per-spend client proof and registry root.

### Is the wallet attestation reusable?

Yes. It is intended as a one-time-per-wallet or one-time-per-key proof. The per-spend proof proves membership of the resulting `reg_leaf`.

### Why use Poseidon for `C_sk`?

It was much cheaper in Compact than the Pedersen commitment option while still giving the binding needed for this PoC. See [spike-results.md](spike-results.md).

### Does the server ever receive the raw `sk`?

No. The server receives public values and proofs, but not the secret key, attestation blinding value, salt, or registry Merkle path.

### Why does this require a patched ledger?

Stock nodes do not know how to decode or verify the v4 split proof bundle or client derivation proof.

### Is this recursive proof aggregation?

No. The current approach verifies the client and server proofs directly in ledger admission logic instead of recursively aggregating them into one proof.

### What is the main compatibility tradeoff?

The outer zswap wire format stays mostly compatible, but nodes and indexers must run the patched verifier logic and bundled verifier keys.

## Approaches Tried And Discarded

- A two-proof split where the proof server only verified `clientDerivationProof` off-chain before producing `spend-split`. This was not enough because ledger admission still trusted the server gate; someone could bypass the endpoint unless the ledger also verified the client proof.
- Plain fallback verifier dispatch: try stock `SPEND_VK`, then fallback to `SPEND_SPLIT_VK`. This made nodes accept split proofs, but it did not carry or verify the extra client-proof linkage needed for sound split admission.
- Keeping `coinCommitment` as a wallet/client-proved public output. This was moved into the server `spendSplitUser` circuit so the server proof computes canonical `H(coin, pk)` and checks it against the Merkle path leaf.
- Using `skCommitment` as a placeholder witness in the server split circuit. It pinned almost nothing by itself, so the design shifted to explicit shared public inputs: `pk`, `nullifier`, `coinBindingTag`, and later `C_sk`.
- Using a split-only Poseidon nullifier. It improved client proof cost, but broke stock-vs-split double-spend semantics because normal wallet spends use the canonical zswap nullifier. The fix was to make split spends prove the canonical wallet nullifier.
- Recomputing `pk = H_persistent(sk)` inside every per-spend client proof. This was sound but too expensive; the client proof became almost as heavy as the server proof. It was replaced by a one-time wallet attestation plus a cheaper per-spend opening.
- A v2 split bundle without wallet attestation. It could verify per-spend derivation, but did not cheaply bind the per-spend `sk` back to the canonical `pk = H(sk)` without paying that hash every spend. The v3 bundle added `attestation_proof`; v4 replaced the on-chain attestation fields with `registryRoot`.
- A Pedersen-style `C_sk` commitment. It was considered for the wallet attestation link, but was much more expensive than the Poseidon/transient-hash commitment in Compact, so the PoC moved to Poseidon `C_sk`.
- Treating `sk` as a single field/scalar for commitment. That risks scalar-reduction ambiguity, so the final proof-side design uses an injective limb encoding of the 32-byte secret before committing and checking it.
