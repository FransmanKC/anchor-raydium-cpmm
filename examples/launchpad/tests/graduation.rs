//! Graduation into Raydium CPMM, run against Raydium's real mainnet program
//! and accounts (tests/fixtures/). Each test guards one failure mode.

use {
    anchor_lang::{
        prelude::Pubkey,
        solana_program::{
            instruction::{AccountMeta, Instruction},
            program_pack::Pack,
        },
        system_program, AccountDeserialize, InstructionData, ToAccountMetas,
    },
    anchor_raydium_cpmm::{ids, pda, GraduateError, RENT_ALLOWANCE},
    anchor_spl::{
        associated_token::{self, get_associated_token_address_with_program_id as ata},
        token::{spl_token::native_mint, ID as TOKEN_ID},
        token_2022::{
            spl_token_2022::{
                extension::StateWithExtensions,
                state::{Account as TokenAccount, AccountState, Mint},
            },
            ID as TOKEN_2022_ID,
        },
    },
    launchpad::{LaunchpadError, GRADUATION_SEED, LAUNCH_SEED, POOL_SEED},
    litesvm::{
        types::{TransactionMetadata, TransactionResult},
        LiteSVM,
    },
    solana_keypair::Keypair,
    solana_message::{Message, VersionedMessage},
    solana_signer::Signer,
    solana_transaction::versioned::VersionedTransaction,
};

const SOL: u64 = 1_000_000_000;
const DECIMALS: u8 = 6;
const SOL_AMOUNT: u64 = 85 * SOL;
const TOKEN_AMOUNT: u64 = 206_900_000 * 1_000_000;
/// Raydium fee tier 0 on mainnet (0.15 SOL pool-creation fee).
const AMM_CONFIG: Pubkey = Pubkey::from_str_const("D4FPEruKEHrG5TenZ2mpDGEfu1iUvTiqBxvpU8HLBvC2");
const POOL_CREATION_FEE: u64 = 150_000_000;
const TX_FEE: u64 = 5_000;
const COMPUTE_BUDGET: Pubkey =
    Pubkey::from_str_const("ComputeBudget111111111111111111111111111111");
const RENT_SYSVAR: Pubkey = Pubkey::from_str_const("SysvarRent111111111111111111111111111111111");
const INSTRUCTIONS_SYSVAR: Pubkey =
    Pubkey::from_str_const("Sysvar1nstructions1111111111111111111111111");

struct Launch {
    svm: LiteSVM,
    mint: Pubkey,
    base_program: Pubkey,
}

fn pda(seed: &[u8], mint: &Pubkey) -> Pubkey {
    Pubkey::find_program_address(&[seed, mint.as_ref()], &launchpad::ID).0
}

fn authority(l: &Launch) -> Pubkey {
    pda(GRADUATION_SEED, &l.mint)
}

fn pool(l: &Launch) -> Pubkey {
    pda(POOL_SEED, &l.mint)
}

/// A launch holding SOL_AMOUNT and TOKEN_AMOUNT, ready to graduate and burn
/// its LP. The mint sorts before or after wrapped SOL as asked, since
/// Raydium orders the pool's two sides by address.
fn launch(base_program: Pubkey, base_before_wsol: bool) -> Launch {
    launch_with(base_program, base_before_wsol, None, &|svm, mint| {
        set_mint(svm, mint, base_program, TOKEN_AMOUNT)
    })
}

/// Like [`launch`], with an LP recipient and a custom mint.
fn launch_with(
    base_program: Pubkey,
    base_before_wsol: bool,
    lp_recipient: Option<Pubkey>,
    setup_mint: &dyn Fn(&mut LiteSVM, Pubkey),
) -> Launch {
    let mut svm = LiteSVM::new();
    let so = std::fs::read(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../target/deploy/launchpad.so"
    ))
    .expect("launchpad.so is missing: run `make build` first");
    svm.add_program(launchpad::ID, &so).unwrap();
    load_fixtures(&mut svm);

    let mint = loop {
        let k = Keypair::new().pubkey();
        if (k < native_mint::ID) == base_before_wsol {
            break k;
        }
    };
    let creator = funded(&mut svm);
    setup_mint(&mut svm, mint);
    let creator_tokens = ata(&creator.pubkey(), &mint, &base_program);
    set_token_account(
        &mut svm,
        creator_tokens,
        mint,
        creator.pubkey(),
        base_program,
        TOKEN_AMOUNT,
    );

    let ix = Instruction::new_with_bytes(
        launchpad::ID,
        &launchpad::instruction::CreateLaunch {
            sol_amount: SOL_AMOUNT,
            token_amount: TOKEN_AMOUNT,
            lp_recipient,
        }
        .data(),
        launchpad::accounts::CreateLaunch {
            creator: creator.pubkey(),
            mint,
            launch: pda(LAUNCH_SEED, &mint),
            vault: ata(&pda(LAUNCH_SEED, &mint), &mint, &base_program),
            creator_token_account: creator_tokens,
            token_program: base_program,
            associated_token_program: associated_token::ID,
            system_program: system_program::ID,
        }
        .to_account_metas(None),
    );
    send(&mut svm, &[ix], &[&creator]).unwrap();
    Launch {
        svm,
        mint,
        base_program,
    }
}

fn prepare_ix(l: &Launch) -> Instruction {
    Instruction::new_with_bytes(
        launchpad::ID,
        &launchpad::instruction::Prepare {}.data(),
        launchpad::accounts::Prepare {
            launch: pda(LAUNCH_SEED, &l.mint),
            graduation_authority: authority(l),
            instructions: INSTRUCTIONS_SYSVAR,
        }
        .to_account_metas(None),
    )
}

fn graduate_ix(l: &Launch, payer: &Pubkey, lp_recipient: Option<Pubkey>) -> Instruction {
    let (pool, auth, mint) = (pool(l), authority(l), l.mint);
    let lp_mint = pda::lp_mint(&pool).0;
    Instruction::new_with_bytes(
        launchpad::ID,
        &launchpad::instruction::Graduate {}.data(),
        launchpad::accounts::Graduate {
            payer: *payer,
            launch: pda(LAUNCH_SEED, &mint),
            mint,
            vault: ata(&pda(LAUNCH_SEED, &mint), &mint, &l.base_program),
            graduation_authority: auth,
            authority_base_account: ata(&auth, &mint, &l.base_program),
            authority_wsol_account: ata(&auth, &native_mint::ID, &TOKEN_ID),
            authority_lp_account: ata(&auth, &lp_mint, &TOKEN_ID),
            pool_state: pool,
            raydium_program: ids::CPMM_PROGRAM_ID,
            amm_config: AMM_CONFIG,
            raydium_authority: pda::pool_authority().0,
            lp_mint,
            base_vault: pda::vault(&pool, &mint).0,
            wsol_vault: pda::vault(&pool, &native_mint::ID).0,
            observation_state: pda::observation(&pool).0,
            create_pool_fee_receiver: ids::CREATE_POOL_FEE_RECEIVER,
            wsol_mint: native_mint::ID,
            lp_recipient,
            lp_recipient_token_account: lp_recipient.map(|r| ata(&r, &lp_mint, &TOKEN_ID)),
            token_program: TOKEN_ID,
            base_token_program: l.base_program,
            associated_token_program: associated_token::ID,
            system_program: system_program::ID,
            rent: RENT_SYSVAR,
        }
        .to_account_metas(None),
    )
}

fn compute_budget_ix(units: u32) -> Instruction {
    let mut data = vec![2];
    data.extend_from_slice(&units.to_le_bytes());
    Instruction::new_with_bytes(COMPUTE_BUDGET, &data, vec![])
}

/// prepare + graduate in one transaction, as a real launchpad sends them.
#[allow(clippy::result_large_err)]
fn graduate(l: &mut Launch, payer: &Keypair, lp_recipient: Option<Pubkey>) -> TransactionResult {
    let ixs = [
        compute_budget_ix(1_400_000),
        prepare_ix(l),
        graduate_ix(l, &payer.pubkey(), lp_recipient),
    ];
    send(&mut l.svm, &ixs, &[payer])
}

/// Checks everything a graduation promises and returns the LP amount.
fn assert_graduated(l: &Launch, meta: &TransactionMetadata, payer_spent: u64) -> u64 {
    let (pool, auth) = (pool(l), authority(l));
    assert!(meta.compute_units_consumed < 400_000);

    // The pool is ours, and opens at exactly sol_amount / token_amount.
    assert_eq!(
        l.svm.get_account(&pool).unwrap().owner,
        ids::CPMM_PROGRAM_ID
    );
    assert_eq!(
        balance(&l.svm, &pda::vault(&pool, &native_mint::ID).0),
        SOL_AMOUNT
    );
    assert_eq!(balance(&l.svm, &pda::vault(&pool, &l.mint).0), TOKEN_AMOUNT);
    let launch: launchpad::Launch = fetch(&l.svm, &pda(LAUNCH_SEED, &l.mint));
    assert_eq!(launch.pool, Some(pool));

    // Temporary accounts are closed and the authority keeps nothing.
    let lp_mint = pda::lp_mint(&pool).0;
    for gone in [
        auth,
        ata(&auth, &l.mint, &l.base_program),
        ata(&auth, &native_mint::ID, &TOKEN_ID),
        ata(&auth, &lp_mint, &TOKEN_ID),
    ] {
        assert!(lamports(&l.svm, &gone) == 0, "{gone} still exists");
    }

    // The payer paid Raydium's fee plus rent, and got the rest back.
    let (lp_amount, cost) = parse_outcome(&meta.logs);
    println!(
        "graduate: {} CU, cost {cost} lamports",
        meta.compute_units_consumed
    );
    assert_eq!(payer_spent, cost + TX_FEE);
    assert!(cost > POOL_CREATION_FEE && cost < POOL_CREATION_FEE + RENT_ALLOWANCE);
    assert!(lp_amount > 0);
    lp_amount
}

fn graduates_and_burns(base_program: Pubkey, base_before_wsol: bool) {
    let mut l = launch(base_program, base_before_wsol);
    let payer = funded(&mut l.svm);
    let before = lamports(&l.svm, &payer.pubkey());
    let meta = graduate(&mut l, &payer, None).unwrap();
    let spent = before - lamports(&l.svm, &payer.pubkey());
    assert_graduated(&l, &meta, spent);
    // Every LP token was burned: nobody can pull the liquidity.
    assert_eq!(mint_supply(&l.svm, &pda::lp_mint(&pool(&l)).0), 0);
}

#[test]
fn spl_token_mint_sorting_before_wsol() {
    graduates_and_burns(TOKEN_ID, true);
}

#[test]
fn spl_token_mint_sorting_after_wsol() {
    graduates_and_burns(TOKEN_ID, false);
}

#[test]
fn token_2022_mint_sorting_before_wsol() {
    graduates_and_burns(TOKEN_2022_ID, true);
}

#[test]
fn token_2022_mint_sorting_after_wsol() {
    graduates_and_burns(TOKEN_2022_ID, false);
}

#[test]
fn send_to_delivers_every_lp_token() {
    let recipient = Pubkey::new_unique();
    let mut l = launch_with(TOKEN_2022_ID, false, Some(recipient), &|svm, mint| {
        set_mint(svm, mint, TOKEN_2022_ID, TOKEN_AMOUNT)
    });
    let payer = funded(&mut l.svm);
    let before = lamports(&l.svm, &payer.pubkey());
    let meta = graduate(&mut l, &payer, Some(recipient)).unwrap();
    let spent = before - lamports(&l.svm, &payer.pubkey());
    let lp_amount = assert_graduated(&l, &meta, spent);
    let lp_mint = pda::lp_mint(&pool(&l)).0;
    assert_eq!(
        balance(&l.svm, &ata(&recipient, &lp_mint, &TOKEN_ID)),
        lp_amount
    );
    assert_eq!(mint_supply(&l.svm, &lp_mint), lp_amount);
}

#[test]
fn donations_are_ignored_and_stray_tokens_burned() {
    // What a clean graduation costs, for comparison.
    let mut clean = launch(TOKEN_2022_ID, true);
    let payer = funded(&mut clean.svm);
    let (_, clean_cost) = parse_outcome(&graduate(&mut clean, &payer, None).unwrap().logs);

    let mut l = launch(TOKEN_2022_ID, true);
    let (auth, mint) = (authority(&l), l.mint);
    // Someone sends SOL to the authority and the pool address, and plants
    // wrapped SOL and tokens in the authority's temporary accounts.
    let stray_tokens = 1_000 * 1_000_000;
    l.svm.airdrop(&auth, 3 * SOL).unwrap();
    l.svm.airdrop(&pool(&l), SOL).unwrap();
    set_token_account(
        &mut l.svm,
        ata(&auth, &mint, &TOKEN_2022_ID),
        mint,
        auth,
        TOKEN_2022_ID,
        stray_tokens,
    );
    set_mint(&mut l.svm, mint, TOKEN_2022_ID, TOKEN_AMOUNT + stray_tokens);
    let wsol = ata(&auth, &native_mint::ID, &TOKEN_ID);
    set_token_account(&mut l.svm, wsol, native_mint::ID, auth, TOKEN_ID, 2 * SOL);

    let payer = funded(&mut l.svm);
    let before = lamports(&l.svm, &payer.pubkey());
    let meta = graduate(&mut l, &payer, None).unwrap();
    // Everything sent to the authority ends up with the payer: the 3 SOL,
    // the planted wrapped SOL and the rent of both planted accounts. The
    // SOL sent to the pool address stays there, and covers its rent.
    let received = lamports(&l.svm, &payer.pubkey()) - before;
    let pool_rent = rent(&l.svm, l.svm.get_account(&pool(&l)).unwrap().data.len());
    let donated = 3 * SOL + 2 * SOL + 2 * rent(&l.svm, TokenAccount::LEN) + pool_rent;
    assert_eq!(received + TX_FEE + clean_cost, donated);
    assert_eq!(parse_outcome(&meta.logs).1, 0);
    // The pool still gets exactly the tracked amounts.
    assert_eq!(
        balance(&l.svm, &pda::vault(&pool(&l), &native_mint::ID).0),
        SOL_AMOUNT
    );
    assert_eq!(
        balance(&l.svm, &pda::vault(&pool(&l), &mint).0),
        TOKEN_AMOUNT
    );
    // The planted tokens are burned, not left behind or added to the pool.
    assert_eq!(mint_supply(&l.svm, &mint), TOKEN_AMOUNT);
    assert_eq!(lamports(&l.svm, &auth), 0);
}

#[test]
fn graduating_twice_fails() {
    let mut l = launch(TOKEN_ID, true);
    let payer = funded(&mut l.svm);
    graduate(&mut l, &payer, None).unwrap();
    let res = graduate(&mut l, &payer, None);
    assert_error(res, u32::from(LaunchpadError::AlreadyGraduated));
    // Skipping the example's own check, the crate refuses too: the
    // authority no longer holds the SOL.
    let ixs = [
        compute_budget_ix(1_400_000),
        graduate_ix(&l, &payer.pubkey(), None),
    ];
    let res = send(&mut l.svm, &ixs, &[&payer]);
    assert!(res.is_err());
}

#[test]
fn graduate_without_prepare_fails() {
    let mut l = launch(TOKEN_ID, true);
    let payer = funded(&mut l.svm);
    let ix = graduate_ix(&l, &payer.pubkey(), None);
    let res = send(&mut l.svm, &[ix], &[&payer]);
    assert_error(res, u32::from(LaunchpadError::NotPrepared));
}

#[test]
fn amm_config_not_owned_by_raydium_fails() {
    let mut l = launch(TOKEN_ID, true);
    // The pinned tier's address, holding the real bytes with a 0
    // pool-creation fee but owned by someone else. (The example's seeds
    // constraint passes; the crate's owner check must catch it.)
    let mut fake = l.svm.get_account(&AMM_CONFIG).unwrap();
    fake.data[ids::AMM_CONFIG_CREATE_POOL_FEE_OFFSET..][..8].fill(0);
    fake.owner = Pubkey::new_unique();
    l.svm.set_account(AMM_CONFIG, fake).unwrap();

    let payer = funded(&mut l.svm);
    let res = graduate(&mut l, &payer, None);
    assert_error(res, u32::from(GraduateError::InvalidAmmConfig));
}

// --- regressions from the security review -----------------------------------

/// Was critical: the example let whoever called `graduate` name the LP
/// recipient, take every LP token and withdraw the pool.
#[test]
fn caller_cannot_choose_the_lp_recipient() {
    let wrong = u32::from(LaunchpadError::WrongLpRecipient);
    // A launch that burns: a recipient the caller passes is ignored and
    // every LP token is still burned.
    let mut l = launch(TOKEN_ID, true);
    let attacker = funded(&mut l.svm);
    graduate(&mut l, &attacker, Some(attacker.pubkey())).unwrap();
    let lp_mint = pda::lp_mint(&pool(&l)).0;
    assert_eq!(mint_supply(&l.svm, &lp_mint), 0);
    assert_eq!(
        lamports(&l.svm, &ata(&attacker.pubkey(), &lp_mint, &TOKEN_ID)),
        0
    );
    // A launch with a recipient: another one, or none, is refused.
    let recipient = Pubkey::new_unique();
    let mut l = launch_with(TOKEN_2022_ID, false, Some(recipient), &|svm, mint| {
        set_mint(svm, mint, TOKEN_2022_ID, TOKEN_AMOUNT)
    });
    let attacker = funded(&mut l.svm);
    assert_error(graduate(&mut l, &attacker, Some(attacker.pubkey())), wrong);
    assert_error(graduate(&mut l, &attacker, None), wrong);
    graduate(&mut l, &attacker, Some(recipient)).unwrap();
}

/// Was medium: the fee tier wasn't pinned, so the first caller chose it.
#[test]
fn fee_tier_is_pinned() {
    let mut l = launch(TOKEN_ID, true);
    // A Raydium-owned tier like mainnet's 4% one.
    let mut acc = l.svm.get_account(&AMM_CONFIG).unwrap();
    acc.data[10..12].copy_from_slice(&3u16.to_le_bytes());
    acc.data[12..20].copy_from_slice(&40_000u64.to_le_bytes());
    let four_percent = Pubkey::new_unique();
    l.svm.set_account(four_percent, acc).unwrap();

    let payer = funded(&mut l.svm);
    let mut ix = graduate_ix(&l, &payer.pubkey(), None);
    ix.accounts
        .iter_mut()
        .find(|m| m.pubkey == AMM_CONFIG)
        .unwrap()
        .pubkey = four_percent;
    let ixs = [compute_budget_ix(1_400_000), prepare_ix(&l), ix];
    let res = send(&mut l.svm, &ixs, &[&payer]);
    assert_error(res, 2006); // Anchor ConstraintSeeds
}

/// Was low: `prepare` alone moved the SOL; if graduation then couldn't
/// succeed, the SOL was stranded on the authority for good.
#[test]
fn prepare_only_runs_right_before_graduate() {
    let must_follow = u32::from(LaunchpadError::GraduateMustFollow);
    let mut l = launch(TOKEN_ID, true);
    let stranger = funded(&mut l.svm);
    let alone = [prepare_ix(&l)];
    assert_error(send(&mut l.svm, &alone, &[&stranger]), must_follow);
    let ixs = [prepare_ix(&l), compute_budget_ix(1_400_000)];
    assert_error(send(&mut l.svm, &ixs, &[&stranger]), must_follow);

    // A launch that can't graduate (its mint takes a 1% transfer fee):
    // the transaction reverts as a whole and the SOL stays put.
    let mut l = launch_with(TOKEN_2022_ID, true, None, &|svm, k| {
        set_fee_mint(svm, k, 100)
    });
    let payer = funded(&mut l.svm);
    let launch_lamports = lamports(&l.svm, &pda(LAUNCH_SEED, &l.mint));
    assert!(graduate(&mut l, &payer, None).is_err());
    assert_eq!(
        lamports(&l.svm, &pda(LAUNCH_SEED, &l.mint)),
        launch_lamports
    );
    assert_eq!(lamports(&l.svm, &authority(&l)), 0);
    let launch: launchpad::Launch = fetch(&l.svm, &pda(LAUNCH_SEED, &l.mint));
    assert!(!launch.prepared);
}

/// A transfer-fee mint set to 0 bps graduates (whoever holds its fee
/// authority could raise the fee later and block graduation, though).
#[test]
fn zero_bps_transfer_fee_mint_graduates() {
    let mut l = launch_with(TOKEN_2022_ID, false, None, &|svm, k| {
        set_fee_mint(svm, k, 0)
    });
    let payer = funded(&mut l.svm);
    graduate(&mut l, &payer, None).unwrap();
}

/// Lamports sent ahead of time to any address the graduation creates don't
/// block it. Each target in its own launch.
#[test]
fn prefunded_addresses_do_not_block_graduation() {
    let recipient = Pubkey::new_unique();
    let mut failures = vec![];
    for target in 0..8 {
        for (program, before) in [(TOKEN_ID, true), (TOKEN_2022_ID, false)] {
            let mut l = launch_with(program, before, Some(recipient), &|svm, mint| {
                set_mint(svm, mint, program, TOKEN_AMOUNT)
            });
            let (p, auth) = (pool(&l), authority(&l));
            let lp_mint = pda::lp_mint(&p).0;
            let key = [
                lp_mint,
                pda::vault(&p, &l.mint).0,
                pda::vault(&p, &native_mint::ID).0,
                pda::observation(&p).0,
                ata(&auth, &lp_mint, &TOKEN_ID),
                ata(&recipient, &lp_mint, &TOKEN_ID),
                ata(&auth, &l.mint, &program),
                ata(&auth, &native_mint::ID, &TOKEN_ID),
            ][target];
            let r = rent(&l.svm, 0);
            l.svm.airdrop(&key, r).unwrap();
            let payer = funded(&mut l.svm);
            if let Err(e) = graduate(&mut l, &payer, Some(recipient)) {
                failures.push(format!("target {target} ({program}): {:?}", e.err));
            }
        }
    }
    assert!(failures.is_empty(), "blocked by: {failures:#?}");
}

#[test]
fn a_rival_pool_created_first_does_not_block_graduation() {
    let mut l = launch(TOKEN_2022_ID, false);
    let mint = l.mint;
    // An attacker holding some tokens and wrapped SOL.
    let attacker = funded(&mut l.svm);
    let tokens = 1_000 * 1_000_000;
    let a = attacker.pubkey();
    set_token_account(
        &mut l.svm,
        ata(&a, &mint, &TOKEN_2022_ID),
        mint,
        a,
        TOKEN_2022_ID,
        tokens,
    );
    set_mint(&mut l.svm, mint, TOKEN_2022_ID, TOKEN_AMOUNT + tokens);
    set_token_account(
        &mut l.svm,
        ata(&a, &native_mint::ID, &TOKEN_ID),
        native_mint::ID,
        a,
        TOKEN_ID,
        SOL,
    );

    // 1. Nobody else can create a pool at our address: Raydium requires a
    //    non-default pool address to sign, and only the launchpad can.
    let ix = raydium_initialize_ix(&l, &a, &pool(&l), tokens, SOL / 20);
    assert!(send(
        &mut l.svm,
        &[compute_budget_ix(1_400_000), ix],
        &[&attacker]
    )
    .is_err());

    // 2. Anyone can open Raydium's default pool for the pair, at a price of
    //    their choosing...
    let rival = Pubkey::find_program_address(
        &[
            b"pool",
            AMM_CONFIG.as_ref(),
            native_mint::ID.as_ref(),
            mint.as_ref(),
        ],
        &ids::CPMM_PROGRAM_ID,
    )
    .0;
    let ix = raydium_initialize_ix(&l, &a, &rival, tokens, SOL / 20);
    send(
        &mut l.svm,
        &[compute_budget_ix(1_400_000), ix],
        &[&attacker],
    )
    .unwrap();
    assert!(l.svm.get_account(&rival).is_some());

    // ...which leaves ours untouched: its own address, its exact liquidity.
    let payer = funded(&mut l.svm);
    graduate(&mut l, &payer, None).unwrap();
    assert_eq!(
        balance(&l.svm, &pda::vault(&pool(&l), &native_mint::ID).0),
        SOL_AMOUNT
    );
    assert_eq!(
        balance(&l.svm, &pda::vault(&pool(&l), &mint).0),
        TOKEN_AMOUNT
    );
}

/// Raw Raydium `initialize` by `creator` (a wallet, not a PDA), for the
/// rival-pool test. `pool` never signs: the attacker can't sign for ours.
fn raydium_initialize_ix(
    l: &Launch,
    creator: &Pubkey,
    pool: &Pubkey,
    tokens: u64,
    sol: u64,
) -> Instruction {
    let lp_mint = pda::lp_mint(pool).0;
    let base = (
        l.mint,
        ata(creator, &l.mint, &l.base_program),
        pda::vault(pool, &l.mint).0,
        l.base_program,
        tokens,
    );
    let wsol = (
        native_mint::ID,
        ata(creator, &native_mint::ID, &TOKEN_ID),
        pda::vault(pool, &native_mint::ID).0,
        TOKEN_ID,
        sol,
    );
    let (s0, s1) = if l.mint < native_mint::ID {
        (base, wsol)
    } else {
        (wsol, base)
    };
    let mut data = ids::INITIALIZE_DISCRIMINATOR.to_vec();
    for v in [s0.4, s1.4, 0] {
        data.extend_from_slice(&v.to_le_bytes());
    }
    Instruction::new_with_bytes(
        ids::CPMM_PROGRAM_ID,
        &data,
        vec![
            AccountMeta::new(*creator, true),
            AccountMeta::new_readonly(AMM_CONFIG, false),
            AccountMeta::new_readonly(pda::pool_authority().0, false),
            AccountMeta::new(*pool, false),
            AccountMeta::new_readonly(s0.0, false),
            AccountMeta::new_readonly(s1.0, false),
            AccountMeta::new(lp_mint, false),
            AccountMeta::new(s0.1, false),
            AccountMeta::new(s1.1, false),
            AccountMeta::new(ata(creator, &lp_mint, &TOKEN_ID), false),
            AccountMeta::new(s0.2, false),
            AccountMeta::new(s1.2, false),
            AccountMeta::new(ids::CREATE_POOL_FEE_RECEIVER, false),
            AccountMeta::new(pda::observation(pool).0, false),
            AccountMeta::new_readonly(TOKEN_ID, false),
            AccountMeta::new_readonly(s0.3, false),
            AccountMeta::new_readonly(s1.3, false),
            AccountMeta::new_readonly(associated_token::ID, false),
            AccountMeta::new_readonly(system_program::ID, false),
            AccountMeta::new_readonly(RENT_SYSVAR, false),
        ],
    )
}

// --- helpers ---------------------------------------------------------------

/// Loads Raydium CPMM, the deployed token programs and the mainnet accounts
/// Raydium needs, from snapshots in tests/fixtures/.
fn load_fixtures(svm: &mut LiteSVM) {
    svm.add_program(TOKEN_ID, include_bytes!("fixtures/spl_token.so"))
        .unwrap();
    svm.add_program(TOKEN_2022_ID, include_bytes!("fixtures/spl_token_2022.so"))
        .unwrap();
    svm.add_program(
        ids::CPMM_PROGRAM_ID,
        include_bytes!("fixtures/raydium_cpmm.so"),
    )
    .unwrap();
    for json in [
        include_str!("fixtures/amm_config_0.json"),
        include_str!("fixtures/create_pool_fee_receiver.json"),
        include_str!("fixtures/wsol_mint.json"),
    ] {
        use base64::Engine;
        let v: serde_json::Value = serde_json::from_str(json).unwrap();
        let a = &v["account"];
        let account = solana_account::Account {
            lamports: a["lamports"].as_u64().unwrap(),
            data: base64::engine::general_purpose::STANDARD
                .decode(a["data"][0].as_str().unwrap())
                .unwrap(),
            owner: a["owner"].as_str().unwrap().parse().unwrap(),
            executable: a["executable"].as_bool().unwrap(),
            rent_epoch: 0,
        };
        svm.set_account(v["pubkey"].as_str().unwrap().parse().unwrap(), account)
            .unwrap();
    }
}

fn funded(svm: &mut LiteSVM) -> Keypair {
    let kp = Keypair::new();
    svm.airdrop(&kp.pubkey(), 1_000 * SOL).unwrap();
    kp
}

fn rent(svm: &LiteSVM, len: usize) -> u64 {
    svm.minimum_balance_for_rent_exemption(len)
}

/// Writes a mint with no mint authority (supply fixed), as a launchpad's
/// would be.
fn set_mint(svm: &mut LiteSVM, key: Pubkey, program: Pubkey, supply: u64) {
    let mut data = vec![0; Mint::LEN];
    Mint {
        supply,
        decimals: DECIMALS,
        is_initialized: true,
        ..Mint::default()
    }
    .pack_into_slice(&mut data);
    set(svm, key, program, data);
}

/// A Token-2022 mint with a transfer fee of `bps` and no fee authority.
fn set_fee_mint(svm: &mut LiteSVM, key: Pubkey, bps: u16) {
    use anchor_spl::token_2022::spl_token_2022::extension::{
        transfer_fee::{TransferFee, TransferFeeConfig},
        BaseStateWithExtensionsMut, ExtensionType, StateWithExtensionsMut,
    };
    let len = ExtensionType::try_calculate_account_len::<Mint>(&[ExtensionType::TransferFeeConfig])
        .unwrap();
    let mut data = vec![0; len];
    let mut state = StateWithExtensionsMut::<Mint>::unpack_uninitialized(&mut data).unwrap();
    let fee = TransferFee {
        epoch: 0.into(),
        maximum_fee: u64::MAX.into(),
        transfer_fee_basis_points: bps.into(),
    };
    let ext = state.init_extension::<TransferFeeConfig>(true).unwrap();
    ext.older_transfer_fee = fee;
    ext.newer_transfer_fee = fee;
    state.base = Mint {
        supply: TOKEN_AMOUNT,
        decimals: DECIMALS,
        is_initialized: true,
        ..Mint::default()
    };
    state.pack_base();
    state.init_account_type().unwrap();
    set(svm, key, TOKEN_2022_ID, data);
}

fn set_token_account(
    svm: &mut LiteSVM,
    key: Pubkey,
    mint: Pubkey,
    owner: Pubkey,
    program: Pubkey,
    amount: u64,
) {
    let native = mint == native_mint::ID;
    let mut data = vec![0; TokenAccount::LEN];
    TokenAccount {
        mint,
        owner,
        amount,
        state: AccountState::Initialized,
        is_native: if native {
            Some(rent(svm, TokenAccount::LEN)).into()
        } else {
            None.into()
        },
        ..TokenAccount::default()
    }
    .pack_into_slice(&mut data);
    let lamports = rent(svm, data.len()) + if native { amount } else { 0 };
    svm.set_account(
        key,
        solana_account::Account {
            lamports,
            data,
            owner: program,
            executable: false,
            rent_epoch: 0,
        },
    )
    .unwrap();
}

fn set(svm: &mut LiteSVM, key: Pubkey, owner: Pubkey, data: Vec<u8>) {
    let lamports = rent(svm, data.len());
    svm.set_account(
        key,
        solana_account::Account {
            lamports,
            data,
            owner,
            executable: false,
            rent_epoch: 0,
        },
    )
    .unwrap();
}

fn lamports(svm: &LiteSVM, key: &Pubkey) -> u64 {
    svm.get_account(key).map_or(0, |a| a.lamports)
}

fn balance(svm: &LiteSVM, key: &Pubkey) -> u64 {
    let account = svm.get_account(key).expect("token account missing");
    StateWithExtensions::<TokenAccount>::unpack(&account.data)
        .unwrap()
        .base
        .amount
}

fn mint_supply(svm: &LiteSVM, key: &Pubkey) -> u64 {
    let account = svm.get_account(key).expect("mint missing");
    StateWithExtensions::<Mint>::unpack(&account.data)
        .unwrap()
        .base
        .supply
}

fn fetch<T: AccountDeserialize>(svm: &LiteSVM, key: &Pubkey) -> T {
    T::try_deserialize(&mut svm.get_account(key).unwrap().data.as_slice()).unwrap()
}

/// (lp_amount, cost) from the example's "graduated: ..." log line.
fn parse_outcome(logs: &[String]) -> (u64, u64) {
    let line = logs
        .iter()
        .find_map(|l| l.split("graduated: ").nth(1))
        .expect("no outcome log");
    let field = |name: &str| {
        line.split(' ')
            .find_map(|kv| kv.strip_prefix(name))
            .unwrap()
            .parse()
            .unwrap()
    };
    (field("lp_amount="), field("cost="))
}

#[allow(clippy::result_large_err)]
fn send(svm: &mut LiteSVM, ixs: &[Instruction], signers: &[&Keypair]) -> TransactionResult {
    let msg = Message::new_with_blockhash(ixs, Some(&signers[0].pubkey()), &svm.latest_blockhash());
    let tx = VersionedTransaction::try_new(VersionedMessage::Legacy(msg), signers).unwrap();
    let res = svm.send_transaction(tx);
    svm.expire_blockhash();
    res
}

fn assert_error(res: TransactionResult, code: u32) {
    let err = res.expect_err("transaction should have failed");
    assert!(
        format!("{:?}", err.err).contains(&format!("Custom({code})")),
        "expected error {code}, got {:?}\n{}",
        err.err,
        err.meta.logs.join("\n")
    );
}
