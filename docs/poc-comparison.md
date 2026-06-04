# Split-Prove: Three Approaches

**Goal:** outsource the heavy zswap spend proof to a server without handing it the wallet's `sk`.
All three approaches do this. They differ in **what the wallet leaks on the wire**.

| | **Direct** | **Registry** | **Recursive** *(abandoned)* |
|---|---|---|---|
| Wire identity | `pk`, `C_sk` (stable) | `registry_root` (membership) | none (looks like normal spend) |
| Extra on-chain state | none | registry contract at genesis | none |
| Per-chain setup | none | one `register(leaf)` per wallet | none |
| Privacy | **linkable** by `pk`/`C_sk` | anonymity-set membership | byte-indistinguishable |
| Per-spend server cost | baseline | baseline + Merkle path | **~192s (~235× wallet)** |

---

## Direct — simplest, weakest privacy
Three proofs: `π_attest` (once, binds `sk` → `pk`, `C_sk`), `π_sk` (per spend, nullifier + tag),
`π_spend` (server). Node verifies all three and checks shared public inputs line up.

- **+** No extra contract, no registration, easy to reason about.
- **−** `pk` and `C_sk` are on the wire → repeated spends from the same wallet are trivially linkable.

## Registry — stronger privacy, more machinery
Wallet registers a leaf bound to `sk` **once per chain**. Per-spend client proof shows
*membership in the tree rooted at `registry_root`* instead of publishing identity. Node admits
the spend if `registry_root` is in the contract's historic-roots set.

- **+** `pk`/`C_sk` never on wire; wallet hides in anonymity set; historic roots → in-flight spends
  survive concurrent registrations.
- **−** Genesis-deployed contract + permissionless registration (anonymity set, **not** an auth gate);
  spends still bucketable by which root they anchored to until wallets converge.

## Recursive wrapper — perfect privacy, prohibitive cost
Server proves a `π_wrap` that recursively verifies `π_attest + π_sk + π_spend` and exposes
only normal spend fields. Chain sees an ordinary zswap spend.

**It worked end-to-end on `recursive-privacy-wrapper`.** Abandoned for two reasons:

1. **~192,755 ms (~3.2 min) per server-side spend vs. 818 ms wallet proof — ~235×.**
2. **Forces a chain-wide proof-format migration.** Midnight's `VerifierGadget` uses a Poseidon
   Fiat-Shamir transcript; stock proofs use Blake2b. Wrapping stock proofs directly fails
   (accumulator mismatch) — every inner proof must be regenerated with Poseidon, breaking
   compatibility across ledger/prover/node/indexer.

Registry is the pivot: same wire-level identity removal at a fraction of the cost, trading
perfect indistinguishability for anonymity-set membership.
