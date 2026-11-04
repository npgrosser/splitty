//! Allocation-free admission estimate for serde_json's protocol objects.
//!
//! Count syntax outside strings, not wire length times a fixed multiplier:
//! compact arrays can amplify dramatically while Base64 strings do not.
use std::io;

pub fn allocation_bound(bytes: &[u8]) -> io::Result<usize> {
    estimate(bytes, false)
}

pub fn typed_allocation_bound(bytes: &[u8]) -> io::Result<usize> {
    estimate(bytes, true)
}

fn estimate(bytes: &[u8], typed: bool) -> io::Result<usize> {
    fn add(total: &mut usize, amount: usize) -> io::Result<()> {
        *total = total.checked_add(amount).ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidData, "JSON allocation size overflow")
        })?;
        Ok(())
    }
    // Root value, first array allocation and fixed decoder/error bookkeeping.
    let mut total = if typed { 4096usize } else { 1024usize };
    let mut escaped_scratch = 0usize;
    let mut index = 0;
    while let Some(&byte) = bytes.get(index) {
        match byte {
            b'"' => {
                index += 1;
                let start = index;
                let mut escaped = false;
                while let Some(&byte) = bytes.get(index) {
                    if byte == b'"' {
                        break;
                    }
                    if byte == b'\\' {
                        escaped = true;
                        index += 1;
                        if index == bytes.len() {
                            break;
                        }
                    }
                    index += 1;
                }
                let raw = index - start;
                // UTF-8 and JSON escape decoding cannot lengthen the source.
                // Include keys, ignored fields, repeated keys and incomplete
                // strings: the decoder may allocate before rejecting them.
                add(&mut total, raw)?;
                if typed {
                    // A typed serde error may quote a whole invalid string.
                    // Debug escaping (including six-byte DEL escapes), error
                    // boxing and geometric formatting growth need a
                    // separate allowance even when the JSON had no escapes.
                    for _ in 0..24 {
                        add(&mut total, raw)?;
                    }
                }
                if escaped {
                    escaped_scratch = escaped_scratch.max(raw);
                }
            }
            // BTreeMap root/split nodes and array minimum capacity. Charging
            // each property/separator also covers sparse nodes and geometric
            // Vec growth, including old + new allocations during a resize.
            b'{' => add(&mut total, 1024)?,
            b'[' => add(&mut total, 128)?,
            b':' | b',' => add(&mut total, 256)?,
            _ => {}
        }
        index += 1;
    }
    if escaped_scratch != 0 {
        // serde_json reuses one scratch Vec across strings. A growth may hold
        // both its previous and replacement allocation, in addition to the
        // decoded owned String already counted above.
        for _ in 0..4 {
            add(&mut total, escaped_scratch.max(8))?;
        }
    }
    Ok(total)
}

/// Controls have only objects and scalars. Reject duplicate keys before a
/// DOM could overwrite the earlier value and change the typed wire contract.
pub(super) struct ControlValue(pub serde_json::Value);
impl<'de> serde::Deserialize<'de> for ControlValue {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct Visitor;
        impl<'de> serde::de::Visitor<'de> for Visitor {
            type Value = ControlValue;
            fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str("a native control object or scalar")
            }
            fn visit_map<M: serde::de::MapAccess<'de>>(
                self,
                mut map: M,
            ) -> Result<Self::Value, M::Error> {
                let mut fields = serde_json::Map::new();
                while let Some(key) = map.next_key::<String>()? {
                    if fields.contains_key(&key) {
                        return Err(serde::de::Error::custom("duplicate control field"));
                    }
                    let value = map.next_value::<ControlValue>()?;
                    fields.insert(key, value.0);
                }
                Ok(ControlValue(serde_json::Value::Object(fields)))
            }
            fn visit_str<E: serde::de::Error>(self, value: &str) -> Result<Self::Value, E> {
                Ok(ControlValue(serde_json::Value::String(value.to_owned())))
            }
            fn visit_string<E: serde::de::Error>(self, value: String) -> Result<Self::Value, E> {
                Ok(ControlValue(serde_json::Value::String(value)))
            }
            fn visit_bool<E: serde::de::Error>(self, value: bool) -> Result<Self::Value, E> {
                Ok(ControlValue(serde_json::Value::Bool(value)))
            }
            fn visit_u64<E: serde::de::Error>(self, value: u64) -> Result<Self::Value, E> {
                Ok(ControlValue(serde_json::Value::Number(value.into())))
            }
            fn visit_i64<E: serde::de::Error>(self, value: i64) -> Result<Self::Value, E> {
                Ok(ControlValue(serde_json::Value::Number(value.into())))
            }
            fn visit_unit<E: serde::de::Error>(self) -> Result<Self::Value, E> {
                Ok(ControlValue(serde_json::Value::Null))
            }
            fn visit_f64<E: serde::de::Error>(self, _: f64) -> Result<Self::Value, E> {
                Err(E::custom("fractional control field"))
            }
            fn visit_seq<A: serde::de::SeqAccess<'de>>(
                self,
                _: A,
            ) -> Result<Self::Value, A::Error> {
                Err(serde::de::Error::custom("control arrays are unavailable"))
            }
        }
        deserializer.deserialize_any(Visitor)
    }
}

// Validate every field before serde's typed conversion, so an invalid scalar,
// enum tag or unknown key never reaches serde's input-sized error formatter.
// These schemas are deliberately limited to the sealed control wire types.
pub(super) fn valid_control(kind: crate::wire::Kind, value: &serde_json::Value) -> bool {
    use crate::wire::Kind;
    use serde_json::Value;
    type Field = (&'static str, fn(&Value) -> bool);
    fn object(value: &Value, fields: &[Field], optional: &[Field]) -> bool {
        let Some(map) = value.as_object() else {
            return false;
        };
        fields
            .iter()
            .all(|(key, check)| map.get(*key).is_some_and(check))
            && optional
                .iter()
                .all(|(key, check)| map.get(*key).is_none_or(check))
            && map.keys().all(|key| {
                fields
                    .iter()
                    .chain(optional)
                    .any(|(allowed, _)| key == allowed)
            })
    }
    fn u32_value(value: &Value) -> bool {
        value.as_u64().is_some_and(|n| u32::try_from(n).is_ok())
    }
    fn u16_value(value: &Value) -> bool {
        value.as_u64().is_some_and(|n| u16::try_from(n).is_ok())
    }
    fn open_host(value: &Value) -> bool {
        value.is_null()
            || object(
                value,
                &[
                    ("ownerInstance", Value::is_string),
                    ("target", Value::is_string),
                    ("update", Value::is_boolean),
                ],
                &[],
            )
    }
    fn purpose(value: &Value) -> bool {
        match value.get("kind").and_then(Value::as_str) {
            Some("direct" | "owner") => object(value, &[("kind", Value::is_string)], &[]),
            Some("host") => object(
                value,
                &[("kind", Value::is_string), ("target", Value::is_string)],
                &[],
            ),
            _ => false,
        }
    }
    fn limits(value: &Value) -> bool {
        object(
            value,
            &[
                ("frameBytes", u32_value),
                ("apiBytes", u32_value),
                ("pendingRequests", u16_value),
                ("connectionBytes", u32_value),
            ],
            &[],
        )
    }
    fn close_code(value: &Value) -> bool {
        matches!(
            value.as_str(),
            Some(
                "incompatible"
                    | "busy"
                    | "timeout"
                    | "protocol"
                    | "backendLost"
                    | "closed"
                    | "internal"
            )
        )
    }
    match kind {
        Kind::Hello | Kind::Accept => object(
            value,
            &[("transportVersion", u16_value)],
            &[("openHost", open_host)],
        ),
        Kind::Ready => object(
            value,
            &[
                ("generation", u32_value),
                ("instance", Value::is_string),
                ("muxProtocol", u32_value),
                ("purpose", purpose),
                ("limits", limits),
            ],
            &[],
        ),
        Kind::ReadyAccept => object(
            value,
            &[("generation", u32_value), ("instance", Value::is_string)],
            &[],
        ),
        Kind::Cancel => object(value, &[("generation", u32_value), ("id", u32_value)], &[]),
        Kind::Ping | Kind::Pong => object(value, &[("nonce", u32_value)], &[]),
        Kind::Credit => object(
            value,
            &[("generation", u32_value), ("receipt", Value::is_string)],
            &[],
        ),
        Kind::Progress => object(
            value,
            &[("phase", Value::is_string), ("message", Value::is_string)],
            &[],
        ),
        Kind::Closed => object(
            value,
            &[
                ("code", close_code),
                ("message", Value::is_string),
                ("retryable", Value::is_boolean),
            ],
            &[],
        ),
        _ => false,
    }
}

pub fn control_allocation_bound(bytes: &[u8]) -> io::Result<usize> {
    // Cover the DOM and the internal enum conversion buffers simultaneously.
    // Unlike generic typed parsing, prevalidated fields cannot create large
    // serde diagnostics. The fixed allowance covers conversion bookkeeping.
    allocation_bound(bytes)?
        .checked_mul(2)
        .and_then(|n| n.checked_add(4096))
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                "control allocation size overflow",
            )
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        budget::{ConnectionBudget, ProcessBudget},
        codec, wire,
    };

    #[test]
    fn compact_array_is_denied_before_dom_allocation() {
        let mut bytes = br#"{"type":"resize","padding":["#.to_vec();
        for _ in 0..1_048_576 {
            bytes.extend_from_slice(b"0,");
        }
        bytes.extend_from_slice(b"0]}");
        let budget = ConnectionBudget::new(
            ProcessBudget::new(wire::PROCESS_BYTES, 0),
            wire::CONNECTION_BYTES,
            0,
        );
        let wire = budget.reserve(bytes.len(), false).unwrap();
        assert_eq!(
            codec::reserve_json(&bytes, &budget, false)
                .unwrap_err()
                .kind(),
            io::ErrorKind::OutOfMemory
        );
        assert_eq!(
            budget.used(),
            bytes.len(),
            "failed admission did not allocate or retain a DOM lease"
        );
        drop(wire);
        assert_eq!(budget.used(), 0);
    }

    #[test]
    fn large_unescaped_payload_does_not_pay_for_array_amplification() {
        let payload = 10 * 1024 * 1024usize;
        let mut bytes = br#"{"type":"save_image","data":""#.to_vec();
        bytes.resize(bytes.len() + payload.div_ceil(3) * 4, b'a');
        bytes.extend_from_slice(b"\"}");
        let budget = ConnectionBudget::default();
        let wire = budget.reserve(bytes.len(), false).unwrap();
        let decoded = codec::reserve_json(&bytes, &budget, false).unwrap();
        assert!(budget.used() < wire::CONNECTION_BYTES);
        drop((wire, decoded));
        assert_eq!(budget.used(), 0);
    }

    #[test]
    fn strings_escape_parity_and_malformed_suffixes_are_counted() {
        let plain = allocation_bound(br#""{[:,]}""#).unwrap();
        assert_eq!(plain, 1024 + 6);
        let escaped = allocation_bound(br#""a\"b\\c""#).unwrap();
        assert!(escaped >= 1024 + 7 + 32);
        assert!(allocation_bound(br#""abc\\",0,0"#).unwrap() > escaped);
        let malformed = allocation_bound(br#""abc\"unterminated"#).unwrap();
        assert!(malformed >= 1024 + 4 * 8);
    }
}
