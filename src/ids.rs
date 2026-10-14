//! Raydium CPMM addresses and layout constants.
//!
//! Mainnet by default; the `devnet` cargo feature switches to Raydium's
//! devnet deployment. They are compile-time constants so a program can never
//! be pointed at another Raydium (or an impostor) at runtime.

use anchor_lang::prelude::*;

/// Raydium CPMM (constant-product) program.
#[cfg(not(feature = "devnet"))]
pub const CPMM_PROGRAM_ID: Pubkey = pubkey!("CPMMoo8L3F4NbTegBCKVNunggL7H1ZpdTHKxQB5qKP1C");
/// Raydium CPMM (constant-product) program.
#[cfg(feature = "devnet")]
pub const CPMM_PROGRAM_ID: Pubkey = pubkey!("DRaycpLY18LhpbydsBWbVJtxpNv9oXPgjRSfpF2bWpYb");

/// Receives Raydium's pool-creation fee (a wrapped-SOL token account).
#[cfg(not(feature = "devnet"))]
pub const CREATE_POOL_FEE_RECEIVER: Pubkey =
    pubkey!("DNXgeM9EiiaAbaWvwjHj9fQQLAX5ZsfHyvmYUNRAdNC8");
/// Receives Raydium's pool-creation fee (a wrapped-SOL token account).
#[cfg(feature = "devnet")]
pub const CREATE_POOL_FEE_RECEIVER: Pubkey =
    pubkey!("3oE58BKVt8KuYkGxx8zBojugnymWmBiyafWgMrnb6eYy");

pub const AUTH_SEED: &[u8] = b"vault_and_lp_mint_auth_seed";
pub const POOL_LP_MINT_SEED: &[u8] = b"pool_lp_mint";
pub const POOL_VAULT_SEED: &[u8] = b"pool_vault";
pub const OBSERVATION_SEED: &[u8] = b"observation";

/// Anchor discriminator of Raydium's `initialize`: sha256("global:initialize")[..8].
pub const INITIALIZE_DISCRIMINATOR: [u8; 8] = [175, 175, 109, 31, 13, 152, 155, 237];

/// Byte offset of `create_pool_fee: u64` in Raydium's `AmmConfig` account
/// (8 discriminator + bump u8 + disable_create_pool bool + index u16 +
/// trade/protocol/fund fee rates 3 x u64).
pub const AMM_CONFIG_CREATE_POOL_FEE_OFFSET: usize = 36;

#[cfg(test)]
mod tests {
    use super::*;

    /// Guards against the two clusters' addresses being swapped. CI runs
    /// this with and without `--features devnet`.
    #[test]
    fn addresses_match_the_cluster() {
        let (program, receiver) = if cfg!(feature = "devnet") {
            (
                "DRaycpLY18LhpbydsBWbVJtxpNv9oXPgjRSfpF2bWpYb",
                "3oE58BKVt8KuYkGxx8zBojugnymWmBiyafWgMrnb6eYy",
            )
        } else {
            (
                "CPMMoo8L3F4NbTegBCKVNunggL7H1ZpdTHKxQB5qKP1C",
                "DNXgeM9EiiaAbaWvwjHj9fQQLAX5ZsfHyvmYUNRAdNC8",
            )
        };
        assert_eq!(CPMM_PROGRAM_ID.to_string(), program);
        assert_eq!(CREATE_POOL_FEE_RECEIVER.to_string(), receiver);
    }
}
