//! Wallet registration / attestation circuit for split-prove (Solution A).
//!
//! One-time per-wallet flow:
//!   1. Wallet draws `r` and `salt` uniformly at random.
//!   2. Computes `C_sk = transientHash("midnight:sk-commit[v1]", sk, r)`.
//!   3. Computes `reg_leaf = transientHash("midnight:wallet-reg[v1]", C_sk,
//!      salt)`.
//!   4. Runs the attestation circuit (this file's `register_wallet`) to
//!      sanity-check the derivation against the canonical Zswap `pk = H(sk)`
//!      witness binding.
//!   5. Builds a synthetic first-registration witness for the local POC.
//!   6. Persists locally:
//!         `WalletRegistration { r, salt, mt_index, leaf, merkle_path }`
//!      The `merkle_path` is generated client-side for local proving.
//!
//! Per-spend, the wallet feeds `(r, salt, merkle_path)` to the client circuit
//! (see `src/client.rs`). The per-spend proof opens `reg_leaf` to a
//! Merkle-path member; the admission verifier checks the resulting
//! `registry_root` through the host-installed registry-root checker. The local
//! proof-server/node/indexer binaries install a permissive checker.
//!
//! Public outputs of the *attestation* circuit shrink to a single
//! `reg_leaf` field — `pk` and `C_sk` are no longer disclosed. The chain of
//! soundness now flows through the registry-membership path (see
//! `circuits/sk_proof.compact` for the spend-side argument).

use midnight_coin_structure::coin::SecretKey as CoinSecretKey;
use midnight_onchain_runtime::ops::Op;
use midnight_onchain_runtime::program_fragments::Cell_write;
use midnight_onchain_runtime::result_mode::ResultModeVerify;
use midnight_onchain_runtime::state::StateValue;
use midnight_storage::arena::Sp;
use midnight_storage::db::InMemoryDB;
use midnight_transient_crypto::curve::Fr;
use midnight_transient_crypto::hash::{transient_hash, upgrade_from_transient};
use midnight_transient_crypto::proofs::{
    KeyLocation, ParamsProver, ParamsProverProvider, ProofPreimage, ProvingKeyMaterial, Resolver,
};
use midnight_transient_crypto::repr::FieldRepr;
use rand::rngs::OsRng;
use rand::Rng;
use serde::{Deserialize, Serialize};
use std::borrow::Cow;

/// Resolver key location for the one-time wallet attestation circuit.
pub const WALLET_ATTESTATION_KEY_LOCATION: &str = "split/wallet/attestation";

/// Result of a one-time wallet registration. The wallet persists this locally
/// (encrypted, alongside `sk`) and references it on every split spend. None of
/// `(r, salt)` ever leave the wallet.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WalletRegistration {
    /// Poseidon blinding for `C_sk = transientHash(sep_sk, sk, r)`.
    pub blinding: Fr,
    /// Per-registration blinding for
    /// `reg_leaf = transientHash(sep_reg, C_sk, salt)`. Without `salt` an
    /// observer who somehow learned `C_sk` could still link two
    /// registrations of the same wallet; `salt` protects against that and
    /// supports leaf rotation if a wallet ever needs it.
    pub salt: Fr,
    /// Registration leaf as a Field element. This is the value the
    /// attestation circuit discloses on cell 0.
    pub reg_leaf_fr: Fr,
    /// Hex-encoded attestation proof. Optional in Solution A — kept on the
    /// wallet as off-chain evidence that `reg_leaf` was correctly derived
    /// from `(sk, r, salt)`, but never submitted on-chain.
    pub attestation_proof: String,
}

impl WalletRegistration {
    /// The 32-byte form of `reg_leaf` the wallet submits to the registry
    /// contract via `register(leaf)`. Matches the in-circuit
    /// `upgradeFromTransient(regLeaf)` shape used in `sk_proof.compact` to
    /// cross-check the witnessed Merkle path's leaf.
    pub fn reg_leaf_bytes(&self) -> [u8; 32] {
        midnight_transient_crypto::hash::upgrade_from_transient(self.reg_leaf_fr).0
    }
}

/// Public-input cell index that the attestation circuit discloses. Mirrors
/// the single `ledger regLeaf: Field;` declaration in
/// `circuits/wallet_attestation.compact`.
const CELL_REG_LEAF: u8 = 0;

/// Build the `ProofPreimage` for the attestation circuit. Inputs (in
/// field-repr order) are: `sk` (2 Fr limbs) + `r` (1 Fr) + `salt` (1 Fr).
pub fn build_wallet_attestation_preimage(
    sk: &CoinSecretKey,
    r: Fr,
    salt: Fr,
    reg_leaf_fr: Fr,
) -> ProofPreimage {
    let mut inputs = Vec::new();
    sk.0 .0.field_repr(&mut inputs);
    inputs.push(r);
    inputs.push(salt);

    ProofPreimage {
        inputs,
        private_transcript: Vec::new(),
        public_transcript_inputs: wallet_attestation_public_transcript_inputs(reg_leaf_fr),
        public_transcript_outputs: Vec::new(),
        binding_input: 0.into(),
        communications_commitment: None,
        key_location: KeyLocation(Cow::Borrowed(WALLET_ATTESTATION_KEY_LOCATION)),
    }
}

/// Encode the attestation's public output (`reg_leaf`) as the on-chain
/// transcript cells. The verifier reconstructs the same encoding to compare
/// against the proof's claimed outputs.
pub fn wallet_attestation_public_transcript_inputs(reg_leaf_fr: Fr) -> Vec<Fr> {
    let mut inputs = Vec::new();
    extend_ops(
        &mut inputs,
        Cell_write!(
            [midnight_onchain_runtime::ops::Key::Value(
                CELL_REG_LEAF.into()
            )],
            false,
            Fr,
            reg_leaf_fr
        ),
    );
    inputs
}

fn extend_ops<const N: usize>(inputs: &mut Vec<Fr>, ops: [Op<ResultModeVerify, InMemoryDB>; N]) {
    for op in ops.into_iter().filter(|op| match op {
        Op::Idx { path, .. } => !path.is_empty(),
        Op::Ins { n, .. } => *n != 0,
        _ => true,
    }) {
        op.field_repr(inputs);
    }
}

/// Embedded artifacts for the attestation circuit. Mirror of
/// `circuits/static/wallet-attestation/wallet_attest.*` compiled by
/// `compactc`.
pub fn wallet_attestation_proving_data() -> ProvingKeyMaterial {
    ProvingKeyMaterial {
        prover_key: include_bytes!("../circuits/static/wallet-attestation/wallet_attest.prover")
            .to_vec(),
        verifier_key: include_bytes!(
            "../circuits/static/wallet-attestation/wallet_attest.verifier"
        )
        .to_vec(),
        ir_source: include_bytes!("../circuits/static/wallet-attestation/wallet_attest.bzkir")
            .to_vec(),
    }
}

/// Resolver that bundles the wallet-attestation proving artifact with the
/// client-derivation artifact already shipped by `client::ClientDerivationResolver`.
/// Falls through to the inner resolver for everything else.
pub struct WalletAttestationResolver<P> {
    pub inner: P,
}

impl<P> WalletAttestationResolver<P> {
    pub fn new(inner: P) -> Self {
        Self { inner }
    }
}

impl<P> Resolver for WalletAttestationResolver<P>
where
    P: Resolver + Sync,
{
    async fn resolve_key(&self, key: KeyLocation) -> std::io::Result<Option<ProvingKeyMaterial>> {
        if key.0.as_ref() == WALLET_ATTESTATION_KEY_LOCATION {
            Ok(Some(wallet_attestation_proving_data()))
        } else {
            self.inner.resolve_key(key).await
        }
    }
}

impl<P> ParamsProverProvider for WalletAttestationResolver<P>
where
    P: ParamsProverProvider + Sync,
{
    async fn get_params(&self, k: u8) -> std::io::Result<ParamsProver> {
        self.inner.get_params(k).await
    }
}

/// Off-circuit derivation of `(C_sk, reg_leaf_fr)` from `(sk, r, salt)`,
/// byte-identical to what the attestation and per-spend circuits compute
/// in-circuit.
///
/// `C_sk` is the Poseidon transient hash of `(sep_sk, sk_lo, sk_hi, r)` where
/// `(sk_lo, sk_hi)` are the 8-bit / 248-bit limbs Compact uses internally for
/// `Bytes<32>` — same shared bit witnesses as the in-circuit gadget.
///
/// `reg_leaf_fr = transientHash(sep_reg, C_sk, salt)` (the in-circuit `regLeaf`
/// field). `upgrade_from_transient(reg_leaf_fr)` is the 32-byte registry-leaf
/// form exposed by `WalletRegistration::reg_leaf_bytes`.
pub fn derive_reg_leaf(sk: &CoinSecretKey, r: Fr, salt: Fr) -> (Fr, Fr) {
    let mut sk_limbs = Vec::new();
    sk.0 .0.field_repr(&mut sk_limbs);
    debug_assert_eq!(sk_limbs.len(), 2, "Bytes<32> should produce 2 Fr limbs");

    let sep_sk = ascii_to_fr_le("midnight:sk-commit[v1]");
    let c_sk_fr = transient_hash(&[sep_sk, sk_limbs[0], sk_limbs[1], r]);

    let sep_reg = ascii_to_fr_le("midnight:wallet-reg[v1]");
    let reg_leaf_fr = transient_hash(&[sep_reg, c_sk_fr, salt]);
    (c_sk_fr, reg_leaf_fr)
}

/// Convenience: return the upgraded 32-byte registry-leaf form.
pub fn derive_reg_leaf_bytes(sk: &CoinSecretKey, r: Fr, salt: Fr) -> [u8; 32] {
    let (_, reg_leaf_fr) = derive_reg_leaf(sk, r, salt);
    upgrade_from_transient(reg_leaf_fr).0
}

fn ascii_to_fr_le(s: &str) -> Fr {
    let bytes = s.as_bytes();
    assert!(bytes.len() <= 32, "domain separator too long for one Fr");
    let mut buf = [0u8; 32];
    buf[..bytes.len()].copy_from_slice(bytes);
    Fr::from_le_bytes(&buf).expect("ascii fits in Fr")
}

/// Generate a fresh wallet registration. Run **once** at wallet setup. The
/// returned `WalletRegistration` carries the secrets `(r, salt)`, which must
/// be persisted on the wallet (and never leave it), plus the `reg_leaf` byte
/// representation used by the synthetic local POC registry witness.
///
/// The attestation proof attached to the result is kept locally on the wallet
/// as evidence that `reg_leaf` was correctly derived from `(sk, r, salt)`. In
/// Solution A this proof is not submitted with every spend; the local POC keeps
/// it client-side and uses a synthetic first-registration witness.
pub async fn register_wallet(
    sk: &CoinSecretKey,
    resolver: impl ParamsProverProvider + Resolver,
) -> Result<WalletRegistration, midnight_transient_crypto::proofs::ProvingError> {
    let r: Fr = OsRng.r#gen();
    let salt: Fr = OsRng.r#gen();
    let (_c_sk, reg_leaf_fr) = derive_reg_leaf(sk, r, salt);

    let preimage = build_wallet_attestation_preimage(sk, r, salt, reg_leaf_fr);
    let (proof, _) = preimage
        .prove::<midnight_zkir::IrSource>(OsRng, &resolver, &resolver)
        .await?;
    let attestation_proof = hex::encode(proof.0);

    Ok(WalletRegistration {
        blinding: r,
        salt,
        reg_leaf_fr,
        attestation_proof,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use midnight_base_crypto::hash::HashOutput;
    use std::io::Cursor;

    const JUBJUB_SCALAR_MODULUS_LE: [u8; 32] = [
        0xb7, 0x2c, 0xf7, 0xd6, 0x5e, 0x0e, 0x97, 0xd0, 0x82, 0x10, 0xc8, 0xcc, 0x93, 0x20, 0x68,
        0xa6, 0x00, 0x3b, 0x34, 0x01, 0x01, 0x3b, 0x67, 0x06, 0xa9, 0xaf, 0x33, 0x65, 0xea, 0xb4,
        0x7d, 0x0e,
    ];

    fn reduce_once_mod_jubjub_scalar(mut bytes: [u8; 32]) -> [u8; 32] {
        if le_bytes_ge(&bytes, &JUBJUB_SCALAR_MODULUS_LE) {
            let mut borrow = 0u16;
            for (byte, modulus_byte) in bytes.iter_mut().zip(JUBJUB_SCALAR_MODULUS_LE) {
                let lhs = *byte as i16 - borrow as i16;
                if lhs >= modulus_byte as i16 {
                    *byte = (lhs - modulus_byte as i16) as u8;
                    borrow = 0;
                } else {
                    *byte = (lhs + 256 - modulus_byte as i16) as u8;
                    borrow = 1;
                }
            }
            debug_assert_eq!(borrow, 0);
        }
        bytes
    }

    fn le_bytes_ge(lhs: &[u8; 32], rhs: &[u8; 32]) -> bool {
        lhs.iter()
            .zip(rhs.iter())
            .rev()
            .find_map(|(a, b)| (a != b).then_some(a > b))
            .unwrap_or(true)
    }

    #[test]
    fn derive_reg_leaf_is_deterministic() {
        let sk = CoinSecretKey(OsRng.gen::<HashOutput>());
        let r: Fr = OsRng.r#gen();
        let salt: Fr = OsRng.r#gen();

        let (c1, l1) = derive_reg_leaf(&sk, r, salt);
        let (c2, l2) = derive_reg_leaf(&sk, r, salt);
        assert_eq!(c1, c2);
        assert_eq!(l1, l2);
    }

    #[test]
    fn derive_reg_leaf_distinguishes_salt() {
        let sk = CoinSecretKey(OsRng.gen::<HashOutput>());
        let r: Fr = OsRng.r#gen();
        let s1: Fr = OsRng.r#gen();
        let s2: Fr = OsRng.r#gen();
        assert_ne!(s1, s2);

        let (_, l1) = derive_reg_leaf(&sk, r, s1);
        let (_, l2) = derive_reg_leaf(&sk, r, s2);
        assert_ne!(
            l1, l2,
            "different salt must yield different reg_leaf even with same (sk, r)"
        );
    }

    #[test]
    fn derive_reg_leaf_matches_compact_ir() {
        let sk = CoinSecretKey(OsRng.gen::<HashOutput>());
        let r: Fr = OsRng.r#gen();
        let salt: Fr = OsRng.r#gen();
        let (_c_sk, reg_leaf_fr) = derive_reg_leaf(&sk, r, salt);

        let preimage = build_wallet_attestation_preimage(&sk, r, salt, reg_leaf_fr);
        let ir = midnight_serialize::tagged_deserialize::<midnight_zkir::IrSource>(Cursor::new(
            include_bytes!("../circuits/static/wallet-attestation/wallet_attest.bzkir"),
        ))
        .expect("attestation IR should load");

        preimage
            .check(&ir)
            .expect("honest attestation should match attestation circuit");
    }

    #[test]
    fn scalar_reduction_collision_does_not_open_same_commitment() {
        // The Poseidon commitment hashes the exact byte limbs of `sk`. A
        // different 32-byte string `sk'` that is congruent to `sk` mod the
        // Jubjub scalar order must still produce a different `reg_leaf`.
        let sk = CoinSecretKey(HashOutput([0u8; 32]));
        let r: Fr = OsRng.r#gen();
        let salt: Fr = OsRng.r#gen();
        let (_, reg_leaf_honest) = derive_reg_leaf(&sk, r, salt);

        let sk_evil_bytes = JUBJUB_SCALAR_MODULUS_LE;
        assert_eq!(reduce_once_mod_jubjub_scalar(sk.0 .0), [0u8; 32]);
        assert_eq!(reduce_once_mod_jubjub_scalar(sk_evil_bytes), [0u8; 32]);

        let sk_evil = CoinSecretKey(HashOutput(sk_evil_bytes));
        let (_, reg_leaf_evil) = derive_reg_leaf(&sk_evil, r, salt);

        assert_ne!(
            reg_leaf_honest, reg_leaf_evil,
            "a different 32-byte sk must produce a different reg_leaf"
        );
    }
}
