# anchor-raydium-cpmm

**Graduate a token from your Anchor program into a Raydium CPMM pool, and
lock the liquidity.** The last step of every bonding-curve launchpad, done
safely in one function call.

> **For learning and experimenting, not for mainnet (yet).** Unaudited.
> Build and test with it on devnet or localnet; don't put real users' funds
> behind it. The licences come with no warranty.

**Diagrams, the integration guide, the security checklist and the devnet
proof are on [GitHub](https://github.com/KevinGod1234/anchor-raydium-cpmm).**

## What it does

When a launch's bonding curve sells out, the SOL it raised and a token
reserve have to become a DEX pool nobody can pull the liquidity out of.
Inside your instruction, `graduate()`:

1. takes Raydium's pool-creation fee plus a rent allowance from the payer;
2. wraps **exactly** the SOL you name and moves **exactly** the tokens you
   name, never balances that anyone could inflate;
3. creates the Raydium CPMM pool at a **PDA of your program**, so nobody can
   create your pool first and set its price;
4. **burns every LP token** (liquidity locked forever), or sends them to an
   owner you fixed beforehand;
5. burns stray tokens, closes its temporary accounts, and refunds what
   wasn't spent.

## Quick start

```toml
[dependencies]
anchor-raydium-cpmm = "0.1"   # features = ["devnet"] for Raydium's devnet
```

```rust
use anchor_raydium_cpmm::{graduate, GraduateAccounts, GraduateParams, LpLock};

pub fn graduate_launch(ctx: Context<Graduate>) -> Result<()> {
    let launch = &mut ctx.accounts.launch;
    require!(launch.pool.is_none(), MyError::AlreadyGraduated);

    let outcome = graduate(
        GraduateAccounts { /* the 23 accounts from your struct */ },
        GraduateParams {
            sol_amount: launch.sol_raised,       // tracked, never a balance
            token_amount: launch.token_reserve,  // tracked, never a balance
            authority_seeds: &[b"graduation", mint.as_ref(), &[auth_bump]],
            pool_seeds: &[b"pool", mint.as_ref(), &[pool_bump]],
            token_source_seeds: &[b"launch", mint.as_ref(), &[launch.bump]],
            lp_lock: LpLock::Burn,               // liquidity locked forever
            open_time: 0,                        // trading opens right away
        },
    )?;

    launch.pool = Some(outcome.pool);            // never graduate twice
    Ok(())
}
```

Clients send `[SetComputeUnitLimit(400_000), prepare, graduate]` in one
transaction. `prepare` is a small instruction you write yourself; the
[integration guide](https://github.com/KevinGod1234/anchor-raydium-cpmm/blob/main/docs/integration.md)
shows it, along with a complete
[example program](https://github.com/KevinGod1234/anchor-raydium-cpmm/blob/main/examples/launchpad/src/lib.rs)
whose accounts struct you can copy.

## Tested

- 23 LiteSVM tests against Raydium CPMM, SPL Token and Token-2022 **as
  deployed on mainnet**, not mocks.
- [Verified on devnet](https://explorer.solana.com/tx/2J3Cxh58GHr1tNQb1eNNTGPgAXcWAMDbvYkHwbjz5bpAPWNPf5wPDbABJgcQteaodyzcHRuXEUbiGNskp19JSAZK?cluster=devnet):
  exact amounts in the pool, LP supply 0, temporary accounts closed.
- Two internal adversarial security reviews (not an audit); every finding is
  fixed and has a regression test.

Supports Anchor 1.2, SPL Token and Token-2022 base mints, with SOL as the
quote token. Costs the payer about 0.19 SOL on Raydium's 0.25% fee tier.

Before using it, read your
[security checklist](https://github.com/KevinGod1234/anchor-raydium-cpmm/blob/main/docs/security.md#checklist).

## Licence

MIT OR Apache-2.0, at your option.
