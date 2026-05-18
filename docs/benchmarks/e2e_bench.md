# e2e_bench

End-to-end LEZ scenarios driven through the wallet against an in-process sequencer + indexer wired to an external Bedrock node. Times each step and records borsh sizes per block, split by tx variant.

## Machine

| Field | Value |
|---|---|
| Chip | Apple M2 Pro (8P+4E) |
| RAM | 16 GB |
| OS | macOS 15.5 |
| Rust | 1.94.0 |
| Risc0 zkVM | 3.0.5 |
| Profile | release |

## Scenarios

| Scenario | Description |
|---|---|
| token | Sequential public token Send + one shielded recipient setup. |
| amm | Pool create, add liquidity, swap, remove liquidity. All public. |
| fanout | One sender → N recipients, sequential. All public. |
| private | Shielded, deshielded, private→private chained private flow. |
| parallel | N senders submit concurrently into one block. All public. |

## Dev-mode vs real-proving

`RISC0_DEV_MODE=1` makes the prover emit stub receipts instead of running the recursive STARK pipeline. The table compares each quantity in **dev mode vs real proving** for the two classes of scenarios:

| Quantity | Public-only scenarios (dev → real) | PPE-bearing scenarios (dev → real) |
|---|---|---|
| Wall time per step | same in both modes | real adds ~100 s per PPE step |
| `public_tx_bytes` | same in both modes | same in both modes |
| `ppe_tx_bytes` | n/a | dev ≈ 2 KB stub → real ≈ 225 KB (matches `S_agg` from cycle_bench) |
| `block_bytes` | same in both modes | real adds ~225 KB per PPE tx in the block |
| `bedrock_finality_ms` | same in both modes | same in both modes (L1 cadence, not LEZ prover) |
| Blocks captured | similar in both modes | real captures more empty clock-only ticks that fill prove wall-time |

Tables below report dev-mode for all five scenarios. Real-proving numbers are included for `amm_swap_flow` (representative all-public) and `private_chained_flow` (representative chained-private flow); the public-only scenarios converge between modes within run-to-run jitter, so a full real-proving sweep is not run here.

## Step latencies — dev mode (`RISC0_DEV_MODE=1`)

Per-scenario wall time and Bedrock L1-finality latency for the closing tip.

| Scenario | total_ms | total_s | bedrock_finality_ms | bedrock_finality_s |
|---|---:|---:|---:|---:|
| token_onboarding | 60,808 | 60.81 | 24,593 | 24.59 |
| amm_swap_flow | 162,058 | 162.06 | 19,210 | 19.21 |
| multi_recipient_fanout | 222,206 | 222.21 | 16,020 | 16.02 |
| private_chained_flow | 80,700 | 80.70 | 23,963 | 23.96 |
| parallel_fanout | 244,387 | 244.39 | 23,770 | 23.77 |

Total dev-mode wall time across all five: 912.9 s.

## Step latencies — real proving (selected scenarios)

| Scenario | total_ms | total_s | bedrock_finality_ms | bedrock_finality_s | Δ vs dev |
|---|---:|---:|---:|---:|---:|
| amm_swap_flow | 162,437 | 162.44 | ~19,210 | ~19.21 | ~0 (all-public) |
| private_chained_flow | 354,843 | 354.84 | 23,778 | 23.78 | +274.14 s (≈ 91 s per PPE step × 3) |

Per-step breakdown for `private_chained_flow` in real proving:

| Step | submit_ms | inclusion_ms | total_ms | total_s |
|---|---:|---:|---:|---:|
| token_new_fungible (public) | 1.1 | 20,276.0 | 20,291.2 | 20.29 |
| shielded_transfer (PPE) | 111,683.3 | 1.0 | 111,730.4 | 111.73 |
| deshielded_transfer (PPE) | 111,454.7 | 1.1 | 111,511.2 | 111.51 |
| private_to_private (PPE) | 111,237.0 | 1.1 | 111,293.0 | 111.29 |

PPE steps move the cost from `inclusion_ms` (waiting for the next sealed block) to `submit_ms` (the wallet itself proving the PPE circuit before sending). Each PPE prove is ≈ 111 s on this CPU.

## Block + tx sizes (borsh) — dev mode

Per scenario, every produced block is fetched via `getBlock(BlockId)` and serialized with `borsh::to_vec(&Block)`. Each transaction is serialized individually and counted by variant. The empty clock-only ticks at `min` give the per-block fixed-cost baseline (≈ 334 bytes across all scenarios).

| Scenario | blocks | block_bytes (mean) | block_bytes (min..max) | public_tx (mean / n) | ppe_tx (mean / n) |
|---|---:|---:|---|---:|---:|
| token_onboarding | 6 | 881 | 334..2,890 | 206 / 8 | 2,556 / 1 |
| amm_swap_flow | 16 | 553 | 334..1,011 | 248 / 24 | n/a |
| multi_recipient_fanout | 22 | 513 | 334..707 | 221 / 33 | n/a |
| private_chained_flow | 8 | 1,399 | 334..3,565 | 177 / 9 | 2,715 / 3 |
| parallel_fanout | 24 | 646 | 334..3,904 | 248 / 45 | n/a |

## Block + tx sizes (borsh) — real proving

| Scenario | blocks | block_bytes (mean) | block_bytes (min..max) | public_tx (mean / n) | ppe_tx (mean / n) |
|---|---:|---:|---|---:|---:|
| amm_swap_flow | 16 | 553 | 334..1,011 | 248 / 24 | n/a |
| private_chained_flow | 35 | 19,692 | 334..226,578 | 159 / 36 | 225,728 / 3 |

`amm_swap_flow` is byte-identical between dev and real (no proof payload). `private_chained_flow`'s `ppe_tx_bytes` matches the cycle_bench `S_agg` measurement (≈ 225 KB borsh InnerReceipt). The `block_bytes` max (226,578) is the block containing the largest PPE transaction.

## Findings

- Public-only scenarios converge between dev mode and real proving in both latency and byte counts. Either mode is suitable to characterize them.
- PPE transactions are ≈ 225 KB on the wire in real proving, dominated by the outer succinct proof. Dev mode emits a ≈ 2 KB stub that does not represent the L1 payload — fee-model storage gas inputs must come from a real-proving run.
- Per-PPE-step prove cost on M2 Pro CPU is ≈ 110-120 s, paid on the wallet side at submit time (not on the sequencer). For a single-program chained flow the cost stacks linearly.
- Empty clock-only ticks set the per-block fixed-cost baseline at ≈ 334 bytes across all scenarios and both modes.
- Bedrock L1 finality stays around 20 s regardless of proving mode, because finality is paced by L1 cadence, not the LEZ prover.

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
- Some scenarios share account state via the same wallet; this is intentional (mirrors `integration_tests::TestContext`) and not a realistic multi-wallet workload.
