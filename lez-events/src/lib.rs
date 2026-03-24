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

pub mod buffer {
    use super::EventRecord;
    use std::cell::RefCell;
    use std::sync::atomic::{AtomicU32, Ordering};

    static SEQUENCE: AtomicU32 = AtomicU32::new(0);

    thread_local! {
        static EVENT_BUFFER: RefCell<Vec<EventRecord>> = RefCell::new(Vec::new());
    }

    pub fn emit<T: borsh::BorshSerialize>(discriminant: u32, event: &T) {
        let record = EventRecord {
            discriminant,
            sequence: SEQUENCE.fetch_add(1, Ordering::SeqCst),
            payload: super::encode_payload(event),
        };
        EVENT_BUFFER.with(|buf| buf.borrow_mut().push(record));
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

pub fn emit_event<T: BorshSerialize>(discriminant: u32, event: &T) {
    buffer::emit(discriminant, event);
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
        emit_event(5, &event);
        emit_event(5, &event);
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
        emit_event(1, &42u64);
        drain_events();
        assert!(drain_events().is_empty());
    }
}
