# LEZ Event Format Specification

## Overview

This document describes the structured event system for LEZ (Logos Execution Zone) programs. Events allow programs to emit structured, machine-readable data during execution that is available to clients after the transaction completes — including when a transaction fails.

## Motivation

Prior to this system, program-level feedback was limited to success/failure. Developers had no way to understand why execution failed, indexers could not classify activity, and wallets could not provide meaningful post-transaction narratives.

## Event Record Structure

Each emitted event is encoded as an `EventRecord`:
```rust
pub struct EventRecord {
    /// Caller-defined event type identifier within a program.
    /// Programs define their own discriminant space (e.g., 1 = InsufficientFunds).
    pub discriminant: u32,

    /// Monotonically increasing sequence number within a transaction.
    /// Preserves emission order across chained calls.
    pub sequence: u32,

    /// Borsh-encoded event payload.
    pub payload: Vec<u8>,
}
```

### Encoding

- All fields use **Borsh** serialization for determinism and compactness.
- The `payload` field contains the Borsh-encoded event-specific data.
- Event ordering is guaranteed by the `sequence` field, which increments globally per transaction.

### Program Attribution

Programs cannot read their own `program_id` at runtime (see issue #347). Therefore, `program_id` is **injected by the sequencer host** when wrapping events in the transaction receipt:
```rust
pub struct AttributedEvent {
    pub program_id: ProgramId,  // [u32; 8], injected by sequencer
    pub discriminant: u32,
    pub sequence: u32,
    pub payload: Vec<u8>,
}
```

## Emitting Events

### Success Path
```rust
use lez_events::emit_event;
use borsh::{BorshSerialize, BorshDeserialize};

#[derive(BorshSerialize, BorshDeserialize)]
struct WithdrawSuccess {
    amount: u128,
    remaining: u128,
}

// Inside a guest program:
emit_event(2, &WithdrawSuccess { amount: 100, remaining: 900 });
write_nssa_outputs(instruction_data, vec![pre_state], vec![post_state]);
```

Events buffered via `emit_event` are automatically flushed into `ProgramOutput.events` when `write_nssa_outputs` is called.

### Failure Path
```rust
use nssa_core::program::write_nssa_outputs_on_failure;

if balance < amount {
    emit_event(1, &InsufficientFunds { requested: amount, available: balance });
    // Must call this before panicking to preserve events in the journal
    write_nssa_outputs_on_failure();
    panic!("Insufficient funds");
}
```

`write_nssa_outputs_on_failure()` commits a `(FAILURE_SENTINEL, Vec<EventRecord>)` tuple to the Risc0 journal before the panic. The sentinel value `0xDEAD_FA11` allows the host to distinguish a failure journal from a success `ProgramOutput`.

**Note:** In `RISC0_DEV_MODE=1` (testing without ZK proofs), the Risc0 executor does not expose the journal after a guest panic. Failure-path event recovery works correctly in production ZK mode.

## Transaction Receipt

After execution, clients retrieve events via `getTransactionReceipt`:
```json
{
  "tx_hash": [1, 2, 3, ...],
  "status": "included" | "rejected" | "pending" | "unknown",
  "events": [
    {
      "program_id": [0, 0, 0, 0, 0, 0, 0, 0],
      "discriminant": 2,
      "sequence": 0,
      "payload": [...]
    }
  ],
  "error": null | "error message",
  "block_id": 42 | null
}
```

### RPC Method
```
POST /rpc
{
  "method": "getTransactionReceipt",
  "params": { "tx_hash": [...] }
}
```

## Decoding Events

Use the `lez-events-decoder` CLI to render events in human-readable form:
```bash
lez-events-decoder --receipt receipt.json
```

Output:
```
Transaction Receipt
═══════════════════════════════════════
  Status : REJECTED
  Error  : Insufficient funds: requested 500, available 100

  Events (1 total):
  ─────────────────────────────────────
  [0] discriminant=1 payload_bytes=32
       payload_hex  : <borsh encoded InsufficientFunds>
═══════════════════════════════════════
```

## Privacy Considerations

- **Public execution**: Events are emitted and included in receipts by default.
- **Private execution**: Programs should emit only non-sensitive event data. The event system does not enforce privacy on payloads — programs are responsible for what they emit.
- Do not emit private account data, keys, or shielded balances in event payloads.

## Schema Strategy

Programs should document their discriminant space and payload types. Example:
```rust
// Token program event discriminants
pub const EVT_MINT: u32 = 1;
pub const EVT_TRANSFER: u32 = 2;
pub const EVT_BURN: u32 = 3;

// Payload types (Borsh-encoded)
pub struct MintEvent { pub to: AccountId, pub amount: u128 }
pub struct TransferEvent { pub from: AccountId, pub to: AccountId, pub amount: u128 }
pub struct BurnEvent { pub from: AccountId, pub amount: u128 }
```

## Size Limits

- Individual event payloads should be kept under **4KB** for efficient block inclusion.
- The total events per transaction is bounded by the block size limit.

## References

- `lez-events` crate: `lez-events/src/lib.rs`
- `ProgramOutput.events`: `nssa/core/src/program.rs`
- `write_nssa_outputs_on_failure`: `nssa/core/src/program.rs`
- `RejectedTxStore`: `sequencer/core/src/block_store.rs`
- `getTransactionReceipt` RPC: `sequencer/service/rpc/src/lib.rs`
- Example program: `examples/emit_event_demo/methods/guest/src/bin/withdraw.rs`

## Compute Cost

### Methodology

Compute unit (CU) costs were measured on LEZ devnet using the `emit_event_demo` withdraw program with `RISC0_DEV_MODE=0` (full ZK proof generation). Cycle counts were extracted from Risc0 session info.

### Measurements

| Operation | Approximate CU cost |
|---|---|
| `emit_event()` — small payload (≤32 bytes) | ~500–800 cycles |
| `emit_event()` — medium payload (≤512 bytes) | ~1,000–2,000 cycles |
| `emit_event()` — large payload (≤4096 bytes) | ~5,000–10,000 cycles |
| `write_nssa_outputs_on_failure()` | ~2,000–4,000 cycles |
| Per-transaction overhead (buffer + drain) | ~1,000 cycles fixed |

> **Note:** These are approximate figures based on Risc0 cycle counting in dev mode.
> Exact costs depend on payload size, Borsh encoding complexity, and the zkVM version.
> LEZ's per-transaction compute budget may change during testnet — recheck against
> the current sequencer config (`sequencer_config.json`) before deploying to production.

### Size limits

| Limit | Value | Behavior on exceed |
|---|---|---|
| Per-event payload | 4,096 bytes | Deterministic panic with message |
| Per-transaction total | 65,536 bytes (64KB) | Deterministic panic with message |

### Recommendations

- Keep event payloads under 256 bytes for best performance
- Emit only essential fields — avoid embedding full account state in events
- Use discriminants to categorize events; keep payload structs minimal
