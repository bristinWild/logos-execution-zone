# integration_bench

End-to-end LEZ scenarios driven through the wallet against a docker-compose Bedrock node + in-process sequencer + indexer (via `test_fixtures::TestContext`). Times each step and records borsh sizes per block, split by tx variant.

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

Per scenario, every produced block is fetched via `getBlock(BlockId)` and serialized with `borsh::to_vec(&Block)`. Each transaction is serialized individually and counted by variant. Empty clock-only ticks give the per-block fixed-cost baseline. Wall time is captured per step (submit + inclusion + wallet sync) and aggregated to the per-scenario `total_s`. The one-time stack-setup cost (`shared_setup_s` at the run level) and the closing bedrock finality wait (`bedrock_finality_s` per scenario) are reported separately, not folded into `total_s`.

## Reproduce

Prerequisite: a running local Docker daemon (the `bedrock/docker-compose.yml` is brought up by the bench).

```sh
# Dev-mode sweep (fast)
RISC0_DEV_MODE=1 cargo run --release -p integration_bench -- --scenario all

# Real-proving for representative private flow
cargo run --release -p integration_bench -- --scenario private

# Real-proving for representative public flow
cargo run --release -p integration_bench -- --scenario amm
```

JSON output: `target/integration_bench_dev.json` / `target/integration_bench_prove.json` (suffix toggled by `RISC0_DEV_MODE`).

## Caveats

- Dev-mode `ppe_tx_bytes` and PPE-step latencies are not representative of production; use real-proving numbers for any fee-model input that touches the storage or prover-cost components.
- Single-host run, no GPU acceleration. Real-proving on production prover hardware will move per-step latencies by orders of magnitude; byte counts will not change.
- Bedrock running locally via docker-compose; no real network latency between sequencer and Bedrock.
- Bedrock L1 finality (`bedrock_finality_s`) is set by the bedrock config in `bedrock/docker-compose.yml` (block cadence × confirmation depth). Different configs will shift `bedrock_finality_s` materially.
- All scenarios share a single TestContext for the run (one bedrock + sequencer + indexer + wallet for the whole run, chain state accumulating across scenarios), which matches how the node runs in production.
