//! Minimal reference event indexer for LEZ.
//! Polls getTransactionReceipt until tx is included or rejected,
//! then prints all emitted events.

use clap::Parser;
use serde::{Deserialize, Serialize};
use std::time::Duration;

#[derive(Parser)]
#[command(name = "event_indexer")]
#[command(about = "Minimal LEZ event indexer — polls getTransactionReceipt and logs events")]
struct Cli {
    #[arg(long, default_value = "http://localhost:3040")]
    rpc: String,
    #[arg(long)]
    tx_hash: String,
    #[arg(long, default_value = "500")]
    poll_ms: u64,
    #[arg(long, default_value = "60")]
    max_polls: u32,
}

#[derive(Debug, Deserialize, Serialize)]
struct AttributedEvent {
    program_id: serde_json::Value,
    discriminant: u32,
    sequence: u32,
    payload: Vec<u8>,
}

#[derive(Debug, Deserialize, Serialize)]
struct TxReceipt {
    tx_hash: serde_json::Value,
    status: String,
    events: Vec<AttributedEvent>,
    error: Option<String>,
    block_id: Option<u64>,
}

#[derive(Debug, Serialize)]
struct RpcRequest {
    method: String,
    params: serde_json::Value,
}

#[derive(Debug, Deserialize)]
struct RpcResponse {
    result: Option<TxReceipt>,
}

fn hex_encode(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn main() {
    let cli = Cli::parse();

    let tx_hash: Vec<u32> = cli.tx_hash
        .split(',')
        .filter_map(|s| s.trim().parse().ok())
        .collect();

    if tx_hash.is_empty() {
        eprintln!("Error: invalid tx_hash. Use comma-separated u32 values.");
        std::process::exit(1);
    }

    let client = reqwest::blocking::Client::new();
    let interval = Duration::from_millis(cli.poll_ms);

    println!("LEZ Event Indexer");
    println!("═══════════════════════════════════════");
    println!("  RPC : {}", cli.rpc);
    println!("═══════════════════════════════════════");

    for attempt in 1..=cli.max_polls {
        let req = RpcRequest {
            method: "getTransactionReceipt".to_string(),
            params: serde_json::json!({ "tx_hash": tx_hash }),
        };

        if let Ok(resp) = client.post(&cli.rpc).json(&req).send() {
            if let Ok(rpc) = resp.json::<RpcResponse>() {
                if let Some(receipt) = rpc.result {
                    match receipt.status.as_str() {
                        "pending" | "unknown" => {
                            print!("\r  [{attempt}/{}] Waiting...", cli.max_polls);
                            let _ = std::io::Write::flush(&mut std::io::stdout());
                        }
                        status => {
                            println!("\n  Status : {}", status.to_uppercase());
                            if let Some(b) = receipt.block_id { println!("  Block  : {b}"); }
                            if let Some(e) = &receipt.error { println!("  Error  : {e}"); }
                            if receipt.events.is_empty() {
                                println!("\n  No events emitted.");
                            } else {
                                println!("\n  Events ({} total):", receipt.events.len());
                                for ev in &receipt.events {
                                    println!("  [{}] discriminant={} bytes={}",
                                        ev.sequence, ev.discriminant, ev.payload.len());
                                    match std::str::from_utf8(&ev.payload) {
                                        Ok(s) if s.chars().all(|c| c.is_ascii_graphic() || c == ' ') => {
                                            println!("       utf8 : {s}");
                                        }
                                        _ => println!("       hex  : {}", hex_encode(&ev.payload)),
                                    }
                                }
                            }
                            println!("═══════════════════════════════════════");
                            return;
                        }
                    }
                }
            }
        }
        std::thread::sleep(interval);
    }

    println!("\n  Timed out after {} polls.", cli.max_polls);
    std::process::exit(1);
}
