//! Server-side split proving.
//!
//! Receives a `ClientHandoff`, builds `ProofPreimage` without raw sk,
//! and proves using `prove_split()` with committed instances.

use crate::client::{
    handoff_commitment, handoff_nullifier, handoff_pk, handoff_sk_commitment_fr, ClientHandoff,
};
use midnight_base_crypto::hash::HashOutput;
use midnight_coin_structure::coin::QualifiedInfo as QualifiedCoinInfo;
use midnight_coin_structure::coin::ShieldedTokenType;
use midnight_coin_structure::contract::ContractAddress;
use midnight_storage::db::DB;
use midnight_storage::Storable;
use midnight_transient_crypto::merkle_tree::MerkleTree;
use midnight_transient_crypto::proofs::ProofPreimage;
use midnight_zswap::{AuthorizedClaim, Input};
use rand::rngs::OsRng;
use std::fmt::Debug;

/// Build a spend ProofPreimage from a ClientHandoff.
/// The resulting ProofPreimage has sk_commitment in inputs[0] instead of raw sk.
pub fn build_spend_preimage<A: Debug + Storable<D>, D: DB>(
    handoff: &ClientHandoff,
    tree: &MerkleTree<A, D>,
) -> Result<Input<ProofPreimage, D>, String> {
    let coin = reconstruct_coin(handoff)?;
    let contract = handoff
        .contract_address
        .map(|a| ContractAddress(HashOutput(a)));

    Input::new_split(
        &mut OsRng,
        &coin,
        None, // segment
        handoff_nullifier(handoff),
        handoff_commitment(handoff),
        handoff_sk_commitment_fr(handoff),
        contract,
        tree,
    )
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
        handoff_sk_commitment_fr(handoff),
    )
    .map_err(|e| format!("build sign preimage: {:?}", e))
}

/// Number of field elements that sk occupies in the witness.
/// For SenderEvidence::User: [discriminant=1, sk_hash_output_field]
/// For the split path we put a single sk_commitment Fr.
pub const SK_COMMITTED_FIELD_COUNT: usize = 1;

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
            .try_update_hash(0, commitment.0, ())
            .expect("valid tree index")
            .rehash()
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
        let handoff = client_prepare(&sk, &coin, None);

        let input = build_spend_preimage::<(), InMemoryDB>(&handoff, &tree);
        assert!(
            input.is_ok(),
            "build_spend_preimage failed: {:?}",
            input.err()
        );

        let input = input.unwrap();
        // inputs[0] should be sk_commitment, not raw sk
        let sk_com_fr = handoff_sk_commitment_fr(&handoff);
        assert_eq!(input.proof.inputs[0], sk_com_fr);

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
            sk_commitment: [1u8; 32],
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
        assert_eq!(claim.proof.inputs[2], handoff_sk_commitment_fr(&handoff));
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
        let handoff = client_prepare(&sk, &coin, None);

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
