# Split Proof Design

Split-Prove divides a zswap spend into wallet-local proof work and server proof work. The server can prove the expensive spend circuit without receiving the wallet's raw zswap secret key.

## Proof Boundary

### Wallet setup

The wallet generates `pi_attest` once per key. It proves:

- the wallet knows `sk`;
- `pk` was derived from that `sk`;
- `C_sk` commits to the same `sk` using wallet-only randomness `r`.

This moves the expensive `pk = H(sk)` relation out of the per-spend client proof.

### Per spend, client side

The wallet generates `pi_sk` for each spend. It proves:

- the wallet knows the same `sk` committed in `C_sk`;
- the canonical zswap nullifier was derived from that `sk` and the coin;
- the coin-binding tag was derived from the coin and `pk`.

The client proof uses the normal zswap nullifier relation, so a stock spend and split spend of the same coin collide in the same nullifier set.

### Per spend, proof server side

The proof server generates `pi_spend`. It proves:

- the coin commitment was built from the coin and `pk`;
- the coin commitment is in the Merkle tree;
- the declared nullifier is inserted;
- the value commitment and spend rules are valid.

The server receives handoff values, coin metadata, Merkle data, and proofs. It does not receive `sk` or the attestation blinding value `r`.

After generating `pi_spend`, the proof server generates `pi_wrap`, a recursive wrapper proof. The wrapper verifies `pi_attest`, `pi_sk`, and `pi_spend` privately and exposes only the normal spend admission data plus an aggregate accumulator.

### Transaction submission

The patched node verifies the recursive wrapper proof and checks its aggregate accumulator. The node-visible wrapper public inputs are:

- `nullifier`;
- Merkle root;
- value commitment;
- contract address/none;
- segment.

The split-only linkage values `pk`, `C_sk`, `coinBindingTag`, and `coinCommitment` are private wrapper witnesses. They are still seen by the proof server as part of the current handoff, but they are no longer ledger/public fields.

The transaction is accepted only if the wrapper proof verifies, the aggregate accumulator checks against the fixed inner verifier-key bases, and the normal spend admission inputs match the transaction.

## Current Tradeoff

The earlier direct split bundle fixed the proof-server trust issue by making the node verify all three proofs directly, but it exposed stable split-only linkage values in the ledger-visible proof envelope. The recursive wrapper fixes that public privacy leak: ledger admission sees one wrapper proof and an aggregate accumulator, not raw `pk`, `C_sk`, `coinBindingTag`, or `coinCommitment`.

The tradeoff is proof-system compatibility. Midnight's recursive `VerifierGadget` uses a Poseidon Fiat-Shamir transcript, while the previous ledger proof path used `blake2b_simd::State`. This branch intentionally switches newly generated proofs to the Poseidon transcript so the wrapper can verify the inner proofs recursively. Existing Blake2b-transcript direct split proof bytes are not wrapper-compatible.

This is acceptable for the independent proof-of-concept branch, but it means the branch is not wire/proof compatible with stock Midnight proof artifacts. A production version would need a migration plan, a Blake2b-compatible recursive verifier gadget, or a fully Poseidon-based proof stack agreed across ledger/prover tooling.

## Why The Wallet Proof Is Smaller

The client does less repeated work because `C_sk` is opened with a cheap Poseidon/transient-hash relation instead of recomputing the expensive `pk = H(sk)` SHA-256 relation on every spend.

The expensive `pk = H(sk)` proof is paid once in the wallet attestation. Each spend only proves that the same committed `sk` is used to derive the nullifier and coin-binding tag.

The current proof-side design commits to an injective limb encoding of the 32-byte secret key instead of treating `sk` as one field/scalar. That avoids scalar-reduction ambiguity.

## Common Questions

### Does split proving preserve normal double-spend semantics?

Yes. The client proof derives the canonical zswap nullifier, so a stock spend and a split spend of the same coin collide in the same nullifier set.

### What prevents the server from changing `pk`, `nullifier`, or coin-binding data?

The wrapper circuit constrains the private inner statements against the public spend admission inputs and shared private witnesses. Tampering with `pk`, `nullifier`, `coinBindingTag`, `coinCommitment`, or `C_sk` breaks the wrapper proof or its aggregate accumulator check.

### Why add a wallet attestation proof?

It avoids recomputing `pk = H(sk)` inside every per-spend client proof while still binding the wallet key to the per-spend proof.

### Is the wallet attestation reusable?

Yes. It is intended as a one-time-per-wallet or one-time-per-key proof. The per-spend proof opens the same `C_sk`.

### Why use Poseidon for `C_sk`?

It was much cheaper in Compact than the Pedersen commitment option while still giving the binding needed for this PoC. See [spike-results.md](spike-results.md).

### Does the server ever receive the raw `sk`?

No. The server receives public values and proofs, but not the secret key or attestation blinding value.

### Does the ledger see `pk`, `C_sk`, `coinBindingTag`, or `coinCommitment`?

No, in the recursive wrapper design. Those values are private wrapper witnesses. They remain visible to the proof server in the current handoff, so this improves ledger/public privacy rather than proof-server privacy.

### Why does this require a patched ledger?

Stock nodes do not know how to decode or verify the wrapper envelope, wrapper verifier key, or aggregate accumulator.

### Is this recursive proof aggregation?

Yes. The recursive wrapper path uses a Rust wrapper relation around `midnight-circuits::verifier::VerifierGadget` to verify the three inner proofs and publish one wrapper proof plus an aggregate accumulator.

### What is the main compatibility tradeoff?

The outer zswap input shape stays mostly compatible, but the opaque proof envelope and proof transcript are not stock-compatible. Nodes and indexers must run the patched verifier logic, and split-wrapper proofs must be generated from Poseidon-transcript inner proofs. Existing Blake2b-transcript direct split bundles are rejected.

## Approaches Tried And Discarded

- A two-proof split where the proof server only verified `clientDerivationProof` off-chain before producing `spend-split`. This was not enough because ledger admission still trusted the server gate; someone could bypass the endpoint unless the ledger also verified the client proof.
- Plain fallback verifier dispatch: try stock `SPEND_VK`, then fallback to `SPEND_SPLIT_VK`. This made nodes accept split proofs, but it did not carry or verify the extra client-proof linkage needed for sound split admission.
- Keeping `coinCommitment` as a wallet/client-proved public output. This was moved into the server `spendSplitUser` circuit so the server proof computes canonical `H(coin, pk)` and checks it against the Merkle path leaf.
- Using `skCommitment` as a placeholder witness in the server split circuit. It pinned almost nothing by itself, so the design shifted to explicit shared public inputs: `pk`, `nullifier`, `coinBindingTag`, and later `C_sk`.
- Using a split-only Poseidon nullifier. It improved client proof cost, but broke stock-vs-split double-spend semantics because normal wallet spends use the canonical zswap nullifier. The fix was to make split spends prove the canonical wallet nullifier.
- Recomputing `pk = H_persistent(sk)` inside every per-spend client proof. This was sound but too expensive; the client proof became almost as heavy as the server proof. It was replaced by a one-time wallet attestation plus a cheaper per-spend opening.
- A split bundle without wallet attestation. It could verify per-spend derivation, but did not cheaply bind the per-spend `sk` back to the canonical `pk = H(sk)` without paying that hash every spend. The next direct-bundle iteration added `attestation_proof`.
- A Pedersen-style `C_sk` commitment. It was considered for the wallet attestation link, but was much more expensive than the Poseidon/transient-hash commitment in Compact, so the PoC moved to Poseidon `C_sk`.
- Treating `sk` as a single field/scalar for commitment. That risks scalar-reduction ambiguity, so the final proof-side design uses an injective limb encoding of the 32-byte secret before committing and checking it.
- A recursive wrapper over unchanged Blake2b-transcript direct split proofs. The wrapper circuit could be built, but `VerifierGadget` uses Poseidon transcript challenges, so the in-circuit accumulator did not match the off-circuit Blake2b proof accumulator. The branch now regenerates/proves inner material with the Poseidon transcript instead.
