//! Split-Prove SDK: client/server split for ZK proving without exposing sk.
//!
//! # Client side (wallet)
//! ```ignore
//! let handoff = client_prepare(&sk, &coin, None);
//! // serialize and POST to /v2/prove
//! ```
//!
//! # Server side (prover)
//! ```ignore
//! let input = server_build_spend(&handoff, &tree)?;
//! let proof = server_prove_split(&ir, &params, &pk, &input.proof, sk_field_count)?;
//! ```

pub mod client;
pub mod server;
