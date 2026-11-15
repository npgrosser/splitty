use super::*;
use splitty_core::ids::{PaneId, ScreenId};
use splitty_core::mux::Op;

async fn next_mux(socket: &mut Socket) -> serde_json::Value {
    tokio::time::timeout(WAIT, async {
        loop {
            let event = json_of(next(socket).await);
            if event["type"]
                .as_str()
                .is_some_and(|kind| kind.starts_with("mux."))
            {
                return event;
            }
        }
    })
    .await
    .expect("mux event in time")
}

async fn snapshot(socket: &mut Socket) -> serde_json::Value {
    socket
        .send(Message::Text(
            json!({"type":"mux.resync","screenId":"shared"})
                .to_string()
                .into(),
        ))
        .await
        .expect("resync");
    let event = next_mux(socket).await;
    assert_eq!(event["type"], "mux.screen_snapshot");
    event["screen"].clone()
}

#[tokio::test]
async fn new_tabs_sync_as_tabs_between_clients_and_after_reconnect() {
    if !splitty_core::pty::testing::pty_available() {
        return;
    }
    let dir = tempfile::tempdir().expect("fixture");
    let host = ScriptedHost::new(dir.path());
    let served = Served::start_with(hub(&host, dir.path()), |addr| {
        let state = state(addr, dir.path());
        *state.shell_profiles.write().expect("profiles") =
            splitty_core::shell_profiles::state_from_settings(&Default::default());
        state
            .mux_store
            .dispatch(Op::CreateScreen {
                name: None,
                screen_id: Some(ScreenId::from("shared")),
                new_pane_id: Some(PaneId::from("anchor")),
                shell_profile_id: None,
            })
            .expect("initial screen");
        state
    })
    .await;
    let mut first = served.open_local().await;
    let mut second = served.open_local().await;
    assert_eq!(next_mux(&mut first).await["type"], "mux.welcome");
    assert_eq!(next_mux(&mut second).await["type"], "mux.welcome");

    for (index, op) in [
        json!({"type":"new_tab","paneId":"anchor","newPaneId":"tab-a"}),
        json!({"type":"new_tab","paneId":"anchor","newPaneId":"tab-b"}),
        json!({"type":"split_pane","paneId":"tab-a","newPaneId":"split","axis":"vertical"}),
        json!({"type":"new_tab","paneId":"split","newPaneId":"split-tab"}),
    ]
    .into_iter()
    .enumerate()
    {
        let (origin, peer) = if index % 2 == 0 {
            (&mut first, &mut second)
        } else {
            (&mut second, &mut first)
        };
        let mut op = op;
        op["cwd"] = json!(dir.path());
        origin
            .send(Message::Text(
                json!({"type":"mux.op","screenId":"shared","op":op})
                    .to_string()
                    .into(),
            ))
            .await
            .expect("layout op");
        let spawned = next_mux(origin).await;
        assert_eq!(spawned["type"], "mux.pane_spawned", "{spawned}");
        assert_eq!(spawned["paneId"], op["newPaneId"]);
        let peer_spawned = next_mux(peer).await;
        assert_eq!(peer_spawned["type"], "mux.pane_spawned");
        assert_eq!(peer_spawned["sessionId"], spawned["sessionId"]);
        assert_eq!(next_mux(peer).await["type"], "mux.nudge");
        let peer_state = snapshot(peer).await;
        // The origin receives no nudge; both resyncs must describe the same layout.
        assert_eq!(snapshot(origin).await["root"], peer_state["root"]);
        if index < 2 {
            assert_eq!(peer_state["root"]["type"], "slot");
            let tabs = if index == 0 {
                json!(["anchor", "tab-a"])
            } else {
                json!(["anchor", "tab-a", "tab-b"])
            };
            assert_eq!(peer_state["root"]["paneIds"], tabs);
        } else {
            let root = &peer_state["root"];
            assert_eq!(root["type"], "split");
            assert_eq!(root["axis"], "vertical");
            assert_eq!(root["children"].as_array().expect("children").len(), 2);
            assert_eq!(root["sizes"], json!([0.5, 0.5]));
            assert_eq!(
                root["children"][0]["paneIds"],
                json!(["anchor", "tab-a", "tab-b"])
            );
            let tabs = if index == 2 {
                json!(["split"])
            } else {
                json!(["split", "split-tab"])
            };
            assert_eq!(root["children"][1]["paneIds"], tabs);
        }
    }
    let before = snapshot(&mut first).await;
    second.close(None).await.expect("disconnect");
    let mut second = served.open_local().await;
    assert_eq!(next_mux(&mut second).await["type"], "mux.welcome");
    assert_eq!(snapshot(&mut second).await["root"], before["root"]);

    first
        .send(Message::Text(
            json!({"type":"mux.op","screenId":"shared","op":{"type":"close_screen"}})
                .to_string()
                .into(),
        ))
        .await
        .expect("cleanup sessions");
    assert_eq!(next_mux(&mut second).await["type"], "mux.nudge");
    assert!(served
        .state
        .mux_store
        .get_screen(&ScreenId::from("shared"))
        .is_none());
}
