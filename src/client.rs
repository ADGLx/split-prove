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
/// Contains all sk-dependent derived values but NOT the raw sk.
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

    /// Serialized proof that binds sk to pk, nullifier, and coin_binding_tag.
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
    _blinding: Fr,
) -> ClientHandoff {
    let sender_evidence = if let Some(addr) = contract {
        SenderEvidence::Contract(addr)
    } else {
        SenderEvidence::User(Cow::Borrowed(sk))
    };

    // Derive pk
    let pk = sk.public_key();

    // Compute nullifier
    let coin_info = CoinInfo::from(coin);
    let nullifier = coin_info.nullifier(&sender_evidence);

    // Compute coin commitment
    let commitment_hash = coin_info.commitment(&Recipient::from(sender_evidence));

    let coin_binding_tag = midnight_zswap::split_coin_binding_tag(&coin_info, pk);

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

pub fn build_client_derivation_preimage(
    sk: &CoinSecretKey,
    _sk_blinding: Fr,
    _coin: &QualifiedCoinInfo,
    handoff: &ClientHandoff,
) -> ProofPreimage {
    let mut inputs = Vec::new();
    sk.0 .0.field_repr(&mut inputs);
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
    )
}

pub fn client_derivation_public_transcript_inputs_from_parts(
    pk: CoinPublicKey,
    nullifier: [u8; 32],
    coin_binding_tag: Fr,
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
}
