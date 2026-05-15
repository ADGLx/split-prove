//! Wallet attestation circuit for split-prove v3.
//!
//! One-time per-wallet proof binding the canonical Zswap public key
//! `pk = persistentHash("midnight:zswap-pk[v1]", sk)` to a Poseidon
//! commitment `C_sk = transientHash("midnight:sk-commit[v1]", sk, r)`.
//!
//! The per-spend `sk_prove` circuit (see `src/client.rs`) then *opens* `C_sk`
//! to recover `sk` instead of paying SHA-256(sk) on every spend. The Rust
//! admission verifier in `deps/midnight-ledger/zswap/src/verify.rs` cross-
//! checks the attestation's `(pk, C_sk)` public outputs against the per-spend
//! proof's `(pk, C_sk)` public outputs.
//!
//! See `/Users/adgl/.claude/plans/currently-split-prove-works-and-tingly-breeze.md`
//! and `docs/spike-results.md` for the design rationale and prover-key
//! measurements that made Poseidon the chosen commitment primitive.

use midnight_base_crypto::hash::HashOutput;
use midnight_coin_structure::coin::SecretKey as CoinSecretKey;
use midnight_onchain_runtime::ops::Op;
use midnight_onchain_runtime::program_fragments::Cell_write;
use midnight_onchain_runtime::result_mode::ResultModeVerify;
use midnight_onchain_runtime::state::StateValue;
use midnight_storage::arena::Sp;
use midnight_storage::db::InMemoryDB;
use midnight_transient_crypto::curve::Fr;
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

/// Result of a one-time wallet attestation. Wallet persists this locally and
/// attaches `attestation_proof_bytes` to every split-send bundle it emits.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WalletAttestation {
    /// Canonical Zswap public key: `pk = persistentHash("midnight:zswap-pk[v1]", sk)`.
    pub pk: [u8; 32],

    /// Poseidon commitment to `sk`:
    /// `C_sk = transientHash("midnight:sk-commit[v1]", sk, r)` as an Fr,
    /// little-endian 32-byte serialization.
    pub commitment_sk: [u8; 32],

    /// Blinding factor used in the commitment. Stays on the wallet — *never*
    /// sent to the server. Re-supplied to the per-spend circuit as a private
    /// witness so it can recompute `C_sk` from `(sk, r)`.
    pub blinding: Fr,

    /// Hex-encoded serialized proof produced by the attestation circuit.
    /// Carried verbatim in every split-send bundle.
    pub attestation_proof: String,
}

/// Public-input cell indices that the attestation circuit discloses.
/// These mirror the `ledger publicKey` / `ledger commitmentSk` declarations
/// at the top of `circuits/wallet_attestation.compact`.
const CELL_PK: u8 = 0;
const CELL_COMMITMENT_SK: u8 = 1;

/// Build the `ProofPreimage` for the attestation circuit. Inputs (in field-repr
/// order) are: `sk` (2 Fr limbs) + `r` (1 Fr).
pub fn build_wallet_attestation_preimage(
    sk: &CoinSecretKey,
    r: Fr,
    pk: [u8; 32],
    commitment_sk: [u8; 32],
) -> ProofPreimage {
    let mut inputs = Vec::new();
    sk.0 .0.field_repr(&mut inputs);
    inputs.push(r);

    ProofPreimage {
        inputs,
        private_transcript: Vec::new(),
        public_transcript_inputs: wallet_attestation_public_transcript_inputs(pk, commitment_sk),
        public_transcript_outputs: Vec::new(),
        binding_input: 0.into(),
        communications_commitment: None,
        key_location: KeyLocation(Cow::Borrowed(WALLET_ATTESTATION_KEY_LOCATION)),
    }
}

/// Encode the attestation's public outputs (`pk`, `C_sk`) as the on-chain
/// transcript cells. The verifier reconstructs the same encoding to compare
/// against the proof's claimed outputs.
pub fn wallet_attestation_public_transcript_inputs(
    pk: [u8; 32],
    commitment_sk: [u8; 32],
) -> Vec<Fr> {
    use midnight_coin_structure::coin::PublicKey as CoinPublicKey;
    let pk_typed = CoinPublicKey(HashOutput(pk));
    let c_sk_fr = Fr::from_le_bytes(&commitment_sk).expect("valid Fr from commitment_sk bytes");

    let mut inputs = Vec::new();
    extend_ops(
        &mut inputs,
        Cell_write!(
            [midnight_onchain_runtime::ops::Key::Value(CELL_PK.into())],
            false,
            CoinPublicKey,
            pk_typed
        ),
    );
    extend_ops(
        &mut inputs,
        Cell_write!(
            [midnight_onchain_runtime::ops::Key::Value(
                CELL_COMMITMENT_SK.into()
            )],
            false,
            Fr,
            c_sk_fr
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
/// `compactc 0.31.0`.
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

/// Off-circuit derivation of `(pk, C_sk)` from `(sk, r)`, byte-identical to
/// what the attestation circuit computes in-circuit. Used by the wallet to
/// build the preimage *before* running the prover, and by tests to construct
/// expected values.
///
/// `pk` matches `coin_info.public_key()` (canonical Zswap derivation).
/// `C_sk` is the Poseidon transient hash of `(sep, sk_lo, sk_hi, r)` where
/// `(sk_lo, sk_hi)` are the 8-bit / 248-bit limbs Compact uses internally
/// for `Bytes<32>` — same shared bit witnesses as the in-circuit gadget.
pub fn derive_attestation_outputs(sk: &CoinSecretKey, r: Fr) -> ([u8; 32], [u8; 32]) {
    use midnight_transient_crypto::hash::transient_hash;

    let pk = sk.public_key().0 .0;

    // Same field decomposition compactc emits for `Bytes<32>`: 8-bit limb +
    // 248-bit limb, both little-endian. See the constrain_bits emissions in
    // `circuits/static/wallet-attestation/wallet_attest.zkir`.
    let mut sk_limbs = Vec::new();
    sk.0 .0.field_repr(&mut sk_limbs);
    debug_assert_eq!(sk_limbs.len(), 2, "Bytes<32> should produce 2 Fr limbs");

    // Domain separator: ASCII "midnight:sk-commit[v1]" — exact bytes the
    // attestation circuit loads via `load_imm`. Treated as a Field
    // little-endian, matching the Compact `as Field` lowering.
    let sep = ascii_to_fr_le("midnight:sk-commit[v1]");

    let c_sk_fr = transient_hash(&[sep, sk_limbs[0], sk_limbs[1], r]);
    let c_sk_bytes = c_sk_fr
        .0
        .to_bytes_le()
        .try_into()
        .expect("Fr le bytes is 32-byte");

    (pk, c_sk_bytes)
}

fn ascii_to_fr_le(s: &str) -> Fr {
    let bytes = s.as_bytes();
    assert!(bytes.len() <= 32, "domain separator too long for one Fr");
    let mut buf = [0u8; 32];
    buf[..bytes.len()].copy_from_slice(bytes);
    Fr::from_le_bytes(&buf).expect("ascii fits in Fr")
}

/// Generate a fresh wallet attestation. Run **once** at wallet setup. Stores
/// the returned `WalletAttestation` on the wallet — the `blinding` is the
/// secret half of the commitment and never leaves the wallet.
pub async fn register_wallet(
    sk: &CoinSecretKey,
    resolver: impl ParamsProverProvider + Resolver,
) -> Result<WalletAttestation, midnight_transient_crypto::proofs::ProvingError> {
    let r: Fr = OsRng.r#gen();
    let (pk, commitment_sk) = derive_attestation_outputs(sk, r);

    let preimage = build_wallet_attestation_preimage(sk, r, pk, commitment_sk);
    let (proof, _) = preimage
        .prove::<midnight_zkir::IrSource>(OsRng, &resolver, &resolver)
        .await?;
    let attestation_proof = hex::encode(proof.0);

    Ok(WalletAttestation {
        pk,
        commitment_sk,
        blinding: r,
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
    fn derive_outputs_matches_compact_ir() {
        let sk = CoinSecretKey(OsRng.gen::<HashOutput>());
        let r: Fr = OsRng.r#gen();
        let (pk, commitment_sk) = derive_attestation_outputs(&sk, r);

        let preimage = build_wallet_attestation_preimage(&sk, r, pk, commitment_sk);
        let ir = midnight_serialize::tagged_deserialize::<midnight_zkir::IrSource>(Cursor::new(
            include_bytes!("../circuits/static/wallet-attestation/wallet_attest.bzkir"),
        ))
        .expect("attestation IR should load");

        preimage
            .check(&ir)
            .expect("honest attestation should match attestation circuit");
    }

    #[test]
    fn tampered_pk_rejected() {
        let sk = CoinSecretKey(OsRng.gen::<HashOutput>());
        let r: Fr = OsRng.r#gen();
        let (mut pk, commitment_sk) = derive_attestation_outputs(&sk, r);
        pk[0] ^= 1; // flip a bit
        let preimage = build_wallet_attestation_preimage(&sk, r, pk, commitment_sk);
        let ir = midnight_serialize::tagged_deserialize::<midnight_zkir::IrSource>(Cursor::new(
            include_bytes!("../circuits/static/wallet-attestation/wallet_attest.bzkir"),
        ))
        .expect("attestation IR should load");

        assert!(
            preimage.check(&ir).is_err(),
            "tampered pk must not satisfy the attestation circuit"
        );
    }

    #[test]
    fn tampered_commitment_sk_rejected() {
        let sk = CoinSecretKey(OsRng.gen::<HashOutput>());
        let r: Fr = OsRng.r#gen();
        let (pk, mut commitment_sk) = derive_attestation_outputs(&sk, r);
        commitment_sk[0] ^= 1; // flip a bit
        let preimage = build_wallet_attestation_preimage(&sk, r, pk, commitment_sk);
        let ir = midnight_serialize::tagged_deserialize::<midnight_zkir::IrSource>(Cursor::new(
            include_bytes!("../circuits/static/wallet-attestation/wallet_attest.bzkir"),
        ))
        .expect("attestation IR should load");

        assert!(
            preimage.check(&ir).is_err(),
            "tampered C_sk must not satisfy the attestation circuit"
        );
    }

    #[test]
    fn scalar_reduction_collision_does_not_open_same_commitment() {
        // Plan / Verification §3 — use the actual Jubjub scalar modulus `q`.
        // A naive `sk_to_scalar(sk)` design would reduce both byte strings
        // below to scalar zero. The v3 Poseidon commitment hashes the exact
        // byte limbs, so `0` and `q` must produce different commitments.
        let sk = CoinSecretKey(HashOutput([0u8; 32]));
        let r: Fr = OsRng.r#gen();
        let (_pk, c_sk_honest) = derive_attestation_outputs(&sk, r);

        let sk_evil_bytes = JUBJUB_SCALAR_MODULUS_LE;
        assert_eq!(reduce_once_mod_jubjub_scalar(sk.0 .0), [0u8; 32]);
        assert_eq!(reduce_once_mod_jubjub_scalar(sk_evil_bytes), [0u8; 32]);

        let sk_evil = CoinSecretKey(HashOutput(sk_evil_bytes));
        let (_pk_evil, c_sk_evil) = derive_attestation_outputs(&sk_evil, r);

        assert_ne!(
            c_sk_honest, c_sk_evil,
            "a different 32-byte sk must produce a different Poseidon commitment"
        );
    }
}
