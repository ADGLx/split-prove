//! Client-side SDK for split proving.
//!
//! The client (wallet) holds the secret key and computes all sk-dependent
//! values. These are packaged into a `ClientHandoff` and sent to the server.
//! The server never sees the raw secret key.

use midnight_base_crypto::hash::HashOutput;
use midnight_coin_structure::coin::{
    Commitment, Info as CoinInfo, Nullifier, PublicKey as CoinPublicKey,
    QualifiedInfo as QualifiedCoinInfo, SecretKey as CoinSecretKey,
};
use midnight_coin_structure::contract::ContractAddress;
use midnight_coin_structure::transfer::{Recipient, SenderEvidence};
use midnight_onchain_runtime::ops::Op;
use midnight_onchain_runtime::program_fragments::Cell_write;
use midnight_onchain_runtime::result_mode::ResultModeVerify;
use midnight_onchain_runtime::state::StateValue;
use midnight_storage::arena::Sp;
use midnight_storage::db::InMemoryDB;
use midnight_transient_crypto::curve::Fr;
use midnight_transient_crypto::proofs::{
    KeyLocation, ParamsProver, ParamsProverProvider, Proof, ProofPreimage, ProvingKeyMaterial,
    Resolver,
};
use midnight_transient_crypto::repr::FieldRepr;
use rand::rngs::OsRng;
use rand::Rng;
use serde::{Deserialize, Serialize};
use std::borrow::Cow;

/// Data the client sends to the server for split proving.
/// Contains all sk-dependent derived values but NOT the raw sk and NOT
/// the per-wallet blinding `r` (which the wallet keeps to re-open `C_sk`).
///
/// v3 carries a one-time wallet attestation alongside the per-spend proof; the
/// node admission verifier cross-checks the attestation's `(pk, C_sk)` against
/// the per-spend proof's `(pk, C_sk)` to bind the chain
/// `pk = H(sk) ↔ C_sk = transientHash(sk, r) ↔ nullifier = H(coin, sk)`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClientHandoff {
    /// ZK-friendly tag binding the client proof and server split proof to the same coin.
    pub coin_binding_tag: [u8; 32],

    /// Nullifier = H(sk, coin_info)
    pub nullifier: [u8; 32],

    /// Public key = H(sk)
    pub pk: [u8; 32],

    /// Coin commitment = commit(pk, coin)
    pub commitment_hash: [u8; 32],

    /// Poseidon commitment `C_sk = transientHash("midnight:sk-commit[v1]", sk, r)`
    /// from the per-wallet attestation. Disclosed publicly by both the
    /// attestation proof and the per-spend proof; the admission verifier
    /// asserts byte-equality between the two and uses Poseidon binding to
    /// conclude that the per-spend `sk` is the same one the attestation bound
    /// to `pk`.
    #[serde(default)]
    pub attested_commitment_sk: [u8; 32],

    /// Hex-encoded serialized proof for `circuits/wallet_attestation.compact`,
    /// produced once at wallet setup. Carried verbatim in every split-send
    /// bundle so the admission verifier can recheck `pk = H(sk) ∧
    /// C_sk = transientHash(sk, r)` without consulting any ledger registry.
    #[serde(default)]
    pub attestation_proof: Option<String>,

    /// Coin value
    pub coin_value: u128,

    /// Coin color (token type)
    pub coin_color: [u8; 32],

    /// Coin nonce
    pub coin_nonce: [u8; 32],

    /// Merkle tree index of the coin
    pub mt_index: u64,

    /// Whether this is a contract-owned coin (Some(address)) or user-owned (None)
    pub contract_address: Option<[u8; 32]>,

    /// Serialized proof that binds sk to nullifier, coin_binding_tag, and the
    /// disclosed `pk` / `attested_commitment_sk` (the latter recomputed
    /// in-circuit from `(sk, r)` and compared by the admission verifier to the
    /// attestation's public output).
    #[serde(default)]
    pub client_derivation_proof: Option<String>,
}

pub const CLIENT_DERIVATION_KEY_LOCATION: &str = "split/client/sk-derivation";

pub struct ClientDerivationResolver<P> {
    pub params_and_fallback: P,
}

impl<P> ClientDerivationResolver<P> {
    pub fn new(params_and_fallback: P) -> Self {
        Self {
            params_and_fallback,
        }
    }
}

impl<P> Resolver for ClientDerivationResolver<P>
where
    P: Resolver + Sync,
{
    async fn resolve_key(&self, key: KeyLocation) -> std::io::Result<Option<ProvingKeyMaterial>> {
        if key.0.as_ref() == CLIENT_DERIVATION_KEY_LOCATION {
            Ok(Some(client_derivation_proving_data()))
        } else {
            self.params_and_fallback.resolve_key(key).await
        }
    }
}

impl<P> ParamsProverProvider for ClientDerivationResolver<P>
where
    P: ParamsProverProvider + Sync,
{
    async fn get_params(&self, k: u8) -> std::io::Result<ParamsProver> {
        self.params_and_fallback.get_params(k).await
    }
}

/// Prepare a client handoff for a shielded coin spend.
///
/// This is the ONLY function that touches the secret key.
/// It runs in ~100µs — no heavy crypto.
///
/// # Arguments
/// * `sk` - The coin secret key (NEVER sent to server)
/// * `coin` - The qualified coin info (value, type, nonce, mt_index)
/// * `contract` - If the coin is contract-owned, the contract address
pub fn client_prepare(
    sk: &CoinSecretKey,
    coin: &QualifiedCoinInfo,
    contract: Option<ContractAddress>,
) -> ClientHandoff {
    client_prepare_with_blinding(sk, coin, contract, OsRng.r#gen())
}

pub fn client_prepare_with_blinding(
    sk: &CoinSecretKey,
    coin: &QualifiedCoinInfo,
    contract: Option<ContractAddress>,
    blinding: Fr,
) -> ClientHandoff {
    let sender_evidence = if let Some(addr) = contract {
        SenderEvidence::Contract(addr)
    } else {
        SenderEvidence::User(Cow::Borrowed(sk))
    };

    // Derive pk
    let pk = sk.public_key();

    // Compute the canonical Zswap nullifier so split and non-split spends
    // collide in the same ledger nullifier set.
    let coin_info = CoinInfo::from(coin);
    let nullifier = split_nullifier(&coin_info, sk);

    // Compute coin commitment
    let commitment_hash = coin_info.commitment(&Recipient::from(sender_evidence));

    let coin_binding_tag = midnight_zswap::split_coin_binding_tag(&coin_info, pk);

    // v3: derive the Poseidon commitment `C_sk` from `(sk, blinding)` so the
    // per-spend circuit can disclose it. Soundness still flows through the
    // wallet attestation — the admission verifier insists `attested_commitment_sk`
    // matches whatever the wallet stamped here. Test-only callers that supply
    // a known blinding get a deterministic `attested_commitment_sk`; production
    // callers should use `client_prepare_with_attestation` so the value is
    // pinned to the registered attestation.
    let (_, attested_commitment_sk) =
        crate::attestation::derive_attestation_outputs(sk, blinding);

    // Serialize coin nonce
    let mut nonce_bytes = [0u8; 32];
    let nonce_fr_bytes = coin_info.nonce.0 .0;
    nonce_bytes.copy_from_slice(&nonce_fr_bytes);

    // Serialize the token type as its canonical 32-byte hash.
    let color_bytes = coin_info.type_.0 .0;

    ClientHandoff {
        coin_binding_tag: coin_binding_tag
            .0
            .to_bytes_le()
            .try_into()
            .unwrap_or([0u8; 32]),
        nullifier: nullifier.0 .0,
        pk: pk.0 .0,
        commitment_hash: commitment_hash.0 .0,
        attested_commitment_sk,
        attestation_proof: None,
        coin_value: {
            let mut v_fields = Vec::new();
            coin_info.value.field_repr(&mut v_fields);
            // value is stored as Fr, extract u128
            let bytes = v_fields
                .get(0)
                .map(|f| f.0.to_bytes_le())
                .unwrap_or([0u8; 32]);
            u128::from_le_bytes(bytes[..16].try_into().unwrap_or([0u8; 16]))
        },
        coin_color: color_bytes,
        coin_nonce: nonce_bytes,
        mt_index: coin.mt_index,
        contract_address: contract.map(|a| a.0 .0),
        client_derivation_proof: None,
    }
}

/// Build a fully-populated v3 handoff from a previously registered
/// `WalletAttestation`. The wallet generates the attestation once via
/// `attestation::register_wallet`, persists it, and then calls this on every
/// spend. The per-spend circuit's `commitmentSk` public output will equal the
/// attestation's `commitment_sk` byte-for-byte, and the bundle carries the
/// attestation proof verbatim so the node can re-check `pk = H(sk)` and
/// `C_sk = transientHash(sk, r)` without consulting any ledger state.
pub fn client_prepare_with_attestation(
    sk: &CoinSecretKey,
    coin: &QualifiedCoinInfo,
    contract: Option<ContractAddress>,
    attestation: &crate::attestation::WalletAttestation,
) -> ClientHandoff {
    let mut handoff = client_prepare_with_blinding(sk, coin, contract, attestation.blinding);
    // The blinding-derived C_sk should already equal attestation.commitment_sk;
    // we overwrite to make the byte-for-byte equality explicit and resilient
    // to any future drift in `derive_attestation_outputs` rounding.
    debug_assert_eq!(
        handoff.attested_commitment_sk, attestation.commitment_sk,
        "blinding does not reproduce attested C_sk — wallet state is inconsistent"
    );
    debug_assert_eq!(
        handoff.pk, attestation.pk,
        "sk does not derive the attested pk — wallet state is inconsistent"
    );
    handoff.attested_commitment_sk = attestation.commitment_sk;
    handoff.attestation_proof = Some(attestation.attestation_proof.clone());
    handoff
}

/// Compute the canonical Zswap nullifier off-circuit. Must produce the same
/// 32 bytes that both the stock Zswap spend circuit and `sk_prove` disclose.
pub fn split_nullifier(coin: &CoinInfo, sk: &CoinSecretKey) -> Nullifier {
    coin.nullifier(&SenderEvidence::User(Cow::Borrowed(sk)))
}

/// Build the v3 client-derivation preimage. Witness layout matches the v3
/// `sk_prove` circuit declaration order — `(sk, pk, r, coin)` — which Compact
/// lowers into 10 field-element witnesses (sk: 2, pk: 2, r: 1, coin.nonce: 2,
/// coin.color: 2, coin.value: 1). The `sk_blinding` parameter is the same `r`
/// stored on the wallet from `attestation::register_wallet`.
pub fn build_client_derivation_preimage(
    sk: &CoinSecretKey,
    sk_blinding: Fr,
    _coin: &QualifiedCoinInfo,
    handoff: &ClientHandoff,
) -> ProofPreimage {
    let mut inputs = Vec::new();
    // Position 0..1: sk limbs (matches v3 circuit signature first parameter).
    sk.0 .0.field_repr(&mut inputs);
    // Position 2..3: pk limbs. pk is a private witness in v3 (was derived
    // from sk in v2). The admission verifier checks it against the attestation.
    handoff.pk.field_repr(&mut inputs);
    // Position 4: blinding r for the Poseidon C_sk open.
    inputs.push(sk_blinding);
    // Position 5..9: coin (nonce limbs, color limbs, value).
    handoff.coin_nonce.field_repr(&mut inputs);
    handoff.coin_color.field_repr(&mut inputs);
    inputs.push(Fr::from(handoff.coin_value));

    ProofPreimage {
        inputs,
        private_transcript: Vec::new(),
        public_transcript_inputs: client_derivation_public_transcript_inputs(handoff),
        public_transcript_outputs: Vec::new(),
        binding_input: 0.into(),
        communications_commitment: None,
        key_location: KeyLocation(Cow::Borrowed(CLIENT_DERIVATION_KEY_LOCATION)),
    }
}

pub async fn client_prove_derivation(
    preimage: &ProofPreimage,
    resolver: impl ParamsProverProvider + Resolver,
) -> Result<Proof, midnight_transient_crypto::proofs::ProvingError> {
    let (proof, _) = preimage
        .prove::<midnight_zkir::IrSource>(OsRng, &resolver, &resolver)
        .await?;
    Ok(proof)
}

pub fn client_derivation_proving_data() -> ProvingKeyMaterial {
    ProvingKeyMaterial {
        prover_key: include_bytes!("../circuits/static/client-derivation/sk_prove.prover").to_vec(),
        verifier_key: include_bytes!("../circuits/static/client-derivation/sk_prove.verifier")
            .to_vec(),
        ir_source: include_bytes!("../circuits/static/client-derivation/sk_prove.bzkir").to_vec(),
    }
}

pub fn client_derivation_public_transcript_inputs(handoff: &ClientHandoff) -> Vec<Fr> {
    client_derivation_public_transcript_inputs_from_parts(
        handoff_pk(handoff),
        handoff.nullifier,
        handoff_coin_binding_tag_fr(handoff),
        handoff_commitment_sk_fr(handoff),
    )
}

pub fn client_derivation_public_transcript_inputs_from_parts(
    pk: CoinPublicKey,
    nullifier: [u8; 32],
    coin_binding_tag: Fr,
    commitment_sk: Fr,
) -> Vec<Fr> {
    let mut inputs = Vec::new();
    extend_ops(
        &mut inputs,
        Cell_write!(
            [midnight_onchain_runtime::ops::Key::Value(0u8.into())],
            false,
            CoinPublicKey,
            pk
        ),
    );
    extend_ops(
        &mut inputs,
        Cell_write!(
            [midnight_onchain_runtime::ops::Key::Value(1u8.into())],
            false,
            [u8; 32],
            nullifier
        ),
    );
    extend_ops(
        &mut inputs,
        Cell_write!(
            [midnight_onchain_runtime::ops::Key::Value(2u8.into())],
            false,
            Fr,
            coin_binding_tag
        ),
    );
    // v3 cell 3 — Poseidon commitment `C_sk` opened by the per-spend proof.
    // Cross-checked against the attestation's `commitment_sk` in admission.
    extend_ops(
        &mut inputs,
        Cell_write!(
            [midnight_onchain_runtime::ops::Key::Value(3u8.into())],
            false,
            Fr,
            commitment_sk
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

/// Reconstruct the Fr `C_sk` (Poseidon commitment to sk) from the handoff bytes.
pub fn handoff_commitment_sk_fr(handoff: &ClientHandoff) -> Fr {
    Fr::from_le_bytes(&handoff.attested_commitment_sk)
        .expect("valid Fr from attested_commitment_sk bytes")
}

/// Reconstruct the Fr coin-binding tag from the handoff bytes.
pub fn handoff_coin_binding_tag_fr(handoff: &ClientHandoff) -> Fr {
    Fr::from_le_bytes(&handoff.coin_binding_tag).expect("valid Fr from coin binding tag bytes")
}

/// Reconstruct the Nullifier from the handoff bytes.
pub fn handoff_nullifier(handoff: &ClientHandoff) -> Nullifier {
    Nullifier(HashOutput(handoff.nullifier))
}

/// Reconstruct the PublicKey from the handoff bytes.
pub fn handoff_pk(handoff: &ClientHandoff) -> CoinPublicKey {
    CoinPublicKey(HashOutput(handoff.pk))
}

/// Reconstruct the Commitment from the handoff bytes.
pub fn handoff_commitment(handoff: &ClientHandoff) -> Commitment {
    Commitment(HashOutput(handoff.commitment_hash))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn client_prepare_produces_valid_handoff() {
        let sk = CoinSecretKey(OsRng.gen::<HashOutput>());
        let coin = QualifiedCoinInfo {
            value: 1000u64.into(),
            type_: Default::default(),
            nonce: OsRng.r#gen(),
            mt_index: 42,
        };

        let handoff = client_prepare(&sk, &coin, None);

        // Nullifier should be non-zero
        assert!(!handoff.nullifier.iter().all(|&b| b == 0));
        // PK should be non-zero
        assert!(!handoff.pk.iter().all(|&b| b == 0));
        // Commitment should be non-zero
        assert!(!handoff.commitment_hash.iter().all(|&b| b == 0));
        // coin_binding_tag should be non-zero
        assert!(!handoff.coin_binding_tag.iter().all(|&b| b == 0));
        // mt_index preserved
        assert_eq!(handoff.mt_index, 42);
    }

    #[test]
    fn handoff_serializes_to_json() {
        let sk = CoinSecretKey(OsRng.gen::<HashOutput>());
        let coin = QualifiedCoinInfo {
            value: 500u64.into(),
            type_: Default::default(),
            nonce: OsRng.r#gen(),
            mt_index: 0,
        };

        let handoff = client_prepare(&sk, &coin, None);
        let json = serde_json::to_string(&handoff).unwrap();
        let deserialized: ClientHandoff = serde_json::from_str(&json).unwrap();

        assert_eq!(handoff.nullifier, deserialized.nullifier);
        assert_eq!(handoff.pk, deserialized.pk);
        assert_eq!(handoff.coin_binding_tag, deserialized.coin_binding_tag);
        assert_eq!(handoff.mt_index, deserialized.mt_index);
    }

    #[test]
    fn client_prepare_preserves_token_type_bytes() {
        let sk = CoinSecretKey(OsRng.gen::<HashOutput>());
        let coin = QualifiedCoinInfo {
            value: 500u64.into(),
            type_: midnight_coin_structure::coin::ShieldedTokenType(HashOutput([9u8; 32])),
            nonce: OsRng.r#gen(),
            mt_index: 0,
        };

        let handoff = client_prepare(&sk, &coin, None);

        assert_eq!(handoff.coin_color, coin.type_.0 .0);
    }

    #[test]
    fn split_nullifier_matches_canonical_user_nullifier() {
        let sk = CoinSecretKey(OsRng.gen::<HashOutput>());
        let coin = QualifiedCoinInfo {
            value: 500u64.into(),
            type_: Default::default(),
            nonce: OsRng.r#gen(),
            mt_index: 0,
        };
        let coin_info = CoinInfo::from(&coin);

        assert_eq!(
            split_nullifier(&coin_info, &sk),
            coin_info.nullifier(&SenderEvidence::User(Cow::Borrowed(&sk)))
        );
    }

    #[test]
    fn client_derivation_preimage_checks_against_compact_ir() {
        let sk = CoinSecretKey(OsRng.gen::<HashOutput>());
        let blinding = OsRng.r#gen();
        let coin = QualifiedCoinInfo {
            value: 500u64.into(),
            type_: Default::default(),
            nonce: OsRng.r#gen(),
            mt_index: 0,
        };
        let handoff = client_prepare_with_blinding(&sk, &coin, None, blinding);
        let preimage = build_client_derivation_preimage(&sk, blinding, &coin, &handoff);
        let ir = midnight_serialize::tagged_deserialize::<midnight_zkir::IrSource>(Cursor::new(
            include_bytes!("../circuits/static/client-derivation/sk_prove.bzkir"),
        ))
        .expect("client derivation IR should load");

        preimage
            .check(&ir)
            .expect("honest handoff should match client derivation circuit");
    }

    #[test]
    fn client_derivation_preimage_rejects_tampered_nullifier() {
        let sk = CoinSecretKey(OsRng.gen::<HashOutput>());
        let blinding = OsRng.r#gen();
        let coin = QualifiedCoinInfo {
            value: 500u64.into(),
            type_: Default::default(),
            nonce: OsRng.r#gen(),
            mt_index: 0,
        };
        let mut handoff = client_prepare_with_blinding(&sk, &coin, None, blinding);
        handoff.nullifier[0] ^= 1;
        let preimage = build_client_derivation_preimage(&sk, blinding, &coin, &handoff);
        let ir = midnight_serialize::tagged_deserialize::<midnight_zkir::IrSource>(Cursor::new(
            include_bytes!("../circuits/static/client-derivation/sk_prove.bzkir"),
        ))
        .expect("client derivation IR should load");

        assert!(preimage.check(&ir).is_err());
    }

    #[test]
    fn repeated_prepare_produces_stable_binding_values() {
        let sk = CoinSecretKey(OsRng.gen::<HashOutput>());
        let coin = QualifiedCoinInfo {
            value: 100u64.into(),
            type_: Default::default(),
            nonce: OsRng.r#gen(),
            mt_index: 0,
        };

        let h1 = client_prepare(&sk, &coin, None);
        let h2 = client_prepare(&sk, &coin, None);

        // Same sk → same nullifier and pk
        assert_eq!(h1.nullifier, h2.nullifier);
        assert_eq!(h1.pk, h2.pk);
        assert_eq!(h1.coin_binding_tag, h2.coin_binding_tag);
    }

    // ─── v3 cross-check tests ────────────────────────────────────────────────
    //
    // These tests exercise the soundness chain implemented in
    // `deps/midnight-ledger/zswap/src/verify.rs::split_well_formed`. We do not
    // run the full node here; we exercise the in-circuit binding between the
    // per-spend `commitmentSk` public output and the wallet's blinding `r`.

    /// The v3 per-spend circuit's `commitmentSk` public output equals the
    /// off-circuit `transientHash` of the same `(sk, r)`. If a wallet's
    /// blinding becomes inconsistent with its registered commitment, the
    /// admission verifier will catch it because `commitmentSk` no longer
    /// matches the attestation's public output.
    #[test]
    fn v3_per_spend_commitment_matches_attestation_when_blinding_consistent() {
        let sk = CoinSecretKey(OsRng.gen::<HashOutput>());
        let blinding = OsRng.r#gen();
        let coin = QualifiedCoinInfo {
            value: 250u64.into(),
            type_: Default::default(),
            nonce: OsRng.r#gen(),
            mt_index: 7,
        };

        let handoff = client_prepare_with_blinding(&sk, &coin, None, blinding);
        let (_, expected_c_sk) =
            crate::attestation::derive_attestation_outputs(&sk, blinding);

        assert_eq!(
            handoff.attested_commitment_sk, expected_c_sk,
            "v3 handoff's C_sk must match the off-circuit derivation"
        );

        // And the preimage must satisfy the v3 IR with this `C_sk`.
        let preimage = build_client_derivation_preimage(&sk, blinding, &coin, &handoff);
        let ir = midnight_serialize::tagged_deserialize::<midnight_zkir::IrSource>(Cursor::new(
            include_bytes!("../circuits/static/client-derivation/sk_prove.bzkir"),
        ))
        .expect("v3 client derivation IR should load");
        preimage
            .check(&ir)
            .expect("honest v3 handoff should satisfy sk_prove_v3");
    }

    /// Mismatched blinding between the wallet's stored attestation and the
    /// per-spend witness must break the per-spend proof: the in-circuit
    /// `transientHash(sk, r_wallet_local)` will not equal the disclosed
    /// `attested_commitment_sk` and the admission verifier's cross-check
    /// will fire. The IR `check()` catches this without needing the full
    /// node admission path.
    #[test]
    fn v3_per_spend_circuit_rejects_blinding_mismatch_with_handoff() {
        let sk = CoinSecretKey(OsRng.gen::<HashOutput>());
        let registered_blinding: Fr = OsRng.r#gen();
        let coin = QualifiedCoinInfo {
            value: 100u64.into(),
            type_: Default::default(),
            nonce: OsRng.r#gen(),
            mt_index: 0,
        };

        let honest_handoff =
            client_prepare_with_blinding(&sk, &coin, None, registered_blinding);

        // Attacker tries to spend with a different blinding. The handoff still
        // carries the honest `attested_commitment_sk` (so the admission's
        // cross-check would pass), but the in-circuit `commitmentSk` will be
        // recomputed from `(sk, attacker_blinding)` and disagree.
        let attacker_blinding: Fr = OsRng.r#gen();
        assert_ne!(attacker_blinding, registered_blinding);

        let bad_preimage =
            build_client_derivation_preimage(&sk, attacker_blinding, &coin, &honest_handoff);
        let ir = midnight_serialize::tagged_deserialize::<midnight_zkir::IrSource>(Cursor::new(
            include_bytes!("../circuits/static/client-derivation/sk_prove.bzkir"),
        ))
        .expect("v3 client derivation IR should load");

        assert!(
            bad_preimage.check(&ir).is_err(),
            "v3 circuit must reject a blinding that does not match the disclosed C_sk"
        );
    }

    /// Plan §"Encoding pitfall" — the scalar-reduction attack `sk' = sk + n·q`
    /// is blocked by the *Poseidon commitment changing*, not by the limb range
    /// check. Demonstrate this at the unit-test layer: a different 32-byte
    /// `sk'` produces a different `commitmentSk`, so the per-spend circuit
    /// (which discloses `commitmentSk`) commits to a value the attestation
    /// did not register.
    #[test]
    fn v3_scalar_reduction_collision_does_not_open_same_commitment() {
        let sk_honest = CoinSecretKey(OsRng.gen::<HashOutput>());
        let blinding: Fr = OsRng.r#gen();

        let (_, c_sk_honest) =
            crate::attestation::derive_attestation_outputs(&sk_honest, blinding);

        // Same blinding, different sk byte string. (For the scalar-reduction
        // family `sk + n·q` we'd construct a specific overflow; here a
        // single-bit flip suffices to make the same point: different bytes
        // ⇒ different limbs ⇒ different Poseidon image.)
        let mut sk_evil_bytes = sk_honest.0 .0;
        sk_evil_bytes[31] ^= 0x80; // flip the top bit of the high limb
        let sk_evil = CoinSecretKey(HashOutput(sk_evil_bytes));
        let (_, c_sk_evil) =
            crate::attestation::derive_attestation_outputs(&sk_evil, blinding);

        assert_ne!(
            c_sk_honest, c_sk_evil,
            "scalar-reduction-style sk' must produce a different Poseidon C_sk"
        );
    }
}
