//! Server-side split proving.
//!
//! Receives a `ClientHandoff`, builds `ProofPreimage` without raw sk,
//! and proves using `prove_split()` with committed instances.

use crate::client::{
    handoff_coin_binding_tag_fr, handoff_commitment, handoff_nullifier, handoff_pk, ClientHandoff,
};
use midnight_base_crypto::hash::HashOutput;
use midnight_coin_structure::coin::QualifiedInfo as QualifiedCoinInfo;
use midnight_coin_structure::coin::ShieldedTokenType;
use midnight_coin_structure::contract::ContractAddress;
use midnight_storage::db::DB;
use midnight_storage::Storable;
use midnight_transient_crypto::merkle_tree::MerkleTree;
use midnight_transient_crypto::proofs::{Proof, ProofPreimage};
use midnight_zswap::{AuthorizedClaim, Input};
use rand::rngs::OsRng;
use std::fmt::Debug;

/// Build a spend ProofPreimage from a ClientHandoff.
/// The resulting ProofPreimage has pk/coin data in its inputs, but never raw sk.
pub fn build_spend_preimage<A: Debug + Storable<D>, D: DB>(
    handoff: &ClientHandoff,
    tree: &MerkleTree<A, D>,
) -> Result<Input<ProofPreimage, D>, String> {
    let coin = reconstruct_coin(handoff)?;
    let contract = handoff
        .contract_address
        .map(|a| ContractAddress(HashOutput(a)));
    let client_derivation_proof = handoff
        .client_derivation_proof
        .as_deref()
        .map(hex::decode)
        .transpose()
        .map_err(|e| format!("decode client derivation proof: {e}"))?
        .map(Proof)
        .ok_or("client derivation proof is required for split spend preimages")?;

    Input::new_split(
        &mut OsRng,
        &coin,
        None, // segment
        handoff_nullifier(handoff),
        handoff_commitment(handoff),
        handoff_pk(handoff),
        handoff_coin_binding_tag_fr(handoff),
        client_derivation_proof,
        contract,
        tree,
    )
    .map(|split_input| split_input.into_preimage())
    .map_err(|e| format!("build spend preimage: {:?}", e))
}

/// Build a sign ProofPreimage from a ClientHandoff.
pub fn build_sign_preimage(
    handoff: &ClientHandoff,
) -> Result<AuthorizedClaim<ProofPreimage>, String> {
    let coin = reconstruct_coin(handoff)?;
    let coin_info = midnight_coin_structure::coin::Info::from(&coin);

    AuthorizedClaim::new_split::<OsRng, midnight_storage::db::InMemoryDB>(
        &mut OsRng,
        coin_info,
        handoff_pk(handoff),
    )
    .map_err(|e| format!("build sign preimage: {:?}", e))
}

/// Number of field elements that sk occupies in the split witness.
pub const SK_COMMITTED_FIELD_COUNT: usize = 0;

/// Reconstruct QualifiedCoinInfo from a ClientHandoff.
fn reconstruct_coin(handoff: &ClientHandoff) -> Result<QualifiedCoinInfo, String> {
    use midnight_coin_structure::coin::Nonce;

    let nonce = Nonce(HashOutput(handoff.coin_nonce));

    Ok(QualifiedCoinInfo {
        value: handoff.coin_value.into(),
        type_: ShieldedTokenType(HashOutput(handoff.coin_color)),
        nonce,
        mt_index: handoff.mt_index,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::client::client_prepare;
    use midnight_base_crypto::hash::HashOutput;
    use midnight_coin_structure::coin::{
        Info as CoinInfo, QualifiedInfo as QualifiedCoinInfo, SecretKey as CoinSecretKey,
        ShieldedTokenType,
    };
    use midnight_coin_structure::transfer::{Recipient, SenderEvidence};
    use midnight_storage::db::InMemoryDB;
    use midnight_transient_crypto::merkle_tree::MerkleTree;
    use midnight_transient_crypto::repr::FieldRepr;
    use rand::rngs::OsRng;
    use rand::Rng;
    use std::borrow::Cow;

    fn setup_tree(sk: &CoinSecretKey, coin: &QualifiedCoinInfo) -> MerkleTree<(), InMemoryDB> {
        let sender_evidence = SenderEvidence::User(Cow::Borrowed(sk));
        let coin_info = CoinInfo::from(coin);
        let commitment = coin_info.commitment(&Recipient::from(sender_evidence));
        MerkleTree::<(), InMemoryDB>::blank(32)
            .update_hash(0, commitment.0, ())
            .rehash()
    }

    fn attach_dummy_client_proof(handoff: &mut ClientHandoff) {
        handoff.client_derivation_proof = Some(hex::encode([]));
    }

    #[test]
    fn build_spend_preimage_succeeds() {
        let sk = CoinSecretKey(OsRng.gen::<HashOutput>());
        let coin = QualifiedCoinInfo {
            value: 1000u64.into(),
            type_: Default::default(),
            nonce: OsRng.r#gen(),
            mt_index: 0,
        };
        let tree = setup_tree(&sk, &coin);
        let mut handoff = client_prepare(&sk, &coin, None);
        attach_dummy_client_proof(&mut handoff);

        let input = build_spend_preimage::<(), InMemoryDB>(&handoff, &tree);
        assert!(
            input.is_ok(),
            "build_spend_preimage failed: {:?}",
            input.err()
        );

        let input = input.unwrap();
        let pk = handoff_pk(&handoff);
        let mut pk_fields = Vec::new();
        pk.field_repr(&mut pk_fields);
        assert_eq!(input.proof.inputs[0], pk_fields[0]);
        assert_eq!(input.proof.inputs[1], pk_fields[1]);

        // key_location should be spend-split
        assert_eq!(
            input.proof.key_location.0.as_ref(),
            "midnight/zswap/spend-split"
        );
    }

    #[test]
    fn reconstruct_coin_preserves_token_type() {
        let token = ShieldedTokenType(HashOutput([7u8; 32]));
        let handoff = ClientHandoff {
            coin_binding_tag: [1u8; 32],
            nullifier: [2u8; 32],
            pk: [3u8; 32],
            commitment_hash: [4u8; 32],
            coin_value: 123,
            coin_color: token.0 .0,
            coin_nonce: [5u8; 32],
            mt_index: 9,
            contract_address: None,
            client_derivation_proof: None,
        };

        let coin = reconstruct_coin(&handoff).unwrap();
        assert_eq!(coin.type_, token);
        assert_eq!(coin.mt_index, 9);
    }

    #[test]
    fn build_spend_preimage_rejects_tampered_commitment() {
        let sk = CoinSecretKey(OsRng.gen::<HashOutput>());
        let coin = QualifiedCoinInfo {
            value: 1000u64.into(),
            type_: Default::default(),
            nonce: OsRng.r#gen(),
            mt_index: 0,
        };
        let tree = setup_tree(&sk, &coin);
        let mut handoff = client_prepare(&sk, &coin, None);
        attach_dummy_client_proof(&mut handoff);
        handoff.commitment_hash[0] ^= 1;

        let err = build_spend_preimage::<(), InMemoryDB>(&handoff, &tree)
            .expect_err("tampered commitment must not build a split preimage");
        assert!(
            err.contains("CommitmentNotInTree"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn build_sign_preimage_succeeds() {
        let sk = CoinSecretKey(OsRng.gen::<HashOutput>());
        let coin = QualifiedCoinInfo {
            value: 500u64.into(),
            type_: Default::default(),
            nonce: OsRng.r#gen(),
            mt_index: 0,
        };
        let handoff = client_prepare(&sk, &coin, None);

        let claim = build_sign_preimage(&handoff);
        assert!(
            claim.is_ok(),
            "build_sign_preimage failed: {:?}",
            claim.err()
        );

        let claim = claim.unwrap();
        let pk = handoff_pk(&handoff);
        let mut pk_fields = Vec::new();
        pk.field_repr(&mut pk_fields);
        assert_eq!(claim.proof.inputs, pk_fields);
    }

    #[test]
    fn spend_preimage_does_not_contain_raw_sk() {
        let sk = CoinSecretKey(OsRng.gen::<HashOutput>());
        let coin = QualifiedCoinInfo {
            value: 1000u64.into(),
            type_: Default::default(),
            nonce: OsRng.r#gen(),
            mt_index: 0,
        };
        let tree = setup_tree(&sk, &coin);
        let mut handoff = client_prepare(&sk, &coin, None);
        attach_dummy_client_proof(&mut handoff);

        let input = build_spend_preimage::<(), InMemoryDB>(&handoff, &tree).unwrap();

        // Extract raw sk field value
        let mut sk_fields = Vec::new();
        sk.field_repr(&mut sk_fields);
        let raw_sk = sk_fields[0];

        // Ensure raw sk is NOT anywhere in the inputs
        for (i, inp) in input.proof.inputs.iter().enumerate() {
            assert_ne!(
                *inp, raw_sk,
                "raw sk found in inputs[{}] — security violation!",
                i
            );
        }
    }
}
