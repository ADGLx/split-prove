//! # Split-Prove Prototype: Pedersen Commitment Handoff
//!
//! End-to-end demonstration of split proving where:
//!   - **Client** (lightweight): holds sk, computes nullifier/pk/commitment + sk commitment
//!   - **Server** (heavy): builds ProofPreimage using pre-computed values, proves without sk
//!
//! Uses the modified midnight-ledger circuits (new_split constructors) that accept
//! pre-computed values instead of raw sk.

use midnight_base_crypto::hash::HashOutput;
use midnight_coin_structure::coin::{
    Commitment, Info as CoinInfo, Nullifier, PublicKey as CoinPublicKey,
    QualifiedInfo as QualifiedCoinInfo, SecretKey as CoinSecretKey,
};
use midnight_coin_structure::transfer::{Recipient, SenderEvidence};
use midnight_storage::db::InMemoryDB;
use midnight_transient_crypto::curve::Fr;
use midnight_transient_crypto::hash::transient_hash;
use midnight_transient_crypto::merkle_tree::MerkleTree;
use midnight_transient_crypto::proofs::ProofPreimage;
use midnight_transient_crypto::repr::FieldRepr;
use midnight_zswap::{AuthorizedClaim, Input};
use rand::rngs::OsRng;
use rand::Rng;
use std::borrow::Cow;
use std::time::Instant;

// ─── Client Handoff ─────────────────────────────────────────────────────────

/// Values the client computes from sk and sends to the server.
/// The server never sees the raw sk.
#[derive(Debug, Clone)]
pub struct ClientHandoff {
    /// Pedersen commitment to sk: C = H(sk, r)
    /// In production: C = sk·G + r·H on BLS12-381 G1
    pub sk_commitment: Fr,

    /// Nullifier = H(sk, coin_info)
    pub nullifier: Nullifier,

    /// Public key derived from sk
    pub pk: CoinPublicKey,

    /// Coin commitment = commit(pk, coin)
    pub commitment_hash: Commitment,

    /// Coin info (not secret)
    pub coin_info: CoinInfo,

    /// Qualified coin info with merkle index
    pub qualified_coin_info: QualifiedCoinInfo,

    /// Whether this is a contract-owned coin
    pub is_contract: Option<midnight_coin_structure::contract::ContractAddress>,
}

/// CLIENT SIDE: compute all sk-dependent values.
/// This runs in the wallet — ~100µs, no heavy crypto.
pub fn client_prepare(
    sk: &CoinSecretKey,
    coin: &QualifiedCoinInfo,
    is_contract: Option<midnight_coin_structure::contract::ContractAddress>,
) -> ClientHandoff {
    let sender_evidence = if let Some(addr) = is_contract {
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

    // Commit to sk (hiding commitment — server can't extract sk)
    let blinding: Fr = OsRng.r#gen();
    let mut sk_fields = Vec::new();
    sk.field_repr(&mut sk_fields);
    let sk_commitment = transient_hash(&[sk_fields[0], blinding]);

    ClientHandoff {
        sk_commitment,
        nullifier,
        pk,
        commitment_hash,
        coin_info,
        qualified_coin_info: coin.clone(),
        is_contract,
    }
}

// ─── Server Side ────────────────────────────────────────────────────────────

/// SERVER SIDE: build the ProofPreimage for zswap/spend using the client handoff.
/// The server NEVER sees the raw secret key.
pub fn server_build_spend_preimage<D: midnight_storage::db::DB>(
    handoff: &ClientHandoff,
    tree: &MerkleTree<(), D>,
) -> Result<Input<ProofPreimage, D>, String> {
    Input::new_split(
        &mut OsRng,
        &handoff.qualified_coin_info,
        None, // segment
        handoff.nullifier,
        handoff.commitment_hash,
        handoff.sk_commitment,
        handoff.is_contract,
        tree,
    )
    .map_err(|e| format!("build spend preimage: {:?}", e))
}

/// SERVER SIDE: build the ProofPreimage for zswap/sign using the client handoff.
pub fn server_build_sign_preimage(
    handoff: &ClientHandoff,
) -> Result<AuthorizedClaim<ProofPreimage>, String> {
    AuthorizedClaim::new_split::<OsRng, InMemoryDB>(
        &mut OsRng,
        handoff.coin_info.clone(),
        handoff.pk,
        handoff.sk_commitment,
    )
    .map_err(|e| format!("build sign preimage: {:?}", e))
}

// ─── Demo ───────────────────────────────────────────────────────────────────

fn main() {
    println!("=== Split-Prove Prototype v2: End-to-End ===\n");

    // ── Setup ──
    let sk = CoinSecretKey(OsRng.gen::<HashOutput>());
    let coin = QualifiedCoinInfo {
        value: 1000u64.into(),
        type_: Default::default(),
        nonce: OsRng.r#gen(),
        mt_index: 0,
    };

    // Build a merkle tree with the coin's commitment
    let sender_evidence = SenderEvidence::User(Cow::Borrowed(&sk));
    let coin_info = CoinInfo::from(&coin);
    let commitment = coin_info.commitment(&Recipient::from(sender_evidence));
    let tree = MerkleTree::<(), InMemoryDB>::blank(32)
        .try_update_hash(0, commitment.0, ())
        .expect("valid tree index")
        .rehash();

    println!("Setup:");
    println!("  coin value:      {}", coin.value);
    println!("  merkle root:     {:?}", tree.root().unwrap());

    // ── CLIENT: compute handoff (~100µs) ──
    println!("\n--- CLIENT (wallet) ---");
    let start = Instant::now();
    let handoff = client_prepare(&sk, &coin, None);
    let client_time = start.elapsed();
    println!("  Time:            {:?}", client_time);
    println!("  nullifier:       {}", hex::encode(handoff.nullifier.0 .0));
    println!("  sk_commitment:   {:?}", handoff.sk_commitment);
    println!("  pk:              {:?}", handoff.pk);

    // ── Verify sk is NOT in the handoff ──
    let mut sk_fields = Vec::new();
    sk.field_repr(&mut sk_fields);
    let sk_fr = sk_fields[0];
    assert_ne!(
        handoff.sk_commitment, sk_fr,
        "sk_commitment must NOT equal raw sk"
    );
    println!("  sk NOT exposed:  ✓ (commitment ≠ raw sk)");

    // ── SERVER: build ProofPreimage WITHOUT sk ──
    println!("\n--- SERVER (prover) ---");
    let start = Instant::now();
    let spend_input =
        server_build_spend_preimage::<InMemoryDB>(&handoff, &tree).expect("spend preimage");
    let server_build_time = start.elapsed();
    println!("  Build time:      {:?}", server_build_time);
    println!("  key_location:    {}", spend_input.proof.key_location.0);
    println!("  inputs count:    {}", spend_input.proof.inputs.len());
    println!(
        "  pub_tx_inputs:   {}",
        spend_input.proof.public_transcript_inputs.len()
    );

    // Verify that the proof preimage's inputs[0] is sk_commitment, NOT raw sk
    assert_eq!(
        spend_input.proof.inputs[0], handoff.sk_commitment,
        "inputs[0] should be sk_commitment, not raw sk"
    );
    assert_ne!(
        spend_input.proof.inputs[0], sk_fr,
        "inputs[0] must NOT be raw sk"
    );
    println!("  inputs[0] = commitment (not sk): ✓");

    // ── Build sign preimage too ──
    let start = Instant::now();
    let sign_claim = server_build_sign_preimage(&handoff).expect("sign preimage");
    let sign_build_time = start.elapsed();
    println!("\n  Sign preimage:");
    println!("    Build time:    {:?}", sign_build_time);
    println!("    key_location:  {}", sign_claim.proof.key_location.0);
    println!("    inputs count:  {}", sign_claim.proof.inputs.len());
    assert_eq!(
        sign_claim.proof.inputs[0], handoff.sk_commitment,
        "sign inputs[0] should be sk_commitment"
    );
    println!("    inputs[0] = commitment (not sk): ✓");

    // ── Compare with original (sk-exposing) flow ──
    println!("\n--- COMPARISON: original flow (exposes sk) ---");
    let original_input = Input::<ProofPreimage, InMemoryDB>::new_from_secret_key(
        &mut OsRng,
        &coin,
        None,
        SenderEvidence::User(Cow::Borrowed(&sk)),
        &tree,
    )
    .expect("original spend");
    // SenderEvidence::User serializes as [discriminant=1, sk_field_0, sk_field_1, ...]
    // So inputs[0] = 1 (discriminant), inputs[1] = first sk field
    assert_eq!(
        original_input.proof.inputs[0],
        Fr::from(1u64),
        "original inputs[0] should be User discriminant (1)"
    );
    assert_eq!(
        original_input.proof.inputs[1], sk_fr,
        "original inputs[1] should be raw sk"
    );
    println!("  original inputs[1] = RAW SK: ✗ (exposed!)");
    println!("  split    inputs[0] = COMMITMENT: ✓ (hidden!)");

    // ── Summary ──
    println!("\n=== Results ===");
    println!(
        "  Client computation: {:?} (nullifier + pk + commitment)",
        client_time
    );
    println!(
        "  Server build:       {:?} (ProofPreimage without sk)",
        server_build_time
    );
    println!("  Server would then:  prove() → 2-10s (PLONK, no sk needed)");
    println!("\n  SECURITY:");
    println!("    Original: server sees sk in inputs[0] ✗");
    println!("    Split:    server sees sk_commitment in inputs[0] ✓");
    println!("    sk_commitment = H(sk, random) — cannot extract sk");

    println!("\n=== Production TODO ===");
    println!("  1. New circuit IR (.bzkir) for 'midnight/zswap/spend-split'");
    println!("     that verifies sk_commitment against nullifier/pk via");
    println!("     committed-instance column");
    println!("  2. Add /v2/prove endpoint to api-gateway accepting ClientHandoff");
    println!("  3. Client SDK: client_prepare() → POST /v2/prove");
    println!("  4. Key generation for the new split circuits");
}
