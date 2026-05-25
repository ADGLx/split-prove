//! Client-side SDK for split proving (Solution A).
//!
//! The wallet holds the secret key and computes all sk-dependent values. These
//! are packaged into a `ClientHandoff` and sent to the server. The server
//! never sees `sk`, `r`, `salt`, or the Merkle membership path.
//!
//! Unlinkability surface:
//!   - The handoff now carries `registry_root` (shared across every spend
//!     against the same registry state) instead of `pk` / `attested_commitment_sk`
//!     (which were per-wallet and trivially linkable). `pk` is still passed to
//!     the server through `pk_for_server_proof` — the server's spend-split
//!     circuit needs it for the on-chain `coinCommitment` computation — but
//!     `pk` itself is never published on-chain by either circuit.
//!   - `coin_binding_tag` (per coin) is still public; per the plan's "known
//!     limitations" it links a single spend to its originating output but is
//!     out of scope for Solution A.

use crate::attestation::WalletRegistration;
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
use midnight_transient_crypto::merkle_tree::{MerklePath, MerkleTreeDigest};
use midnight_transient_crypto::proofs::{
    KeyLocation, ParamsProver, ParamsProverProvider, Proof, ProofPreimage, ProvingKeyMaterial,
    Resolver,
};
use midnight_transient_crypto::repr::FieldRepr;
use rand::rngs::OsRng;
use serde::{Deserialize, Serialize};
use std::borrow::Cow;

/// Height of the wallet-registry Merkle tree. Must match the constant in
/// `circuits/wallet_registry.compact` (`HistoricMerkleTree<20, …>`) and
/// `circuits/sk_proof.compact` (`MerkleTreePath<20, Bytes<32>>`).
pub const REGISTRY_TREE_HEIGHT: u8 = 20;

/// Compute the merkle root from a `(leaf_hash, path)` pair *without* applying
/// any further leaf hash. This is the off-circuit equivalent of Compact's
/// `merkleTreePathRootNoLeafHash<H>(path)` and matches the registry reference
/// circuit's no-extra-leaf-hash semantics.
///
/// Copy of `raw_leaf_hash_root` from
/// `deps/midnight-ledger/zswap/src/construct.rs` — same logic but reproduced
/// here so the split-prove client doesn't have to depend on zswap's
/// internal helpers.
pub fn raw_leaf_hash_root<T>(leaf_hash: HashOutput, path: &MerklePath<T>) -> MerkleTreeDigest {
    use midnight_transient_crypto::hash::{degrade_to_transient, transient_hash};
    MerkleTreeDigest(
        path.path
            .iter()
            .fold(degrade_to_transient(leaf_hash), |acc, entry| {
                if entry.goes_left {
                    transient_hash(&[acc, entry.sibling.0])
                } else {
                    transient_hash(&[entry.sibling.0, acc])
                }
            }),
    )
}

/// Data the client sends to the server for split proving.
///
/// Solution A: `pk` and `attested_commitment_sk` are gone. The only
/// wallet-identifying public value is `registry_root`, which is shared across
/// every spend against the same registry tree state. The wallet's `pk` still
/// travels privately to the server (it's needed for the server's
/// `coinCommitment` recomputation) but never appears in the bundle's public
/// inputs.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClientHandoff {
    /// ZK-friendly tag binding the client proof and server split proof to the
    /// same coin: `transientHash("midnight:zswap-split-coin[v1]", coin, pk)`.
    pub coin_binding_tag: [u8; 32],

    /// Canonical Zswap nullifier:
    /// `persistentHash("midnight:zswap-cn[v1]", coin, true, sk)`.
    pub nullifier: [u8; 32],

    /// Coin owner key. Private to the wallet–server handoff (not bundled into
    /// the on-chain `SplitPublicInputs`). The server's spend-split circuit
    /// witnesses `pk` to recompute the coin commitment.
    pub pk: [u8; 32],

    /// Coin commitment `commit(pk, coin)`. Private to the wallet–server
    /// handoff — used to look up the merkle path; not disclosed on-chain.
    pub commitment_hash: [u8; 32],

    /// Registry-tree root the membership path resolves to. This **is** the
    /// only wallet-identifying public value. See `SplitPublicInputs` for the
    /// admission cross-check semantics.
    pub registry_root: [u8; 32],

    /// Coin value
    pub coin_value: u128,

    /// Coin color (token type)
    pub coin_color: [u8; 32],

    /// Coin nonce
    pub coin_nonce: [u8; 32],

    /// Merkle tree index of the coin in the Zswap tree
    pub mt_index: u64,

    /// Contract-owned coin address, if present. Split-prove currently
    /// supports user-owned shielded coins only; server preimage construction
    /// rejects `Some`.
    pub contract_address: Option<[u8; 32]>,

    /// Hex-encoded serialized proof for `circuits/sk_proof.compact`. Binds
    /// `(sk, r, salt, pk, coin, merkle_path)` to the disclosed
    /// `(nullifier, coin_binding_tag, registry_root)`.
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

/// Witness data the wallet must supply for the per-spend circuit. Contains
/// the secret pieces of the registration record plus the live Merkle path
/// (fetched from contract state). The wallet keeps this object opaque — it
/// flows from `client_prepare_with_registry` into the per-spend
/// `ProofPreimage` builder and nowhere else.
#[derive(Debug, Clone)]
pub struct RegistryWitness {
    /// Poseidon blinding `r` for `C_sk`.
    pub blinding: Fr,
    /// Per-registration salt for `reg_leaf`.
    pub salt: Fr,
    /// Merkle path from `reg_leaf` (as 32-byte HashOutput) to a registry-tree
    /// root. `merkle_path.leaf` must equal `upgrade_from_transient(reg_leaf)`
    /// — the wallet is responsible for ensuring this is still aligned with
    /// a recent registry-contract root (`refresh_registry_path`).
    ///
    /// The leaf type is `((), HashOutput)` to match the lowering Compact uses
    /// for `MerkleTreePath<20, Bytes<32>>` (zero-sized aux + the upgraded
    /// 32-byte leaf hash; see `Input::new_split` in
    /// `deps/midnight-ledger/zswap/src/construct.rs` for the same pattern).
    pub merkle_path: MerklePath<((), HashOutput)>,
    /// Convenience: the registry root the path resolves to. Cross-checked
    /// against `merkle_path.root()` in `client_prepare_with_registry` so a
    /// stale witness is rejected before the proof is even started.
    pub registry_root: MerkleTreeDigest,
}

impl RegistryWitness {
    /// Construct a witness for a fresh registration: builds a height-20 path
    /// containing only `reg_leaf` at index 0. This is the local POC's
    /// synthetic first-registration witness.
    ///
    /// The leaf bytes go in directly with no leaf-hash step. We mirror that
    /// off-circuit: the in-memory tree stores the upgrade bytes as the
    /// leaf-level hash, and the path verifier uses `NoLeafHash` to match.
    ///
    /// A production registry design would replace this with a live path
    /// refresh. The current POC intentionally keeps the witness client-side.
    pub fn for_first_registration(
        blinding: Fr,
        salt: Fr,
        reg_leaf_bytes: [u8; 32],
    ) -> Result<Self, String> {
        let leaf_hash = HashOutput(reg_leaf_bytes);
        let mt = midnight_transient_crypto::merkle_tree::MerkleTree::<(), InMemoryDB>::blank(
            REGISTRY_TREE_HEIGHT,
        )
        .update_hash(0, leaf_hash, ())
        .rehash();
        let merkle_path = mt
            .path_for_leaf(0, ((), leaf_hash))
            .map_err(|e| format!("path_for_leaf failed: {e}"))?;
        // Compute the root the "NoLeafHash" way — that's what the
        // sk_proof.compact circuit uses (`merkleTreePathRootNoLeafHash`).
        // `MerklePath::root()` would re-hash the leaf, which is wrong here.
        let registry_root = raw_leaf_hash_root(leaf_hash, &merkle_path);
        Ok(Self {
            blinding,
            salt,
            merkle_path,
            registry_root,
        })
    }

    /// Lower 32-byte LE representation of the root. Convenience for
    /// stamping into `ClientHandoff::registry_root`.
    pub fn registry_root_le_bytes(&self) -> [u8; 32] {
        self.registry_root
            .0
            .as_le_bytes()
            .try_into()
            .expect("Fr little-endian repr must be 32 bytes")
    }
}

/// Prepare a client handoff for a shielded coin spend (Solution A).
///
/// Compared to v3 this is identical at the off-chain hashing layer — the
/// nullifier, coin-binding tag, and coin commitment are byte-identical. What
/// changed is the *public* side: `pk` and `C_sk` are no longer disclosed on
/// chain, so the handoff no longer carries `attested_commitment_sk`.
///
/// Callers that need to actually prove (Solution A admission requires it
/// always) must use `client_prepare_with_registry` so the handoff is bound
/// to a registered wallet's `(r, salt)` and a real Merkle path.
pub fn client_prepare(
    sk: &CoinSecretKey,
    coin: &QualifiedCoinInfo,
    contract: Option<ContractAddress>,
) -> ClientHandoff {
    client_prepare_with_registry_root(sk, coin, contract, [0u8; 32])
}

/// Internal: build a handoff with an explicit registry root. Used by both the
/// public `client_prepare` (which stamps a zero root and is unsuitable for
/// real admission) and `client_prepare_with_registry` (which supplies the
/// caller's registered root). Test code can hit this directly with a known
/// root if it wants to bypass `WalletRegistration` entirely.
pub fn client_prepare_with_registry_root(
    sk: &CoinSecretKey,
    coin: &QualifiedCoinInfo,
    contract: Option<ContractAddress>,
    registry_root: [u8; 32],
) -> ClientHandoff {
    let sender_evidence = if let Some(addr) = contract {
        SenderEvidence::Contract(addr)
    } else {
        SenderEvidence::User(Cow::Borrowed(sk))
    };

    let pk = sk.public_key();
    let coin_info = CoinInfo::from(coin);
    let nullifier = split_nullifier(&coin_info, sk);
    let commitment_hash = coin_info.commitment(&Recipient::from(sender_evidence));
    let coin_binding_tag = midnight_zswap::split_coin_binding_tag(&coin_info, pk);

    let mut nonce_bytes = [0u8; 32];
    nonce_bytes.copy_from_slice(&coin_info.nonce.0 .0);
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
        registry_root,
        coin_value: {
            let mut v_fields = Vec::new();
            coin_info.value.field_repr(&mut v_fields);
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

/// Build a fully-populated handoff from a previously registered
/// `WalletRegistration` and the wallet's locally-cached Merkle membership
/// path. The wallet calls this on every spend.
///
/// The caller is responsible for supplying a `RegistryWitness` whose
/// `merkle_path` resolves to the stamped `registry_root`; local POC
/// admission accepts that root through a permissive host checker.
pub fn client_prepare_with_registry(
    sk: &CoinSecretKey,
    coin: &QualifiedCoinInfo,
    contract: Option<ContractAddress>,
    _registration: &WalletRegistration,
    witness: &RegistryWitness,
) -> ClientHandoff {
    client_prepare_with_registry_root(sk, coin, contract, witness.registry_root_le_bytes())
}

/// Compute the canonical Zswap nullifier off-circuit. Must produce the same
/// 32 bytes that both the stock Zswap spend circuit and `sk_prove` disclose.
pub fn split_nullifier(coin: &CoinInfo, sk: &CoinSecretKey) -> Nullifier {
    coin.nullifier(&SenderEvidence::User(Cow::Borrowed(sk)))
}

/// Build the client-derivation preimage for Solution A's `sk_prove` circuit.
/// Witness layout matches the new circuit signature in declaration order:
///   (sk, pk, r, salt, coin, merkle_path)
///
///   • sk          → 2 Fr limbs   (Bytes<32>)
///   • pk          → 2 Fr limbs   (Bytes<32>)
///   • r           → 1 Fr         (blinding for C_sk)
///   • salt        → 1 Fr         (blinding for reg_leaf)
///   • coin        → 5 Fr         (nonce: 2, color: 2, value: 1)
///   • merkle_path → 2 + 20*2 Fr  (leaf: 2 limbs, 20 entries × 2 Fr each)
///
/// Total: 53 Fr private witnesses. The `merkle_path` lowering mirrors what
/// `MerklePath::field_repr` produces — leaf first (as ((), HashOutput) =
/// 2 limbs), then each `MerklePathEntry` as (sibling, goes_left).
pub fn build_client_derivation_preimage(
    sk: &CoinSecretKey,
    witness: &RegistryWitness,
    _coin: &QualifiedCoinInfo,
    handoff: &ClientHandoff,
) -> Result<ProofPreimage, String> {
    // The path must have exactly REGISTRY_TREE_HEIGHT entries — otherwise
    // the circuit's fixed-size MerkleTreePath<20, _> can't be constructed.
    if witness.merkle_path.path.len() != REGISTRY_TREE_HEIGHT as usize {
        return Err(format!(
            "merkle_path must have exactly {} entries (got {})",
            REGISTRY_TREE_HEIGHT,
            witness.merkle_path.path.len()
        ));
    }
    // Cross-check: the path's root must match the registry_root the wallet
    // is claiming. A path that resolves to a different root cannot satisfy
    // the circuit, so fail fast off-circuit. Use the no-leaf-hash variant
    // because the contract stored the leaf via `insertHash`.
    let leaf_bytes = witness.merkle_path.leaf.1;
    let derived_root = raw_leaf_hash_root(leaf_bytes, &witness.merkle_path);
    if derived_root != witness.registry_root {
        return Err(format!(
            "merkle_path resolves to {:?} but registry_root is {:?}",
            derived_root, witness.registry_root
        ));
    }
    // And the handoff's stamped 32-byte registry_root must match the digest.
    let expected_root_bytes = witness.registry_root_le_bytes();
    if handoff.registry_root != expected_root_bytes {
        return Err(
            "handoff.registry_root does not match witness.registry_root — wallet inconsistency"
                .to_string(),
        );
    }

    let mut inputs = Vec::new();
    // sk limbs.
    sk.0 .0.field_repr(&mut inputs);
    // pk limbs (private witness; constrained against sk in-circuit).
    handoff.pk.field_repr(&mut inputs);
    // Blinding r for the Poseidon C_sk open.
    inputs.push(witness.blinding);
    // Salt for the reg_leaf blinding.
    inputs.push(witness.salt);
    // Coin (nonce limbs, color limbs, value).
    handoff.coin_nonce.field_repr(&mut inputs);
    handoff.coin_color.field_repr(&mut inputs);
    inputs.push(Fr::from(handoff.coin_value));
    // Merkle path: leaf first, then 20 entries of (sibling, goes_left).
    witness.merkle_path.field_repr(&mut inputs);

    Ok(ProofPreimage {
        inputs,
        private_transcript: Vec::new(),
        public_transcript_inputs: client_derivation_public_transcript_inputs(handoff)?,
        public_transcript_outputs: Vec::new(),
        binding_input: 0.into(),
        communications_commitment: None,
        key_location: KeyLocation(Cow::Borrowed(CLIENT_DERIVATION_KEY_LOCATION)),
    })
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

pub fn client_derivation_public_transcript_inputs(
    handoff: &ClientHandoff,
) -> Result<Vec<Fr>, String> {
    Ok(client_derivation_public_transcript_inputs_from_parts(
        handoff.nullifier,
        try_handoff_coin_binding_tag_fr(handoff)?,
        try_handoff_registry_root_fr(handoff)?,
    ))
}

pub fn client_derivation_public_transcript_inputs_from_parts(
    nullifier: [u8; 32],
    coin_binding_tag: Fr,
    registry_root: Fr,
) -> Vec<Fr> {
    // Mirrors `sk_proof.compact` ledger declaration order:
    //   cell 0 → nullifier (Bytes<32>),
    //   cell 1 → coinBindingTag (Field),
    //   cell 2 → registryRoot (MerkleTreeDigest — lowers to a single Field cell
    //            with Field alignment in Compact).
    let mut inputs = Vec::new();
    extend_ops(
        &mut inputs,
        Cell_write!(
            [midnight_onchain_runtime::ops::Key::Value(0u8.into())],
            false,
            [u8; 32],
            nullifier
        ),
    );
    extend_ops(
        &mut inputs,
        Cell_write!(
            [midnight_onchain_runtime::ops::Key::Value(1u8.into())],
            false,
            Fr,
            coin_binding_tag
        ),
    );
    extend_ops(
        &mut inputs,
        Cell_write!(
            [midnight_onchain_runtime::ops::Key::Value(2u8.into())],
            false,
            Fr,
            registry_root
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
    try_handoff_coin_binding_tag_fr(handoff).expect("valid Fr from coin binding tag bytes")
}

pub fn try_handoff_coin_binding_tag_fr(handoff: &ClientHandoff) -> Result<Fr, String> {
    Fr::from_le_bytes(&handoff.coin_binding_tag)
        .ok_or_else(|| "invalid coin_binding_tag field element".to_string())
}

/// Reconstruct the Fr registry root from the handoff bytes.
pub fn try_handoff_registry_root_fr(handoff: &ClientHandoff) -> Result<Fr, String> {
    Fr::from_le_bytes(&handoff.registry_root)
        .ok_or_else(|| "invalid registry_root field element".to_string())
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

/// Refresh the wallet's locally-cached Merkle membership path. Production
/// wallets should call this whenever the registry tree grows past their
/// cached path's depth — see `tools/refresh_registry_path.mjs` for a
/// reference implementation that reads contract state via the indexer.
///
/// This stub mirrors the wallet-side API the rest of the SDK expects; the
/// concrete implementation lives in the `tools/` script layer because it
/// requires a live RPC connection to the indexer.
pub async fn refresh_registry_path(
    _registry_contract_address: ContractAddress,
    _registration: &WalletRegistration,
) -> Result<RegistryWitness, String> {
    Err(
        "refresh_registry_path: implement against the indexer/contract-state \
         API in tools/; see comments in src/client.rs for the wire format"
            .to_string(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use rand::Rng;

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
    fn client_prepare_produces_valid_handoff() {
        let sk = CoinSecretKey(OsRng.gen::<HashOutput>());
        let coin = QualifiedCoinInfo {
            value: 1000u64.into(),
            type_: Default::default(),
            nonce: OsRng.r#gen(),
            mt_index: 42,
        };

        let handoff = client_prepare(&sk, &coin, None);

        assert!(!handoff.nullifier.iter().all(|&b| b == 0));
        assert!(!handoff.pk.iter().all(|&b| b == 0));
        assert!(!handoff.commitment_hash.iter().all(|&b| b == 0));
        assert!(!handoff.coin_binding_tag.iter().all(|&b| b == 0));
        assert_eq!(handoff.mt_index, 42);
        // Registry root is the only wallet-identifying public value now.
        // `client_prepare` (without registration) stamps zero — production
        // callers must use `client_prepare_with_registry` to get a real root.
        assert_eq!(handoff.registry_root, [0u8; 32]);
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
        assert_eq!(handoff.registry_root, deserialized.registry_root);
        assert_eq!(handoff.mt_index, deserialized.mt_index);
    }

    #[test]
    fn client_prepare_preserves_token_type_bytes() {
        let sk = CoinSecretKey(OsRng.gen::<HashOutput>());
        let coin = QualifiedCoinInfo {
            value: 500u64.into(),
            type_: Default::default(),
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

        assert_eq!(h1.nullifier, h2.nullifier);
        assert_eq!(h1.pk, h2.pk);
        assert_eq!(h1.coin_binding_tag, h2.coin_binding_tag);
    }

    #[test]
    fn build_client_derivation_preimage_rejects_handoff_root_mismatch() {
        let sk = CoinSecretKey(OsRng.gen::<HashOutput>());
        let coin = QualifiedCoinInfo {
            value: 250u64.into(),
            type_: Default::default(),
            nonce: OsRng.r#gen(),
            mt_index: 7,
        };
        let r: Fr = OsRng.r#gen();
        let salt: Fr = OsRng.r#gen();
        let reg_leaf = crate::attestation::derive_reg_leaf_bytes(&sk, r, salt);
        let witness = RegistryWitness::for_first_registration(r, salt, reg_leaf)
            .expect("first-registration witness");
        // Stamp a *wrong* registry_root on the handoff so the cross-check fires.
        let mut handoff =
            client_prepare_with_registry_root(&sk, &coin, None, witness.registry_root_le_bytes());
        handoff.registry_root = [9u8; 32];

        let err = build_client_derivation_preimage(&sk, &witness, &coin, &handoff)
            .expect_err("handoff/witness root mismatch must fail preimage construction");
        assert!(
            err.contains("does not match witness.registry_root"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn build_client_derivation_preimage_checks_against_compact_ir() {
        let sk = CoinSecretKey(OsRng.gen::<HashOutput>());
        let coin = QualifiedCoinInfo {
            value: 250u64.into(),
            type_: Default::default(),
            nonce: OsRng.r#gen(),
            mt_index: 7,
        };
        let r: Fr = OsRng.r#gen();
        let salt: Fr = OsRng.r#gen();
        let reg_leaf = crate::attestation::derive_reg_leaf_bytes(&sk, r, salt);
        let witness = RegistryWitness::for_first_registration(r, salt, reg_leaf)
            .expect("first-registration witness");
        let handoff =
            client_prepare_with_registry_root(&sk, &coin, None, witness.registry_root_le_bytes());

        let preimage = build_client_derivation_preimage(&sk, &witness, &coin, &handoff)
            .expect("valid witness should build preimage");
        let ir = midnight_serialize::tagged_deserialize::<midnight_zkir::IrSource>(
            std::io::Cursor::new(include_bytes!(
                "../circuits/static/client-derivation/sk_prove.bzkir"
            )),
        )
        .expect("client derivation IR should load");

        preimage
            .check(&ir)
            .expect("honest Solution A witness should satisfy sk_prove");
    }

    // ─── Plan §Verification 2: tampering tests ──────────────────────────────
    //
    // Per the plan's Verification §2: confirm the per-spend circuit rejects
    //   - wrong `salt` → rejected
    //   - wrong `merkle_path` → rejected
    //   - right path against wrong root → rejected
    //   - honest path → accepted (covered by the previous test).
    //
    // Each tampering case uses an otherwise-honest witness, mutates one
    // field, and asserts that `preimage.check(&ir)` rejects the result.

    fn load_sk_prove_ir() -> midnight_zkir::IrSource {
        midnight_serialize::tagged_deserialize::<midnight_zkir::IrSource>(std::io::Cursor::new(
            include_bytes!("../circuits/static/client-derivation/sk_prove.bzkir"),
        ))
        .expect("client derivation IR should load")
    }

    #[test]
    fn build_client_derivation_preimage_rejects_wrong_salt() {
        let sk = CoinSecretKey(OsRng.gen::<HashOutput>());
        let coin = QualifiedCoinInfo {
            value: 250u64.into(),
            type_: Default::default(),
            nonce: OsRng.r#gen(),
            mt_index: 7,
        };
        let r: Fr = OsRng.r#gen();
        let registered_salt: Fr = OsRng.r#gen();
        let reg_leaf = crate::attestation::derive_reg_leaf_bytes(&sk, r, registered_salt);
        let honest = RegistryWitness::for_first_registration(r, registered_salt, reg_leaf)
            .expect("honest witness");

        // Build the handoff against the honest root, then swap in a witness
        // that uses a *different* salt. The preimage builder will reuse the
        // honest merkle_path but with the attacker's salt — the in-circuit
        // `regLeaf = transientHash(sep, C_sk, attacker_salt)` no longer
        // matches the path leaf.
        let handoff =
            client_prepare_with_registry_root(&sk, &coin, None, honest.registry_root_le_bytes());
        let attacker_salt: Fr = OsRng.r#gen();
        assert_ne!(attacker_salt, registered_salt);
        let bad_witness = RegistryWitness {
            blinding: honest.blinding,
            salt: attacker_salt,
            merkle_path: honest.merkle_path.clone(),
            registry_root: honest.registry_root,
        };

        let preimage = build_client_derivation_preimage(&sk, &bad_witness, &coin, &handoff)
            .expect("preimage construction itself succeeds (the path/root match honestly)");
        let ir = load_sk_prove_ir();
        assert!(
            preimage.check(&ir).is_err(),
            "wrong salt must not satisfy sk_prove"
        );
    }

    #[test]
    fn build_client_derivation_preimage_rejects_wrong_merkle_path() {
        let sk = CoinSecretKey(OsRng.gen::<HashOutput>());
        let coin = QualifiedCoinInfo {
            value: 250u64.into(),
            type_: Default::default(),
            nonce: OsRng.r#gen(),
            mt_index: 7,
        };
        let r: Fr = OsRng.r#gen();
        let salt: Fr = OsRng.r#gen();
        let reg_leaf = crate::attestation::derive_reg_leaf_bytes(&sk, r, salt);
        let honest =
            RegistryWitness::for_first_registration(r, salt, reg_leaf).expect("honest witness");

        // Build a *different* witness using an unrelated (sk', r', salt')
        // and graft its merkle_path onto the honest record. The path's leaf
        // is no longer the honest `upgradeFromTransient(reg_leaf)`, so
        // `assert(upgradeFromTransient(regLeaf) == merkle_path.leaf)` fails.
        let sk_other = CoinSecretKey(OsRng.gen::<HashOutput>());
        let r_other: Fr = OsRng.r#gen();
        let salt_other: Fr = OsRng.r#gen();
        let reg_leaf_other =
            crate::attestation::derive_reg_leaf_bytes(&sk_other, r_other, salt_other);
        let other = RegistryWitness::for_first_registration(r_other, salt_other, reg_leaf_other)
            .expect("other witness");

        let bad_witness = RegistryWitness {
            blinding: honest.blinding,
            salt: honest.salt,
            // Path from a *different* leaf, with the corresponding root
            // (otherwise the off-circuit cross-check fires first).
            merkle_path: other.merkle_path.clone(),
            registry_root: other.registry_root,
        };
        let handoff = client_prepare_with_registry_root(
            &sk,
            &coin,
            None,
            bad_witness.registry_root_le_bytes(),
        );

        let preimage = build_client_derivation_preimage(&sk, &bad_witness, &coin, &handoff)
            .expect("preimage construction succeeds (the off-circuit root/path agree)");
        let ir = load_sk_prove_ir();
        assert!(
            preimage.check(&ir).is_err(),
            "merkle_path for a different leaf must not satisfy sk_prove"
        );
    }

    #[test]
    fn build_client_derivation_preimage_rejects_right_path_wrong_root() {
        let sk = CoinSecretKey(OsRng.gen::<HashOutput>());
        let coin = QualifiedCoinInfo {
            value: 250u64.into(),
            type_: Default::default(),
            nonce: OsRng.r#gen(),
            mt_index: 7,
        };
        let r: Fr = OsRng.r#gen();
        let salt: Fr = OsRng.r#gen();
        let reg_leaf = crate::attestation::derive_reg_leaf_bytes(&sk, r, salt);
        let honest =
            RegistryWitness::for_first_registration(r, salt, reg_leaf).expect("honest witness");

        // Right path + wrong claimed root. `build_client_derivation_preimage`
        // catches this off-circuit before building the IR witnesses.
        let bad_witness = RegistryWitness {
            blinding: honest.blinding,
            salt: honest.salt,
            merkle_path: honest.merkle_path.clone(),
            registry_root: midnight_transient_crypto::merkle_tree::MerkleTreeDigest(Fr::from(
                0xdead_beef_u64,
            )),
        };
        // Stamp the *bad* root on the handoff too, so the handoff-vs-witness
        // cross-check passes and we land on the path-vs-claimed-root one.
        let handoff = client_prepare_with_registry_root(
            &sk,
            &coin,
            None,
            bad_witness.registry_root_le_bytes(),
        );

        let err = build_client_derivation_preimage(&sk, &bad_witness, &coin, &handoff)
            .expect_err("right path against wrong root must fail preimage construction");
        assert!(
            err.contains("merkle_path resolves to") && err.contains("but registry_root is"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn build_client_derivation_preimage_rejects_wrong_blinding() {
        // Bonus tampering test: a different blinding `r` means `C_sk` no
        // longer matches the leaf hash, so `assert(upgradeFromTransient(
        // transientHash(sep_reg, C_sk, salt)) == merkle_path.leaf)` fires
        // in-circuit.
        let sk = CoinSecretKey(OsRng.gen::<HashOutput>());
        let coin = QualifiedCoinInfo {
            value: 250u64.into(),
            type_: Default::default(),
            nonce: OsRng.r#gen(),
            mt_index: 7,
        };
        let registered_r: Fr = OsRng.r#gen();
        let salt: Fr = OsRng.r#gen();
        let reg_leaf = crate::attestation::derive_reg_leaf_bytes(&sk, registered_r, salt);
        let honest = RegistryWitness::for_first_registration(registered_r, salt, reg_leaf)
            .expect("honest witness");

        let attacker_r: Fr = OsRng.r#gen();
        assert_ne!(attacker_r, registered_r);
        let bad_witness = RegistryWitness {
            blinding: attacker_r,
            salt: honest.salt,
            merkle_path: honest.merkle_path.clone(),
            registry_root: honest.registry_root,
        };
        let handoff =
            client_prepare_with_registry_root(&sk, &coin, None, honest.registry_root_le_bytes());

        let preimage = build_client_derivation_preimage(&sk, &bad_witness, &coin, &handoff)
            .expect("preimage construction succeeds (off-circuit checks pass)");
        let ir = load_sk_prove_ir();
        assert!(
            preimage.check(&ir).is_err(),
            "wrong blinding r must not satisfy sk_prove"
        );
    }

    #[test]
    fn scalar_reduction_collision_does_not_reuse_registry_root() {
        // Solution A's spend-side soundness — a different 32-byte sk' that
        // is congruent to sk mod the Jubjub scalar order must still produce a
        // different reg_leaf, even with the same (r, salt). We exercise that
        // here by re-running `derive_reg_leaf` directly.
        use crate::attestation::derive_reg_leaf;
        let sk_honest = CoinSecretKey(HashOutput([0u8; 32]));
        let r: Fr = OsRng.r#gen();
        let salt: Fr = OsRng.r#gen();
        let (_, leaf_honest) = derive_reg_leaf(&sk_honest, r, salt);

        let sk_evil_bytes = JUBJUB_SCALAR_MODULUS_LE;
        assert_eq!(reduce_once_mod_jubjub_scalar(sk_honest.0 .0), [0u8; 32]);
        assert_eq!(reduce_once_mod_jubjub_scalar(sk_evil_bytes), [0u8; 32]);

        let sk_evil = CoinSecretKey(HashOutput(sk_evil_bytes));
        let (_, leaf_evil) = derive_reg_leaf(&sk_evil, r, salt);

        assert_ne!(
            leaf_honest, leaf_evil,
            "scalar-reduction-style sk' must produce a different reg_leaf"
        );
    }
}
