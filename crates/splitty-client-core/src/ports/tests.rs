use super::*;
#[test]
fn requests_validate_exact_targets_ports_and_labels() {
    for target in ["", "-option", "host;cmd", "host\ncommand"] {
        assert!(MappingKey::new(target, 3000).is_err());
    }
    assert!(MappingKey::new("host", 0).is_err());
    let key = MappingKey::new(" host ", 3000).unwrap();
    assert_eq!(key.target(), "host");
    assert!(ForwardRequest::start(key.clone(), Some(0), false, None).is_err());
    assert!(ForwardRequest::save(key.clone(), None, Some("x".repeat(201))).is_err());
    assert!(ForwardRequest::save(key.clone(), None, Some("a\nb".into())).is_err());
    assert!(ForwardRequest::save(key.clone(), None, None)
        .unwrap()
        .label
        .is_none());
    assert_eq!(
        ForwardRequest::save(key, None, Some(String::new()))
            .unwrap()
            .label
            .as_deref(),
        Some("")
    );
}
#[test]
fn admitted_url_only_keeps_http_parts_after_forwarding() {
    let source = crate::workspace::SourceWorkspace::new(Uuid::new_v4());
    let terminal = TerminalToken::new(source.source_token(), "pane", "session");
    for (raw, port, expected) in [
        (
            "http://localhost/path?q=1#two",
            80,
            "http://localhost:4173/path?q=1#two",
        ),
        (
            "https://user:secret@[::ffff:127.0.0.1]/app?q=1#f",
            443,
            "https://localhost:4173/app?q=1#f",
        ),
        (
            "http://dev.localhost:3000/app",
            3000,
            "http://localhost:4173/app",
        ),
    ] {
        let request = ForwardRequest::open_link(
            MappingKey::new("remote", port).unwrap(),
            terminal.clone(),
            WebUrl::parse(raw).unwrap(),
        )
        .unwrap();
        assert_eq!(rewritten_url(&request, 4173).unwrap().as_str(), expected);
    }
    assert!(ForwardRequest::open_link(
        MappingKey::new("remote", 80).unwrap(),
        terminal,
        WebUrl::parse("https://localhost").unwrap()
    )
    .is_err());
}

#[cfg(unix)]
mod api_cases {
    use super::*;
    use crate::hub::tests::fake_peer::scripted_operations;
    use serde_json::json;
    use std::time::Duration;
    fn owner() -> SourceToken {
        crate::workspace::SourceWorkspace::new(Uuid::new_v4()).source_token()
    }
    fn request() -> ForwardRequest {
        ForwardRequest::open(MappingKey::new("vm1", 3000).unwrap(), None, false, None).unwrap()
    }
    fn consent() -> super::super::runtime::Consent {
        super::super::runtime::Consent {
            allocation: None,
            active: Arc::new(AtomicBool::new(true)),
            expected: std::sync::atomic::AtomicU16::new(0),
        }
    }
    const ACTIVE: &str = r#"{"target":"vm1","remote":3000,"local":4173,"url":"https://malicious.invalid","state":"up"}"#;
    async fn run(
        replies: Vec<(u16, String, bool)>,
        operations: Vec<(&'static str, serde_json::Value)>,
        request: ForwardRequest,
        consent: Option<&super::super::runtime::Consent>,
    ) -> ForwardResult {
        let (mut client, task, path) =
            scripted_operations(replies, None, None, Some(operations)).await;
        let result = tokio::time::timeout(
            Duration::from_secs(5),
            api::mutate(
                &mut client,
                owner(),
                Uuid::new_v4(),
                &request,
                consent,
                &std::sync::atomic::AtomicU64::new(0),
            ),
        )
        .await
        .unwrap();
        let _ = client.finish().await;
        task.await.unwrap();
        std::fs::remove_dir_all(path).unwrap();
        result
    }
    #[tokio::test]
    async fn only_exact_up_ack_and_passive_confirmation_authorize_browser() {
        for (reply, uncertain, confirmation, expected) in [
            (
                ACTIVE,
                false,
                Some(format!("[{ACTIVE}]")),
                ForwardOutcome::Succeeded,
            ),
            (ACTIVE, false, Some("[]".into()), ForwardOutcome::Partial),
            (
                ACTIVE,
                false,
                Some(format!("[{}]", ACTIVE.replace("\"up\"", "\"down\""))),
                ForwardOutcome::Partial,
            ),
            (
                ACTIVE,
                false,
                Some(format!("[{}]", ACTIVE.replace("4173", "4174"))),
                ForwardOutcome::Partial,
            ),
            (
                r#"{"target":"vm2","remote":3000,"local":4173,"state":"up"}"#,
                false,
                None,
                ForwardOutcome::Uncertain,
            ),
            ("{}", false, None, ForwardOutcome::Uncertain),
            (ACTIVE, true, None, ForwardOutcome::Uncertain),
        ] {
            let mut replies = vec![(200, reply.into(), uncertain)];
            let mut operations = vec![(
                "forward.open",
                json!({"body":{"target":"vm1","port":3000,"preserveRestore":true}}),
            )];
            if let Some(confirmation) = confirmation {
                replies.push((200, confirmation, false));
                operations.push(("forward.list", json!({})));
            }
            let consent = consent();
            let result = run(replies, operations, request(), Some(&consent)).await;
            assert_eq!(result.outcome(), expected);
            if expected == ForwardOutcome::Succeeded {
                let open = result.eligible_open().unwrap();
                assert_eq!(open.url().as_str(), "http://localhost:4173/");
                let copy = open.clone();
                assert!(open.consume());
                assert!(!copy.consume());
            } else {
                assert!(result.eligible_open().is_none());
            }
        }
    }
    #[tokio::test]
    async fn lost_ack_never_replays_and_conflicts_remain_explicit() {
        for (status, body, expected) in [
            (0, "", ForwardOutcome::Uncertain),
            (
                409,
                r#"{"code":"local_port_conflict","error":"port occupied"}"#,
                ForwardOutcome::Failed,
            ),
        ] {
            let result = run(
                vec![(status, body.into(), false)],
                vec![(
                    "forward.open",
                    json!({"body":{"target":"vm1","port":3000,"preserveRestore":true}}),
                )],
                request(),
                Some(&consent()),
            )
            .await;
            assert_eq!(result.outcome(), expected);
            assert!(result.eligible_open().is_none());
            if status == 409 {
                assert!(result.reason().unwrap().contains("local_port_conflict"));
            }
        }
    }
    #[tokio::test]
    async fn remember_failure_preserves_live_mapping_and_unsaved_diagnostic() {
        for verify in [format!("[{ACTIVE}]"), "[]".into()] {
            let request = ForwardRequest::open(
                MappingKey::new("vm1", 3000).unwrap(),
                None,
                true,
                Some("App".into()),
            )
            .unwrap();
            let result = run(
                vec![
                    (200, ACTIVE.into(), false),
                    (
                        500,
                        r#"{"error":"storage failed","code":"storage_failure"}"#.into(),
                        false,
                    ),
                    (200, verify.clone(), false),
                ],
                vec![
                    (
                        "forward.open",
                        json!({"body":{"target":"vm1","port":3000,"preserveRestore":true}}),
                    ),
                    (
                        "forwardPreferences.save",
                        json!({"body":{"target":"vm1","port":3000,"local":4173,"label":"App"}}),
                    ),
                    ("forward.list", json!({})),
                ],
                request,
                Some(&consent()),
            )
            .await;
            assert_eq!(result.outcome(), ForwardOutcome::Partial);
            assert!(result.reason().unwrap().contains("Remember failed"));
            assert_eq!(result.eligible_open().is_some(), verify != "[]");
        }
    }
    #[tokio::test]
    async fn saving_restoration_requires_an_exact_acknowledgement() {
        for enabled in [false, true] {
            let result = run(vec![(200, json!({"target":"vm1","remote":3000,"local":4173,"label":"Personal","enabled":enabled}).to_string(), false)],
                vec![("forwardPreferences.save", json!({"body":{"target":"vm1","port":3000,"local":4173,"label":"Personal","enabled":false}}))],
                ForwardRequest::save(MappingKey::new("vm1",3000).unwrap(), Some(4173), Some("Personal".into())).unwrap().with_restore(false), None).await;
            assert_eq!(
                result.outcome(),
                if enabled {
                    ForwardOutcome::Uncertain
                } else {
                    ForwardOutcome::Succeeded
                }
            );
        }
    }
    #[tokio::test]
    async fn consent_revoked_during_start_prevents_verification_and_open() {
        let (release, gate) = tokio::sync::oneshot::channel();
        let (observed, started) = tokio::sync::oneshot::channel();
        let (mut client, task, path) = scripted_operations(
            vec![(200, ACTIVE.into(), false)],
            Some(gate),
            Some(observed),
            Some(vec![(
                "forward.open",
                json!({"body":{"target":"vm1","port":3000,"preserveRestore":true}}),
            )]),
        )
        .await;
        let consent = consent();
        let active = consent.active.clone();
        let request = request();
        let revoke = async {
            started.await.unwrap();
            active.store(false, Ordering::Release);
            release.send(()).unwrap();
        };
        let revision = std::sync::atomic::AtomicU64::new(0);
        let (result, ()) = tokio::join!(
            api::mutate(
                &mut client,
                owner(),
                Uuid::new_v4(),
                &request,
                Some(&consent),
                &revision
            ),
            revoke
        );
        assert_eq!(result.outcome(), ForwardOutcome::Succeeded);
        assert!(result.eligible_open().is_none());
        client.finish().await.unwrap();
        task.await.unwrap();
        std::fs::remove_dir_all(path).unwrap();
    }
    #[tokio::test]
    async fn partial_resource_reads_keep_wire_errors_and_reject_bad_documents() {
        let saved = r#"{"version":1,"forwards":[{"target":"vm1","remote":3000,"local":4173,"label":"App","enabled":true,"lastError":{"code":"local_port_conflict","message":"occupied","hint":"choose a port"}}]}"#;
        let (mut client, task, path) = scripted_operations(
            vec![(500, "{}".into(), false), (200, saved.into(), false)],
            None,
            None,
            Some(vec![
                ("forward.list", json!({})),
                ("forwardPreferences.get", json!({})),
            ]),
        )
        .await;
        assert!(api::read_active(&mut client).await.is_err());
        let saved = api::read_saved(&mut client).await.unwrap();
        assert_eq!(
            saved.forwards[0].last_error.as_ref().unwrap().code,
            "local_port_conflict"
        );
        drop(saved);
        client.finish().await.unwrap();
        task.await.unwrap();
        std::fs::remove_dir_all(path).unwrap();
    }
}

#[test]
fn response_decoding_bounds_malformed_input_and_projects_without_directory_membership() {
    use splitty_transport::{budget::ConnectionBudget, wire};
    let budget = ConnectionBudget::default();
    let decode_active = |body: &str| {
        crate::api_stream::decode_with_budget::<Vec<ActiveForward>>(
            &budget,
            &wire::Reply {
                id: 1,
                status: 200,
                content_type: wire::ContentType::Json,
                body: body.into(),
                uncertain: false,
            },
        )
    };
    for bad in [
        "{",
        r#"[{"target":"host","remote":70000,"local":3000,"state":"up"}]"#,
        r#"[{"target":"host","remote":3000,"local":3000,"state":"unexpected"}]"#,
    ] {
        assert!(decode_active(bad).is_err());
        assert_eq!(budget.used(), 0);
    }
    assert!(decode_active(&"x".repeat(splitty_core::native_api::MAX_API_BODY + 1)).is_err());
    let active=decode_active(r#"[{"target":"forgotten","remote":3000,"local":4173,"state":"up"},{"target":"rebuild","remote":3001,"local":4174,"state":"down"}]"#).unwrap();
    api::validate_active(&active).unwrap();
    let saved=crate::api_stream::decode_with_budget::<Preferences>(&budget,&wire::Reply {id:1,status:200,content_type:wire::ContentType::Json,body:r#"{"version":1,"forwards":[{"target":"forgotten","remote":3000,"local":4173,"label":"App","enabled":true},{"target":"paused","remote":3002,"local":4175,"label":"Paused","enabled":false},{"target":"failed","remote":3003,"local":4176,"label":"Failed","enabled":true,"lastError":{"code":"local_port_conflict","message":"occupied"}}]}"#.into(),uncertain:false}).unwrap();
    let window = WindowWorkspace::new(Uuid::new_v4());
    let mut snapshot = ForwardSnapshot::new(window.lifecycle_token(), true);
    snapshot.active = Some(Arc::new(active));
    snapshot.saved = Some(Arc::new(saved));
    assert_eq!(window.hosts().count(), 0);
    let rows = snapshot.rows(&window);
    assert_eq!(rows.len(), 4);
    for (target, state) in [
        ("forgotten", ForwardState::Up),
        ("rebuild", ForwardState::Rebuilding),
        ("paused", ForwardState::Paused),
        ("failed", ForwardState::Failed),
    ] {
        assert_eq!(
            rows.iter()
                .find(|row| row.key().target() == target)
                .unwrap()
                .state(),
            state
        );
    }
    assert!(rows
        .iter()
        .find(|row| row.key().target() == "forgotten")
        .unwrap()
        .is_saved());
    drop(snapshot);
    assert_eq!(budget.used(), 0);
}
