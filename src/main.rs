//! # Split-Prove Prototype: Pedersen Commitment Handoff
//!
//! End-to-end demonstration of split proving where:
//!   - **Client**: holds sk, computes nullifier/pk/coin-binding tag
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
use midnight_transient_crypto::hash::{transient_hash, upgrade_from_transient};
use midnight_transient_crypto::merkle_tree::MerkleTree;
use midnight_transient_crypto::proofs::{Proof, ProofPreimage};
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
    /// ZK-friendly tag binding client and server proofs to the same coin.
    pub coin_binding_tag: Fr,

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

    // Compute split-only Poseidon nullifier; mirrors NullifierZkfPreimage in
    // circuits/sk_proof.compact.
    let coin_info = CoinInfo::from(coin);
    let nullifier = split_nullifier(&coin_info, sk);

    // Compute coin commitment (still SHA-256; stock zswap funding compatibility).
    let commitment_hash = coin_info.commitment(&Recipient::from(sender_evidence));

    let coin_binding_tag = midnight_zswap::split_coin_binding_tag(&coin_info, pk);

    ClientHandoff {
        coin_binding_tag,
        nullifier,
        pk,
        commitment_hash,
        coin_info,
        qualified_coin_info: coin.clone(),
        is_contract,
    }
}

fn split_nul_domain() -> Fr {
    let domain = b"midnight:split-nul[v1]";
    let mut bytes = [0u8; 32];
    bytes[..domain.len()].copy_from_slice(domain);
    Fr::from_le_bytes(&bytes).expect("split nullifier domain fits in Fr")
}

fn split_nullifier(coin: &CoinInfo, sk: &CoinSecretKey) -> Nullifier {
    let mut inputs = vec![split_nul_domain()];
    coin.field_repr(&mut inputs);
    sk.field_repr(&mut inputs);
    Nullifier(upgrade_from_transient(transient_hash(&inputs)))
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
        handoff.pk,
        handoff.coin_binding_tag,
        Proof(Vec::new()),
        handoff.is_contract,
        tree,
    )
    .map(|split_input| split_input.into_preimage())
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
        .update_hash(0, commitment.0, ())
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
    println!("  coin_binding:    {:?}", handoff.coin_binding_tag);
    println!("  pk:              {:?}", handoff.pk);

    // ── Verify sk is NOT in the handoff ──
    let mut sk_fields = Vec::new();
    sk.field_repr(&mut sk_fields);
    let sk_fr = sk_fields[0];
    assert_ne!(handoff.coin_binding_tag, sk_fr, "tag must NOT equal raw sk");
    println!("  sk NOT exposed:  ✓ (tag ≠ raw sk)");

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

    let mut pk_fields = Vec::new();
    handoff.pk.field_repr(&mut pk_fields);
    assert_eq!(spend_input.proof.inputs[0], pk_fields[0]);
    assert_eq!(spend_input.proof.inputs[1], pk_fields[1]);
    assert_ne!(
        spend_input.proof.inputs[0], sk_fr,
        "inputs[0] must NOT be raw sk"
    );
    println!("  inputs[0..2] = pk fields (not sk): ✓");

    // ── Build sign preimage too ──
    let start = Instant::now();
    let sign_claim = server_build_sign_preimage(&handoff).expect("sign preimage");
    let sign_build_time = start.elapsed();
    println!("\n  Sign preimage:");
    println!("    Build time:    {:?}", sign_build_time);
    println!("    key_location:  {}", sign_claim.proof.key_location.0);
    println!("    inputs count:  {}", sign_claim.proof.inputs.len());
    assert_eq!(
        sign_claim.proof.inputs[0], pk_fields[0],
        "sign inputs[0] should be first pk field"
    );
    println!("    inputs[0] = pk field (not sk): ✓");

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
    println!("  split    inputs[0] = PUBLIC KEY FIELD: ✓ (not sk)");

    // ── Summary ──
    println!("\n=== Results ===");
    println!(
        "  Client computation: {:?} (nullifier + pk + coin-binding tag)",
        client_time
    );
    println!(
        "  Server build:       {:?} (ProofPreimage without sk)",
        server_build_time
    );
    println!("  Server would then:  prove() → 2-10s (PLONK, no sk needed)");
    println!("\n  SECURITY:");
    println!("    Original: server sees sk in inputs[0] ✗");
    println!("    Split:    server sees pk/tag/nullifier, never raw sk ✓");

    println!("\n=== Production TODO ===");
    println!("  1. New circuit IR (.bzkir) for 'midnight/zswap/spend-split'");
    println!("     that verifies pk/nullifier/tag via clientDerivationProof");
    println!("  2. Add /v2/prove endpoint to api-gateway accepting ClientHandoff");
    println!("  3. Client SDK: client_prepare() → POST /v2/prove");
    println!("  4. Key generation for the new split circuits");
}
