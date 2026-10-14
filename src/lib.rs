//! Graduate a token from an Anchor program into a Raydium CPMM pool and lock
//! the liquidity: the last step of every bonding-curve launchpad.
//!
//! [`graduate`] wraps exactly the SOL you name, moves exactly the tokens you
//! name, creates the pool at an address only your program can sign for,
//! burns (or delivers) the LP tokens, and cleans up after itself. See the
//! README for the two-instruction flow and `examples/launchpad` for a full
//! program.
//!
//! **Status: educational.** Not audited and not yet meant for mainnet. Build
//! and test with it on devnet or localnet, and review it yourself first.

use anchor_lang::{
    prelude::*,
    solana_program::{
        instruction::{AccountMeta, Instruction},
        program::invoke_signed,
    },
    system_program::{transfer, Transfer},
};
use anchor_spl::{
    associated_token::{create_idempotent, Create},
    token::{self, spl_token::native_mint},
    token_2022,
    token_interface::{self, Mint, TokenAccount},
};

pub mod ids;
pub mod pda;

/// SOL the payer fronts for rent on top of Raydium's pool-creation fee;
/// whatever isn't spent is refunded in the same instruction.
pub const RENT_ALLOWANCE: u64 = 100_000_000;

/// Offset keeps these codes clear of the calling program's own (6000+).
#[error_code(offset = 9000)]
pub enum GraduateError {
    #[msg("AmmConfig is not a Raydium CPMM account")]
    InvalidAmmConfig,
    #[msg("Account is not the expected program or mint")]
    InvalidAccount,
    #[msg("SOL and token amounts must both be above zero")]
    ZeroAmount,
    #[msg("Authority holds less SOL than sol_amount")]
    InsufficientSol,
    #[msg("Arithmetic overflow")]
    MathOverflow,
}

/// Reads `create_pool_fee` from a Raydium `AmmConfig` account, after
/// checking Raydium owns it (anyone can create an account with that layout).
pub fn create_pool_fee(amm_config: &AccountInfo) -> Result<u64> {
    require_keys_eq!(
        *amm_config.owner,
        ids::CPMM_PROGRAM_ID,
        GraduateError::InvalidAmmConfig
    );
    let data = amm_config.try_borrow_data()?;
    let bytes: [u8; 8] = data
        .get(ids::AMM_CONFIG_CREATE_POOL_FEE_OFFSET..ids::AMM_CONFIG_CREATE_POOL_FEE_OFFSET + 8)
        .and_then(|b| b.try_into().ok())
        .ok_or(GraduateError::InvalidAmmConfig)?;
    Ok(u64::from_le_bytes(bytes))
}

/// Accounts [`graduate`] needs. Address checks on the Raydium PDAs and the
/// temporary token accounts are done by Raydium and the Associated Token
/// program; your `#[derive(Accounts)]` should still pin them (see the
/// example) so a bad transaction fails early with a clear error.
pub struct GraduateAccounts<'info> {
    /// Signer who fronts Raydium's fee plus [`RENT_ALLOWANCE`]; gets back
    /// what isn't spent.
    pub payer: AccountInfo<'info>,
    /// System-owned PDA of your program that signs as the pool creator.
    /// Must already hold `sol_amount` (move it in an earlier instruction).
    /// Dedicate it to graduation: everything it holds besides `sol_amount`
    /// is refunded to `payer`.
    pub authority: AccountInfo<'info>,
    /// The token being graduated (SPL Token or Token-2022). Writable: stray
    /// tokens are burned.
    pub base_mint: AccountInfo<'info>,
    /// Holds the tokens to graduate.
    pub token_source: AccountInfo<'info>,
    /// Owner of `token_source`; signs with `token_source_seeds`.
    pub token_source_authority: AccountInfo<'info>,
    /// `authority`'s associated token account for `base_mint`; created and
    /// closed here.
    pub authority_base_account: AccountInfo<'info>,
    /// `authority`'s associated wrapped-SOL account; created and closed here.
    pub authority_wsol_account: AccountInfo<'info>,
    /// `authority`'s associated LP token account; Raydium creates it, it is
    /// emptied and closed here.
    pub authority_lp_account: AccountInfo<'info>,
    /// The pool address: a PDA of your program (signs with `pool_seeds`), so
    /// nobody can create your pool first and set its price.
    pub pool_state: AccountInfo<'info>,
    /// Raydium fee tier; its owner is checked.
    pub amm_config: AccountInfo<'info>,
    /// [`pda::pool_authority`].
    pub raydium_authority: AccountInfo<'info>,
    /// [`pda::lp_mint`].
    pub lp_mint: AccountInfo<'info>,
    /// [`pda::vault`] for `base_mint`.
    pub base_vault: AccountInfo<'info>,
    /// [`pda::vault`] for wrapped SOL.
    pub wsol_vault: AccountInfo<'info>,
    /// [`pda::observation`].
    pub observation_state: AccountInfo<'info>,
    /// [`ids::CREATE_POOL_FEE_RECEIVER`].
    pub create_pool_fee_receiver: AccountInfo<'info>,
    pub wsol_mint: AccountInfo<'info>,
    /// SPL Token: wrapped SOL and the LP mint live here.
    pub token_program: AccountInfo<'info>,
    /// The program that owns `base_mint`: SPL Token or Token-2022.
    pub base_token_program: AccountInfo<'info>,
    pub associated_token_program: AccountInfo<'info>,
    pub system_program: AccountInfo<'info>,
    pub rent: AccountInfo<'info>,
    /// [`ids::CPMM_PROGRAM_ID`].
    pub raydium_program: AccountInfo<'info>,
}

/// What happens to the LP tokens.
pub enum LpLock<'info> {
    /// Burn them all: nobody, you included, can ever pull the liquidity.
    Burn,
    /// Send them to `owner`'s associated LP token account (`token_account`),
    /// created here if needed. Use for a locker program or a multisig.
    SendTo {
        owner: AccountInfo<'info>,
        token_account: AccountInfo<'info>,
    },
}

pub struct GraduateParams<'a, 'info> {
    /// SOL for the pool. Pass your tracked amount, never a balance: anyone
    /// can send SOL to an account.
    pub sol_amount: u64,
    /// Tokens for the pool. Tracked amount, never a balance.
    pub token_amount: u64,
    /// Seeds (with bump) of `authority`.
    pub authority_seeds: &'a [&'a [u8]],
    /// Seeds (with bump) of `pool_state`.
    pub pool_seeds: &'a [&'a [u8]],
    /// Seeds (with bump) of `token_source_authority`; empty if it signs the
    /// transaction itself.
    pub token_source_seeds: &'a [&'a [u8]],
    pub lp_lock: LpLock<'info>,
    /// Unix time trading opens; 0 for as soon as possible.
    pub open_time: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GraduateOutcome {
    pub pool: Pubkey,
    /// LP tokens minted to the authority (then burned or sent).
    pub lp_amount: u64,
    /// Lamports the payer spent net of refunds: Raydium's fee plus rent of
    /// the pool accounts (and of the LP recipient's account, if created).
    /// 0 if donations to the authority left the payer ahead.
    pub cost: u64,
}

/// Creates a Raydium CPMM pool with exactly `sol_amount` SOL and
/// `token_amount` tokens, so it opens at exactly that price, and locks the
/// liquidity per `lp_lock`.
///
/// Call it as the only cross-program work in its instruction's tail: the SOL
/// must reach `authority` in an earlier instruction (Solana rejects a direct
/// lamport move followed by a CPI that doesn't include both accounts).
pub fn graduate<'info>(
    a: GraduateAccounts<'info>,
    p: GraduateParams<'_, 'info>,
) -> Result<GraduateOutcome> {
    require!(
        p.sol_amount > 0 && p.token_amount > 0,
        GraduateError::ZeroAmount
    );
    let base_program = a.base_token_program.key();
    require!(
        a.raydium_program.key() == ids::CPMM_PROGRAM_ID
            && a.wsol_mint.key() == native_mint::ID
            && a.token_program.key() == token::ID
            && (base_program == token::ID || base_program == token_2022::ID)
            && *a.base_mint.owner == base_program,
        GraduateError::InvalidAccount
    );
    let auth = &a.authority;
    let auth_signer: &[&[&[u8]]] = &[p.authority_seeds];

    // 1. The payer fronts the cost of creating the pool.
    let budget = create_pool_fee(&a.amm_config)?
        .checked_add(RENT_ALLOWANCE)
        .ok_or(GraduateError::MathOverflow)?;
    // What the authority held besides the pool's SOL (normally nothing;
    // anything someone sent it is refunded to the payer with the rest).
    let start_extra = auth
        .lamports()
        .checked_sub(p.sol_amount)
        .ok_or(GraduateError::InsufficientSol)?;
    transfer(
        CpiContext::new(
            a.system_program.key(),
            Transfer {
                from: a.payer.clone(),
                to: auth.clone(),
            },
        ),
        budget,
    )?;

    // 2. Temporary token accounts for the authority. Idempotent: someone
    // may have created them already to get in the way.
    for (account, mint, program) in [
        (
            &a.authority_base_account,
            &a.base_mint,
            &a.base_token_program,
        ),
        (&a.authority_wsol_account, &a.wsol_mint, &a.token_program),
    ] {
        create_idempotent(CpiContext::new_with_signer(
            a.associated_token_program.key(),
            Create {
                payer: auth.clone(),
                associated_token: account.clone(),
                authority: auth.clone(),
                mint: mint.clone(),
                system_program: a.system_program.clone(),
                token_program: program.clone(),
            },
            auth_signer,
        ))?;
    }

    // 3. Exactly `sol_amount`, wrapped, and exactly `token_amount`.
    transfer(
        CpiContext::new_with_signer(
            a.system_program.key(),
            Transfer {
                from: auth.clone(),
                to: a.authority_wsol_account.clone(),
            },
            auth_signer,
        ),
        p.sol_amount,
    )?;
    token_interface::sync_native(CpiContext::new(
        token::ID,
        token_interface::SyncNative {
            account: a.authority_wsol_account.clone(),
        },
    ))?;
    let source_signer: &[&[&[u8]]] = if p.token_source_seeds.is_empty() {
        &[]
    } else {
        &[p.token_source_seeds]
    };
    token_interface::transfer_checked(
        CpiContext::new_with_signer(
            base_program,
            token_interface::TransferChecked {
                from: a.token_source.clone(),
                mint: a.base_mint.clone(),
                to: a.authority_base_account.clone(),
                authority: a.token_source_authority.clone(),
            },
            source_signer,
        ),
        p.token_amount,
        mint_decimals(&a.base_mint)?,
    )?;

    // 4. Create the pool. Raydium orders the two mints by address.
    let base_first = a.base_mint.key() < native_mint::ID;
    let (amount_0, amount_1) = if base_first {
        (p.token_amount, p.sol_amount)
    } else {
        (p.sol_amount, p.token_amount)
    };
    let mut data = ids::INITIALIZE_DISCRIMINATOR.to_vec();
    data.extend_from_slice(&amount_0.to_le_bytes());
    data.extend_from_slice(&amount_1.to_le_bytes());
    data.extend_from_slice(&p.open_time.to_le_bytes());
    let base_side = [
        &a.base_mint,
        &a.authority_base_account,
        &a.base_vault,
        &a.base_token_program,
    ];
    let sol_side = [
        &a.wsol_mint,
        &a.authority_wsol_account,
        &a.wsol_vault,
        &a.token_program,
    ];
    let (s0, s1) = if base_first {
        (base_side, sol_side)
    } else {
        (sol_side, base_side)
    };
    // Order, writability and signers exactly as Raydium's `Initialize`
    // declares them; the creator (0) and pool address (3) sign.
    let metas: [(&AccountInfo<'info>, bool); 20] = [
        (auth, true),
        (&a.amm_config, false),
        (&a.raydium_authority, false),
        (&a.pool_state, true),
        (s0[0], false),
        (s1[0], false),
        (&a.lp_mint, true),
        (s0[1], true),
        (s1[1], true),
        (&a.authority_lp_account, true),
        (s0[2], true),
        (s1[2], true),
        (&a.create_pool_fee_receiver, true),
        (&a.observation_state, true),
        (&a.token_program, false),
        (s0[3], false),
        (s1[3], false),
        (&a.associated_token_program, false),
        (&a.system_program, false),
        (&a.rent, false),
    ];
    let accounts = metas
        .iter()
        .enumerate()
        .map(|(i, (info, writable))| {
            let signer = i == 0 || i == 3;
            if *writable {
                AccountMeta::new(info.key(), signer)
            } else {
                AccountMeta::new_readonly(info.key(), signer)
            }
        })
        .collect();
    let mut infos: Vec<AccountInfo<'info>> = metas.iter().map(|(i, _)| (*i).clone()).collect();
    infos.push(a.raydium_program.clone());
    invoke_signed(
        &Instruction {
            program_id: ids::CPMM_PROGRAM_ID,
            accounts,
            data,
        },
        &infos,
        &[p.authority_seeds, p.pool_seeds],
    )?;

    // 5. Lock the liquidity.
    let lp_amount = token_balance(&a.authority_lp_account)?;
    match &p.lp_lock {
        LpLock::Burn => token_interface::burn(
            CpiContext::new_with_signer(
                token::ID,
                token_interface::Burn {
                    mint: a.lp_mint.clone(),
                    from: a.authority_lp_account.clone(),
                    authority: auth.clone(),
                },
                auth_signer,
            ),
            lp_amount,
        )?,
        LpLock::SendTo {
            owner,
            token_account,
        } => {
            create_idempotent(CpiContext::new_with_signer(
                a.associated_token_program.key(),
                Create {
                    payer: auth.clone(),
                    associated_token: token_account.clone(),
                    authority: owner.clone(),
                    mint: a.lp_mint.clone(),
                    system_program: a.system_program.clone(),
                    token_program: a.token_program.clone(),
                },
                auth_signer,
            ))?;
            token_interface::transfer_checked(
                CpiContext::new_with_signer(
                    token::ID,
                    token_interface::TransferChecked {
                        from: a.authority_lp_account.clone(),
                        mint: a.lp_mint.clone(),
                        to: token_account.clone(),
                        authority: auth.clone(),
                    },
                    auth_signer,
                ),
                lp_amount,
                mint_decimals(&a.lp_mint)?,
            )?;
        }
    }

    // 6. Close the temporary accounts, returning their rent. Tokens someone
    // sent to the authority's base account would block closing it, so burn
    // them first. (Extra wrapped SOL doesn't block: closing unwraps it.)
    let stray_tokens = token_balance(&a.authority_base_account)?;
    if stray_tokens > 0 {
        token_interface::burn(
            CpiContext::new_with_signer(
                base_program,
                token_interface::Burn {
                    mint: a.base_mint.clone(),
                    from: a.authority_base_account.clone(),
                    authority: auth.clone(),
                },
                auth_signer,
            ),
            stray_tokens,
        )?;
    }
    for (account, program) in [
        (&a.authority_base_account, base_program),
        (&a.authority_wsol_account, token::ID),
        (&a.authority_lp_account, token::ID),
    ] {
        token_interface::close_account(CpiContext::new_with_signer(
            program,
            token_interface::CloseAccount {
                account: account.clone(),
                destination: auth.clone(),
                authority: auth.clone(),
            },
            auth_signer,
        ))?;
    }

    // 7. Refund what's left to the payer.
    let leftover = auth.lamports();
    transfer(
        CpiContext::new_with_signer(
            a.system_program.key(),
            Transfer {
                from: auth.clone(),
                to: a.payer.clone(),
            },
            auth_signer,
        ),
        leftover,
    )?;
    // Saturating: wrapped SOL someone planted in the temporary account comes
    // back on closing it, so the payer can end up ahead. That must not
    // block graduation.
    let cost = start_extra
        .checked_add(budget)
        .ok_or(GraduateError::MathOverflow)?
        .saturating_sub(leftover);

    Ok(GraduateOutcome {
        pool: a.pool_state.key(),
        lp_amount,
        cost,
    })
}

fn mint_decimals(mint: &AccountInfo) -> Result<u8> {
    Ok(Mint::try_deserialize(&mut &mint.try_borrow_data()?[..])?.decimals)
}

fn token_balance(account: &AccountInfo) -> Result<u64> {
    Ok(TokenAccount::try_deserialize(&mut &account.try_borrow_data()?[..])?.amount)
}
