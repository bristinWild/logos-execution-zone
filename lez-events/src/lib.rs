use borsh::{BorshDeserialize, BorshSerialize};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, BorshSerialize, BorshDeserialize, Serialize, Deserialize, PartialEq, Eq)]
pub struct EventRecord {
    pub discriminant: u32,
    pub sequence: u32,
    pub payload: Vec<u8>,
}

impl EventRecord {
    pub fn decode_payload<T: BorshDeserialize>(&self) -> Result<T, std::io::Error> {
        T::try_from_slice(&self.payload)
    }
}

pub fn encode_payload<T: BorshSerialize>(event: &T) -> Vec<u8> {
    borsh::to_vec(event).expect("event serialization should not fail")
}

/// Maximum payload size per event in bytes.
pub const MAX_EVENT_PAYLOAD_BYTES: usize = 4096;

/// Maximum total event buffer size per transaction in bytes.
pub const MAX_TOTAL_EVENT_BYTES: usize = 64 * 1024;

pub mod buffer {
    use super::EventRecord;
    use std::cell::RefCell;
    use std::sync::atomic::{AtomicU32, Ordering};

    static SEQUENCE: AtomicU32 = AtomicU32::new(0);

    thread_local! {
        static EVENT_BUFFER: RefCell<Vec<EventRecord>> = RefCell::new(Vec::new());
    }

    pub fn emit<T: borsh::BorshSerialize>(discriminant: u32, event: &T) {
        let payload = super::encode_payload(event);
        emit_raw(discriminant, payload);
    }

    pub fn emit_raw(discriminant: u32, payload: Vec<u8>) {
        let record = EventRecord {
            discriminant,
            sequence: SEQUENCE.fetch_add(1, Ordering::SeqCst),
            payload,
        };
        EVENT_BUFFER.with(|buf| buf.borrow_mut().push(record));
    }

    pub fn total_payload_bytes() -> usize {
        EVENT_BUFFER.with(|buf| buf.borrow().iter().map(|r| r.payload.len()).sum())
    }

    pub fn drain() -> Vec<EventRecord> {
        EVENT_BUFFER.with(|buf| buf.borrow_mut().drain(..).collect())
    }

    #[cfg(test)]
    pub fn reset() {
        SEQUENCE.store(0, Ordering::SeqCst);
        EVENT_BUFFER.with(|buf| buf.borrow_mut().clear());
    }
}

/// Emit a structured event from a LEZ program.
///
/// # Panics
/// Panics if the encoded payload exceeds MAX_EVENT_PAYLOAD_BYTES (4096 bytes),
/// or if the total accumulated event payload exceeds MAX_TOTAL_EVENT_BYTES (64KB).
/// Error type returned by `emit_event` when size limits are exceeded.
#[derive(Debug, PartialEq, Eq)]
pub enum EventError {
    /// Single event payload exceeds MAX_EVENT_PAYLOAD_BYTES.
    PayloadTooLarge { size: usize, max: usize },
    /// Total accumulated event buffer exceeds MAX_TOTAL_EVENT_BYTES.
    TotalBufferTooLarge { size: usize, max: usize },
}

impl core::fmt::Display for EventError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            EventError::PayloadTooLarge { size, max } =>
                write!(f, "event payload too large: {size} bytes exceeds MAX_EVENT_PAYLOAD_BYTES ({max})"),
            EventError::TotalBufferTooLarge { size, max } =>
                write!(f, "total event buffer too large: {size} bytes exceeds MAX_TOTAL_EVENT_BYTES ({max})"),
        }
    }
}

pub fn emit_event<T: BorshSerialize>(discriminant: u32, event: &T) -> Result<(), EventError> {
    let payload = encode_payload(event);
    if payload.len() > MAX_EVENT_PAYLOAD_BYTES {
        return Err(EventError::PayloadTooLarge {
            size: payload.len(),
            max: MAX_EVENT_PAYLOAD_BYTES,
        });
    }
    let total = buffer::total_payload_bytes() + payload.len();
    if total > MAX_TOTAL_EVENT_BYTES {
        return Err(EventError::TotalBufferTooLarge {
            size: total,
            max: MAX_TOTAL_EVENT_BYTES,
        });
    }
    buffer::emit_raw(discriminant, payload);
    Ok(())
}

pub fn drain_events() -> Vec<EventRecord> {
    buffer::drain()
}

#[cfg(test)]
mod tests {
    use super::*;
    use borsh::{BorshDeserialize, BorshSerialize};

    #[derive(BorshSerialize, BorshDeserialize, Debug, PartialEq)]
    struct TestEvent {
        value: u64,
        label: String,
    }

    #[test]
    fn encode_decode_roundtrip() {
        let event = TestEvent { value: 42, label: "hello".to_string() };
        let payload = encode_payload(&event);
        let record = EventRecord { discriminant: 1, sequence: 0, payload };
        let decoded: TestEvent = record.decode_payload().unwrap();
        assert_eq!(decoded, event);
    }

    #[test]
    fn event_record_borsh_roundtrip() {
        let record = EventRecord { discriminant: 7, sequence: 3, payload: vec![1, 2, 3, 4] };
        let bytes = borsh::to_vec(&record).unwrap();
        let decoded = EventRecord::try_from_slice(&bytes).unwrap();
        assert_eq!(record, decoded);
    }

    #[test]
    fn emit_and_drain() {
        buffer::reset();
        let event = TestEvent { value: 99, label: "test".to_string() };
        emit_event(5, &event).unwrap();
        emit_event(5, &event).unwrap();
        let events = drain_events();
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].sequence, 0);
        assert_eq!(events[1].sequence, 1);
        let decoded: TestEvent = events[0].decode_payload().unwrap();
        assert_eq!(decoded, event);
    }

    #[test]
    fn drain_is_empty_after_drain() {
        buffer::reset();
        emit_event(1, &42u64).unwrap();
        drain_events();
        assert!(drain_events().is_empty());
    }

    #[test]
    fn payload_within_limit_succeeds() {
        buffer::reset();
        let payload = vec![0u8; MAX_EVENT_PAYLOAD_BYTES - 8];
        emit_event(1, &payload).unwrap();
        let events = drain_events();
        assert_eq!(events.len(), 1);
    }

    #[test]
    fn payload_exceeds_limit_returns_error() {
        buffer::reset();
        let payload = vec![0u8; MAX_EVENT_PAYLOAD_BYTES + 100];
        let result = emit_event(1, &payload);
        assert!(matches!(result, Err(EventError::PayloadTooLarge { .. })));
    }

    #[test]
    fn total_payload_bytes_tracked() {
        buffer::reset();
        emit_event(1, &42u64).unwrap();
        emit_event(2, &42u64).unwrap();
        assert!(buffer::total_payload_bytes() > 0);
        drain_events();
        assert_eq!(buffer::total_payload_bytes(), 0);
    }
}
