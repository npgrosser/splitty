//! Pasted files and offered downloads against the real session protocol. The
//! server stores pastes in the user's cache and the client saves downloads in
//! the user's home, so this runs in a child with a private home.
use super::*;
use crate::workspace::SourceWorkspace;
use uuid::Uuid;

const NAME: &str = "native_workspace::files_tests::pastes_are_stored_on_the_server_and_offers_are_saved_only_after_save";

#[test]
fn pastes_are_stored_on_the_server_and_offers_are_saved_only_after_save() {
    const CHILD: &str = "SPLITTY_FILES_TEST_CHILD";
    if std::env::var_os(CHILD).is_none() {
        let dir = std::env::temp_dir().join(format!("splitty-files-{}", Uuid::new_v4()));
        std::fs::create_dir(&dir).unwrap();
        let result = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", NAME, "--nocapture"])
            .env(CHILD, "1")
            .env("HOME", &dir)
            .env("XDG_CACHE_HOME", dir.join("cache"))
            .env("XDG_CONFIG_HOME", dir.join("config"))
            .env("SPLITTY_CONFIG_DIR", dir.join("config"))
            .output()
            .unwrap();
        std::fs::remove_dir_all(dir).unwrap();
        assert!(
            result.status.success(),
            "child failed: {}\n{}",
            String::from_utf8_lossy(&result.stdout),
            String::from_utf8_lossy(&result.stderr)
        );
        return;
    }
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(real_service());
}

async fn next(updates: &mut mpsc::Receiver<Update>) -> Update {
    tokio::time::timeout(Duration::from_secs(20), updates.recv())
        .await
        .expect("files worker timeout")
        .expect("files worker alive")
}

async fn next_file(updates: &mut mpsc::Receiver<Update>) -> FileUpdate {
    loop {
        if let Update::Files(update) = next(updates).await {
            return update;
        }
    }
}

async fn real_service() {
    if !splitty_core::pty::testing::pty_available() {
        return;
    }
    let home = dirs::home_dir().unwrap();
    let (runtime, _) = crate::appearance::local::tests::fixture();
    let state = runtime.state.clone();
    let service =
        crate::owned_service::OwnedService::from(crate::EmbeddedService::from_runtime(runtime));
    let (commands, receiver) = mpsc::channel(64);
    let (sender, mut updates) = mpsc::channel(64);
    let stop = Arc::new(AtomicBool::new(false));
    let cleanup = Arc::new(AtomicBool::new(false));
    let worker = tokio::spawn(run_service(
        service,
        receiver,
        sender,
        stop.clone(),
        cleanup.clone(),
        ColorScheme::Dark,
    ));
    let mut model: Option<Box<SourceWorkspace>> = None;
    let mut attached = false;
    while model.is_none() || !attached {
        match next(&mut updates).await {
            Update::Workspace(window) => {
                let local = window.local_source();
                if local.snapshot().is_some() && !local.routes().is_empty() {
                    model = Some(Box::new(local.clone()));
                }
            }
            Update::Attached { .. } => attached = true,
            _ => {}
        }
    }
    let model = model.unwrap();
    let scope = model.source_token();
    let session = model.routes()[0].session_id.clone();

    // One image by save_image and one file in two upload chunks, in order.
    let image = [b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR".as_slice(), &[7u8; 4096]].concat();
    let notes: Vec<u8> = (0..1_500_000u32).map(|n| (n % 251) as u8).collect();
    let local_notes = home.join("My Notes.bin");
    std::fs::write(&local_notes, &notes).unwrap();
    let request_id = Uuid::new_v4();
    commands
        .send(Command::new(
            scope,
            CommandKind::StoreFiles {
                request_id,
                items: vec![
                    StoreItem::Image {
                        extension: "png",
                        bytes: image.clone(),
                    },
                    StoreItem::File(local_notes),
                ],
                dropped: false,
                directory: None,
            },
        ))
        .await
        .unwrap();
    let paths = loop {
        match next_file(&mut updates).await {
            FileUpdate::Stored {
                request_id: id,
                paths,
                ..
            } => {
                assert_eq!(id, request_id);
                break paths;
            }
            FileUpdate::StoreProgress { stored, total, .. } => {
                assert!(stored <= total);
                assert_eq!(total, (image.len() + notes.len()) as u64);
            }
            other => panic!("unexpected file update: {other:?}"),
        }
    };
    assert_eq!(paths.len(), 2);
    assert!(paths[0].ends_with(".png") && paths[0].contains("/splitty/paste/paste-"));
    assert!(paths[1].ends_with("/My_Notes.bin"), "{}", paths[1]);
    assert_eq!(std::fs::read(&paths[0]).unwrap(), image);
    assert_eq!(std::fs::read(&paths[1]).unwrap(), notes);

    // A folder is refused before anything is sent.
    let refused = Uuid::new_v4();
    commands
        .send(Command::new(
            scope,
            CommandKind::StoreFiles {
                request_id: refused,
                items: vec![StoreItem::File(home.clone())],
                dropped: false,
                directory: None,
            },
        ))
        .await
        .unwrap();
    assert!(matches!(
        next_file(&mut updates).await,
        FileUpdate::StoreFailed { request_id, reason, .. }
            if request_id == refused && reason.contains("not a file")
    ));

    // A dropped file goes to the directory the user chose, under its name.
    let kept_dir = home.join("kept");
    std::fs::create_dir(&kept_dir).unwrap();
    let small = home.join("small.txt");
    std::fs::write(&small, b"keep me").unwrap();
    let kept = Uuid::new_v4();
    commands
        .send(Command::new(
            scope,
            CommandKind::StoreFiles {
                request_id: kept,
                items: vec![StoreItem::File(small)],
                dropped: true,
                directory: Some(kept_dir.to_string_lossy().into_owned()),
            },
        ))
        .await
        .unwrap();
    let kept_paths = loop {
        match next_file(&mut updates).await {
            FileUpdate::Stored {
                request_id, paths, ..
            } if request_id == kept => break paths,
            FileUpdate::StoreProgress { .. } => {}
            other => panic!("unexpected file update: {other:?}"),
        }
    };
    assert_eq!(kept_paths, [kept_dir.join("small.txt").to_string_lossy()]);
    assert_eq!(std::fs::read(&kept_paths[0]).unwrap(), b"keep me");
    let relative = Uuid::new_v4();
    commands
        .send(Command::new(
            scope,
            CommandKind::StoreFiles {
                request_id: relative,
                items: Vec::new(),
                dropped: true,
                directory: Some("kept".into()),
            },
        ))
        .await
        .unwrap();
    assert!(matches!(
        next_file(&mut updates).await,
        FileUpdate::StoreFailed { request_id, reason, .. }
            if request_id == relative && reason.contains("absolute")
    ));

    // A pane offers a file: nothing is written before Save.
    let offered = home.join("report.bin");
    let report: Vec<u8> = (0..1_200_000u32).map(|n| (n % 241) as u8).collect();
    std::fs::write(&offered, &report).unwrap();
    let offers = state.client_state.file_offers.clone();
    let offer = |path: std::path::PathBuf| {
        let offers = offers.clone();
        let session_id = session.clone();
        tokio::spawn(async move {
            let request = splitty_server::client_session::files::OfferRequest {
                session_id,
                path: path.to_string_lossy().into_owned(),
            };
            let file = splitty_server::client_session::files::open(&path).unwrap();
            offers.offer(&request, file).await
        })
    };
    let declined = offer(offered.clone());
    let FileUpdate::Offered {
        offer_id,
        name,
        size,
        ..
    } = next_file(&mut updates).await
    else {
        panic!("expected an offer");
    };
    assert_eq!((name.as_str(), size), ("report.bin", report.len() as u64));
    commands
        .send(Command::new(
            scope,
            CommandKind::AnswerFileOffer {
                offer_id,
                save: false,
            },
        ))
        .await
        .unwrap();
    assert_eq!(declined.await.unwrap().unwrap_err().1, "declined");

    let saved = offer(offered.clone());
    let FileUpdate::Offered { offer_id, .. } = next_file(&mut updates).await else {
        panic!("expected a second offer");
    };
    commands
        .send(Command::new(
            scope,
            CommandKind::AnswerFileOffer {
                offer_id: offer_id.clone(),
                save: true,
            },
        ))
        .await
        .unwrap();
    let path = loop {
        match next_file(&mut updates).await {
            FileUpdate::Downloaded {
                offer_id: id, path, ..
            } => {
                assert_eq!(id, offer_id);
                break path;
            }
            FileUpdate::DownloadProgress { received, size, .. } => {
                assert!(received <= size);
            }
            other => panic!("unexpected file update: {other:?}"),
        }
    };
    // The offered file itself has that name in the download folder.
    assert_eq!(path, home.join("report (1).bin"));
    assert_eq!(std::fs::read(&path).unwrap(), report);
    saved.await.unwrap().unwrap();
    assert!(
        std::fs::read_dir(&home).unwrap().all(|entry| !entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .ends_with(".splitty-part")),
        "no partial file remains"
    );

    stop.store(true, Ordering::Release);
    tokio::time::timeout(Duration::from_secs(10), worker)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
}
