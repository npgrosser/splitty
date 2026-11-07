//! Bounded renderer/service bridge and the experimental window.
use crate::{
    document::{self, DocumentView},
    terminal::{self, Terminal},
};
use gpui_kit::{prelude::*, *};
use serde_json::{json, Value};
use splitty_client_core::{AnnotationDraft, Document, Event};
use std::{
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    thread,
    time::{Duration, Instant},
};
use tokio::sync::mpsc;

pub enum Command {
    Annotate(AnnotationDraft),
    Resync,
    Reconnect,
}
enum Update {
    // Keep the 64-slot queue compact; the event retains its original delivery leases.
    Event(Box<Event>),
    Document(Document),
    Ready { screen: String, viewer: String },
    Status(String),
    Attached,
    Reset(u64),
}

pub struct Worker {
    stop: Arc<AtomicBool>,
    thread: Option<thread::JoinHandle<anyhow::Result<()>>>,
}
impl Worker {
    fn start(
        commands: mpsc::Receiver<Command>,
        terminal: mpsc::Receiver<terminal::Command>,
        updates: mpsc::Sender<Update>,
        smoke: bool,
    ) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let stopping = stop.clone();
        let thread = thread::spawn(move || {
            let result = tokio::runtime::Builder::new_multi_thread()
                .worker_threads(2)
                .enable_all()
                .build();
            match result {
                Ok(runtime) => runtime.block_on(async {
                    let result =
                        run_service(commands, terminal, updates.clone(), stopping, smoke).await;
                    if let Err(error) = &result {
                        let _ = tokio::time::timeout(
                            Duration::from_millis(200),
                            updates.send(Update::Status(format!("Service failed: {error}"))),
                        )
                        .await;
                    }
                    result
                }),
                Err(error) => {
                    let _ = updates.try_send(Update::Status(format!("Runtime failed: {error}")));
                    Err(error.into())
                }
            }
        });
        Self {
            stop,
            thread: Some(thread),
        }
    }
    pub fn shutdown(mut self) -> anyhow::Result<()> {
        self.stop.store(true, Ordering::Release);
        if let Some(thread) = self.thread.take() {
            thread
                .join()
                .map_err(|_| anyhow::anyhow!("GPUI service worker panicked during shutdown"))??;
        }
        Ok(())
    }
}

async fn send_update(
    updates: &mpsc::Sender<Update>,
    update: Update,
    stop: &AtomicBool,
) -> anyhow::Result<()> {
    loop {
        if stop.load(Ordering::Acquire) {
            return Err(
                std::io::Error::new(std::io::ErrorKind::Interrupted, "Probe is closing").into(),
            );
        }
        tokio::select! {
            permit = updates.reserve() => {
                match permit {
                    Ok(permit) => { permit.send(update); return Ok(()); }
                    Err(_) if stop.load(Ordering::Acquire) => return Err(std::io::Error::new(std::io::ErrorKind::Interrupted, "Probe is closing").into()),
                    Err(error) => return Err(error.into()),
                }
            }
            _ = tokio::time::sleep(Duration::from_millis(16)) => {}
        }
    }
}

#[cfg(not(target_os = "windows"))]
async fn run_service(
    mut commands: mpsc::Receiver<Command>,
    mut terminal: mpsc::Receiver<terminal::Command>,
    updates: mpsc::Sender<Update>,
    stop: Arc<AtomicBool>,
    smoke: bool,
) -> anyhow::Result<()> {
    use splitty_client_core::EmbeddedService;
    let service = EmbeddedService::start();
    let result = async {
        let mut client = service.connect().await?;
        let mut ids = None::<splitty_client_core::ScreenIds>;
        let mut viewer = None::<String>;
        let mut session = None::<String>;
        let mut read = None::<splitty_client_core::DocumentRead>;
        let mut document_loaded = false;
        let mut attached = false;
        let mut fixture_sent = false;
        let mut size = (80, 24);
        let mut generation = 0;
        let mut tick = tokio::time::interval(Duration::from_millis(16));
        enum Next {
            Event(std::io::Result<Option<Box<Event>>>),
            Command(Option<Command>),
            Terminal(Option<terminal::Command>),
            Tick,
        }
        loop {
            let next = tokio::select! {
                event = client.next_event() => Next::Event(event.map(|event| event.map(Box::new))),
                command = commands.recv() => Next::Command(command),
                command = terminal.recv() => Next::Terminal(command),
                _ = tick.tick() => Next::Tick,
            };
            if stop.load(Ordering::Acquire) {
                break;
            }
            match next {
                Next::Tick => {}
                Next::Command(None) | Next::Terminal(None) => break,
                Next::Command(Some(Command::Annotate(draft))) => {
                    if let (Some(ids), Some(viewer)) = (&ids, &viewer) {
                        client.put_annotation(&ids.screen_id, viewer, draft)?;
                    }
                }
                Next::Command(Some(Command::Resync)) => {
                    if let Some(ids) = &ids {
                        client.resync(&ids.screen_id)?;
                    }
                }
                Next::Command(Some(Command::Reconnect)) => {
                    generation += 1;
                    send_update(&updates, Update::Reset(generation), &stop).await?;
                    read = None;
                    document_loaded = false;
                    client.finish().await?;
                    client = service.connect().await?;
                    attached = false;
                    if let Some(ids) = &ids {
                        client.resync(&ids.screen_id)?;
                    }
                    if let Some(session) = &session {
                        client.attach(session)?;
                    }
                    send_update(
                        &updates,
                        Update::Status("Reattached to the existing service/session".into()),
                        &stop,
                    )
                    .await?;
                }
                Next::Terminal(Some(terminal::Command::Resize(cols, rows))) => {
                    size = (cols, rows);
                    if attached {
                        if let Some(session) = &session {
                            client.resize(session, cols, rows, true)?;
                        }
                    }
                }
                Next::Terminal(Some(terminal::Command::Input {
                    generation: input_generation,
                    bytes,
                })) => {
                    if input_generation != generation {
                        continue;
                    }
                    if attached {
                        if let Some(session) = &session {
                            client.input_bytes(session, &bytes)?;
                        }
                    } else {
                        send_update(
                            &updates,
                            Update::Status("Input rejected while the session is attaching".into()),
                            &stop,
                        )
                        .await?;
                    }
                }
                Next::Event(event) => {
                    let Some(event) = event? else {
                        anyhow::bail!("Native stream closed");
                    };
                    if let Some(json) = event.json() {
                        match json.get("type").and_then(Value::as_str) {
                            Some("mux.welcome") if ids.is_none() => {
                                ids = Some(client.create_screen("Native UI probe")?);
                            }
                            Some("mux.pane_spawned") => {
                                if ids.as_ref().is_some_and(|ids| {
                                    json["paneId"].as_str() == Some(&ids.pane_id)
                                }) {
                                    if let Some(id) = json["sessionId"].as_str() {
                                        session = Some(id.to_owned());
                                        client.attach(id)?;
                                    }
                                }
                            }
                            Some("attached") => {
                                attached = true;
                                client.activity(true)?;
                                if let Some(session) = &session {
                                    client.resize(session, size.0, size.1, true)?;
                                    if !fixture_sent {
                                        let command = if smoke {
                                            "i=0; while [ \"$i\" -lt 1200 ]; do printf 'native load %04d — scrolling output\\n' \"$i\"; i=$((i+1)); done; printf '\\033[36mNative GPUI terminal\\033[0m\\nCafé 日本語 👩🏽‍💻\\n'; printf 'SPLITTY_GPUI_%s\\n' SMOKE_OK\r"
                                        } else {
                                            "printf '\\033[36mSplitty native terminal probe\\033[0m\\nCafé 日本語 👩🏽‍💻\\n'\r"
                                        };
                                        client.input(session, command)?;
                                        fixture_sent = true;
                                    }
                                }
                                send_update(&updates, Update::Attached, &stop).await?;
                            }
                            Some("mux.screen_snapshot") => {
                                if let Some(ids) = &ids {
                                    if json["screen"]["id"].as_str() == Some(&ids.screen_id) {
                                        if viewer.is_none() {
                                            let id = client.open_markdown(
                                                &ids.screen_id,
                                                &ids.pane_id,
                                                document::FIXTURE,
                                                "Native Markdown probe",
                                            )?;
                                            send_update(
                                                &updates,
                                                Update::Ready {
                                                    screen: ids.screen_id.clone(),
                                                    viewer: id.clone(),
                                                },
                                                &stop,
                                            )
                                            .await?;
                                            viewer = Some(id);
                                        } else if !document_loaded && read.is_none() {
                                            if let Some(viewer) = &viewer {
                                                if json["screen"]["panes"].get(viewer).is_some() {
                                                    read = Some(client.read_document(viewer)?);
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                            Some("mux.nudge") => {
                                if let Some(ids) = &ids {
                                    client.resync(&ids.screen_id)?;
                                }
                            }
                            Some("mux.op_failed") | Some("error") => {
                                send_update(
                                    &updates,
                                    Update::Status(format!("Service rejected operation: {json}")),
                                    &stop,
                                )
                                .await?;
                            }
                            _ => {}
                        }
                    }
                    if let Some(reader) = &mut read {
                        if let Some(document) = reader.accept(&client, &event)? {
                            send_update(&updates, Update::Document(document), &stop).await?;
                            document_loaded = true;
                            read = None;
                        }
                    }
                    // Awaiting the bounded queue retains Event's native delivery and decode charges.
                    if send_update(&updates, Update::Event(event), &stop)
                        .await
                        .is_err()
                    {
                        break;
                    }
                }
            }
        }
        client.finish().await?;
        Ok::<(), anyhow::Error>(())
    }.await;
    let cleanup = service.close().await;
    let result = match result {
        Err(error)
            if stop.load(Ordering::Acquire)
                && error
                    .downcast_ref::<std::io::Error>()
                    .is_some_and(|error| error.kind() == std::io::ErrorKind::Interrupted) =>
        {
            Ok(())
        }
        other => other,
    };
    cleanup?;
    result
}
#[cfg(target_os = "windows")]
async fn run_service(
    _: mpsc::Receiver<Command>,
    _: mpsc::Receiver<terminal::Command>,
    _: mpsc::Sender<Update>,
    _: Arc<AtomicBool>,
    _: bool,
) -> anyhow::Result<()> {
    anyhow::bail!("Windows probe requires an explicit owned WSL/native connection; this experimental launcher has not been wired on Windows")
}

pub struct Probe {
    terminal: Entity<Terminal>,
    document: DocumentView,
    updates: mpsc::Receiver<Update>,
    commands: mpsc::Sender<Command>,
    screen: Option<String>,
    viewer: Option<String>,
    status: String,
    terminal_status: String,
    snapshot: Option<Box<Event>>,
    document_owner: Option<Document>,
    started: Instant,
    smoke: bool,
    smoke_annotation_sent: bool,
    smoke_reconnected: bool,
    attachments: usize,
    allow_close: bool,
    close_pending: bool,
    _poll: Task<()>,
    outcome: Arc<AtomicBool>,
    stopping: Arc<AtomicBool>,
    screenshot: Option<std::path::PathBuf>,
    web: crate::offscreen::OffscreenProbe,
    web_enabled: bool,
    web_image: Option<Arc<RenderImage>>,
    web_sequence: u64,
    web_bounds: Bounds<Pixels>,
    poll_ticks: u64,
    window_notifications: u64,
}
impl Probe {
    pub fn new(
        smoke: bool,
        web_enabled: bool,
        outcome: Arc<AtomicBool>,
        screenshot: Option<std::path::PathBuf>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> (Self, Worker) {
        let (commands, rx_commands) = mpsc::channel(32);
        let (tx_terminal, rx_terminal) = mpsc::channel(64);
        let (tx_updates, updates) = mpsc::channel(64);
        let worker = Worker::start(rx_commands, rx_terminal, tx_updates, smoke);
        let terminal = cx.new(|cx| {
            Terminal::new(
                move |command| {
                    tx_terminal
                        .try_send(command)
                        .map_err(|error| error.to_string())
                },
                crate::terminal::LinkOrigin::Local,
                cx,
            )
        });
        terminal.update(cx, |terminal, cx| terminal.focus(window, cx));
        let document = DocumentView::new(cx);
        let poll = cx.spawn_in(window, async move |this, cx| loop {
            cx.background_executor()
                .timer(Duration::from_millis(16))
                .await;
            if this
                .update_in(cx, |this, window, cx| this.poll(window, cx))
                .is_err()
            {
                break;
            }
        });
        let entity = cx.entity().downgrade();
        window.on_window_should_close(cx, move |window, cx| {
            entity
                .update(cx, |this, cx| {
                    if this.allow_close || this.smoke || this.document.count() == 0 {
                        let _ = this.prepare_exit();
                        cx.quit();
                        return true;
                    }
                    if !this.close_pending {
                        this.close_pending = true;
                        let prompt = window.prompt(
                            PromptLevel::Warning,
                            "End this probe and its annotations?",
                            Some("This isolated service keeps annotations only for this run."),
                            &["Keep open", "End probe"],
                            cx,
                        );
                        cx.spawn(async move |this, cx| {
                            let confirmed = prompt.await.ok() == Some(1);
                            let _ = this.update(cx, |this, cx| {
                                this.close_pending = false;
                                if confirmed {
                                    this.allow_close = true;
                                    let _ = this.prepare_exit();
                                    cx.quit();
                                }
                            });
                        })
                        .detach();
                    }
                    false
                })
                .unwrap_or(true)
        });
        (
            Self {
                terminal,
                document,
                updates,
                commands,
                screen: None,
                viewer: None,
                status: "Starting isolated native service…".into(),
                terminal_status: "Connecting…".into(),
                snapshot: None,
                document_owner: None,
                started: Instant::now(),
                smoke,
                smoke_annotation_sent: false,
                smoke_reconnected: false,
                attachments: 0,
                allow_close: false,
                close_pending: false,
                _poll: poll,
                outcome,
                stopping: worker.stop.clone(),
                screenshot,
                web: crate::offscreen::OffscreenProbe::new(web_enabled),
                web_enabled,
                web_image: None,
                web_sequence: 0,
                web_bounds: Bounds::default(),
                poll_ticks: 0,
                window_notifications: 0,
            },
            worker,
        )
    }
    fn poll(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.poll_ticks += 1;
        let mut dirty = false;
        let mut bytes = 0;
        while bytes < 256 * 1024 {
            let Ok(update) = self.updates.try_recv() else {
                break;
            };
            match update {
                Update::Event(event) => {
                    if let Some(output) = event.output() {
                        bytes += output.bytes.len();
                        self.terminal.update(cx, |terminal, cx| {
                            terminal.feed(output.bytes, output.replay, cx)
                        });
                    }
                    if let Some(json) = event.json() {
                        if json["type"] == "mux.screen_snapshot"
                            && self.screen.as_deref() == json["screen"]["id"].as_str()
                        {
                            if let Some(viewer) = &self.viewer {
                                let annotations = json["screen"]["panes"][viewer]["viewer"]
                                    ["annotations"]
                                    .as_array()
                                    .map(Vec::as_slice)
                                    .unwrap_or(&[]);
                                dirty |= self.document.snapshot(annotations);
                            }
                            self.snapshot = Some(event);
                        }
                    }
                }
                Update::Document(document) => match std::str::from_utf8(document.bytes()) {
                    Ok(source) => {
                        self.document.set_source(source.to_owned(), cx);
                        self.document_owner = Some(document);
                        dirty = true;
                    }
                    Err(error) => {
                        self.status = format!("Document is not UTF-8: {error}");
                        dirty = true;
                    }
                },
                Update::Ready { screen, viewer } => {
                    self.screen = Some(screen);
                    self.viewer = Some(viewer);
                }
                Update::Status(status) => {
                    dirty |= self.status != status;
                    self.status = status;
                }
                Update::Reset(generation) => {
                    dirty |= self.document.snapshot(&[]);
                    self.terminal
                        .update(cx, |terminal, cx| terminal.reset(generation, cx));
                    self.snapshot = None;
                    self.document_owner = None;
                }
                Update::Attached => {
                    self.attachments += 1;
                    self.status =
                        "Native service attached · isolated workspace · terminal owns focus".into();
                    dirty = true;
                    self.terminal.update(cx, |terminal, _| {
                        if terminal.engine_ready() {
                            terminal.status = "Attached".into();
                        }
                        terminal.report_size();
                    });
                }
            }
        }
        dirty |= self.document.refresh_marks(cx);
        let terminal_status = &self.terminal.read(cx).status;
        if self.terminal_status != *terminal_status {
            self.terminal_status.clone_from(terminal_status);
            dirty = true;
        }
        // CEF's message loop and frame handoff must run even when no GPUI paint is pending.
        if let Some(mut frame) = self.web.poll() {
            // RenderImage expects BGRA; CEF adapter exposes RGBA for explicit ownership.
            for pixel in frame.rgba.as_chunks_mut::<4>().0 {
                pixel.swap(0, 2);
            }
            if let Some(buffer) = image::RgbaImage::from_raw(frame.width, frame.height, frame.rgba)
            {
                if let Some(previous) = self.web_image.take() {
                    let _ = window.drop_image(previous);
                }
                self.web_image = Some(Arc::new(RenderImage::new(vec![image::Frame::new(buffer)])));
                self.web_sequence = frame.sequence;
                dirty = true;
            }
        }
        if self.smoke && self.document.loaded() && !self.smoke_annotation_sent {
            if let Ok(draft) = self.document.smoke_annotation() {
                self.send(Command::Annotate(draft), cx);
                self.smoke_annotation_sent = true;
            }
        }
        if self.smoke
            && !self.smoke_reconnected
            && self.document.count() > 0
            && self.terminal.read(cx).saw_smoke
            && self.started.elapsed() >= Duration::from_secs(2)
        {
            self.send(Command::Reconnect, cx);
            self.smoke_reconnected = true;
        }
        if self.smoke {
            let terminal = self.terminal.read(cx);
            let complete = terminal.saw_smoke
                && self.document.loaded()
                && self.document_owner.is_some()
                && self.attachments >= 2
                && self.document.count() > 0
                && terminal.painted_current()
                && self.document.painted_current()
                && (!self.web_enabled || (self.web_sequence > 1 && self.web.script_ticks() >= 2));
            if (complete && self.started.elapsed() >= Duration::from_secs(4))
                || self.started.elapsed() > Duration::from_secs(25)
            {
                let capture_error = self
                    .screenshot
                    .as_ref()
                    .and_then(|path| capture(window, path).err())
                    .map(|error| error.to_string());
                let engine_close_error = self.prepare_exit();
                let complete = complete && capture_error.is_none() && engine_close_error.is_none();
                self.outcome.store(complete, Ordering::Release);
                eprintln!(
                    "{}",
                    json!({"engineCloseError":engine_close_error,"attachments":self.attachments,"reconnected":self.smoke_reconnected,"screenshot":self.screenshot,"captureError":capture_error,"offscreenFrame":self.web_sequence,"scriptTicks":self.web.script_ticks(),"terminalPaintCurrent":terminal.painted_current(),"documentPaintCurrent":self.document.painted_current(),"offscreenStatus":self.web.status(),"probe":"splitty-gpui", "passed":complete,"terminalMarker":terminal.saw_smoke,"terminalBytes":terminal.bytes,"paintedFrames":terminal.frames,"documentLoaded":self.document.loaded(),"annotations":self.document.count(),"pollTicks":self.poll_ticks,"windowNotifications":self.window_notifications,"elapsedMs":self.started.elapsed().as_millis(),"status":self.status,"manualImeClipboard": "unverified"})
                );
                cx.quit();
            }
        }
        if dirty {
            self.notify(cx);
        }
    }
    fn prepare_exit(&mut self) -> Option<String> {
        self.stopping.store(true, Ordering::Release);
        self.updates.close();
        while self.updates.try_recv().is_ok() {}
        self.snapshot = None;
        self.document_owner = None;
        let error = self.web.close().err();
        if let Some(error) = &error {
            eprintln!("Offscreen cleanup failed: {error}");
            self.outcome.store(false, Ordering::Release);
        }
        error
    }
    fn notify(&mut self, cx: &mut Context<Self>) {
        self.window_notifications += 1;
        cx.notify();
    }
    fn send(&mut self, command: Command, cx: &mut Context<Self>) {
        if let Err(error) = self.commands.try_send(command) {
            self.status = format!("Action not sent: {error}");
            self.notify(cx);
        }
    }
    fn annotate(&mut self, cx: &mut Context<Self>) {
        match self.document.selection(cx) {
            Ok(draft) => self.send(Command::Annotate(draft), cx),
            Err(error) => {
                self.status = error;
                self.notify(cx);
            }
        }
    }
}
impl Render for Probe {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let entity = cx.entity();
        let web_panel = div()
            .id("offscreen")
            .relative()
            .h(px(180.))
            .flex_none()
            .overflow_hidden()
            .bg(rgb(0x203047))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, event: &MouseDownEvent, _, _| {
                    let position = event.position - this.web_bounds.origin;
                    this.web
                        .click(f32::from(position.x) as i32, f32::from(position.y) as i32);
                }),
            )
            .when_some(self.web_image.clone(), |view, image| {
                view.child(
                    img(image)
                        .w(px(crate::offscreen::WIDTH as f32))
                        .h(px(crate::offscreen::HEIGHT as f32)),
                )
            })
            .child(
                canvas(
                    move |bounds, _, cx| entity.update(cx, |this, _| this.web_bounds = bounds),
                    |_, (), _, _| {},
                )
                .absolute()
                .top_0()
                .left_0()
                .size_full(),
            )
            .child(
                div()
                    .absolute()
                    .top_2()
                    .left_2()
                    .p_2()
                    .bg(rgba(0x10151cdd))
                    .text_color(rgb(0xffffff))
                    .child(format!("Native overlay · {}", self.web.status()))
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation()),
            );
        div().size_full().flex().flex_col().bg(rgb(0x10151c)).text_color(rgb(0xd9e2ef)).text_size(px(14.))
            .child(div().p_3().flex().gap_4().child("SPLITTY · native UI phase 0")
                .child(div().id("annotate").cursor_pointer().px_2().bg(rgb(0x2c4358)).child("Annotate selection").on_click(cx.listener(|this, _, _, cx| this.annotate(cx))))
                .child(div().id("resync").cursor_pointer().px_2().child("Resync").on_click(cx.listener(|this, _, _, cx| this.send(Command::Resync, cx))))
                .child(div().id("reconnect").cursor_pointer().px_2().child("Reattach").on_click(cx.listener(|this, _, _, cx| this.send(Command::Reconnect, cx)))))
            .child(div().flex().flex_1().min_h_0().gap_2().px_2()
                .child(div().w(relative(0.55)).h_full().overflow_hidden().child(self.terminal.clone()))
                .child(div().flex_1().h_full().min_w_0().p_3().bg(rgb(0x1b2430)).overflow_hidden().child(self.document.render())))
            .child(web_panel)
            .child(div().p_2().text_size(px(12.)).child(format!("{} · {} annotations · {} · {}", self.status, self.document.count(), self.document.status, self.terminal.read(cx).status)))
            .child(div().px_2().pb_2().text_size(px(11.)).child("Experimental · isolated service · Ctrl+Shift+C/V copy/paste · annotations end with this run"))
    }
}

fn capture(window: &Window, path: &std::path::Path) -> anyhow::Result<()> {
    #[cfg(feature = "capture")]
    {
        let image = window.render_to_image().map_err(|error| {
            #[cfg(target_os = "linux")]
            { anyhow::anyhow!("GPUI snapshot 0.3.11 does not implement native-window capture on Linux; use external compositor/window capture. Requested screenshot was not produced: {error}") }
            #[cfg(not(target_os = "linux"))]
            { anyhow::anyhow!("Native-window capture failed; requested screenshot was not produced: {error}") }
        })?;
        image.save(path)?;
        Ok(())
    }
    #[cfg(not(feature = "capture"))]
    {
        let _ = (window, path);
        anyhow::bail!("Screenshot capture is unavailable; build with --features capture")
    }
}

#[cfg(test)]
mod tests {
    use super::{send_update, Update};
    use std::sync::atomic::{AtomicBool, Ordering};
    #[tokio::test]
    async fn shutdown_interrupts_a_full_ui_queue_without_consuming_another_slot() {
        let (tx, mut rx) = tokio::sync::mpsc::channel(1);
        tx.send(Update::Status("held".into()))
            .await
            .expect("fill queue");
        let stop = AtomicBool::new(false);
        let pending = send_update(&tx, Update::Status("waiting".into()), &stop);
        let cancel = async {
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            stop.store(true, Ordering::Release);
        };
        let (result, ()) = tokio::join!(pending, cancel);
        assert!(result.is_err());
        assert!(rx.try_recv().is_ok());
        assert!(rx.try_recv().is_err());
    }
}
