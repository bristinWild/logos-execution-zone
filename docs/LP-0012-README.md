# LP-0012 — Structured Event System for LEZ Programs

This document explains the full architecture, data flow, and implementation details of the structured event system added to the Logos Execution Zone (LEZ) as part of the LP-0012 prize submission.

---

## Table of Contents

1. [Overview](#overview)
2. [Architecture](#architecture)
3. [Component Breakdown](#component-breakdown)
   - [lez-events (Guest SDK)](#lez-events-guest-sdk)
   - [ProgramOutput Integration](#programoutput-integration)
   - [Host-Side Event Extraction](#host-side-event-extraction)
   - [Sequencer Storage](#sequencer-storage)
   - [RPC Interface](#rpc-interface)
   - [CLI Decoder](#cli-decoder)
4. [Data Flow: Success Path](#data-flow-success-path)
5. [Data Flow: Failure Path](#data-flow-failure-path)
6. [Event Format & Encoding](#event-format--encoding)
7. [Size Limits](#size-limits)
8. [Writing a Program that Emits Events](#writing-a-program-that-emits-events)
9. [Querying Events](#querying-events)
10. [Decoding Events](#decoding-events)
11. [Key Design Decisions](#key-design-decisions)
12. [Known Limitations](#known-limitations)
13. [File Reference](#file-reference)

---

## Overview

Before this implementation, LEZ programs executed silently — there was no way for a program to signal structured output beyond state changes, and no way for clients to observe what happened inside a transaction beyond its final account state diff.

This implementation adds a first-class event system:

- Programs call `emit_event(discriminant, &payload)` during execution
- Events are buffered in a thread-local, then flushed atomically into `ProgramOutput`
- The sequencer preserves events for both successful and failed transactions
- Clients call `getTransactionReceipt` to retrieve events keyed by transaction hash
- A CLI tool decodes receipt JSON into human-readable output

---

## Architecture

```
┌─────────────────────────────────────────────────────┐
│                    Guest Program                     │
│                  (RISC0 zkVM guest)                  │
│                                                      │
│  emit_event(discriminant, &MyEvent { ... })          │
│       │                                              │
│       ▼                                              │
│  thread-local EVENT_BUFFER (Vec<EventRecord>)        │
│       │                                              │
│  write_nssa_outputs()  ──── drain_events() ──────►  │
│  write_nssa_outputs_on_failure()  ─ FAILURE_SENTINEL │
└─────────────────────┬───────────────────────────────┘
                      │ Risc0 journal commit
                      ▼
┌─────────────────────────────────────────────────────┐
│                  NSSA Host (nssa crate)              │
│                                                      │
│  program.execute() → ProgramOutput { events, ... }  │
│       │                                              │
│  ExitCode::Halted(0)  → extract ProgramOutput       │
│  ExitCode non-zero    → extract (SENTINEL, events)  │
└─────────────────────┬───────────────────────────────┘
                      │
                      ▼
┌─────────────────────────────────────────────────────┐
│              Sequencer Core                          │
│                                                      │
│  Success → IncludedTxStore { events, block_id }      │
│  Failure → RejectedTxStore { events, error }         │
└─────────────────────┬───────────────────────────────┘
                      │
                      ▼
┌─────────────────────────────────────────────────────┐
│           getTransactionReceipt RPC                  │
│                                                      │
│  Returns TxReceipt { status, events, error, block }  │
└─────────────────────────────────────────────────────┘
```

---

## Component Breakdown

### lez-events (Guest SDK)

**File:** `lez-events/src/lib.rs`

This is the crate that guest programs depend on. It provides:

#### `EventRecord`

```rust
pub struct EventRecord {
    pub discriminant: u32,  // caller-defined event type identifier
    pub sequence: u32,      // monotonically increasing emission order
    pub payload: Vec<u8>,   // Borsh-encoded event data
}
```

`discriminant` is a u32 the program assigns to identify the event type (e.g., `1 = InsufficientFunds`, `2 = WithdrawSuccess`). The host does not interpret it — it is purely for the application layer.

`sequence` is assigned atomically from a global counter (`AtomicU32`) so that even across chained calls, events from different programs can be ordered deterministically.

`payload` is the Borsh-serialized bytes of whatever struct the program passes to `emit_event`.

#### `emit_event`

```rust
pub fn emit_event<T: BorshSerialize>(discriminant: u32, event: &T) -> Result<(), EventError>
```

- Borsh-encodes `event` into bytes
- Checks that the payload does not exceed `MAX_EVENT_PAYLOAD_BYTES` (4096 bytes)
- Checks that the running total in the buffer does not exceed `MAX_TOTAL_EVENT_BYTES` (64KB)
- Pushes an `EventRecord` with the next sequence number into the thread-local `EVENT_BUFFER`
- Returns `Err(EventError)` on size violation instead of panicking — this gives programs a clean way to handle oversized events without corrupting the journal

#### Internal buffer

```rust
thread_local! {
    static EVENT_BUFFER: RefCell<Vec<EventRecord>> = RefCell::new(Vec::new());
}
static SEQUENCE: AtomicU32 = AtomicU32::new(0);
```

The buffer lives in thread-local storage, which is correct for the RISC0 zkVM guest (single-threaded). The sequence counter is global (`AtomicU32`) so it persists across multiple `emit_event` calls within one transaction, including across chained program calls.

#### `drain_events`

```rust
pub fn drain_events() -> Vec<EventRecord>
```

Called by `write_nssa_outputs()` on the host side — drains and returns all buffered events, leaving the buffer empty.

---

### ProgramOutput Integration

**File:** `nssa/core/src/program.rs`

`ProgramOutput` is the struct that guest programs commit to the RISC0 journal. The `events` field was added alongside the existing validity window and chained call fields:

```rust
pub struct ProgramOutput {
    pub self_program_id: ProgramId,
    pub caller_program_id: Option<ProgramId>,
    pub instruction_data: InstructionData,
    pub pre_states: Vec<AccountWithMetadata>,
    pub post_states: Vec<AccountPostState>,
    pub chained_calls: Vec<ChainedCall>,
    pub events: Vec<lez_events::EventRecord>,   // ← added by LP-0012
    pub block_validity_window: BlockValidityWindow,
    pub timestamp_validity_window: TimestampValidityWindow,
}
```

Events are added via the builder pattern:

```rust
impl ProgramOutput {
    pub fn with_events(mut self, events: Vec<lez_events::EventRecord>) -> Self {
        self.events = events;
        self
    }
}
```

#### Helper functions

Three helper functions are provided for programs to write their outputs:

**`write_nssa_outputs`** — success path, no chained calls:
```rust
pub fn write_nssa_outputs(
    self_program_id: ProgramId,
    caller_program_id: Option<ProgramId>,
    instruction_data: InstructionData,
    pre_states: Vec<AccountWithMetadata>,
    post_states: Vec<AccountPostState>,
) {
    let events = lez_events::drain_events();  // flush buffer
    ProgramOutput::new(self_program_id, caller_program_id, instruction_data, pre_states, post_states)
        .with_events(events)
        .write();  // env::commit(&self)
}
```

**`write_nssa_outputs_with_chained_call`** — same but includes chained calls.

**`write_nssa_outputs_on_failure`** — failure path:
```rust
pub const FAILURE_SENTINEL: u32 = 0xDEAD_FA11;

pub fn write_nssa_outputs_on_failure() {
    let events = lez_events::drain_events();
    env::commit(&(FAILURE_SENTINEL, events));
}
```

This is the key to failure-path event preservation. Instead of committing a full `ProgramOutput`, the program commits a `(u32, Vec<EventRecord>)` tuple with the sentinel value `0xDEAD_FA11` as the first element. The host can then detect this pattern and distinguish it from a valid `ProgramOutput`.

---

### Host-Side Event Extraction

**File:** `nssa/src/program.rs`

When the RISC0 executor finishes running the guest, the host reads the journal. The exit code determines how to parse it:

- **`ExitCode::Halted(0)`** (success) → deserialize as `ProgramOutput`, read `output.events`
- **non-zero exit code** → attempt to deserialize as `(FAILURE_SENTINEL, Vec<EventRecord>)`, extract events, store in `RejectedTx`

The `NssaError::ProgramExecutionFailed` variant carries the partial output so events are not lost even when execution fails:

```rust
pub enum NssaError {
    // ...
    ProgramExecutionFailed {
        exit_code: u32,
        partial_output: Option<Box<nssa_core::program::ProgramOutput>>,
        message: String,
    },
    // ...
}
```

`program_id` attribution is injected at the sequencer host level (not in the guest) because guests cannot read their own program ID at runtime (tracked in upstream issue #347). The sequencer wraps each `EventRecord` in an `AttributedEventRecord` that adds the `program_id` from the `ChainedCall` context.

---

### Sequencer Storage

**File:** `sequencer/core/src/block_store.rs`

Two in-memory stores are maintained by the sequencer:

#### `RejectedTxStore` — failed transactions

```rust
pub struct RejectedTx {
    pub error: String,
    pub events: Vec<lez_events::EventRecord>,
    pub block_height: u64,
}

pub struct RejectedTxStore {
    inner: HashMap<HashType, RejectedTx>,
}
```

When a transaction fails execution, the sequencer extracts events from the partial journal (if the sentinel pattern was committed) and stores them here keyed by transaction hash. This store is in-memory and cleared on restart — sufficient for testnet usage.

#### `IncludedTxStore` — successful transactions

```rust
pub struct AttributedEventRecord {
    pub program_id: [u32; 8],
    pub event: lez_events::EventRecord,
}

pub struct IncludedTx {
    pub events: Vec<AttributedEventRecord>,
    pub block_id: u64,
}

pub struct IncludedTxStore {
    inner: HashMap<HashType, IncludedTx>,
}
```

When a transaction is successfully included in a block, the sequencer maps events from `ProgramOutput.events` into `AttributedEventRecord` entries (injecting `program_id` from the chain call context), and stores them here.

Both stores are held as fields on `SequencerCore`:

```rust
pub struct SequencerCore<BP: BlockPublisherTrait = ZoneSdkPublisher> {
    // ...
    rejected_tx_store: crate::block_store::RejectedTxStore,
    included_tx_store: crate::block_store::IncludedTxStore,
}
```

Accessors are provided:
```rust
pub const fn rejected_tx_store(&self) -> &RejectedTxStore { &self.rejected_tx_store }
pub const fn included_tx_store(&self) -> &IncludedTxStore { &self.included_tx_store }
```

---

### RPC Interface

**File:** `sequencer/service/rpc/src/lib.rs`

A new `getTransactionReceipt` method is added to the JSON-RPC interface:

```rust
pub enum TxStatus {
    Pending,
    Included,
    Rejected,
    Unknown,
}

pub struct AttributedEvent {
    pub program_id: [u32; 8],
    pub discriminant: u32,
    pub sequence: u32,
    pub payload: Vec<u8>,
}

pub struct TxReceipt {
    pub tx_hash: HashType,
    pub status: TxStatus,
    pub events: Vec<AttributedEvent>,
    pub error: Option<String>,
    pub block_id: Option<u64>,
}
```

**File:** `sequencer/service/src/service.rs`

The RPC implementation looks up the transaction in three places in order:

1. **Block store** (included) → return `TxStatus::Included` with events from `IncludedTxStore`
2. **`RejectedTxStore`** → return `TxStatus::Rejected` with preserved events and error message
3. **Neither** → return `TxStatus::Unknown`

```rust
async fn get_transaction_receipt(&self, tx_hash: HashType) -> Result<TxReceipt, ErrorObjectOwned> {
    let sequencer = self.sequencer.lock().await;

    if sequencer.block_store().get_transaction_by_hash(tx_hash).is_some() {
        // Build receipt from IncludedTxStore
        let (events, block_id) = sequencer.included_tx_store()
            .get(&tx_hash)
            .map_or_else(|| (vec![], None), |inc| { ... });
        return Ok(TxReceipt { status: TxStatus::Included, events, block_id, .. });
    }

    if let Some(rejected) = sequencer.rejected_tx_store().get(&tx_hash) {
        // Build receipt from RejectedTxStore
        return Ok(TxReceipt { status: TxStatus::Rejected, events, error: Some(rejected.error.clone()), .. });
    }

    Ok(TxReceipt { status: TxStatus::Unknown, events: vec![], .. })
}
```

---

### CLI Decoder

**File:** `lez-events-decoder/src/main.rs`

```bash
lez-events-decoder --receipt receipt.json
```

Reads a JSON receipt file (or stdin) and prints a human-readable summary:

```
Transaction: 0xabcd1234...
Status: Included (block 42)

Events (3):
  [0] program_id: [1234, 0, 0, 0, 0, 0, 0, 0]
      discriminant: 2
      sequence: 0
      payload (utf-8): "WithdrawSuccess:100tokens"

  [1] program_id: [1234, 0, 0, 0, 0, 0, 0, 0]
      discriminant: 2
      sequence: 1
      payload (hex): 0x0a000000...
```

The decoder first attempts UTF-8 decoding of the payload; if that fails it falls back to hex. This works because Borsh-encoded strings are valid UTF-8 for simple types, but complex structs will display as hex that developers can decode further.

---

## Data Flow: Success Path

```
1. Guest calls emit_event(2, &WithdrawSuccess { amount: 100 })
      → payload = borsh::to_vec(&event)           [8 bytes]
      → sequence = SEQUENCE.fetch_add(1)           [= 0]
      → push EventRecord { discriminant: 2, sequence: 0, payload } to EVENT_BUFFER

2. Guest calls write_nssa_outputs(self_program_id, caller_program_id, ...)
      → events = drain_events()                   [Vec with 1 record]
      → ProgramOutput::new(...).with_events(events).write()
      → env::commit(&output)                      [entire ProgramOutput to journal]

3. Host: program.execute() returns Ok(ProgramOutput)
      → output.events = [EventRecord { discriminant: 2, sequence: 0, ... }]

4. Sequencer: transaction validated, included in block at height N
      → AttributedEventRecord { program_id: chained_call.program_id, event }
      → IncludedTxStore.insert(tx_hash, IncludedTx { events, block_id: N })

5. Client: getTransactionReceipt(tx_hash)
      → TxReceipt { status: Included, events: [AttributedEvent { ... }], block_id: Some(N) }
```

---

## Data Flow: Failure Path

```
1. Guest: balance check fails
      → emit_event(1, &InsufficientFunds { required: 200, available: 50 })
      → EventRecord pushed to EVENT_BUFFER

2. Guest calls write_nssa_outputs_on_failure()
      → events = drain_events()
      → env::commit(&(0xDEAD_FA11u32, events))   [sentinel + events to journal]

3. Guest calls panic!("insufficient funds")
      → Risc0: SysPanic::bail(), ExitCode non-zero
      → journal IS written (committed before panic)

4. Host: program.execute() returns Err(...)
      → reads journal: first 4 bytes = 0xDEAD_FA11 (sentinel match)
      → deserializes remaining bytes as Vec<EventRecord>
      → NssaError::ProgramExecutionFailed { events: [...], ... }

5. Sequencer: transaction rejected
      → RejectedTxStore.insert(tx_hash, RejectedTx { error, events, block_height })

6. Client: getTransactionReceipt(tx_hash)
      → TxReceipt { status: Rejected, events: [AttributedEvent { ... }], error: Some("...") }
```

> **Dev-mode note:** In `RISC0_DEV_MODE=1`, `panic!()` calls `SysPanic::bail()` which abandons the journal before the host can read it. Failure-path event recovery only works in production ZK mode where the journal is preserved. This is a RISC0 platform constraint, not a design limitation of this implementation.

---

## Event Format & Encoding

All event data is Borsh-encoded. Borsh was chosen because:
- The LEZ team standardized on it (issues #131, #260)
- Deterministic field ordering (required for ZK proof consistency)
- Compact binary representation — no field names, no padding
- Zero-copy deserialization possible for fixed-size types

An `EventRecord` on the wire is:
```
[discriminant: u32 LE] [sequence: u32 LE] [payload_len: u32 LE] [payload: bytes]
```

The `payload` field is itself Borsh-encoded bytes of whatever struct the program passed to `emit_event`. Programs define their own payload schemas — the event system is schema-agnostic at the protocol level.

Example payload for `WithdrawSuccess { amount: 100u64 }`:
```
64 00 00 00 00 00 00 00   (100u64 in little-endian)
```

---

## Size Limits

| Limit | Value | Enforcement |
|-------|-------|-------------|
| Per-event payload | 4096 bytes | `emit_event` returns `Err(EventError::PayloadTooLarge)` |
| Total per transaction | 65536 bytes (64KB) | `emit_event` returns `Err(EventError::TotalBufferTooLarge)` |

Both limits return a `Result` error rather than panicking. This is intentional — panicking inside `emit_event` would corrupt the journal by leaving it in an unreadable state. Programs should handle the error and decide whether to skip the event or call `write_nssa_outputs_on_failure()`.

---

## Writing a Program that Emits Events

```rust
use lez_events::emit_event;
use borsh::{BorshSerialize, BorshDeserialize};

#[derive(BorshSerialize, BorshDeserialize)]
struct WithdrawSuccess {
    amount: u64,
    recipient: [u8; 32],
}

#[derive(BorshSerialize, BorshDeserialize)]
struct InsufficientFunds {
    required: u64,
    available: u64,
}

// Discriminant constants — define these per program
const EVT_INSUFFICIENT_FUNDS: u32 = 1;
const EVT_WITHDRAW_SUCCESS: u32 = 2;

fn execute(input: ProgramInput<Instruction>, instruction_data: InstructionData) {
    let balance = get_balance(&input);

    if balance < instruction.amount {
        // Emit event before failure, then write sentinel and panic
        let _ = emit_event(EVT_INSUFFICIENT_FUNDS, &InsufficientFunds {
            required: instruction.amount,
            available: balance,
        });
        write_nssa_outputs_on_failure();
        panic!("insufficient funds");
    }

    // Success path
    let post_states = apply_withdraw(&input, instruction.amount);
    let _ = emit_event(EVT_WITHDRAW_SUCCESS, &WithdrawSuccess {
        amount: instruction.amount,
        recipient: instruction.recipient,
    });
    write_nssa_outputs(
        input.self_program_id,
        input.caller_program_id,
        instruction_data,
        input.pre_states,
        post_states,
    );
}
```

Full working example: `examples/emit_event_demo/methods/guest/src/bin/withdraw.rs`

---

## Querying Events

```bash
# JSON-RPC call
curl -X POST http://localhost:8080 \
  -H 'Content-Type: application/json' \
  -d '{
    "jsonrpc": "2.0",
    "method": "getTransactionReceipt",
    "params": ["0xabcdef1234..."],
    "id": 1
  }'
```

Response:
```json
{
  "tx_hash": "0xabcdef1234...",
  "status": "Included",
  "block_id": 42,
  "events": [
    {
      "program_id": [1234, 0, 0, 0, 0, 0, 0, 0],
      "discriminant": 2,
      "sequence": 0,
      "payload": "ZAAAAAAAAAA="
    }
  ],
  "error": null
}
```

---

## Decoding Events

```bash
# From file
lez-events-decoder --receipt receipt.json

# From stdin
cat receipt.json | lez-events-decoder
```

The decoder also supports programmatic use via the `TxReceiptJson` type exported from the binary.

---

## Key Design Decisions

| Decision | Rationale |
|----------|-----------|
| Borsh encoding | Team direction (#131, #260); deterministic, compact |
| `discriminant` as u32 | Simple, program-defined; host treats as opaque |
| `sequence` from global atomic | Deterministic ordering across chained calls |
| Thread-local buffer | Correct for RISC0 single-threaded zkVM guest |
| `Result` from `emit_event` | Avoids journal corruption on size overflow |
| `FAILURE_SENTINEL = 0xDEAD_FA11` | Unambiguously distinguishes partial failure journal from valid `ProgramOutput` |
| `program_id` injected by host | Guests cannot read their own ID at runtime (issue #347) |
| In-memory stores | Sufficient for testnet; persistent storage is a natural next step |

---

## Known Limitations

1. **Dev-mode failure path**: `RISC0_DEV_MODE=1` does not preserve the journal after `panic!()`. Failure-path event recovery works only in production ZK mode. This is a RISC0 platform constraint tracked in upstream issue #170.

2. **`RejectedTxStore` is in-memory**: Cleared on sequencer restart. Events from rejected transactions are lost on restart. Persistent storage (RocksDB) is the natural next step.

3. **Success-path event extraction and new `ValidatedStateDiff` API**: Upstream refactored transaction processing to use a `ValidatedStateDiff` type after this implementation was written. Success-path event extraction needs re-wiring to the new API path. The host-side machinery (`IncludedTxStore`, `AttributedEventRecord`) is in place.

4. **Private execution**: Private-execution path does not automatically emit events — requires explicit opt-in. Documented in `docs/event-format.md`.

---

## File Reference

| File | Role |
|------|------|
| `lez-events/src/lib.rs` | Guest SDK: `emit_event`, `EventRecord`, `drain_events`, size limits |
| `nssa/core/src/program.rs` | `ProgramOutput.events`, `write_nssa_outputs*`, `FAILURE_SENTINEL` |
| `nssa/src/error.rs` | `NssaError::ProgramExecutionFailed` with `partial_output` |
| `nssa/src/program.rs` | Host-side journal parsing, exit code handling |
| `nssa/src/public_transaction/transaction.rs` | `validate_and_produce_public_state_diff` with event collection |
| `nssa/src/state.rs` | `transition_from_public_transaction` returning attributed events |
| `sequencer/core/src/block_store.rs` | `RejectedTxStore`, `IncludedTxStore`, `AttributedEventRecord` |
| `sequencer/core/src/lib.rs` | Event wiring in sequencer, store accessors |
| `sequencer/service/rpc/src/lib.rs` | `TxReceipt`, `TxStatus`, `getTransactionReceipt` RPC definition |
| `sequencer/service/src/service.rs` | `getTransactionReceipt` RPC implementation |
| `lez-events-decoder/src/main.rs` | CLI decoder for receipt JSON |
| `examples/emit_event_demo/` | End-to-end example (success + failure paths) |
| `docs/event-format.md` | Schema specification, encoding details, CU cost measurements |
