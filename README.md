# anchor-raydium-cpmm

Graduate a token from an Anchor program into a Raydium CPMM pool and lock the
liquidity: the step every bonding-curve launchpad ends with.

```toml
[dependencies]
anchor-raydium-cpmm = { git = "https://github.com/KevinGod1234/anchor-raydium-cpmm" }
# devnet: features = ["devnet"]
```

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
against running twice. If the SOL sits in a program-owned data account, move
it directly:

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
        lp_lock: LpLock::Burn,
        open_time: 0,
    },
)?;
launch.pool = Some(outcome.pool); // and refuse to graduate again
```

**5. Send both in one transaction** with a compute budget: graduation uses
about 190k to 240k compute units (measured; more than the 200k default).

```
[ ComputeBudget::SetComputeUnitLimit(400_000), prepare, graduate ]
```

**6. Pin the fee tier.** The crate checks the `AmmConfig` is Raydium's; your
program should also pin *which* tier it uses (the example accepts any, to
stay short).

## API

| Item | What |
|---|---|
| `ids` | Raydium CPMM program ID, fee receiver, seeds, `initialize` discriminator. Mainnet by default; `devnet` feature for devnet. |
| `pda` | `pool_authority()`, `lp_mint(pool)`, `vault(pool, mint)`, `observation(pool)`. |
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
  owned by Raydium all fail.

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

Not audited. Review it and test against your own program before mainnet.
