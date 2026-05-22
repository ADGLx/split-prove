//! # Split-Prove Prototype: Solution A
//!
//! End-to-end demonstration of split proving where:
//!   - **Client**: holds sk, computes nullifier/pk/coin-binding tag, and
//!                 references its `WalletRegistration` (one-time per wallet).
//!   - **Server** (heavy): builds ProofPreimage using pre-computed values,
//!                 proves without sk.
//!
//! Solution A change vs. v3: the wallet's `(pk, C_sk)` are no longer
//! disclosed on chain. The only wallet-identifying public value the server
//! emits is `registry_root`, the root of the registry contract's
//! `HistoricMerkleTree` at the time of the spend.

use midnight_base_crypto::hash::HashOutput;
use midnight_coin_structure::coin::{
    Commitment, Info as CoinInfo, Nullifier, PublicKey as CoinPublicKey,
    QualifiedInfo as QualifiedCoinInfo, SecretKey as CoinSecretKey,
};
use midnight_coin_structure::transfer::{Recipient, SenderEvidence};
use midnight_storage::db::InMemoryDB;
use midnight_transient_crypto::curve::Fr;
use midnight_transient_crypto::merkle_tree::{MerkleTree, MerkleTreeDigest};
use midnight_transient_crypto::proofs::{Proof, ProofPreimage};
use midnight_transient_crypto::repr::FieldRepr;
use midnight_zswap::Input;
use rand::rngs::OsRng;
use rand::Rng;
use std::borrow::Cow;
use std::time::Instant;

// ─── Client Handoff (demo-only — see src/client.rs for the SDK type) ─────────

#[derive(Debug, Clone)]
pub struct ClientHandoff {
    pub coin_binding_tag: Fr,
    pub nullifier: Nullifier,
    pub pk: CoinPublicKey,
    pub commitment_hash: Commitment,
    pub coin_info: CoinInfo,
    pub qualified_coin_info: QualifiedCoinInfo,
    pub is_contract: Option<midnight_coin_structure::contract::ContractAddress>,
    /// Solution A: registry-tree root the membership path resolves to. Stub
    /// `MerkleTreeDigest(0.into())` in the demo — production callers supply
    /// the real root from their cached `RegistryWitness`.
    pub registry_root: MerkleTreeDigest,
}

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

    let pk = sk.public_key();
    let coin_info = CoinInfo::from(coin);
    let nullifier = split_nullifier(&coin_info, sk);
    let commitment_hash = coin_info.commitment(&Recipient::from(sender_evidence));
    let coin_binding_tag = midnight_zswap::split_coin_binding_tag(&coin_info, pk);

    // Solution A demo placeholder: the production wallet supplies the
    // registry root from its `RegistryWitness`, fetched once per session via
    // `refresh_registry_path`. The demo stubs it to zero — the resulting
    // bundle will fail admission until a real witness is wired in.
    let registry_root = MerkleTreeDigest(Fr::from(0u64));

    ClientHandoff {
        coin_binding_tag,
        nullifier,
        pk,
        commitment_hash,
        coin_info,
        qualified_coin_info: coin.clone(),
        is_contract,
        registry_root,
    }
}

fn split_nullifier(coin: &CoinInfo, sk: &CoinSecretKey) -> Nullifier {
    coin.nullifier(&SenderEvidence::User(Cow::Borrowed(sk)))
}

// ─── Server Side ────────────────────────────────────────────────────────────

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
        handoff.registry_root,
        Proof(Vec::new()), // placeholder client-derivation proof for the demo
        handoff.is_contract,
        tree,
    )
    .map(|split_input| split_input.into_preimage())
    .map_err(|e| format!("build spend preimage: {:?}", e))
}

// ─── Demo ───────────────────────────────────────────────────────────────────

fn main() {
    println!("=== Split-Prove Prototype (Solution A): End-to-End ===\n");

    let sk = CoinSecretKey(OsRng.gen::<HashOutput>());
    let coin = QualifiedCoinInfo {
        value: 1000u64.into(),
        type_: Default::default(),
        nonce: OsRng.r#gen(),
        mt_index: 0,
    };

    let sender_evidence = SenderEvidence::User(Cow::Borrowed(&sk));
    let coin_info = CoinInfo::from(&coin);
    let commitment = coin_info.commitment(&Recipient::from(sender_evidence));
    let tree = MerkleTree::<(), InMemoryDB>::blank(32)
        .update_hash(0, commitment.0, ())
        .rehash();

    println!("Setup:");
    println!("  coin value:      {}", coin.value);
    println!("  merkle root:     {:?}", tree.root().unwrap());

    println!("\n--- CLIENT (wallet) ---");
    let start = Instant::now();
    let handoff = client_prepare(&sk, &coin, None);
    let client_time = start.elapsed();
    println!("  Time:            {:?}", client_time);
    println!("  nullifier:       {}", hex::encode(handoff.nullifier.0 .0));
    println!("  coin_binding:    {:?}", handoff.coin_binding_tag);
    println!("  registry_root:   {:?} (demo stub: zero)", handoff.registry_root);

    let mut sk_fields = Vec::new();
    sk.field_repr(&mut sk_fields);
    let sk_fr = sk_fields[0];
    assert_ne!(handoff.coin_binding_tag, sk_fr, "tag must NOT equal raw sk");
    println!("  sk NOT exposed:  ✓ (tag ≠ raw sk)");

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

    println!("\n--- COMPARISON: original flow (exposes sk) ---");
    let original_input = Input::<ProofPreimage, InMemoryDB>::new_from_secret_key(
        &mut OsRng,
        &coin,
        None,
        SenderEvidence::User(Cow::Borrowed(&sk)),
        &tree,
    )
    .expect("original spend");
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

    println!("\n=== Results ===");
    println!(
        "  Client computation: {:?} (nullifier + coin-binding tag + reg_leaf preimage)",
        client_time
    );
    println!(
        "  Server build:       {:?} (ProofPreimage without sk)",
        server_build_time
    );
    println!("  Server would then:  prove() → ~960 ms (Solution A's +20-Poseidon membership)");

    println!("\n=== Remaining production work (out of scope for this demo binary) ===");
    println!("  - The proof-server installs a *permissive* registry-root checker");
    println!("    at boot (see `install_registry_root_checker_for_demo`). A");
    println!("    production deployment must replace it with one that resolves");
    println!("    the deployed registry contract's `HistoricMerkleTree<20>` from");
    println!("    the live `LedgerState` and only admits roots in its history.");
    println!("  - The node's admission path needs the same hook wired up at");
    println!("    startup (`deps/midnight-node`). For the live preview e2e in");
    println!("    `cargo make e2e` the proof-server's permissive checker is");
    println!("    sufficient.");
    println!("  - `tools/deploy_registry.sh --deploy-registry` flag on");
    println!("    `preview_balance_submit_split_tx.mjs` still needs to be");
    println!("    implemented to submit the registry-contract deploy + register");
    println!("    contract calls against a live node.");
}
