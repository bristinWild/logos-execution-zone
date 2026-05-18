//! End-to-end LEZ scenario bench.
//!
//! Spins up the full stack (native Bedrock node launched per-scenario via
//! `BedrockHandle` + in-process sequencer + indexer + wallet via
//! `BenchContext`) and drives the wallet through configurable scenarios that
//! mirror real user flows. Times each step and records borsh-serialized
//! block + tx sizes per scenario.
//!
//! Required env vars (no defaults; see `tools/e2e_bench/README.md`):
//!   LEZ_BEDROCK_BIN          absolute path to logos-blockchain-node.
//!   LEZ_BEDROCK_CONFIG_DIR   directory with node-config.yaml + deployment template.
//!
//! Run examples:
//!   RISC0_DEV_MODE=1 cargo run --release -p e2e_bench -- --scenario all.
//!   cargo run --release -p e2e_bench -- --scenario amm.
//!
//! `RISC0_DEV_MODE=1` skips proving and produces latency-only numbers in
//! ~minutes; omitting it produces realistic proving-inclusive numbers but
//! the run takes much longer.

#![expect(
    clippy::arbitrary_source_item_ordering,
    clippy::arithmetic_side_effects,
    clippy::as_conversions,
    clippy::doc_markdown,
    clippy::float_arithmetic,
    clippy::let_underscore_must_use,
    clippy::let_underscore_untyped,
    clippy::missing_const_for_fn,
    clippy::print_stderr,
    clippy::print_stdout,
    clippy::single_call_fn,
    clippy::single_match_else,
    clippy::std_instead_of_core,
    clippy::too_many_lines,
    clippy::wildcard_enum_match_arm,
    reason = "Bench tool: matches test-style fixture code"
)]

use std::{path::PathBuf, time::Duration};

use anyhow::{Context as _, Result};
use bedrock_handle::BedrockHandle;
use bench_context::BenchContext;
use clap::{Parser, ValueEnum};
use harness::ScenarioResult;
use serde::Serialize;

mod bedrock_handle;
mod bench_context;
mod harness;
mod scenarios;

#[derive(Copy, Clone, Debug, ValueEnum)]
enum ScenarioName {
    Token,
    Amm,
    Fanout,
    Private,
    Parallel,
    All,
}

#[derive(Parser, Debug)]
#[command(about = "End-to-end LEZ scenario bench")]
struct Cli {
    /// Which scenario(s) to run.
    #[arg(long, value_enum, default_value_t = ScenarioName::All)]
    scenario: ScenarioName,

    /// Optional JSON output path. Defaults to <workspace>/target/e2e_bench.json.
    #[arg(long)]
    json_out: Option<PathBuf>,
}

#[derive(Debug, Serialize)]
struct BenchRunReport {
    risc0_dev_mode: bool,
    scenarios: Vec<ScenarioResult>,
    total_wall_seconds: f64,
}

#[tokio::main(flavor = "multi_thread")]
async fn main() -> Result<()> {
    // integration_tests initializes env_logger via a LazyLock, so we leave logger
    // setup to it. Set RUST_LOG=info before running to see logs.

    let cli = Cli::parse();
    let risc0_dev_mode = std::env::var("RISC0_DEV_MODE").is_ok_and(|v| !v.is_empty() && v != "0");

    eprintln!(
        "e2e_bench: scenario={:?}, RISC0_DEV_MODE={}",
        cli.scenario,
        if risc0_dev_mode { "1" } else { "unset/0" }
    );

    let to_run: Vec<ScenarioName> = match cli.scenario {
        ScenarioName::All => vec![
            ScenarioName::Token,
            ScenarioName::Amm,
            ScenarioName::Fanout,
            ScenarioName::Private,
            ScenarioName::Parallel,
        ],
        other => vec![other],
    };

    let overall_started = std::time::Instant::now();
    let mut all_results = Vec::with_capacity(to_run.len());

    for name in to_run {
        eprintln!("\n=== running scenario: {name:?} ===");
        let setup_started = std::time::Instant::now();
        // Spawn a fresh Bedrock node for this scenario. Each scenario therefore
        // starts with an empty chain so the indexer never has a backlog from a
        // prior scenario.
        let bedrock = BedrockHandle::launch_fresh()
            .await
            .with_context(|| format!("failed to spawn Bedrock for scenario {name:?}"))?;
        let bedrock_addr_string = format!("{}", bedrock.addr());
        // Safety: we restore the previous LEZ_BEDROCK_ADDR value (if any) at scenario teardown.
        // SAFETY: this happens before any threaded setup that reads env.
        unsafe {
            std::env::set_var("LEZ_BEDROCK_ADDR", &bedrock_addr_string);
        }

        let mut ctx = BenchContext::new()
            .await
            .with_context(|| format!("failed to setup BenchContext for scenario {name:?}"))?;
        let setup_ms = elapsed_ms(setup_started);
        eprintln!("setup: {setup_ms:.1} ms");

        let disk_before = ctx.disk_sizes();
        let mut result = run_scenario(name, setup_ms, &mut ctx).await?;
        result.disk_before = Some(disk_before);
        result.disk_after = Some(ctx.disk_sizes());
        result.bedrock_finality_ms = Some(measure_bedrock_finality(&ctx).await?);
        harness::print_table(&result);
        all_results.push(result);

        drop(ctx);
        drop(bedrock);
        // Give Bedrock a moment to shut down before the next scenario.
        tokio::time::sleep(Duration::from_secs(2)).await;
    }

    let total_wall_seconds = overall_started.elapsed().as_secs_f64();
    eprintln!("\nTotal wall time: {total_wall_seconds:.1}s");

    let report = BenchRunReport {
        risc0_dev_mode,
        scenarios: all_results,
        total_wall_seconds,
    };

    let out_path = match cli.json_out {
        Some(p) => p,
        None => {
            let workspace_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("..")
                .join("..")
                .canonicalize()?;
            let suffix = if risc0_dev_mode { "dev" } else { "prove" };
            workspace_root
                .join("target")
                .join(format!("e2e_bench_{suffix}.json"))
        }
    };
    if let Some(parent) = out_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&out_path, serde_json::to_string_pretty(&report)?)?;
    eprintln!("\nJSON written to {}", out_path.display());

    Ok(())
}

async fn run_scenario(
    name: ScenarioName,
    setup_ms: f64,
    ctx: &mut BenchContext,
) -> Result<ScenarioResult> {
    let result = match name {
        ScenarioName::Token => scenarios::token::run(ctx).await?,
        ScenarioName::Amm => scenarios::amm::run(ctx).await?,
        ScenarioName::Fanout => scenarios::fanout::run(ctx).await?,
        ScenarioName::Private => scenarios::private::run(ctx).await?,
        ScenarioName::Parallel => scenarios::parallel::run(ctx).await?,
        ScenarioName::All => unreachable!("dispatched above"),
    };
    Ok(ScenarioResult { setup_ms, ..result })
}

fn elapsed_ms(t: std::time::Instant) -> f64 {
    t.elapsed().as_secs_f64() * 1_000.0
}

/// Poll the indexer's L1-finalised block id until it catches up with the
/// sequencer's last block id. This is effectively the sequencer→Bedrock posting
/// plus Bedrock finalisation plus indexer ingest latency.
async fn measure_bedrock_finality(ctx: &BenchContext) -> Result<f64> {
    use indexer_service_rpc::RpcClient as _;
    use jsonrpsee::ws_client::WsClientBuilder;
    use sequencer_service_rpc::RpcClient as _;

    let indexer_url = format!("ws://{}", ctx.indexer_addr());
    let indexer_ws = WsClientBuilder::default()
        .build(&indexer_url)
        .await
        .context("connect indexer WS")?;
    let sequencer_tip = ctx.sequencer_client().get_last_block_id().await?;

    let started = std::time::Instant::now();
    let deadline = started + Duration::from_secs(60);
    loop {
        match indexer_ws.get_last_finalized_block_id().await {
            Ok(Some(b)) if b >= sequencer_tip => {
                return Ok(started.elapsed().as_secs_f64() * 1_000.0);
            }
            Ok(_) => {}
            Err(err) => eprintln!("indexer last_synced poll error: {err:#}"),
        }
        if std::time::Instant::now() > deadline {
            eprintln!("indexer did not catch up to {sequencer_tip} within 60s");
            return Ok(started.elapsed().as_secs_f64() * 1_000.0);
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
}
