//! Minimal launchpad showing how to graduate with `anchor-raydium-cpmm`.
//!
//! `create_launch` stands in for a sold-out bonding curve: it records the SOL
//! and tokens that will seed the pool. Graduation is then two instructions
//! sent in one transaction: `prepare` moves the tracked SOL to the
//! graduation authority, `graduate` creates the pool and locks it.

use anchor_lang::{
    prelude::*,
    system_program::{transfer, Transfer},
};
use anchor_raydium_cpmm::{ids, GraduateAccounts, GraduateParams, LpLock};
use anchor_spl::{
    associated_token::{get_associated_token_address_with_program_id, AssociatedToken},
    token::{spl_token::native_mint, Token},
    token_interface::{self, Mint, TokenAccount, TokenInterface},
};

declare_id!("EBLM5gfpUpxDnei7B9Hggqp8K8MNMDbiwKhrYhad9Qf5");

pub const LAUNCH_SEED: &[u8] = b"launch";
/// System-owned PDA that signs as the Raydium pool creator.
pub const GRADUATION_SEED: &[u8] = b"graduation";
/// PDA used as the pool address, so nobody can create the pool first.
pub const POOL_SEED: &[u8] = b"pool";

#[program]
pub mod launchpad {
    use super::*;

    /// Records a launch ready to graduate, funded by the creator. A real
    /// launchpad gets here by selling out its curve.
    pub fn create_launch(
        ctx: Context<CreateLaunch>,
        sol_amount: u64,
        token_amount: u64,
    ) -> Result<()> {
        let a = &mut *ctx.accounts;
        a.launch.set_inner(Launch {
            mint: a.mint.key(),
            sol_amount,
            token_amount,
            prepared: false,
            pool: None,
            bump: ctx.bumps.launch,
        });
        transfer(
            CpiContext::new(
                a.system_program.key(),
                Transfer {
                    from: a.creator.to_account_info(),
                    to: a.launch.to_account_info(),
                },
            ),
            sol_amount,
        )?;
        token_interface::transfer_checked(
            CpiContext::new(
                a.token_program.key(),
                token_interface::TransferChecked {
                    from: a.creator_token_account.to_account_info(),
                    mint: a.mint.to_account_info(),
                    to: a.vault.to_account_info(),
                    authority: a.creator.to_account_info(),
                },
            ),
            token_amount,
            a.mint.decimals,
        )
    }

    /// Step 1 of graduation. The launch account holds data, so its lamports
    /// can only be moved directly, and Solana rejects a direct move followed
    /// by a CPI that doesn't include both accounts. Doing it in its own
    /// instruction lets `graduate` use ordinary CPIs throughout.
    pub fn prepare(ctx: Context<Prepare>) -> Result<()> {
        let launch = &mut ctx.accounts.launch;
        require!(launch.pool.is_none(), LaunchpadError::AlreadyGraduated);
        require!(!launch.prepared, LaunchpadError::AlreadyPrepared);
        launch.prepared = true;
        let amount = launch.sol_amount;
        launch.sub_lamports(amount)?;
        ctx.accounts.graduation_authority.add_lamports(amount)?;
        Ok(())
    }

    /// Step 2 of graduation: permissionless. The payer fronts Raydium's
    /// fee and gets back what isn't spent. Burns the LP tokens, unless
    /// both LP recipient accounts are given.
    pub fn graduate(ctx: Context<Graduate>) -> Result<()> {
        let a = &mut *ctx.accounts;
        require!(a.launch.pool.is_none(), LaunchpadError::AlreadyGraduated);
        require!(a.launch.prepared, LaunchpadError::NotPrepared);

        let mint_key = a.mint.key();
        let authority_seeds: &[&[u8]] = &[
            GRADUATION_SEED,
            mint_key.as_ref(),
            &[ctx.bumps.graduation_authority],
        ];
        let pool_seeds: &[&[u8]] = &[POOL_SEED, mint_key.as_ref(), &[ctx.bumps.pool_state]];
        let launch_seeds: &[&[u8]] = &[LAUNCH_SEED, mint_key.as_ref(), &[a.launch.bump]];
        let lp_lock = match (&a.lp_recipient, &a.lp_recipient_token_account) {
            (Some(owner), Some(token_account)) => LpLock::SendTo {
                owner: owner.to_account_info(),
                token_account: token_account.to_account_info(),
            },
            _ => LpLock::Burn,
        };

        let outcome = anchor_raydium_cpmm::graduate(
            GraduateAccounts {
                payer: a.payer.to_account_info(),
                authority: a.graduation_authority.to_account_info(),
                base_mint: a.mint.to_account_info(),
                token_source: a.vault.to_account_info(),
                token_source_authority: a.launch.to_account_info(),
                authority_base_account: a.authority_base_account.to_account_info(),
                authority_wsol_account: a.authority_wsol_account.to_account_info(),
                authority_lp_account: a.authority_lp_account.to_account_info(),
                pool_state: a.pool_state.to_account_info(),
                amm_config: a.amm_config.to_account_info(),
                raydium_authority: a.raydium_authority.to_account_info(),
                lp_mint: a.lp_mint.to_account_info(),
                base_vault: a.base_vault.to_account_info(),
                wsol_vault: a.wsol_vault.to_account_info(),
                observation_state: a.observation_state.to_account_info(),
                create_pool_fee_receiver: a.create_pool_fee_receiver.to_account_info(),
                wsol_mint: a.wsol_mint.to_account_info(),
                token_program: a.token_program.to_account_info(),
                base_token_program: a.base_token_program.to_account_info(),
                associated_token_program: a.associated_token_program.to_account_info(),
                system_program: a.system_program.to_account_info(),
                rent: a.rent.to_account_info(),
                raydium_program: a.raydium_program.to_account_info(),
            },
            GraduateParams {
                sol_amount: a.launch.sol_amount,
                token_amount: a.launch.token_amount,
                authority_seeds,
                pool_seeds,
                token_source_seeds: launch_seeds,
                lp_lock,
                open_time: 0,
            },
        )?;
        msg!(
            "graduated: lp_amount={} cost={}",
            outcome.lp_amount,
            outcome.cost
        );
        a.launch.pool = Some(outcome.pool);
        Ok(())
    }
}

#[account]
#[derive(InitSpace)]
pub struct Launch {
    pub mint: Pubkey,
    /// Tracked amounts: the pool gets exactly these, whatever the balances.
    pub sol_amount: u64,
    pub token_amount: u64,
    pub prepared: bool,
    pub pool: Option<Pubkey>,
    pub bump: u8,
}

#[error_code]
pub enum LaunchpadError {
    #[msg("Already graduated")]
    AlreadyGraduated,
    #[msg("Already prepared")]
    AlreadyPrepared,
    #[msg("Run prepare first, in the same transaction")]
    NotPrepared,
}

#[derive(Accounts)]
pub struct CreateLaunch<'info> {
    #[account(mut)]
    pub creator: Signer<'info>,
    pub mint: InterfaceAccount<'info, Mint>,
    #[account(
        init,
        payer = creator,
        space = 8 + Launch::INIT_SPACE,
        seeds = [LAUNCH_SEED, mint.key().as_ref()],
        bump,
    )]
    pub launch: Account<'info, Launch>,
    #[account(
        init,
        payer = creator,
        associated_token::mint = mint,
        associated_token::authority = launch,
        associated_token::token_program = token_program,
    )]
    pub vault: InterfaceAccount<'info, TokenAccount>,
    #[account(mut, token::mint = mint, token::authority = creator, token::token_program = token_program)]
    pub creator_token_account: InterfaceAccount<'info, TokenAccount>,
    pub token_program: Interface<'info, TokenInterface>,
    pub associated_token_program: Program<'info, AssociatedToken>,
    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
pub struct Prepare<'info> {
    #[account(mut, seeds = [LAUNCH_SEED, launch.mint.as_ref()], bump = launch.bump)]
    pub launch: Account<'info, Launch>,
    #[account(mut, seeds = [GRADUATION_SEED, launch.mint.as_ref()], bump)]
    pub graduation_authority: SystemAccount<'info>,
}

/// The constraints a real program should copy: every address is pinned, so
/// a bad transaction fails here with a clear error.
#[derive(Accounts)]
pub struct Graduate<'info> {
    #[account(mut)]
    pub payer: Signer<'info>,
    #[account(
        mut,
        seeds = [LAUNCH_SEED, mint.key().as_ref()],
        bump = launch.bump,
        has_one = mint,
    )]
    pub launch: Box<Account<'info, Launch>>,
    /// Writable only for burning tokens someone planted in the authority's
    /// token account (burning lowers the supply).
    #[account(mut, mint::token_program = base_token_program)]
    pub mint: Box<InterfaceAccount<'info, Mint>>,
    #[account(
        mut,
        associated_token::mint = mint,
        associated_token::authority = launch,
        associated_token::token_program = base_token_program,
    )]
    pub vault: Box<InterfaceAccount<'info, TokenAccount>>,
    #[account(mut, seeds = [GRADUATION_SEED, mint.key().as_ref()], bump)]
    pub graduation_authority: SystemAccount<'info>,
    /// CHECK: created and closed by the graduation. Address enforced.
    #[account(
        mut,
        address = get_associated_token_address_with_program_id(
            &graduation_authority.key(), &mint.key(), &base_token_program.key()),
    )]
    pub authority_base_account: UncheckedAccount<'info>,
    /// CHECK: created and closed by the graduation. Address enforced.
    #[account(
        mut,
        address = get_associated_token_address_with_program_id(
            &graduation_authority.key(), &native_mint::ID, &token_program.key()),
    )]
    pub authority_wsol_account: UncheckedAccount<'info>,
    /// CHECK: Raydium creates it, the graduation closes it. Address enforced.
    #[account(
        mut,
        address = get_associated_token_address_with_program_id(
            &graduation_authority.key(), &lp_mint.key(), &token_program.key()),
    )]
    pub authority_lp_account: UncheckedAccount<'info>,
    /// CHECK: the pool address, a PDA of this program; Raydium creates it.
    #[account(mut, seeds = [POOL_SEED, mint.key().as_ref()], bump)]
    pub pool_state: UncheckedAccount<'info>,
    /// CHECK: fixed program ID, so this program never CPIs anywhere else.
    #[account(address = ids::CPMM_PROGRAM_ID, executable)]
    pub raydium_program: UncheckedAccount<'info>,
    /// CHECK: any Raydium fee tier; the crate checks its owner. A real
    /// launchpad would pin the one it chose.
    pub amm_config: UncheckedAccount<'info>,
    /// CHECK: Raydium's vault and LP-mint authority.
    #[account(seeds = [ids::AUTH_SEED], bump, seeds::program = ids::CPMM_PROGRAM_ID)]
    pub raydium_authority: UncheckedAccount<'info>,
    /// CHECK: Raydium creates it.
    #[account(
        mut,
        seeds = [ids::POOL_LP_MINT_SEED, pool_state.key().as_ref()],
        bump,
        seeds::program = ids::CPMM_PROGRAM_ID,
    )]
    pub lp_mint: UncheckedAccount<'info>,
    /// CHECK: Raydium creates it.
    #[account(
        mut,
        seeds = [ids::POOL_VAULT_SEED, pool_state.key().as_ref(), mint.key().as_ref()],
        bump,
        seeds::program = ids::CPMM_PROGRAM_ID,
    )]
    pub base_vault: UncheckedAccount<'info>,
    /// CHECK: Raydium creates it.
    #[account(
        mut,
        seeds = [ids::POOL_VAULT_SEED, pool_state.key().as_ref(), native_mint::ID.as_ref()],
        bump,
        seeds::program = ids::CPMM_PROGRAM_ID,
    )]
    pub wsol_vault: UncheckedAccount<'info>,
    /// CHECK: Raydium creates it.
    #[account(
        mut,
        seeds = [ids::OBSERVATION_SEED, pool_state.key().as_ref()],
        bump,
        seeds::program = ids::CPMM_PROGRAM_ID,
    )]
    pub observation_state: UncheckedAccount<'info>,
    /// CHECK: Raydium's fee receiver.
    #[account(mut, address = ids::CREATE_POOL_FEE_RECEIVER)]
    pub create_pool_fee_receiver: UncheckedAccount<'info>,
    #[account(address = native_mint::ID)]
    pub wsol_mint: Box<InterfaceAccount<'info, Mint>>,
    /// CHECK: receives the LP tokens instead of burning them (optional).
    pub lp_recipient: Option<UncheckedAccount<'info>>,
    /// CHECK: `lp_recipient`'s LP token account; the ATA program checks the
    /// address when creating it.
    #[account(mut)]
    pub lp_recipient_token_account: Option<UncheckedAccount<'info>>,
    pub token_program: Program<'info, Token>,
    pub base_token_program: Interface<'info, TokenInterface>,
    pub associated_token_program: Program<'info, AssociatedToken>,
    pub system_program: Program<'info, System>,
    pub rent: Sysvar<'info, Rent>,
}
