//! Where the worker's updates go. The application reads one ordered stream;
//! an update for a window other than the first travels in
//! [`Update::ForWindow`]. State every window shows goes to all of them, the
//! answer to a command to the window that sent it, and what belongs to a
//! Screen to the window that holds the Screen. With a single window every
//! update goes out as it is.
use super::*;
use std::collections::VecDeque;
use std::sync::{Mutex, MutexGuard, PoisonError};

/// What an answer carries of the command it answers.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
enum Asked {
    Request(uuid::Uuid),
    Close(u64),
    Offer(String),
    Editor(String),
}

/// The most answers remembered at once; older ones go to the window in use.
const MAX_ASKED: usize = 512;

pub(super) struct Outbox {
    sender: mpsc::Sender<Update>,
    state: Mutex<State>,
}

struct State {
    /// The window whose command is being handled.
    context: Option<WindowId>,
    asked: HashMap<Asked, WindowId>,
    asked_order: VecDeque<Asked>,
    windows: Vec<WindowId>,
    /// The window that was active last.
    intake: WindowId,
}

enum Audience {
    Everyone,
    Window(WindowId),
}

impl State {
    fn remember(&mut self, key: Asked, window: WindowId) {
        if self.asked.insert(key.clone(), window).is_none() {
            self.asked_order.push_back(key);
            while self.asked_order.len() > MAX_ASKED {
                if let Some(old) = self.asked_order.pop_front() {
                    self.asked.remove(&old);
                }
            }
        }
    }
}

impl Outbox {
    pub fn new(sender: mpsc::Sender<Update>) -> Self {
        Self {
            sender,
            state: Mutex::new(State {
                context: None,
                asked: HashMap::new(),
                asked_order: VecDeque::new(),
                windows: vec![WindowId::FIRST],
                intake: WindowId::FIRST,
            }),
        }
    }
    fn state(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }
    /// The windows of the client and the one that was active last.
    pub fn set_windows(&self, windows: Vec<WindowId>, intake: WindowId) {
        let mut state = self.state();
        state.intake = if windows.contains(&intake) {
            intake
        } else {
            windows.first().copied().unwrap_or(WindowId::FIRST)
        };
        state.windows = windows;
    }
    /// Handle what follows as the command of `window`, or of none.
    pub fn enter(&self, window: Option<WindowId>) {
        let mut state = self.state();
        state.context = window.filter(|window| state.windows.contains(window));
    }
    /// Remember which window a command's later answers go to.
    pub fn asked_by(&self, command: &Command) {
        let window = command.window();
        let mut state = self.state();
        for key in asked_of_command(command.kind()) {
            state.remember(key, window);
        }
    }
    fn audience(&self, update: &Update) -> Audience {
        if let Update::WindowClosed { window } | Update::ForWindow { window, .. } = update {
            return Audience::Window(*window);
        }
        if update.for_everyone() {
            return Audience::Everyone;
        }
        let key = asked_of_update(update);
        let mut state = self.state();
        if let Some(window) = key
            .as_ref()
            .and_then(|key| state.asked.get(key).copied())
            .filter(|window| state.windows.contains(window))
        {
            return Audience::Window(window);
        }
        let window = state.context.unwrap_or(state.intake);
        // What a Screen's process started (a file offer, an editor request)
        // keeps going to the window that got it first.
        if let Some(key @ (Asked::Offer(_) | Asked::Editor(_))) = key {
            state.remember(key, window);
        }
        Audience::Window(window)
    }
    /// The update as window `window` reads it from the one stream.
    pub fn address(window: WindowId, update: Update) -> Update {
        if window == WindowId::FIRST || matches!(update, Update::ForWindow { .. }) {
            update
        } else {
            Update::ForWindow {
                window,
                update: Box::new(update),
            }
        }
    }
    /// The copies of `update` to send, each addressed.
    fn addressed(&self, update: Update) -> Vec<Update> {
        match self.audience(&update) {
            Audience::Window(window) => vec![Self::address(window, update)],
            Audience::Everyone => {
                let windows = self.state().windows.clone();
                let mut out = Vec::with_capacity(windows.len());
                for window in windows.iter().skip(1) {
                    if let Some(copy) = update.duplicate() {
                        out.push(Self::address(*window, copy));
                    }
                }
                if let Some(first) = windows.first() {
                    out.insert(0, Self::address(*first, update));
                }
                out
            }
        }
    }
    /// Send without waiting, for a worker that is closing.
    pub fn try_send(&self, update: Update) {
        for update in self.addressed(update) {
            let _ = self.sender.try_send(update);
        }
    }
}

pub(super) async fn publish(
    updates: &Outbox,
    update: Update,
    stop: &AtomicBool,
) -> anyhow::Result<()> {
    for update in updates.addressed(update) {
        send(&updates.sender, update, stop).await?;
    }
    Ok(())
}

/// Send an update to window `window`, whatever it answers.
pub(super) async fn publish_to(
    updates: &Outbox,
    window: WindowId,
    update: Update,
    stop: &AtomicBool,
) -> anyhow::Result<()> {
    send(&updates.sender, Outbox::address(window, update), stop).await
}

/// Send an update that belongs to a Screen to the window that holds it; any
/// other update goes where [`publish`] sends it.
pub(super) async fn publish_routed(
    updates: &Outbox,
    model: &WindowWorkspace,
    update: Update,
    stop: &AtomicBool,
) -> anyhow::Result<()> {
    if !model.single_window() {
        if let Some(window) = screen_of(model, &update).and_then(|key| model.holder(&key)) {
            return publish_to(updates, window, update, stop).await;
        }
    }
    publish(updates, update, stop).await
}

/// Each window's own view of the model, in an order in which a window that
/// lost a Screen reads its model before the window that got the Screen.
pub(super) async fn publish_model(
    updates: &Outbox,
    model: &mut WindowWorkspace,
    stop: &AtomicBool,
) -> anyhow::Result<()> {
    if model.single_window() {
        let window = model.window();
        return publish_to(
            updates,
            window,
            Update::Workspace(Box::new(model.clone())),
            stop,
        )
        .await;
    }
    model.settle();
    let mut windows = model.window_ids();
    let losers = model.take_losers();
    windows.sort_by_key(|window| !losers.contains(window));
    for window in windows {
        if let Some(projection) = model.projection(window) {
            publish_to(
                updates,
                window,
                Update::Workspace(Box::new(projection)),
                stop,
            )
            .await?;
        }
    }
    Ok(())
}

pub(super) async fn send(
    updates: &mpsc::Sender<Update>,
    update: Update,
    stop: &AtomicBool,
) -> anyhow::Result<()> {
    loop {
        if stop.load(Ordering::Acquire) {
            anyhow::bail!("Local workspace is closing");
        }
        tokio::select! {
            permit = updates.reserve() => { permit?.send(update); return Ok(()); }
            _ = tokio::time::sleep(Duration::from_millis(16)) => {}
        }
    }
}

/// The Screen an update belongs to: that of the session or pane it names.
fn screen_of(model: &WindowWorkspace, update: &Update) -> Option<ScreenKey> {
    let of_pane = |source: &SourceToken, pane: &str| {
        let model = model.source(source.source_id())?;
        Some(model.screen_key(model.screen_for(pane)?))
    };
    let of_session = |source: &SourceToken, session: &str| {
        let model = model.source(source.source_id())?;
        Some(model.screen_key(model.screen_for(model.pane_for(session)?)?))
    };
    match update {
        Update::Attached { source, session }
        | Update::PtySize {
            source, session, ..
        } => of_session(source, session),
        Update::Viewer(update) => of_pane(&update.source, &update.pane),
        Update::ViewerAnchors(update) => of_pane(&update.source, &update.pane),
        Update::ViewerReview(update) => of_pane(&update.source, &update.pane),
        Update::ReviewAnswered(update) => of_pane(&update.source, &update.pane),
        Update::Event { source, event } => {
            if let Some(output) = event.output() {
                return of_session(source, output.session_id);
            }
            let json = event.json()?;
            let text = |value: &serde_json::Value| value.as_str().map(str::to_owned);
            if let Some(screen) = text(&json["screenId"]).or_else(|| text(&json["screen"]["id"])) {
                let model = model.source(source.source_id())?;
                if model
                    .screens()
                    .iter()
                    .any(|info| info.id.as_str() == screen)
                {
                    return Some(model.screen_key(&screen));
                }
            }
            if let Some(pane) = text(&json["paneId"]).or_else(|| text(&json["command"]["paneId"])) {
                if let Some(key) = of_pane(source, &pane) {
                    return Some(key);
                }
            }
            [&json["sessionId"], &json["id"]]
                .into_iter()
                .filter_map(text)
                .find_map(|session| of_session(source, &session))
        }
        _ => None,
    }
}

fn asked_of_command(kind: &CommandKind) -> Vec<Asked> {
    use CommandKind::*;
    let by_id = |id: &uuid::Uuid| vec![Asked::Request(*id)];
    match kind {
        RequestHostUpdate { request_id, .. }
        | DismissHostUpdate { request_id }
        | ManageForward { request_id, .. }
        | ManagePortCatalog { request_id, .. }
        | CancelForwardOpen { request_id }
        | ManageHost { request_id, .. }
        | TemplateApi { request_id, .. }
        | ExtractTemplate { request_id, .. }
        | FloatPane { request_id, .. }
        | DockFloat { request_id, .. }
        | MoveFloat { request_id, .. }
        | TransferPane { request_id, .. }
        | TransferGroup { request_id, .. }
        | Zoom { request_id, .. }
        | UpdateSettings { request_id, .. }
        | SetDrawerVisibility { request_id, .. }
        | StoreFiles { request_id, .. }
        | CancelFileStore { request_id } => by_id(request_id),
        ConfirmHostUpdate { plan } => by_id(&plan.request_id()),
        ApplyTemplate { plan, .. } => by_id(&plan.request_id()),
        CreateRemoteScreen { request } => by_id(&request.request_id()),
        AssessClose { request_id, .. } => vec![Asked::Close(*request_id)],
        RequestClose { plan, .. } => vec![Asked::Close(plan.request_id)],
        AnswerFileOffer { offer_id, .. } => vec![Asked::Offer(offer_id.clone())],
        AnswerEditorRequest { request_id, .. } => vec![Asked::Editor(request_id.clone())],
        _ => Vec::new(),
    }
}

fn asked_of_update(update: &Update) -> Option<Asked> {
    Some(match update {
        Update::RemoteScreenCreateRejected { request_id, .. }
        | Update::HostUpdateRejected { request_id, .. }
        | Update::PortCatalogResult { request_id, .. } => Asked::Request(*request_id),
        Update::HostUpdatePrepared(plan) => Asked::Request(plan.request_id()),
        Update::Forward(result) => Asked::Request(result.request_id()),
        Update::HostManagement(result) => Asked::Request(result.request_id()),
        Update::TemplateApi(result) => Asked::Request(result.request_id),
        Update::TemplateApply(result) => Asked::Request(result.request_id),
        Update::FloatingAction(result) => Asked::Request(result.request_id),
        Update::ScreenTransfer(result) => Asked::Request(result.request_id),
        Update::Zoom(result) => Asked::Request(result.request_id),
        Update::SettingsWrite(result) => Asked::Request(result.request_id()),
        Update::DrawerVisibility(result) => Asked::Request(result.request_id),
        Update::CloseSubmitted { request_id, .. }
        | Update::CloseRejected { request_id, .. }
        | Update::CloseAssessed { request_id, .. }
        | Update::CloseSettled { request_id, .. }
        | Update::WindowCloseApproved { request_id, .. } => Asked::Close(*request_id),
        Update::Files(
            FileUpdate::Offered { offer_id, .. }
            | FileUpdate::OfferClosed { offer_id, .. }
            | FileUpdate::DownloadProgress { offer_id, .. }
            | FileUpdate::Downloaded { offer_id, .. }
            | FileUpdate::DownloadFailed { offer_id, .. },
        ) => Asked::Offer(offer_id.clone()),
        Update::Files(
            FileUpdate::StoreProgress { request_id, .. }
            | FileUpdate::Stored { request_id, .. }
            | FileUpdate::StoreFailed { request_id, .. },
        ) => Asked::Request(*request_id),
        Update::Editor(EditorUpdate::Requested(request)) => {
            Asked::Editor(request.request_id.clone())
        }
        Update::Editor(EditorUpdate::Cancelled { request_id, .. }) => {
            Asked::Editor(request_id.clone())
        }
        _ => return None,
    })
}

impl Update {
    /// Whether every window shows what this update brings.
    fn for_everyone(&self) -> bool {
        matches!(
            self,
            Update::Forwards(_)
                | Update::HostDirectory(_)
                | Update::RememberedScreens(_)
                | Update::Shares(_)
                | Update::SshPrompts(_)
                | Update::HostReconnectFailed { .. }
                | Update::NotificationSettings { .. }
                | Update::ShellProfiles(_)
                | Update::ShellProfilesError { .. }
                | Update::Appearance(_)
                | Update::AppearanceError { .. }
                | Update::SettingsReconciled(_)
                | Update::Startup(_)
                | Update::LocalServiceFailed(_)
        )
    }
    /// A copy of an update that brings state every window shows, for a
    /// window that opens after it was sent; `None` for any other update.
    pub fn for_every_window(&self) -> Option<Update> {
        match self {
            Update::Forwards(_)
            | Update::HostDirectory(_)
            | Update::RememberedScreens(_)
            | Update::Shares(_)
            | Update::NotificationSettings { .. }
            | Update::ShellProfiles(_)
            | Update::Appearance(_) => self.duplicate(),
            _ => None,
        }
    }
    /// A copy for another window, of an update every window gets.
    fn duplicate(&self) -> Option<Update> {
        Some(match self {
            Update::Forwards(snapshot) => Update::Forwards(snapshot.clone()),
            Update::HostDirectory(update) => Update::HostDirectory(update.clone()),
            Update::RememberedScreens(list) => Update::RememberedScreens(list.clone()),
            Update::Shares(list) => Update::Shares(list.clone()),
            Update::SshPrompts(list) => Update::SshPrompts(list.clone()),
            Update::HostReconnectFailed { target } => Update::HostReconnectFailed {
                target: target.clone(),
            },
            Update::NotificationSettings { owner, read } => Update::NotificationSettings {
                owner: *owner,
                read: read.clone(),
            },
            Update::ShellProfiles(snapshot) => Update::ShellProfiles(snapshot.clone()),
            Update::ShellProfilesError { connection, reason } => Update::ShellProfilesError {
                connection: *connection,
                reason: reason.clone(),
            },
            Update::Appearance(snapshot) => Update::Appearance(snapshot.clone()),
            Update::AppearanceError { connection, reason } => Update::AppearanceError {
                connection: *connection,
                reason: reason.clone(),
            },
            Update::SettingsReconciled(result) => Update::SettingsReconciled(result.clone()),
            Update::Startup(phase) => Update::Startup(phase.clone()),
            Update::LocalServiceFailed(reason) => Update::LocalServiceFailed(reason.clone()),
            _ => return None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::zoom::{ZoomAction, ZoomResult};

    const SECOND: WindowId = WindowId::new(1);
    const THIRD: WindowId = WindowId::new(2);

    /// The window an addressed update is for, and whether it is `update`.
    fn window_of(update: &Update) -> WindowId {
        match update {
            Update::ForWindow { window, update } => {
                assert!(!matches!(**update, Update::ForWindow { .. }));
                *window
            }
            _ => WindowId::FIRST,
        }
    }
    fn windows_of(updates: &Outbox, update: Update) -> Vec<WindowId> {
        updates.addressed(update).iter().map(window_of).collect()
    }
    fn zoom_answer(request_id: uuid::Uuid) -> Update {
        Update::Zoom(ZoomResult::rejected(uuid::Uuid::nil(), request_id, "busy"))
    }

    #[tokio::test]
    async fn what_belongs_to_a_screen_goes_to_the_window_that_holds_it() {
        let mut model = WindowWorkspace::new(uuid::Uuid::new_v4());
        let keys = model
            .ui_test_local_screens_fixture(&["work", "notes"])
            .unwrap();
        let source = model.lifecycle_token();
        let event = |value: serde_json::Value| async move {
            Update::Event {
                source,
                event: Box::new(
                    Event::ui_test_json(source.connection_id(), &value)
                        .await
                        .unwrap(),
                ),
            }
        };
        let named = event(serde_json::json!({
            "type": "command", "command": {"type": "pane.clear", "paneId": "p-notes"}
        }))
        .await;
        let unnamed = event(serde_json::json!({
            "type": "command", "command": {"type": "pane.clear"}
        }))
        .await;
        let notice = event(serde_json::json!({
            "type": "process.finished", "sessionId": "s-notes"
        }))
        .await;
        // With one window nothing needs a holder.
        assert_eq!(screen_of(&model, &named), Some(keys[1].clone()));
        assert_eq!(screen_of(&model, &unnamed), None);
        assert_eq!(screen_of(&model, &notice), Some(keys[1].clone()));

        model.open_window(SECOND).unwrap();
        model.move_screen_to_window(&keys[1], SECOND).unwrap();
        let (sender, mut receiver) = mpsc::channel(8);
        let updates = Outbox::new(sender);
        updates.set_windows(model.window_ids(), WindowId::FIRST);
        let stop = AtomicBool::new(false);
        // A command that names a pane and a notice of a session reach the
        // window that holds their Screen; a command that names none reaches
        // the window that was active last.
        for update in [named, notice, unnamed] {
            publish_routed(&updates, &model, update, &stop)
                .await
                .unwrap();
        }
        let windows: Vec<WindowId> = std::iter::from_fn(|| receiver.try_recv().ok())
            .map(|update| window_of(&update))
            .collect();
        assert_eq!(windows, [SECOND, SECOND, WindowId::FIRST]);
        // The terminal's size and attachment follow the Screen as well.
        let attached = Update::Attached {
            source,
            session: "s-notes".into(),
        };
        publish_routed(&updates, &model, attached, &stop)
            .await
            .unwrap();
        assert_eq!(window_of(&receiver.try_recv().unwrap()), SECOND);
    }

    #[test]
    fn an_update_goes_to_the_window_it_answers_to_all_or_to_the_one_in_use() {
        let (sender, _receiver) = mpsc::channel(1);
        let updates = Outbox::new(sender);
        // A single window reads every update as it is.
        assert_eq!(
            windows_of(&updates, Update::Status("x".into())),
            [WindowId::FIRST]
        );
        assert!(matches!(
            Outbox::address(WindowId::FIRST, Update::Status("x".into())),
            Update::Status(_)
        ));
        updates.set_windows(vec![WindowId::FIRST, SECOND, THIRD], THIRD);

        // State every window shows goes to each, the first one unwrapped.
        assert_eq!(
            windows_of(&updates, Update::LocalServiceFailed("gone".into())),
            [WindowId::FIRST, SECOND, THIRD]
        );

        // An answer goes to the window that asked, whenever it comes.
        let request = uuid::Uuid::new_v4();
        let source = crate::workspace::SourceWorkspace::new(uuid::Uuid::new_v4()).source_token();
        updates.asked_by(
            &Command::new(
                source,
                CommandKind::Zoom {
                    connection: uuid::Uuid::nil(),
                    request_id: request,
                    action: ZoomAction::In,
                },
            )
            .in_window(SECOND),
        );
        assert_eq!(windows_of(&updates, zoom_answer(request)), [SECOND]);
        updates.enter(Some(WindowId::FIRST));
        assert_eq!(windows_of(&updates, zoom_answer(request)), [SECOND]);

        // What a command causes at once goes to its window; anything else
        // to the window that was active last.
        assert_eq!(
            windows_of(&updates, Update::Status("x".into())),
            [WindowId::FIRST]
        );
        assert_eq!(
            windows_of(&updates, zoom_answer(uuid::Uuid::new_v4())),
            [WindowId::FIRST]
        );
        updates.enter(None);
        assert_eq!(windows_of(&updates, Update::Status("x".into())), [THIRD]);

        // A file offer stays with the window that got it first.
        let offer = |id: &str| {
            Update::Files(FileUpdate::OfferClosed {
                source,
                offer_id: id.into(),
            })
        };
        assert_eq!(windows_of(&updates, offer("a")), [THIRD]);
        updates.set_windows(vec![WindowId::FIRST, SECOND, THIRD], WindowId::FIRST);
        assert_eq!(windows_of(&updates, offer("a")), [THIRD]);
        assert_eq!(windows_of(&updates, offer("b")), [WindowId::FIRST]);

        // A window that closed is asked nothing and answered nothing.
        updates.set_windows(vec![WindowId::FIRST, THIRD], THIRD);
        assert_eq!(windows_of(&updates, zoom_answer(request)), [THIRD]);
        updates.enter(Some(SECOND));
        assert_eq!(windows_of(&updates, Update::Status("x".into())), [THIRD]);
        assert_eq!(
            windows_of(&updates, Update::WindowClosed { window: SECOND }),
            [SECOND]
        );
    }
}
