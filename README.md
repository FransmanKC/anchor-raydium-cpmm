# anchor-raydium-cpmm

Graduate a token from an Anchor program into a Raydium CPMM pool and lock the
liquidity: the step every bonding-curve launchpad ends with.

> [!WARNING]
> **For learning and experimenting, not for mainnet (yet).** This crate is
> young, has not been audited, and is not published on crates.io. Use it to
> study how a safe graduation works, and to build and test on devnet or
> localnet. Don't put real users' funds behind it on mainnet. If you do so
> anyway, that's your own risk, after your own review and audit. The licences
> come with no warranty.

```toml
[dependencies]
# Pin a commit: the API can change without notice before 1.0.
anchor-raydium-cpmm = { git = "https://github.com/KevinGod1234/anchor-raydium-cpmm", rev = "<commit>" }
# devnet addresses: add  features = ["devnet"]
```

**Contents:** [Why](#why) · [How it works](#how-it-works) ·
[Integration guide](#integration-guide) · [Accounts](#accounts) ·
[Costs](#costs) · [Errors](#errors) · [Security checklist](#security-checklist) ·
[Devnet](#devnet) · [Limitations](#limitations) ·
[Troubleshooting](#troubleshooting) · [API](#api) · [Tests](#tests) ·
[Security review](#security-review) ·
[Alternatives](#alternatives)

Anchor 1.2, Solana 3.x crates, SPL Token or Token-2022 base mints, quote mint
wrapped SOL. Tested with LiteSVM against Raydium's real mainnet program.

## Why

Moving a launch's SOL and token reserve into a DEX pool from inside a program
is easy to get subtly wrong:

- **Front-running the pool.** If the pool lives at Raydium's default address,
  anyone can create it first and set the opening price. Here the pool address
  is a PDA of *your* program, which only your program can sign for.
- **Inflated balances.** Anyone can send SOL or tokens to any account. The
  pool is funded from the amounts you pass in, never from balances, and
  planted tokens are burned so they can't block cleanup.
- **The `initialize` CPI.** 20 accounts with exact writable and signer flags,
  and the two mints ordered by address. Both orderings are tested.
- **Plumbing.** Wrap native SOL, create and close temporary token accounts,
  front Raydium's pool-creation fee and refund what's left, burn the LP.
- **Two instructions.** Solana rejects a direct lamport move followed by a
  CPI that doesn't include both accounts, so graduation is a `prepare` plus
  a `graduate` in one transaction.

## How it works

`graduate` runs, inside your instruction:

1. The payer fronts Raydium's `create_pool_fee` (read from the `AmmConfig`
   after checking its owner) plus `RENT_ALLOWANCE` (0.1 SOL).
2. Creates the authority's temporary base-token and wrapped-SOL accounts.
3. Wraps exactly `sol_amount`; moves exactly `token_amount` from your vault.
4. Calls Raydium `initialize` with your PDA as pool address and creator.
5. Burns all LP tokens (`LpLock::Burn`) or sends them to an owner's LP
   account (`LpLock::SendTo`).
6. Burns stray base tokens, closes the temporary accounts.
7. Refunds everything left on the authority to the payer.

It returns `GraduateOutcome { pool, lp_amount, cost }`.

## Integration guide

[`examples/launchpad`](examples/launchpad/src/lib.rs) is a complete program;
copy its `Graduate` accounts struct. Step by step:

**1. Pick three PDAs** of your program, per launch (e.g. seeded by the mint):

- a **graduation authority**: a system-owned PDA holding nothing else. It
  signs as the Raydium pool creator. Anything it holds besides `sol_amount`
  is refunded to the payer, so don't reuse an account that holds other SOL.
- a **pool address**: the Raydium pool will live here.
- whatever **owns your token vault** (your launch/curve account).

**2. Track the amounts.** Store the SOL raised and the token reserve in your
state. Never compute them from `lamports()` or a token balance.

**3. Write `prepare`**, a separate instruction that moves `sol_amount` from
wherever your program keeps it to the graduation authority, and guards
against running twice. It must **refuse to run unless the next instruction
is your `graduate` for the same launch, and refuse to be called through
another program** (under a CPI, "the next instruction" is the caller's).
Otherwise anyone can move the SOL without graduating, and if graduation
can't succeed afterwards the SOL is stuck on the authority. Check with the
instructions sysvar (`solana-instructions-sysvar` crate):

```rust
let ixs = ctx.accounts.instructions.to_account_info(); // Sysvar1nstructions…
let next = load_current_index_checked(&ixs)?
    .checked_add(1)
    .and_then(|i| load_instruction_at_checked(i.into(), &ixs).ok());
let launch_key = ctx.accounts.launch.key();
require!(
    get_stack_height() == TRANSACTION_LEVEL_STACK_HEIGHT
        && next.is_some_and(|ix| ix.program_id == crate::ID
            && ix.data.starts_with(instruction::Graduate::DISCRIMINATOR)
            // the launch account's position in your Graduate accounts
            && ix.accounts.get(1).is_some_and(|m| m.pubkey == launch_key)),
    MyError::GraduateMustFollow
);
```

Then, if the SOL sits in a program-owned data account, move it directly:

```rust
launch.sub_lamports(amount)?;
ctx.accounts.graduation_authority.add_lamports(amount)?;
```

If it sits in a system-owned PDA, a `system_program::transfer` signed with
its seeds works too.

**4. Write `graduate`.** Pin every address in `#[derive(Accounts)]` (the
example shows each constraint, including `seeds::program` for Raydium's
PDAs), then:

```rust
let outcome = anchor_raydium_cpmm::graduate(
    GraduateAccounts { /* 23 accounts, see the example */ },
    GraduateParams {
        sol_amount: launch.sol_amount,       // tracked
        token_amount: launch.token_amount,   // tracked
        authority_seeds: &[GRADUATION_SEED, mint.as_ref(), &[authority_bump]],
        pool_seeds: &[POOL_SEED, mint.as_ref(), &[pool_bump]],
        token_source_seeds: &[LAUNCH_SEED, mint.as_ref(), &[launch.bump]],
        lp_lock: LpLock::Burn, // or SendTo an owner taken from your state
        open_time: 0,
    },
)?;
launch.pool = Some(outcome.pool); // and refuse to graduate again
```

`graduate` is usually permissionless, so **never let the caller choose
where the LP tokens go**. Whoever holds them can withdraw the whole pool. For
`LpLock::SendTo`, store the owner when the launch is created and check the
passed account against it (the example does this).

**5. Send both in one transaction** with a compute budget. Prepare plus
graduate measured 197k to 258k compute units (it varies with the PDA bump
searches), more than the 200k default.

```
[ ComputeBudget::SetComputeUnitLimit(400_000), prepare, graduate ]
```

**6. Pin the fee tier.** The crate checks the `AmmConfig` is Raydium's, but
not *which* tier it is. Unpinned, the first caller picks the pool's trading
fee forever (mainnet has tiers up to 4%). Pin it with Raydium's seeds, which
also works on devnet:

```rust
#[account(
    seeds = [ids::AMM_CONFIG_SEED, &FEE_TIER.to_be_bytes()], // FEE_TIER: u16 = 0
    bump,
    seeds::program = ids::CPMM_PROGRAM_ID,
)]
pub amm_config: UncheckedAccount<'info>,
```

## Accounts

The 23 fields of `GraduateAccounts`, in the order the example declares them.
"Created by" means the account must not exist yet, or may have been created
by anyone (the crate copes either way).

| Field | Writable | Signer | What / who checks it |
|---|---|---|---|
| `payer` | yes | yes (tx) | Fronts the fee and rent; gets the unspent part back. |
| `authority` | yes | PDA | System-owned PDA of your program; must already hold `sol_amount`. Signs as pool creator. |
| `base_mint` | yes | | The token. Owner must be `base_token_program`, and it must be writable (both checked by the crate): stray tokens are burned. |
| `token_source` | yes | | Your vault holding the tokens. |
| `token_source_authority` | | PDA or tx | Owner of `token_source`. |
| `authority_base_account` | yes | | `authority`'s ATA for the mint. Created and closed here (address checked by the ATA program). |
| `authority_wsol_account` | yes | | `authority`'s wrapped-SOL ATA. Created and closed here. |
| `authority_lp_account` | yes | | `authority`'s LP ATA. Raydium creates it; closed here. |
| `pool_state` | yes | PDA | Your pool PDA. Raydium creates it; only your program can sign for it. |
| `amm_config` | | | Raydium fee tier. Owner checked by the crate; pin the tier yourself (`pda::amm_config(index)`). |
| `raydium_authority` | | | `pda::pool_authority()`; checked by Raydium. |
| `lp_mint` | yes | | `pda::lp_mint(pool)`; created by Raydium. |
| `base_vault` | yes | | `pda::vault(pool, mint)`; created by Raydium. |
| `wsol_vault` | yes | | `pda::vault(pool, WSOL)`; created by Raydium. |
| `observation_state` | yes | | `pda::observation(pool)`; created by Raydium. |
| `create_pool_fee_receiver` | yes | | `ids::CREATE_POOL_FEE_RECEIVER`; checked by Raydium. |
| `wsol_mint` | | | `So111…112`; checked by the crate. |
| `token_program` | | | SPL Token (wrapped SOL and LP mint); checked by the crate. |
| `base_token_program` | | | SPL Token or Token-2022; checked by the crate. |
| `associated_token_program` | | | Associated Token Account program. |
| `system_program` | | | System program. |
| `rent` | | | Rent sysvar (Raydium still asks for it). |
| `raydium_program` | | | `ids::CPMM_PROGRAM_ID`; checked by the crate. |

For `LpLock::SendTo` also pass the LP owner and their LP ATA (writable).
The owner must come from your program's state, not from the caller.

## Costs

Measured in the tests on mainnet fee tier 0 (`D4FPEr…BvC2`):

| | Lamports | SOL |
|---|---|---|
| Raydium pool-creation fee | 150,000,000 | 0.15 |
| Rent of pool state, LP mint, vaults, observation | 40,117,440 | ~0.040 |
| **Total `cost` with `LpLock::Burn`** | **190,117,440** | **~0.190** |
| Plus the LP recipient's ATA, with `SendTo` | +2,039,280 | ~0.002 |

The payer needs `create_pool_fee + RENT_ALLOWANCE` (0.25 SOL on tier 0) plus
the transaction fee available up front; about 0.06 SOL comes back within the
same instruction. The temporary token accounts' rent is refunded when they're
closed. Raydium can change its fees: the crate reads the fee from the
`AmmConfig` at runtime, so nothing is hard-coded.

Who pays in the end is up to you: the example has the caller pay, and riff
reimbursed the caller from protocol fees. `GraduateOutcome::cost` tells you
the amount.

## Errors

The crate's errors start at 9000 so they never clash with your program's own
(Anchor starts those at 6000).

| Code | Error | Usually means |
|---|---|---|
| 9000 | `InvalidAmmConfig` | The `AmmConfig` isn't owned by Raydium, or is too short. Wrong cluster? |
| 9001 | `InvalidAccount` | A program ID, the wrapped-SOL mint, or the base mint's owner doesn't match, or the base mint isn't writable. |
| 9002 | `ZeroAmount` | `sol_amount` or `token_amount` is 0. |
| 9003 | `InsufficientSol` | `authority` holds less than `sol_amount`: `prepare` didn't run in this transaction, or already graduated. |
| 9004 | `MathOverflow` | Should never happen; please open an issue. |

Errors from Raydium (`CPMMoo8…`), the token programs or the ATA program are
passed through unchanged. Check the transaction logs for which program
failed.

## Security checklist

The crate handles the graduation itself. Your program is still responsible
for the following:

- [ ] **Graduate once.** Record the pool (or a flag) and refuse a second
  `prepare`/`graduate`. The crate can't know your state.
- [ ] **Only when ready.** Check your own condition (curve sold out, deadline
  passed…) in both `prepare` and `graduate`.
- [ ] **`prepare` only right before `graduate`.** Called directly (no CPI),
  and the next instruction is `graduate` for the same launch (step 3), so the
  SOL can't be moved and then stranded.
- [ ] **LP recipient from state.** With `LpLock::SendTo`, the owner is fixed
  before graduation and checked; never taken from the caller. Refuse your
  own PDAs as recipient: the authority can't close an LP account holding LP,
  so graduation would never succeed.
- [ ] **Check the launch can graduate when it's created.** Refuse amounts
  with `sqrt(sol_amount × token_amount) <= 100` (Raydium locks 100 LP), and
  Token-2022 mints with an extension Raydium rejects or a non-zero transfer
  fee (see [Limitations](#limitations)). Otherwise the launch can never
  graduate, and you need a way to give the funds back.
- [ ] **Tracked amounts.** `sol_amount` and `token_amount` come from your
  state, never from `lamports()` or a token balance.
- [ ] **Dedicated authority.** The graduation authority holds nothing but
  the SOL for this pool. Everything else on it goes to the payer.
- [ ] **Pinned addresses.** Pin every account in `#[derive(Accounts)]`: the
  seeds of your PDAs, `seeds::program = ids::CPMM_PROGRAM_ID` for Raydium's,
  the program IDs, and **the fee tier you chose** (step 6).
- [ ] **Who creates a launch.** Seed launches so nobody can claim one for
  someone else's mint at their own price. The example's `create_launch` is a
  stand-in that doesn't do this; don't copy it.
- [ ] **Pool PDA.** Use a PDA of your program as `pool_state`, never
  Raydium's default address, so nobody can create your pool first.
- [ ] **Mint authority.** A launch mint normally has no mint authority (fixed
  supply) and no freeze authority. Whoever holds a freeze authority can
  block graduation (by freezing the authority's token account, which anyone
  can create early) and freeze the pool's vault later.
- [ ] **Open time.** `open_time: 0` opens trading immediately. A future time
  delays it; the crate passes it through unchecked, so cap it if it comes
  from user input, or the pool may stay closed indefinitely.
- [ ] **Review and audit** before any mainnet use (see the warning above).

## Devnet

Build with `features = ["devnet"]` to use Raydium's devnet program
(`DRaycp…pYb`) and fee receiver. Use a **devnet** `AmmConfig`. Mainnet fee
tiers don't exist on devnet, and the crate rejects an `AmmConfig` that isn't
owned by the program it was built for. The tests run against mainnet
snapshots only; the devnet build is checked by a unit test of its addresses
and by clippy.

## Limitations

- **Quote mint is wrapped SOL only.** No USDC or other quote tokens yet.
- **Token-2022 extensions.** Tested against the mainnet Raydium program:

  | Graduates | Doesn't graduate (transaction reverts) |
  |---|---|
  | none, metadata pointer, interest-bearing, transfer fee at 0% | permanent delegate, mint close authority, transfer hook (even with no hook program), default account state (even `Initialized`), pausable, transfer fee above 0% |

  A non-zero transfer fee fails because the authority receives less than
  `token_amount`. Whoever holds a mint's transfer-fee authority can raise the
  fee later and so block graduation; launch mints shouldn't have one.
  Raydium accepts some otherwise-rejected mints from its own allowlist, but
  only with extra accounts this crate doesn't pass, so those aren't
  supported either.
- **Minimum size.** Raydium locks 100 LP tokens, so
  `sqrt(sol_amount × token_amount)` must be above 100. At exactly 100 the
  pool is created with 0 LP for you.
- **Creates pools only.** No swaps, adding or removing liquidity, or Raydium
  AMM v4 / CLMM.
- **`prepare` is yours.** How your SOL gets to the authority depends on how
  you store it, so the crate leaves it to you (see step 3 above).
- **Burn is final.** `LpLock::Burn` locks the liquidity forever. Nobody can
  withdraw it, and Raydium's LP fee-claim lockers don't apply.
- **Fixtures age.** Raydium and the token programs can be upgraded. Refresh
  the [fixtures](examples/launchpad/tests/fixtures) and re-run the tests
  before relying on them.

## Troubleshooting

| Symptom | Fix |
|---|---|
| `exceeded CUs meter` / `Computational budget exceeded` | Add `SetComputeUnitLimit(400_000)` to the transaction. |
| `9003 InsufficientSol` | Send `prepare` and `graduate` in the same transaction, in that order. |
| Example's `GraduateMustFollow` | `prepare` must be sent directly (not through another program) and immediately followed by `graduate` for the same launch; a compute-budget instruction goes before `prepare`, not between. |
| `9000 InvalidAmmConfig` on devnet | Build with `--features devnet` and pass a devnet fee tier. |
| LiteSVM won't load your program | Build with `cargo build-sbf --arch v0`: Anchor 1.x defaults to SBPF v3, which LiteSVM's default runtime rejects. |
| `anchor-spl: enable exactly one of the solana-v3 or solana-v4 features` | Add `features = [..., "solana-v3"]` to `anchor-spl` (1.2.1+) with `default-features = false`. |

## API

| Item | What |
|---|---|
| `ids` | Raydium CPMM program ID, fee receiver, seeds, `initialize` discriminator. Mainnet by default; `devnet` feature for devnet. |
| `pda` | `amm_config(index)`, `pool_authority()`, `lp_mint(pool)`, `vault(pool, mint)`, `observation(pool)`. |
| `create_pool_fee(&AccountInfo)` | Raydium's pool-creation fee from an `AmmConfig`, owner checked. |
| `graduate(GraduateAccounts, GraduateParams)` | The whole flow above. |
| `LpLock` | `Burn` (liquidity locked forever) or `SendTo { owner, token_account }`. |
| `GraduateError` | Error codes from 9000, clear of your program's 6000+. |
| `RENT_ALLOWANCE` | Lamports fronted for rent; unspent part refunded. |

`graduate` checks the program IDs, wrapped-SOL mint, base mint's owner and
`AmmConfig` owner itself. Raydium checks its own PDAs and the fee receiver;
the Associated Token program checks the temporary account addresses.

## Tests

```bash
make test   # builds the example for SBPF v0, runs every test (mainnet + devnet IDs)
make lint   # clippy -D warnings
make ci     # everything CI runs (fmt, test, clippy, cargo-audit), locally
```

`examples/launchpad/tests/graduation.rs` runs against Raydium CPMM, SPL Token
and Token-2022 as deployed on mainnet ([fixtures](examples/launchpad/tests/fixtures)):

- base mint sorting before and after wrapped SOL, on SPL Token and Token-2022;
- the pool opens at exactly `sol_amount / token_amount`;
- every LP token burned, or delivered with `SendTo`;
- temporary accounts closed, authority emptied, payer charged exactly `cost`;
- SOL and tokens sent to the authority, its temporary accounts or the pool
  address change nothing, and planted tokens are burned;
- a rival pool at Raydium's default address doesn't block graduation, and
  nobody else can create a pool at ours;
- graduating twice, graduating without `prepare`, and an `AmmConfig` not
  owned by Raydium all fail;
- regressions from the [security reviews](#security-review): the caller can't
  choose the LP recipient, and our own PDAs are refused as recipient; the fee
  tier is pinned; `prepare` only runs directly and right before `graduate` of
  the same launch (a test-only attacker program, `tests/forwarder`, tries it
  through a CPI), and a failing graduation leaves the SOL in place; SOL sent
  ahead of time to any address the graduation creates doesn't block it;
- documented behaviour: which Token-2022 extensions Raydium accepts, the
  minimum size, Raydium creator fees staying off, a freeze authority being
  able to block graduation, and the largest transaction fitting in 1232
  bytes.

## Security review

Two internal adversarial reviews (2026-10, not an audit) tried to break the
crate and the example with LiteSVM proofs. Neither found a way to steal
funds or block a graduation through the crate itself. They found these
issues, all fixed and covered by regression tests:

| Severity | Issue | Fix |
|---|---|---|
| Critical | Example: any caller could name the LP recipient, take every LP token and withdraw the pool. | Recipient fixed at launch creation and checked; `SendTo` documented. |
| Medium | Example: fee tier not pinned; the first caller chose it. | Pinned to tier 0 by seeds. |
| Low | Example: `prepare` alone could strand the SOL if graduation then failed. | `prepare` requires `graduate` next. |
| Low | Crate: didn't check the base mint is writable (a donated token could then block graduation). | Checked. |
| Low | Example: `prepare` called through another program got past the check above, followed by a `graduate` of a different launch. | `prepare` must be called directly, followed by `graduate` of the same launch. |
| Info | Example: the creator could name one of our PDAs as LP recipient, making graduation impossible. | Refused at `create_launch`. |
| Info | Launches that can never graduate (tiny amounts, Token-2022 extensions Raydium rejects), freeze authority, unchecked `open_time`. | Documented in the [checklist](#security-checklist) and [Limitations](#limitations). |

Checked and found safe: Raydium creator fees stay off for these pools, so
nothing accrues to the authority; the program's PDAs can't be made to sign
anything else; the LP recipient's token account can't be substituted.

Requires Rust 1.89 (pinned), Solana CLI 4.1.x (`cargo build-sbf`). Build in
WSL2 or Linux.

## Alternatives

Surveyed 2026-10:

- **Raydium's `raydium-cp-swap`** (crates.io 0.1.x, Anchor 0.30; git `master`
  pins `anchor-lang = "=1.0.2"`) has a `cpi` feature with the `initialize`
  accounts. The exact Anchor pin conflicts with Anchor 1.2 programs, and it
  stops at the CPI: no PDA pool address, wrapping, LP lock or cleanup.
- **[raydium-cpi-example](https://github.com/raydium-io/raydium-cpi-example)**:
  examples, not a library; same Anchor version constraint.
- **`pinocchio-raydium-cpmm-cpi`**: CPI helpers for Pinocchio programs, not
  Anchor.
- **`raydium-cp-swap-cpi`** and client SDKs (`raydium_cpmm`,
  `raydium-cpmm-rust-sdk`): off-chain clients or bare CPI bindings.

None covers the launchpad graduation flow, so this crate builds the 20-account
CPI itself (about 30 lines) instead of taking on a pinned dependency.

## Origin and licence

Extracted from [riff](https://github.com/use-riff), where this graduation
runs on devnet. Dual-licensed under [MIT](LICENSE-MIT) or
[Apache-2.0](LICENSE-APACHE), at your option. The Raydium CPMM program in the
test fixtures is Apache-2.0, Copyright Raydium; see its
[licence](examples/launchpad/tests/fixtures/LICENSE-raydium-cp-swap).

**Educational use.** Not audited and not on crates.io. Review it, test it
against your own program, and get an audit before mainnet.
