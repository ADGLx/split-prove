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
use midnight_transient_crypto::curve::Fr;
use midnight_transient_crypto::hash::transient_hash;
use midnight_transient_crypto::repr::FieldRepr;
use rand::rngs::OsRng;
use rand::Rng;
use serde::{Deserialize, Serialize};
use std::borrow::Cow;

/// Data the client sends to the server for split proving.
/// Contains all sk-dependent derived values but NOT the raw sk.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClientHandoff {
    /// Commitment to sk: H(sk, blinding) — server can't extract sk.
    pub sk_commitment: [u8; 32],

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

    // Commit to sk (hiding commitment)
    let blinding: Fr = OsRng.r#gen();
    let mut sk_fields = Vec::new();
    sk.field_repr(&mut sk_fields);
    let sk_commitment = transient_hash(&[sk_fields[0], blinding]);

    // Serialize coin nonce
    let mut nonce_bytes = [0u8; 32];
    let nonce_fr_bytes = coin_info.nonce.0 .0;
    nonce_bytes.copy_from_slice(&nonce_fr_bytes);

    // Serialize coin color
    let mut color_bytes = [0u8; 32];
    let mut color_fields = Vec::new();
    coin_info.type_.field_repr(&mut color_fields);
    // color is a complex type — just store the first field element bytes
    color_bytes.copy_from_slice(
        &color_fields
            .get(0)
            .map(|f| f.0.to_bytes_le())
            .unwrap_or([0u8; 32]),
    );

    ClientHandoff {
        sk_commitment: sk_commitment
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
    }
}

/// Reconstruct the Fr commitment value from the handoff bytes.
pub fn handoff_sk_commitment_fr(handoff: &ClientHandoff) -> Fr {
    Fr::from_le_bytes(&handoff.sk_commitment).expect("valid Fr from commitment bytes")
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
        // sk_commitment should NOT equal raw sk
        let mut sk_fields = Vec::new();
        sk.field_repr(&mut sk_fields);
        let sk_bytes: [u8; 32] = sk_fields[0].0.to_bytes_le().try_into().unwrap();
        assert_ne!(handoff.sk_commitment, sk_bytes, "commitment must hide sk");
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
        assert_eq!(handoff.sk_commitment, deserialized.sk_commitment);
        assert_eq!(handoff.mt_index, deserialized.mt_index);
    }

    #[test]
    fn different_blinding_produces_different_commitment() {
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
        // Different blinding → different commitment (probabilistic)
        assert_ne!(h1.sk_commitment, h2.sk_commitment);
    }
}
