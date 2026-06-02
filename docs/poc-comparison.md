# Split-Prove: Two PoCs Compared

Both proofs-of-concept solve the same core problem: let a Midnight wallet **outsource the
expensive zswap spend proof to a proof server without ever handing that server the raw zswap
secret key (`sk`)**. The wallet proves the small `sk`-dependent part locally; the server proves
the heavy spend relation from non-secret handoff data; a patched node verifies everything and
admits the split spend.

They differ in **how the wallet's identity is exposed on-chain**, which is the privacy crux of
the design. Each lives on its own branch:

| | **Direct bundle** (`direct-split-bundle`) | **Registry bundle** (`registry-split-bundle`) |
|---|---|---|
| Identity on the wire | Stable per-wallet values: `pk`, `C_sk` | A `registry_root` only (membership, not identity) |
| Extra on-chain state | None | A `wallet_registry` contract deployed at genesis |
| Extra per-chain step | None | One `register(reg_leaf)` call per wallet, per chain |
| Public bundle fields | `π_attest`, `π_sk`, `π_spend`, `pk`, `C_sk`, `nullifier`, `coinBindingTag`, `coinCommitment` | `spend_proof`, `client_derivation_proof`, `coin_binding_tag`, `registry_root` |
| Node admission check | Verify 3 proofs + shared public inputs match | Verify client + server proofs + `registry_root` is a historic root the registry contract held |

---

## Direct bundle — what it accomplishes

Three proofs carry the spend:

- **`π_attest`** (once, at wallet setup) — knows `sk`, derives `pk = H(sk)`, and commits to the
  same `sk` in `C_sk` with wallet-only randomness `r`.
- **`π_sk`** (per spend) — knows the `sk` committed in `C_sk`, derives the canonical zswap
  nullifier from `sk` + coin, and derives the coin-binding tag.
- **`π_spend`** (per spend, on the server) — coin commitment, Merkle membership, nullifier
  insertion, value commitment, spend rules.

The node verifies all three and checks the shared public values (`pk`, `C_sk`, `nullifier`,
`coinBindingTag`, `coinCommitment`) line up. The key optimization: the costly `pk = H(sk)`
relation runs **once** in the attestation, not on every spend — per-spend the wallet only opens
the cheap Poseidon `C_sk` commitment and proves the nullifier relation.

### Pros
- **Simplest** path to the goal: no extra on-chain contract, no registration step, no per-chain
  setup beyond funding.
- Self-contained — admission is a pure proof + public-input check, easy to reason about.
- Fewer moving parts to break in the local stack.

### Cons
- **Weak privacy.** The envelope exposes stable, wallet-linked values (`pk`, `C_sk`) in the
  clear, so repeated split spends from the same wallet are trivially **linkable** to each other.
- Identity material is structurally on the wire — fixing privacy means changing the wire format,
  not just a policy knob.

---

## Registry bundle — what it accomplishes

Adds a `wallet_registry` contract (deployed at genesis). Each wallet computes a registration leaf
`reg_leaf` bound to its `sk` and calls `register(reg_leaf)` **once per chain**. From then on, the
client proof shows membership: *"my `reg_leaf` is in the tree rooted at `registry_root`"* — instead
of publishing `pk`/`C_sk`. The wire shape collapses to `spend_proof`,
`client_derivation_proof`, `coin_binding_tag`, `registry_root`.

The node checks `registry_root` against the configured `split_registry_contract`: it admits the
spend if that root is a member of the contract's **historic-roots** set (the same membership the
in-circuit `checkRoot` enforces). A proof built against root `R` stays valid even after later
registrations advance the current root past `R`.

### Pros
- **Stronger privacy.** `pk` and `C_sk` never hit the wire; the wallet hides inside the
  registry's anonymity set, so spends are no longer linkable by identity values.
- **Liveness decoupled** from concurrent registrations — any historic root is accepted, so
  in-flight spends aren't invalidated when others register.
- Gives a real **policy/anchor point** (the registry contract) for future controls.

### Cons
- **More machinery**: a genesis-deployed contract, a fixed maintenance authority + deploy nonce,
  a per-chain `register-wallet` step, and registry-root validation in admission.
- **Registration is permissionless** — `register(leaf)` takes any 32-byte value with no proof, so
  the registry is an anonymity-set/anchor, **not** a spend-authorization gate. (Spend safety still
  rests entirely on the canonical-nullifier + coin-commitment chain, same as the direct bundle.)
- **Timing-tag privacy remains open**: the wallet still emits the *current* root, so spends are
  bucketable by which root they anchored to until wallets converge on a shared checkpoint root.
- Heavier per-spend client proof (registry Merkle path) and more local state to manage.
