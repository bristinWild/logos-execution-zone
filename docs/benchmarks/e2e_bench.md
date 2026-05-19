# e2e_bench

End-to-end LEZ scenarios driven through the wallet against an in-process sequencer + indexer wired to an external Bedrock node. Times each step and records borsh sizes per block, split by tx variant.

No numeric tables here yet. Absolute wall time and block sizes depend heavily on the bedrock config (block cadence and confirmation depth) and on dev-mode vs real proving; re-run the bench locally to get numbers for your own setup. Canonical numbers will be added once the bench runs against the standard configuration.

## Scenarios

| Scenario | Description |
|---|---|
| token | Sequential public token Send + one shielded recipient setup. |
| amm | Pool create, add liquidity, swap, remove liquidity. All public. |
| fanout | One sender → N recipients, sequential. All public. |
| private | Shielded, deshielded, private→private chained private flow. |
| parallel | N senders submit concurrently into one block. All public. |

## Dev-mode vs real-proving

`RISC0_DEV_MODE=1` makes the prover emit stub receipts instead of running the recursive STARK pipeline. The table compares each quantity in dev mode vs real proving for the two classes of scenarios:

| Quantity | Public-only scenarios (dev → real) | PPE-bearing scenarios (dev → real) |
|---|---|---|
| Wall time per step | same in both modes | real adds ~100 s per PPE step |
| `public_tx_bytes` | same in both modes | same in both modes |
| `ppe_tx_bytes` | n/a | dev ≈ 2 KB stub → real ≈ 225 KB (matches `S_agg` from cycle_bench) |
| `block_bytes` | same in both modes | real adds ~225 KB per PPE tx in the block |
| `bedrock_finality_s` | same in both modes | same in both modes (L1 cadence, not LEZ prover) |
| Blocks captured | similar in both modes | real captures more empty clock-only ticks that fill prove wall-time |

Numbers are intentionally omitted in this document until the canonical run lands. Public-only scenarios converge between modes within run-to-run jitter; the qualitative differences are captured by the table above.

## Methodology

Per scenario, every produced block is fetched via `getBlock(BlockId)` and serialized with `borsh::to_vec(&Block)`. Each transaction is serialized individually and counted by variant. Empty clock-only ticks give the per-block fixed-cost baseline. Wall time is captured per step (submit + inclusion + wallet sync) and per scenario (setup + steps + closing bedrock finality wait).

## Reproduce

```sh
export LEZ_BEDROCK_BIN=/path/to/logos-blockchain/target/release/logos-blockchain-node
export LEZ_BEDROCK_CONFIG_DIR=/path/to/bedrock/configs

# Dev-mode sweep (fast, ~16 min for all five scenarios)
RISC0_DEV_MODE=1 cargo run --release -p e2e_bench -- --scenario all

# Real-proving for representative private flow (~6 min on M2 Pro CPU)
cargo run --release -p e2e_bench -- --scenario private

# Real-proving for representative public flow (~3 min)
cargo run --release -p e2e_bench -- --scenario amm
```

JSON output: `target/e2e_bench_dev.json` / `target/e2e_bench_prove.json` (suffix toggled by `RISC0_DEV_MODE`).

## Caveats

- Dev-mode `ppe_tx_bytes` and PPE-step latencies are not representative of production; use real-proving numbers for any fee-model input that touches the storage or prover-cost components.
- Single-host run, no GPU acceleration. Real-proving on production prover hardware will move per-step latencies by orders of magnitude; byte counts will not change.
- Bedrock running locally; no real network latency between sequencer and Bedrock.
- Bedrock L1 finality (`bedrock_finality_s`) is set by the bedrock config in `LEZ_BEDROCK_CONFIG_DIR` (block cadence × confirmation depth). Different configs will shift `bedrock_finality_s` materially.
- Some scenarios share account state via the same wallet; this is intentional (mirrors `integration_tests::TestContext`) and not a realistic multi-wallet workload.
