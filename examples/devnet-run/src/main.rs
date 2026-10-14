//! Graduates one launch of the example program on devnet, then reads the
//! pool back and checks it. Spends about 0.6 devnet SOL.
//!
//! ```bash
//! cargo run --manifest-path examples/devnet-run/Cargo.toml -- ~/.config/solana/devnet.json
//! ```
//!
//! The example program must already be deployed at `launchpad::ID`, built
//! with `--features devnet` (see the README).

use anchor_lang::{
    prelude::Pubkey,
    solana_program::{instruction::Instruction, program_pack::Pack, system_instruction},
    system_program, InstructionData, ToAccountMetas,
};
use anchor_raydium_cpmm::{ids, pda};
use anchor_spl::{
    associated_token::{self, get_associated_token_address_with_program_id as ata},
    token::spl_token::{self, native_mint, state::Account as TokenAccount, state::Mint},
};
use launchpad::{FEE_TIER, GRADUATION_SEED, LAUNCH_SEED, POOL_SEED};
use solana_commitment_config::CommitmentConfig;
use solana_keypair::{read_keypair_file, Keypair};
use solana_rpc_client::rpc_client::RpcClient;
use solana_signer::Signer;
use solana_transaction::Transaction;

const SOL_AMOUNT: u64 = 300_000_000; // 0.3 SOL
const TOKEN_AMOUNT: u64 = 1_000_000 * 1_000_000; // 1M tokens, 6 decimals
const RENT_SYSVAR: Pubkey = Pubkey::from_str_const("SysvarRent111111111111111111111111111111111");
const INSTRUCTIONS_SYSVAR: Pubkey =
    Pubkey::from_str_const("Sysvar1nstructions1111111111111111111111111");
const COMPUTE_BUDGET: Pubkey =
    Pubkey::from_str_const("ComputeBudget111111111111111111111111111111");

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let path = std::env::args()
        .nth(1)
        .ok_or("usage: devnet-run <keypair.json> [rpc url]")?;
    let url = std::env::args()
        .nth(2)
        .unwrap_or_else(|| "https://api.devnet.solana.com".into());
    let payer = read_keypair_file(&path).map_err(|e| format!("{path}: {e}"))?;
    let rpc = RpcClient::new_with_commitment(url, CommitmentConfig::confirmed());
    let send = |ixs: &[Instruction], signers: &[&Keypair]| {
        let tx = Transaction::new_signed_with_payer(
            ixs,
            Some(&payer.pubkey()),
            signers,
            rpc.get_latest_blockhash()?,
        );
        rpc.send_and_confirm_transaction(&tx)
    };
    let me = payer.pubkey();
    let pda = |seed: &[u8], mint: &Pubkey| {
        Pubkey::find_program_address(&[seed, mint.as_ref()], &launchpad::ID).0
    };
    println!("payer {me}: {} lamports", rpc.get_balance(&me)?);

    // 1. A token with a fixed supply and no freeze authority.
    let mint = Keypair::new();
    let m = mint.pubkey();
    let my_tokens = ata(&me, &m, &spl_token::ID);
    let sig = send(
        &[
            system_instruction::create_account(
                &me,
                &m,
                rpc.get_minimum_balance_for_rent_exemption(Mint::LEN)?,
                Mint::LEN as u64,
                &spl_token::ID,
            ),
            spl_token::instruction::initialize_mint2(&spl_token::ID, &m, &me, None, 6)?,
            associated_token::spl_associated_token_account::instruction::create_associated_token_account(
                &me, &me, &m, &spl_token::ID,
            ),
            spl_token::instruction::mint_to(&spl_token::ID, &m, &my_tokens, &me, &[], TOKEN_AMOUNT)?,
            spl_token::instruction::set_authority(
                &spl_token::ID,
                &m,
                None,
                spl_token::instruction::AuthorityType::MintTokens,
                &me,
                &[],
            )?,
        ],
        &[&payer, &mint],
    )?;
    println!("mint {m}: {sig}");

    // 2. The launch, burning its LP at graduation.
    let launch = pda(LAUNCH_SEED, &m);
    let create = Instruction::new_with_bytes(
        launchpad::ID,
        &launchpad::instruction::CreateLaunch {
            sol_amount: SOL_AMOUNT,
            token_amount: TOKEN_AMOUNT,
            lp_recipient: None,
        }
        .data(),
        launchpad::accounts::CreateLaunch {
            creator: me,
            mint: m,
            launch,
            vault: ata(&launch, &m, &spl_token::ID),
            creator_token_account: my_tokens,
            token_program: spl_token::ID,
            associated_token_program: associated_token::ID,
            system_program: system_program::ID,
        }
        .to_account_metas(None),
    );
    println!("create_launch: {}", send(&[create], &[&payer])?);

    // 3. prepare + graduate in one transaction.
    let (auth, pool) = (pda(GRADUATION_SEED, &m), pda(POOL_SEED, &m));
    let lp_mint = pda::lp_mint(&pool).0;
    let mut budget = vec![2];
    budget.extend_from_slice(&400_000u32.to_le_bytes());
    let prepare = Instruction::new_with_bytes(
        launchpad::ID,
        &launchpad::instruction::Prepare {}.data(),
        launchpad::accounts::Prepare {
            launch,
            graduation_authority: auth,
            instructions: INSTRUCTIONS_SYSVAR,
        }
        .to_account_metas(None),
    );
    let graduate = Instruction::new_with_bytes(
        launchpad::ID,
        &launchpad::instruction::Graduate {}.data(),
        launchpad::accounts::Graduate {
            payer: me,
            launch,
            mint: m,
            vault: ata(&launch, &m, &spl_token::ID),
            graduation_authority: auth,
            authority_base_account: ata(&auth, &m, &spl_token::ID),
            authority_wsol_account: ata(&auth, &native_mint::ID, &spl_token::ID),
            authority_lp_account: ata(&auth, &lp_mint, &spl_token::ID),
            pool_state: pool,
            raydium_program: ids::CPMM_PROGRAM_ID,
            amm_config: pda::amm_config(FEE_TIER).0,
            raydium_authority: pda::pool_authority().0,
            lp_mint,
            base_vault: pda::vault(&pool, &m).0,
            wsol_vault: pda::vault(&pool, &native_mint::ID).0,
            observation_state: pda::observation(&pool).0,
            create_pool_fee_receiver: ids::CREATE_POOL_FEE_RECEIVER,
            wsol_mint: native_mint::ID,
            lp_recipient: None,
            lp_recipient_token_account: None,
            token_program: spl_token::ID,
            base_token_program: spl_token::ID,
            associated_token_program: associated_token::ID,
            system_program: system_program::ID,
            rent: RENT_SYSVAR,
        }
        .to_account_metas(None),
    );
    let before = rpc.get_balance(&me)?;
    let sig = send(
        &[
            Instruction::new_with_bytes(COMPUTE_BUDGET, &budget, vec![]),
            prepare,
            graduate,
        ],
        &[&payer],
    )?;
    println!("graduate: {sig}");
    println!("  https://explorer.solana.com/tx/{sig}?cluster=devnet");

    // 4. Check what's on chain.
    let amount = |key: &Pubkey| -> Result<u64, Box<dyn std::error::Error>> {
        Ok(TokenAccount::unpack(&rpc.get_account(key)?.data)?.amount)
    };
    let pool_account = rpc.get_account(&pool)?;
    assert_eq!(
        pool_account.owner,
        ids::CPMM_PROGRAM_ID,
        "pool not owned by Raydium"
    );
    assert_eq!(amount(&pda::vault(&pool, &native_mint::ID).0)?, SOL_AMOUNT);
    assert_eq!(amount(&pda::vault(&pool, &m).0)?, TOKEN_AMOUNT);
    assert_eq!(
        Mint::unpack(&rpc.get_account(&lp_mint)?.data)?.supply,
        0,
        "LP not burned"
    );
    for gone in [
        auth,
        ata(&auth, &m, &spl_token::ID),
        ata(&auth, &native_mint::ID, &spl_token::ID),
        ata(&auth, &lp_mint, &spl_token::ID),
    ] {
        assert_eq!(rpc.get_balance(&gone)?, 0, "{gone} not closed");
    }
    let spent = before - rpc.get_balance(&me)?;
    println!("pool {pool}: OK");
    println!("  https://explorer.solana.com/address/{pool}?cluster=devnet");
    println!("  vaults hold exactly {SOL_AMOUNT} lamports and {TOKEN_AMOUNT} tokens, LP supply 0, temporary accounts closed");
    println!("  payer spent {spent} lamports on graduation (fee + rent + tx fee)");
    Ok(())
}
