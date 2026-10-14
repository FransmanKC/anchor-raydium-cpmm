//! Addresses Raydium derives for a pool. Raydium checks them itself; these
//! are for building transactions and for `seeds::program` constraints.

use anchor_lang::prelude::*;

use crate::ids::*;

/// Raydium fee tier `index` (0 is the 0.25% tier on mainnet).
pub fn amm_config(index: u16) -> (Pubkey, u8) {
    Pubkey::find_program_address(&[AMM_CONFIG_SEED, &index.to_be_bytes()], &CPMM_PROGRAM_ID)
}

/// Raydium's authority over every pool's vaults and LP mint.
pub fn pool_authority() -> (Pubkey, u8) {
    Pubkey::find_program_address(&[AUTH_SEED], &CPMM_PROGRAM_ID)
}

pub fn lp_mint(pool: &Pubkey) -> (Pubkey, u8) {
    Pubkey::find_program_address(&[POOL_LP_MINT_SEED, pool.as_ref()], &CPMM_PROGRAM_ID)
}

/// The pool's token account for `mint`.
pub fn vault(pool: &Pubkey, mint: &Pubkey) -> (Pubkey, u8) {
    Pubkey::find_program_address(
        &[POOL_VAULT_SEED, pool.as_ref(), mint.as_ref()],
        &CPMM_PROGRAM_ID,
    )
}

/// The pool's price oracle.
pub fn observation(pool: &Pubkey) -> (Pubkey, u8) {
    Pubkey::find_program_address(&[OBSERVATION_SEED, pool.as_ref()], &CPMM_PROGRAM_ID)
}

#[cfg(all(test, not(feature = "devnet")))]
mod tests {
    #[test]
    fn amm_config_matches_mainnet_tier_0() {
        assert_eq!(
            super::amm_config(0).0.to_string(),
            "D4FPEruKEHrG5TenZ2mpDGEfu1iUvTiqBxvpU8HLBvC2"
        );
    }
}
