# Recursive Privacy Wrapper

The recursive privacy wrapper is the current split-prove admission model. It keeps the expensive spend proof on the proof server, keeps the raw wallet secret key off the server, and stops publishing split-only linkage values in the ledger-visible proof envelope.

## What Changed

The previous direct split bundle made node admission verify the wallet attestation proof, client derivation proof, and server spend-split proof as separate public bundle members. That closed the soundness gap where the ledger had to trust the proof server's off-chain pre-verification, but it left stable split-only linkage values visible in the proof envelope.

The wrapper changes the ledger boundary. The proof server still receives the handoff values it needs to build the spend proof, but after proving `spend-split` it creates one recursive wrapper proof that verifies all three inner proofs:

- `pi_attest`: wallet-local, usually one time per key. It proves `pk = H(sk)` and `C_sk = H_transient(sk, r)` without sending `sk` or `r` to the server.
- `pi_sk`: wallet-local, per spend. It proves the wallet knows the same committed `sk`, derives the canonical zswap nullifier, and derives the coin-binding tag.
- `pi_spend`: proof-server-local, per spend. It proves coin commitment, Merkle membership, nullifier insertion, value commitment, and spend rules.
- `pi_wrap`: proof-server-local, per spend. It recursively verifies the three inner proofs and exposes only the normal spend admission fields.

The patched node now verifies `pi_wrap` plus its aggregate accumulator. It no longer receives `pk`, `C_sk`, `coinBindingTag`, or `coinCommitment` as public split-envelope fields.

## Public And Private Boundaries

The current handoff to `/v2/prove-split-spend` still includes:

- `coinBindingTag`;
- `nullifier`;
- `pk`;
- `commitmentHash` / `coinCommitment`;
- coin value, token type, nonce, Merkle index, and optional contract address;
- `clientDerivationProof`;
- `attestedCommitmentSk`;
- `attestationProof`.

The handoff does not include:

- raw `sk`;
- wallet-attestation blinding `r`.

The ledger-visible wrapper public inputs are only:

- Merkle root;
- nullifier;
- value commitment;
- contract address/none;
- segment;
- aggregate accumulator data needed by verifier admission.

This means the recursive wrapper improves ledger/public privacy, not proof-server privacy. The proof server still sees the current handoff metadata and should still be treated as a service that learns split-spend preimage metadata, just not the raw key material.

## Why The Wrapper Exists

The direct bundle design had the right ledger soundness shape: admission verified the client proof instead of trusting the server endpoint. Its privacy problem was that linkage values used only to stitch the split proofs together were also public ledger fields.

The wrapper turns those linkage values into private wrapper witnesses. The node sees one proof that they were consistent across the wallet attestation, client derivation, and spend-split statements, but does not learn those values from the envelope.

The normal double-spend invariant is unchanged. The client proof derives the canonical zswap nullifier, so a stock spend and a split spend of the same coin collide in the same nullifier set.

## Measured E2E Result

Latest live preview e2e:

| Measurement | Result |
|---|---:|
| Client derivation proof, local | 818 ms |
| Remote server recursive proving path | 192,755 ms |
| Split-prove proof-only total | 193,573 ms |
| Server/client proving ratio | 235.64x |
| Handoff non-proving overhead | 66 ms |
| Transaction assembly, output proof, Dust balance, submit | 22,155 ms |
| Full demo wall-clock, scan to finalized tx | 216,810 ms |

The transaction finalized on-chain at block hash `0x1dc62a7f92fecceabc5a41364b5498577cf1adf14329e3c24c9301db9ce0e76d`.

This result is a different cost profile from the earlier direct split bundle. The wallet proof remains small because `pk = H(sk)` is paid once in wallet attestation and each spend opens `C_sk` with the cheaper Poseidon/transient-hash relation. The benchmark now labels the server-side number as recursive proving because it includes both the `spend-split` proof and the privacy wrapper proof. That is the cost that shifts heavily to the server.

## Tradeoffs

**Ledger privacy improves.** Stable split-only linkage values stop appearing in ledger/public proof envelopes.

**Proof-server visibility remains.** The proof server still sees the handoff metadata needed to construct the spend proof. The wrapper is not an MPC or server-blind proving protocol.

**Server recursive proving cost increases sharply.** The latest live run shows about 193 seconds of remote proving versus 818 ms of local per-spend wallet proving. This is acceptable for the current proof-of-concept boundary, but it is the main operational cost to reduce before production use.

**Compatibility changes.** Midnight's recursive `VerifierGadget` uses Poseidon transcript challenges. The wrapper therefore requires Poseidon-transcript inner proof material and regenerated verifier artifacts. Existing Blake2b-transcript direct split proof bytes are not wrapper-compatible.

**Patched infrastructure is mandatory.** Stock preview nodes and indexers do not know how to decode the wrapper envelope, verify the wrapper proof, or check the aggregate accumulator. The local node, indexer, ledger, and proof server must be rebuilt from the matching patched branches.

**Verifier artifacts become coupled.** Changing the wallet attestation circuit, client derivation circuit, spend-split circuit, wrapper relation, or transcript setting requires regenerating the corresponding verifier artifacts and rebuilding the services that embed them.

## Current Open Engineering Work

- Reduce recursive wrapper proving time and memory footprint.
- Replace full `zswapState` handoff with the smallest production-safe Merkle witness shape, if that remains the desired API.
- Move preview-client wallet logic into the real wallet/client integration point.
- Harden endpoint validation, request sizing, and service-level abuse controls.
- Define a migration strategy if the production stack cannot standardize on Poseidon-transcript proof material.
