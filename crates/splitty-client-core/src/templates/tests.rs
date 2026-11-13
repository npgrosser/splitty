use super::*;
use serde_json::json;
#[cfg(not(target_os = "windows"))]
use splitty_core::template::TemplateStore as _;
pub(crate) fn template() -> Template {
    serde_json::from_value(json!({"name":"Saved","panes":{"a":{"command":"printf REVIEWED","restore_mode":"replay","dir":"./src"},"b":{"shell_profile":"default"}},"layout":{"slot":"a"},"floating":[{"layout":{"slot":"b"},"bounds":{"x":0.1,"y":0.2,"w":0.6,"h":0.5}}]})).unwrap()
}
pub(crate) fn draft(owner: Uuid) -> Arc<TemplateDraft> {
    let budget = ConnectionBudget::default();
    let template = template();
    let value = budget.charge(65536, false, || template).unwrap();
    Arc::new(TemplateDraft::new(owner, Uuid::new_v4(), value, budget).unwrap())
}
#[test]
fn templates_are_bounded_applicable_owned_and_copies_are_charged() {
    let owner = Uuid::new_v4();
    let draft = draft(owner);
    let before = draft.budget.used();
    let charge = draft.reserve_copy().unwrap();
    assert!(draft.budget.used() > before);
    drop(charge);
    assert_eq!(draft.budget.used(), before);
    assert!(TemplateApiRequest::Save {
        draft: draft.clone(),
        slug: "safe".into(),
        name: "Saved".into(),
        included_commands: vec!["a".into()],
        overwrite_confirmed: false
    }
    .validate(Uuid::new_v4())
    .is_err());
    for name in ["", "../bad", "a/b"] {
        assert!(slug(name).is_err());
    }
    let mut invalid = template();
    invalid.name = "x".repeat(257);
    assert!(validate(&invalid).is_err());
    let mut invalid = template();
    invalid.floating[0].layout = serde_json::from_value(
        json!({"split":"horizontal","children":[{"slot":"b"},{"slot":"b"}]}),
    )
    .unwrap();
    assert!(validate(&invalid).is_err());
    let mut invalid = template();
    invalid.panes.get_mut("a").unwrap().command = "x".repeat(65537);
    assert!(validate(&invalid).is_err());
}
#[cfg(not(target_os = "windows"))]
#[tokio::test]
async fn library_save_load_overwrite_delete_uses_reviewed_content_and_fair_settings() {
    use crate::{
        appearance::{local::tests::fixture, ColorScheme},
        local_service::{LocalServiceClient, LocalServiceUpdate},
    };
    use splitty_server::{native::client::Connector, service::catalog::Role};
    let (runtime, path) = fixture();
    runtime
        .state
        .template_store
        .save("source", &template())
        .unwrap();
    let api = Connector::isolated()
        .embedded(runtime.state.clone(), Role::Main)
        .await
        .unwrap();
    let budget = api.sender().context().budget.clone();
    let owner = Uuid::new_v4();
    let mut client = LocalServiceClient::new(api, owner, ColorScheme::Dark);
    async fn result(client: &mut LocalServiceClient) -> TemplateApiResult {
        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            loop {
                if let LocalServiceUpdate::Template(result) = client.next_update().await {
                    return result;
                }
            }
        })
        .await
        .unwrap()
    }
    let id = Uuid::new_v4();
    client
        .enqueue_template(
            owner,
            id,
            TemplateApiRequest::Load {
                slug: "source".into(),
            },
        )
        .unwrap();
    assert_eq!(
        client
            .enqueue_template(owner, id, TemplateApiRequest::List)
            .unwrap_err()
            .kind(),
        io::ErrorKind::AlreadyExists
    );
    assert!(client
        .enqueue_template(owner, Uuid::new_v4(), TemplateApiRequest::List)
        .is_err());
    let loaded = result(&mut client).await;
    assert_eq!(loaded.request_id, id);
    let TemplateApiOutcome::Draft(draft) = loaded.outcome else {
        panic!("draft")
    };
    // A changed file cannot mutate the reviewed extraction/load.
    let mut changed = template();
    changed.panes.get_mut("a").unwrap().command = "CHANGED".into();
    runtime
        .state
        .template_store
        .save("source", &changed)
        .unwrap();
    let save = TemplateApiRequest::Save {
        draft: draft.clone(),
        slug: "target".into(),
        name: "Review".into(),
        included_commands: vec!["a".into()],
        overwrite_confirmed: false,
    };
    client
        .enqueue_template(owner, Uuid::new_v4(), save.clone())
        .unwrap();
    // A library loop gives dirty settings a turn between library jobs.
    assert!(matches!(
        client.next_update().await,
        LocalServiceUpdate::Appearance(_)
    ));
    assert!(matches!(
        result(&mut client).await.outcome,
        TemplateApiOutcome::Saved { .. }
    ));
    let stored = runtime.state.template_store.get("target").unwrap();
    assert_eq!(stored.panes["a"].command, "printf REVIEWED");
    assert_eq!(stored.name, "Review");
    client
        .enqueue_template(owner, Uuid::new_v4(), save)
        .unwrap();
    assert!(matches!(
        result(&mut client).await.outcome,
        TemplateApiOutcome::OverwriteRequired { .. }
    ));
    assert_eq!(
        runtime.state.template_store.get("target").unwrap().panes["a"].command,
        "printf REVIEWED"
    );
    client
        .enqueue_template(
            owner,
            Uuid::new_v4(),
            TemplateApiRequest::Save {
                draft: draft.clone(),
                slug: "target".into(),
                name: "No commands".into(),
                included_commands: vec![],
                overwrite_confirmed: true,
            },
        )
        .unwrap();
    assert!(matches!(
        result(&mut client).await.outcome,
        TemplateApiOutcome::Saved { .. }
    ));
    let stored = runtime.state.template_store.get("target").unwrap();
    assert!(stored.panes["a"].command.is_empty());
    assert!(stored.panes["a"].restore_mode.is_empty());
    client
        .enqueue_template(owner, Uuid::new_v4(), TemplateApiRequest::List)
        .unwrap();
    let TemplateApiOutcome::Catalog(catalog) = result(&mut client).await.outcome else {
        panic!("catalog")
    };
    assert_eq!(catalog.entries().len(), 2);
    drop(catalog);
    client
        .enqueue_template(
            owner,
            Uuid::new_v4(),
            TemplateApiRequest::Delete {
                slug: "target".into(),
            },
        )
        .unwrap();
    assert!(matches!(
        result(&mut client).await.outcome,
        TemplateApiOutcome::Deleted { .. }
    ));
    assert!(runtime.state.template_store.get("target").is_err());
    // Extraction reads the authoritative Screen and target-side relative root.
    use splitty_core::{
        ids::{PaneId, ScreenId},
        mux::Op,
    };
    let screen = ScreenId::from("extract");
    runtime
        .state
        .mux_store
        .dispatch(Op::CreateScreen {
            name: Some("Extract".into()),
            screen_id: Some(screen.clone()),
            new_pane_id: Some(PaneId::from("extract-pane")),
            shell_profile_id: None,
        })
        .unwrap();
    let state = runtime.state.mux_store.get_screen(&screen).unwrap();
    let session = state.sessions.keys().next().unwrap().clone();
    runtime
        .state
        .mux_store
        .dispatch(Op::UpdateSessionMetadata {
            session_id: session,
            display_title: String::new(),
            cwd: "/project/src".into(),
            busy: false,
            exited: false,
            exit_code: None,
            close: false,
        })
        .unwrap();
    client
        .enqueue_template(
            owner,
            Uuid::new_v4(),
            TemplateApiRequest::Extract {
                screen_id: "extract".into(),
                name: "Extracted".into(),
                root: "/project".into(),
            },
        )
        .unwrap();
    let TemplateApiOutcome::Draft(extracted) = result(&mut client).await.outcome else {
        panic!("extracted draft")
    };
    assert_eq!(extracted.template().panes["extract-pane"].dir, "./src");
    drop(extracted);
    drop(draft);
    client.finish().await.unwrap();
    assert_eq!(budget.used(), 0);
    runtime.close().await.unwrap();
    let _ = std::fs::remove_dir_all(&path);
}
