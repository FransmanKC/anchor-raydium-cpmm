//! Addresses Raydium derives for a pool. Raydium checks them itself; these
//! are for building transactions and for `seeds::program` constraints.

use anchor_lang::prelude::*;

use crate::ids::*;

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
