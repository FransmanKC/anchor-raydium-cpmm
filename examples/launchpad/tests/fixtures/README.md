# Test fixtures

Snapshots of mainnet programs and accounts the tests run against, so they
exercise what is really deployed rather than a mock or LiteSVM's bundled
copies.

| File | What | Address |
|---|---|---|
| `raydium_cpmm.so` | Raydium CPMM program (sha256 `36537be95ba356056fa38b2847d928078c68bf6cd79b875c140e157e6452cc71`) | `CPMMoo8L3F4NbTegBCKVNunggL7H1ZpdTHKxQB5qKP1C` |
| `amm_config_0.json` | Fee tier 0 (0.25% trade fee, 0.15 SOL pool-creation fee) | `D4FPEruKEHrG5TenZ2mpDGEfu1iUvTiqBxvpU8HLBvC2` |
| `create_pool_fee_receiver.json` | Raydium's pool-creation fee receiver | `DNXgeM9EiiaAbaWvwjHj9fQQLAX5ZsfHyvmYUNRAdNC8` |
| `spl_token.so` | SPL Token as deployed (sha256 `8190d3f7ceb6cb7a7a8d8924bff89f9f611e15ce1f806f2b6237f3311a98f697`) | `TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA` |
| `spl_token_2022.so` | Token-2022 as deployed (sha256 `0999dbf708971e723b08d1caafc988826a59c6001ed6dc02260da07defbe1469`) | `TokenzQdBNbLqP5VEhdkAS6EPFLC1PHnBqCXEpPxuEb` |
| `wsol_mint.json` | Wrapped SOL mint | `So11111111111111111111111111111111111111112` |

Taken 2026-09-26 from mainnet-beta.

## Licences

- Raydium CPMM (`raydium_cpmm.so`) is Apache-2.0, Copyright Raydium:
  <https://github.com/raydium-io/raydium-cp-swap>. Its licence is in
  `LICENSE-raydium-cp-swap`; the program is redistributed unmodified.
- SPL Token and Token-2022 are Apache-2.0, from the Solana Program Library
  (<https://github.com/solana-program>); redistributed unmodified.

## Refreshing

Read-only mainnet calls:

```bash
U=https://api.mainnet-beta.solana.com
solana program dump CPMMoo8L3F4NbTegBCKVNunggL7H1ZpdTHKxQB5qKP1C raydium_cpmm.so --url $U
solana account D4FPEruKEHrG5TenZ2mpDGEfu1iUvTiqBxvpU8HLBvC2 --output json --output-file amm_config_0.json --url $U
solana account DNXgeM9EiiaAbaWvwjHj9fQQLAX5ZsfHyvmYUNRAdNC8 --output json --output-file create_pool_fee_receiver.json --url $U
solana program dump TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA spl_token.so --url $U
solana program dump TokenzQdBNbLqP5VEhdkAS6EPFLC1PHnBqCXEpPxuEb spl_token_2022.so --url $U
solana account So11111111111111111111111111111111111111112 --output json --output-file wsol_mint.json --url $U
```

If Raydium or the token programs are upgraded, refresh these, update the
hashes above and re-run `make test`.
