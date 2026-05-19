# e2e_bench

End-to-end LEZ scenarios driven through the wallet against an in-process sequencer + indexer wired to an external Bedrock node. Times each step (submit, inclusion, wallet sync) and records borsh sizes for every block produced, split into per-tx-variant counts.

## Run

Required env vars (no defaults):

```sh
export LEZ_BEDROCK_BIN=/path/to/logos-blockchain/target/release/logos-blockchain-node
export LEZ_BEDROCK_CONFIG_DIR=/path/to/bedrock/configs
# optional: LEZ_BEDROCK_PORT (default 18080)
```

The config dir must contain `node-config.yaml` and a `deployment-settings.yaml` template with the literal string `PLACEHOLDER_CHAIN_START_TIME` (rewritten per launch).

```sh
# All scenarios, dev-mode proving (fast)
RISC0_DEV_MODE=1 cargo run --release -p e2e_bench -- --scenario all

# One scenario, real proving (slow)
cargo run --release -p e2e_bench -- --scenario amm
```

Scenarios: `token`, `amm`, `fanout`, `private`, `parallel`, `all`.

## What you'll see

Per scenario: a step table (`submit_s`, `inclusion_s`, `sync_s`, `total_s`) and a size summary covering every block captured during the scenario (block_bytes total/mean/min/max; per-tx-variant sizes for public, PPE, and program-deployment transactions).

The fanout, parallel, and private scenarios are the most representative for L1-payload-size measurements since they put multiple txs per block.

JSON output is written to `target/e2e_bench.json`.
