//! Measure actual parser allocation, rather than asserting only our estimate.
//! A thread-local meter isolates each synchronous parse from the test runner.
use serde::Deserialize;
use serde_json::Value;
use splitty_transport::codec::{json_allocation_bound, json_typed_allocation_bound};
use std::{
    alloc::{GlobalAlloc, Layout, System},
    cell::Cell,
};

thread_local! {
    static METER: Cell<Option<(usize, usize)>> = const { Cell::new(None) };
}
struct Meter;
fn allocated(bytes: usize) {
    let _ = METER.try_with(|meter| {
        if let Some((live, peak)) = meter.get() {
            meter.set(Some((live + bytes, peak.max(live + bytes))));
        }
    });
}
fn freed(bytes: usize) {
    let _ = METER.try_with(|meter| {
        if let Some((live, peak)) = meter.get() {
            meter.set(Some((live - bytes, peak)));
        }
    });
}
// SAFETY: delegate all pointer/layout operations to the system allocator.
// The meter uses only fixed-size thread-local cells and never allocates.
unsafe impl GlobalAlloc for Meter {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let pointer = unsafe { System.alloc(layout) };
        if !pointer.is_null() {
            allocated(layout.size());
        }
        pointer
    }
    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        freed(layout.size());
        unsafe {
            System.dealloc(pointer, layout);
        }
    }
    unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        let replacement = unsafe { System.realloc(pointer, layout, size) };
        if !replacement.is_null() {
            // Count both allocations even if the allocator extends in place.
            allocated(size);
            freed(layout.size());
        }
        replacement
    }
}
#[global_allocator]
static ALLOCATOR: Meter = Meter;

fn peak<T>(parse: impl FnOnce() -> T) -> usize {
    METER.with(|meter| meter.set(Some((0, 0))));
    let result = parse();
    let peak = METER.with(|meter| meter.replace(None).unwrap().1);
    drop(result);
    peak
}

#[test]
fn dom_admission_covers_array_growth_sparse_objects_and_escaped_scratch() {
    let array = format!("[{}0]", "0,".repeat(1_048_576));
    let objects = format!("[{}{{}}]", r#"{"a":0},"#.repeat(16_385));
    let properties = (0..4097)
        .map(|i| format!("\"p{i}\":0"))
        .collect::<Vec<_>>()
        .join(",");
    let properties = format!("{{{properties}}}");
    let repeated = format!("{{{}\"a\":0}}", r#""a":{},"#.repeat(8193));
    let escaped = format!("\"{}\\n\"", "a".repeat(1_048_577));
    let alternating = format!(
        "[\"{}\\n\",\"{}\\n\"]",
        "a".repeat(131_073),
        "b".repeat(262_145)
    );
    let malformed = format!("[\"{}\\n\",", "a".repeat(262_145));
    for input in [
        array,
        objects,
        properties,
        repeated,
        escaped,
        alternating,
        malformed,
    ] {
        let measured = peak(|| serde_json::from_str::<Value>(&input));
        let reserved = json_allocation_bound(input.as_bytes()).unwrap();
        assert!(
            measured <= reserved,
            "input={} measured={measured} reserved={reserved}",
            input.len()
        );
    }
}

#[derive(Deserialize)]
struct Scalar {
    cols: u16,
}

#[test]
fn typed_admission_covers_error_formatting_including_unescaped_del() {
    for character in ['a', '\u{7f}', '\u{ad}', '\n', '\\', '"'] {
        let input = serde_json::json!({"cols":character.to_string().repeat(131_073)}).to_string();
        let reserved = json_typed_allocation_bound(input.as_bytes()).unwrap();
        let direct = peak(|| serde_json::from_str::<Scalar>(&input));
        let moved = peak(|| {
            let value: Value = serde_json::from_str(&input).unwrap();
            serde_json::from_value::<Scalar>(value)
        });
        assert!(
            direct <= reserved && moved <= reserved,
            "character={character:?} direct={direct} moved={moved} reserved={reserved}"
        );
    }
    assert_eq!(
        serde_json::from_str::<Scalar>(r#"{"cols":80}"#)
            .unwrap()
            .cols,
        80
    );
}

#[test]
fn control_decoding_bounds_actual_allocations_and_accepts_full_sized_diagnostics() {
    use splitty_transport::{
        budget::ConnectionBudget,
        codec::Frame,
        wire::{self, Kind},
    };
    fn check<T: wire::ControlMessage>(kind: Kind, payload: Vec<u8>, success: bool) {
        assert!(payload.len() <= kind.payload_limit());
        let budget = ConnectionBudget::default();
        let frame = Frame { kind, payload };
        let measured = peak(|| {
            let result = frame.decode_control::<T>(&budget);
            assert_eq!(result.is_ok(), success);
            result
        });
        assert!(
            measured <= budget.control.high_water(),
            "{kind:?}: measured={measured} reserved={}",
            budget.control.high_water()
        );
        assert_eq!(budget.used(), 0);
    }
    for character in ['a', '\\', '\n', '\u{7f}'] {
        for kind in [Kind::Progress, Kind::Closed] {
            let mut value = if kind == Kind::Progress {
                serde_json::json!({"phase":"connecting", "message":""})
            } else {
                serde_json::json!({"code":"backendLost", "message":"", "retryable":true})
            };
            let empty = serde_json::to_vec(&value).unwrap().len();
            let width = serde_json::to_vec(&character.to_string()).unwrap().len() - 2;
            value["message"] = Value::String(
                character
                    .to_string()
                    .repeat((kind.payload_limit() - empty) / width),
            );
            let payload = serde_json::to_vec(&value).unwrap();
            if kind == Kind::Progress {
                check::<wire::Progress>(kind, payload, true);
            } else {
                check::<wire::Closed>(kind, payload, true);
            }
        }
    }
    let ready = serde_json::json!({"generation":1,"instance":"a".repeat(32),"muxProtocol":17,
        "purpose":{"kind":"host","target":"\\".repeat(1750)},"limits":wire::Limits::default()});
    check::<wire::Ready>(Kind::Ready, serde_json::to_vec(&ready).unwrap(), true);
    for payload in [
        br#"{"nonce":1,"nonce":2}"#.to_vec(),
        br#"{"nonce":1,"n\u006fnce":2}"#.to_vec(),
        br#"{"nonce":1,"unknown":2}"#.to_vec(),
        br#"{"nonce":4294967296}"#.to_vec(),
        br#"{"nonce":1.0}"#.to_vec(),
        serde_json::to_vec(&serde_json::json!({"nonce":"\u{7f}".repeat(3000)})).unwrap(),
    ] {
        check::<wire::Ping>(Kind::Ping, payload, false);
    }
    check::<wire::Ready>(Kind::Ready, br#"{"generation":1,"instance":"a","muxProtocol":1,"purpose":{"kind":"host","target":"one","target":"two"},"limits":{"frameBytes":1,"apiBytes":1,"pendingRequests":1,"connectionBytes":1}}"#.to_vec(), false);
}

#[test]
fn outgoing_diagnostics_and_reassigned_ready_cover_actual_allocations() {
    use splitty_transport::{
        budget::ConnectionBudget,
        codec, negotiation,
        wire::{self, ControlMessage},
    };
    for character in ['a', '\\', '\n', '\u{1}', '"', '界'] {
        let diagnostic = serde_json::json!({"type":"remote.error", "message":character.to_string().repeat(512), "hint":"check host key"});
        let budget = ConnectionBudget::default();
        let measured = peak(|| codec::closed_diagnostic(&diagnostic, false, &budget).unwrap());
        assert!(
            measured <= budget.control.high_water(),
            "{character:?}: measured={measured} reserved={}",
            budget.control.high_water()
        );
        assert_eq!(budget.used(), 0);
        let frame = codec::closed_diagnostic(&diagnostic, false, &budget).unwrap();
        assert_eq!(budget.used(), frame.size());
        assert_eq!(frame.payload.capacity(), frame.payload.len());
        let closed: wire::Closed = frame.decode().unwrap();
        assert_eq!(
            serde_json::from_str::<Value>(&closed.message).unwrap(),
            diagnostic
        );
        assert!(!closed.retryable);
        drop(frame);
        assert_eq!(budget.used(), 0);
    }
    let source = wire::Ready {
        generation: 3,
        instance: "a".repeat(32),
        mux_protocol: 17,
        purpose: wire::Purpose::Host {
            target: "old".repeat(1000),
        },
        limits: wire::Limits::default(),
    };
    let budget = ConnectionBudget::default();
    let measured = peak(|| negotiation::host_ready(&source, "new-vm", &budget).unwrap());
    assert!(measured <= budget.control.high_water());
    let copied = negotiation::host_ready(&source, "new-vm", &budget).unwrap();
    assert_eq!(budget.used(), copied.retained_bytes());
    assert_eq!(
        copied.purpose,
        wire::Purpose::Host {
            target: "new-vm".into()
        }
    );
    assert_eq!(copied.instance, source.instance);
    assert_eq!(copied.generation, source.generation);
    drop(copied);
    assert_eq!(budget.used(), 0);
}
