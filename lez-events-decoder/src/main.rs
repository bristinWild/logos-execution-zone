//! lez-events-decoder — CLI tool for decoding LEZ transaction events.
//!
//! Usage:
//!   lez-events-decoder --receipt receipt.json
//!   echo '<json>' | lez-events-decoder --stdin

use clap::Parser;
use serde::{Deserialize, Serialize};
use std::{fs, io::{self, Read}};

#[derive(Parser)]
#[command(name = "lez-events-decoder")]
#[command(about = "Decode and display LEZ transaction events in human-readable form")]
struct Cli {
    /// Path to a JSON receipt file
    #[arg(long)]
    receipt: Option<String>,

    /// Read JSON from stdin
    #[arg(long)]
    stdin: bool,
}

/// Mirrors TxReceipt from sequencer_service_rpc (JSON-compatible)
#[derive(Debug, Deserialize, Serialize)]
pub struct TxReceiptJson {
    pub tx_hash: serde_json::Value,
    pub status: String,
    pub events: Vec<EventJson>,
    pub error: Option<String>,
    pub block_id: Option<u64>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct EventJson {
    pub program_id: serde_json::Value,
    pub discriminant: u32,
    pub sequence: u32,
    pub payload: Vec<u8>,
}

fn decode_and_print(json: &str) {
    let receipt: TxReceiptJson = match serde_json::from_str(json) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("Error parsing receipt JSON: {e}");
            std::process::exit(1);
        }
    };

    println!("Transaction Receipt");
    println!("═══════════════════════════════════════");
    println!("  Status : {}", receipt.status.to_uppercase());
    if let Some(ref err) = receipt.error {
        println!("  Error  : {err}");
    }
    if let Some(block_id) = receipt.block_id {
        println!("  Block  : {block_id}");
    }
    println!();

    if receipt.events.is_empty() {
        println!("  No events emitted.");
    } else {
        println!("  Events ({} total):", receipt.events.len());
        println!("  ─────────────────────────────────────");
        for event in &receipt.events {
            println!("  [{}] discriminant={} payload_bytes={}",
                event.sequence,
                event.discriminant,
                event.payload.len(),
            );
            // Try to decode payload as UTF-8 for display
            match std::str::from_utf8(&event.payload) {
                Ok(s) if s.chars().all(|c| c.is_ascii_graphic() || c == ' ') => {
                    println!("       payload_utf8 : {s}");
                }
                _ => {
                    println!("       payload_hex  : {}", hex_encode(&event.payload));
                }
            }
        }
    }
    println!("═══════════════════════════════════════");
}

fn hex_encode(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn main() {
    let cli = Cli::parse();

    let json = if cli.stdin {
        let mut buf = String::new();
        io::stdin().read_to_string(&mut buf).expect("Failed to read stdin");
        buf
    } else if let Some(path) = cli.receipt {
        fs::read_to_string(&path).unwrap_or_else(|e| {
            eprintln!("Failed to read file {path}: {e}");
            std::process::exit(1);
        })
    } else {
        eprintln!("Provide --receipt <file> or --stdin");
        std::process::exit(1);
    };

    decode_and_print(&json);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decode_empty_receipt() {
        let json = r#"{
            "tx_hash": [1,2,3],
            "status": "included",
            "events": [],
            "error": null,
            "block_id": 5
        }"#;
        let receipt: TxReceiptJson = serde_json::from_str(json).unwrap();
        assert_eq!(receipt.status, "included");
        assert!(receipt.events.is_empty());
        assert_eq!(receipt.block_id, Some(5));
    }

    #[test]
    fn decode_receipt_with_events() {
        let json = r#"{
            "tx_hash": [1,2,3],
            "status": "rejected",
            "events": [
                {"program_id": [0,0,0,0,0,0,0,0], "discriminant": 42, "sequence": 0, "payload": [1,2,3]}
            ],
            "error": "Insufficient funds",
            "block_id": null
        }"#;
        let receipt: TxReceiptJson = serde_json::from_str(json).unwrap();
        assert_eq!(receipt.status, "rejected");
        assert_eq!(receipt.events.len(), 1);
        assert_eq!(receipt.events[0].discriminant, 42);
        assert_eq!(receipt.error, Some("Insufficient funds".to_string()));
    }

    #[test]
    fn hex_encode_works() {
        assert_eq!(hex_encode(&[0xde, 0xad, 0xbe, 0xef]), "deadbeef");
    }
}
