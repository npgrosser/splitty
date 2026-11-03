//! The same checked-in byte transcripts are decoded by the TypeScript carrier.
use serde::Deserialize;
use splitty_transport::{
    budget::ConnectionBudget,
    codec,
    wire::{self, Kind},
};

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Fixture {
    magic_hex: String,
    frames: Vec<Example>,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Example {
    name: String,
    kind: u8,
    payload_hex: String,
    frame_hex: String,
}

fn unhex(text: &str) -> Vec<u8> {
    text.as_bytes()
        .chunks_exact(2)
        .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap())
        .collect()
}

#[tokio::test]
async fn shared_native_v1_transcripts_roundtrip() {
    let fixture: Fixture =
        serde_json::from_str(include_str!("../../../docs/native-transport-v1.json")).unwrap();
    assert_eq!(unhex(&fixture.magic_hex), wire::MAGIC);
    let budget = ConnectionBudget::default();
    for example in fixture.frames {
        let bytes = unhex(&example.frame_hex);
        let mut input = bytes.as_slice();
        let charged = codec::read(&mut input, &budget).await.unwrap().unwrap();
        let frame = &charged.value;
        assert!(input.is_empty(), "{}: complete frame", example.name);
        assert_eq!(frame.kind as u8, example.kind, "{}", example.name);
        assert_eq!(
            frame.payload,
            unhex(&example.payload_hex),
            "{}",
            example.name
        );
        assert_eq!(frame.encode().unwrap(), bytes, "{}", example.name);
        match frame.kind {
            Kind::Hello | Kind::Accept => assert_eq!(
                frame
                    .decode_control::<wire::Hello>(&budget)
                    .unwrap()
                    .transport_version,
                1
            ),
            Kind::Ready => {
                let ready = frame.decode_control::<wire::Ready>(&budget).unwrap();
                assert!(ready.limits.valid());
                if matches!(ready.purpose, wire::Purpose::Host { .. }) {
                    assert_eq!(
                        ready.mux_protocol, 17,
                        "mux and carrier version are independent"
                    );
                }
            }
            Kind::ReadyAccept => {
                frame.decode_control::<wire::ReadyAccept>(&budget).unwrap();
            }
            Kind::Request => {
                frame.decode::<wire::Request>().unwrap();
            }
            Kind::Reply => {
                frame.decode::<wire::Reply>().unwrap();
            }
            Kind::Cancel => {
                frame.decode_control::<wire::Cancel>(&budget).unwrap();
            }
            Kind::Ping | Kind::Pong => {
                frame.decode_control::<wire::Ping>(&budget).unwrap();
            }
            Kind::Credit => {
                frame.decode_control::<wire::Credit>(&budget).unwrap();
            }
            Kind::Progress => {
                frame.decode_control::<wire::Progress>(&budget).unwrap();
            }
            Kind::Closed => {
                frame.decode_control::<wire::Closed>(&budget).unwrap();
            }
            Kind::Text => {
                frame.decode::<serde_json::Value>().unwrap();
            }
            Kind::Binary => assert!(frame.application().unwrap().1.contains(&255)),
        }
        drop(charged);
        assert_eq!(budget.used(), 0);
    }
}
