//! Files between this window and a pane's server: images and files pasted
//! into a terminal or scratchpad are stored where the pane runs and arrive as
//! paths; files a pane offers with `splitty download` are saved here after
//! the user's Save. The worker owns every transfer; this shows and asks.
use super::scratchpad::ScratchpadSessionKey;
use super::*;
use crate::terminal::PastedFile;
use gpui_kit::component::input::TextareaState;
use splitty_client_core::native_workspace::{
    preferred_editor, quote_path, AppEditor, EditorOutcome, EditorUpdate, FileUpdate, PathKind,
    StoreItem,
};

/// A paste of fewer bytes shows no progress.
const PROGRESS_MIN_BYTES: u64 = 4 * 1024 * 1024;
const SAVED_NOTICE: Duration = Duration::from_secs(10);
const ERROR_NOTICE: Duration = Duration::from_secs(8);
const ACTIVITY_NOTICE: Duration = Duration::from_secs(5);
const MAX_NOTICES: usize = 6;
/// Offers asked or read at once, across every connection of this window.
const MAX_SHOWN_OFFERS: usize = 3;
/// Notices: a 300 px card with 12px text over 10.5px detail.
const NOTICE_WIDTH: f32 = 300.;
const NOTICE_TEXT: f32 = 12.;
const NOTICE_DETAIL: f32 = 10.5;
const NOTICE_BUTTON: f32 = 24.;

/// Where stored files came from; a drop may name the folder that keeps them.
enum StoreOrigin {
    Paste,
    Drop(Option<String>),
}

pub(super) enum StoreTarget {
    Terminal {
        route: TerminalToken,
        terminal: WeakEntity<Terminal>,
    },
    Scratchpad {
        key: ScratchpadSessionKey,
        input: WeakEntity<TextareaState>,
    },
}

#[derive(Clone, PartialEq)]
enum NoticeKind {
    /// A pane offers a file; nothing is read before Save.
    Offer {
        offer_id: String,
    },
    Download {
        offer_id: String,
    },
    Saved {
        path: std::path::PathBuf,
    },
    /// Dropped files wait for the user's Send.
    Drop {
        request_id: Uuid,
    },
    Storing {
        request_id: Uuid,
    },
    Failed,
    /// A pane's process asks to open a path in an editor; nothing opens
    /// before the user's Open.
    Editor {
        request_id: String,
        choice: super::transfer::EditorChoice,
        uri: String,
        path: String,
        remote: bool,
    },
    /// Activity in another terminal, with the way to it.
    Activity {
        screen: Option<ScreenKey>,
        pane: Option<PaneKey>,
    },
}

pub(super) struct FileNotice {
    key: String,
    source: SourceToken,
    title: SharedString,
    detail: SharedString,
    kind: NoticeKind,
    until: Option<Instant>,
}

pub(super) fn format_bytes(bytes: u64) -> String {
    const UNITS: [&str; 4] = ["B", "KB", "MB", "GB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024. && unit + 1 < UNITS.len() {
        value /= 1024.;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else if value >= 10. {
        format!("{value:.0} {}", UNITS[unit])
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

fn percent(done: u64, total: u64) -> u64 {
    (done.min(total) * 100).checked_div(total).unwrap_or(100)
}

impl Workspace {
    fn file_origin(&self, source: SourceToken) -> String {
        self.model
            .as_ref()
            .and_then(|model| model.source_target(source.source_id()))
            .map(super::hosts::display)
            .unwrap_or_else(|| "this computer".into())
    }

    fn set_file_notice(&mut self, notice: FileNotice, cx: &mut Context<Self>) {
        match self
            .file_notices
            .iter_mut()
            .find(|current| current.key == notice.key)
        {
            Some(current) => *current = notice,
            None => {
                if self.file_notices.len() >= MAX_NOTICES {
                    // Make room by dropping the oldest notice that only informs.
                    if let Some(index) = self.file_notices.iter().position(|notice| {
                        matches!(
                            notice.kind,
                            NoticeKind::Saved { .. }
                                | NoticeKind::Failed
                                | NoticeKind::Activity { .. }
                        )
                    }) {
                        self.file_notices.remove(index);
                    }
                }
                self.file_notices.push(notice);
            }
        }
        cx.notify();
    }

    fn drop_file_notice(&mut self, key: &str, cx: &mut Context<Self>) {
        let before = self.file_notices.len();
        self.file_notices.retain(|notice| notice.key != key);
        if self.file_notices.len() != before {
            cx.notify();
        }
    }

    fn file_failure(
        &mut self,
        key: String,
        source: SourceToken,
        title: impl Into<SharedString>,
        reason: String,
        cx: &mut Context<Self>,
    ) {
        self.set_file_notice(
            FileNotice {
                key,
                source,
                title: title.into(),
                detail: super::hosts::display(&reason).into(),
                kind: NoticeKind::Failed,
                until: Some(Instant::now() + ERROR_NOTICE),
            },
            cx,
        );
    }

    /// A host that was to come back at start did not. The notice stays
    /// until it is dismissed: nobody may be looking when a connection gives up.
    pub(super) fn host_reconnect_failed(&mut self, target: &str, cx: &mut Context<Self>) {
        let Some(source) = self.lifecycle_token() else {
            return;
        };
        self.set_file_notice(
            FileNotice {
                key: format!("reconnect-{target}"),
                source,
                title: format!("Could not reconnect {}", super::hosts::display(target)).into(),
                detail: "Its Screens are still offered under Hosts.".into(),
                kind: NoticeKind::Failed,
                until: None,
            },
            cx,
        );
    }

    /// Activity in a terminal the user is not looking at
    /// (`notifications.toasts`). It goes away on its own.
    pub(super) fn activity_notice(
        &mut self,
        fresh: &splitty_client_core::notifications::FreshNotification,
        cx: &mut Context<Self>,
    ) {
        let Some(source) = self.lifecycle_token() else {
            return;
        };
        self.activity_serial = self.activity_serial.wrapping_add(1);
        self.set_file_notice(
            FileNotice {
                key: format!("activity-{}", self.activity_serial),
                source,
                title: fresh.title.clone().into(),
                detail: SharedString::default(),
                kind: NoticeKind::Activity {
                    screen: fresh.screen.clone(),
                    pane: fresh.pane.clone(),
                },
                until: Some(Instant::now() + ACTIVITY_NOTICE),
            },
            cx,
        );
    }

    /// A pane's process asks to open a file or a folder in the user's editor,
    /// or its request ended. The request is only shown: Open launches.
    pub(super) fn editor_update(&mut self, update: EditorUpdate, cx: &mut Context<Self>) {
        let request = match update {
            EditorUpdate::Cancelled { request_id, .. } => {
                return self.drop_file_notice(&format!("editor-{request_id}"), cx);
            }
            EditorUpdate::Requested(request) => request,
        };
        let source = request.source;
        let target = self
            .model
            .as_ref()
            .filter(|model| source.source_id() != model.local_source().source_id())
            .and_then(|model| model.source_target(source.source_id()))
            .map(str::to_owned);
        let connection = target
            .as_ref()
            .map_or_else(|| "local".to_owned(), |target| format!("host:{target}"));
        let choice = match preferred_editor(
            &self.editor_prefs,
            &connection,
            &request.path,
            request.kind,
            request.editor,
        ) {
            AppEditor::VsCode => super::transfer::EditorChoice::VsCode,
            AppEditor::Zed => super::transfer::EditorChoice::Zed,
        };
        let Some(uri) = super::transfer::editor_uri(choice, target.as_deref(), &request.path)
        else {
            self.answer_editor(source, &request.request_id, EditorOutcome::Failed, cx);
            return self.file_failure(
                format!("editor-{}", request.request_id),
                source,
                "Cannot open in editor",
                "The path is not one an editor can be given.".into(),
                cx,
            );
        };
        let what = match request.kind {
            PathKind::File => "file",
            PathKind::Folder => "folder",
        };
        self.set_file_notice(
            FileNotice {
                key: format!("editor-{}", request.request_id),
                source,
                title: format!("Open {what} in {}?", choice.label()).into(),
                detail: format!(
                    "{} · {}",
                    self.file_origin(source),
                    super::hosts::display(&request.path)
                )
                .into(),
                kind: NoticeKind::Editor {
                    request_id: request.request_id,
                    choice,
                    uri,
                    path: request.path,
                    remote: target.is_some(),
                },
                until: Some(Instant::now() + request.expires_in),
            },
            cx,
        );
    }

    fn answer_editor(
        &mut self,
        source: SourceToken,
        request_id: &str,
        outcome: EditorOutcome,
        cx: &mut Context<Self>,
    ) {
        self.send_captured(
            source,
            Command::AnswerEditorRequest {
                request_id: request_id.to_owned(),
                outcome,
            },
            cx,
        );
    }

    /// Notices that only inform go away on their own.
    pub(super) fn expire_file_notices(&mut self, cx: &mut Context<Self>) {
        let now = Instant::now();
        // An editor request nobody answered in time is told so.
        let expired: Vec<_> = self
            .file_notices
            .iter()
            .filter(|notice| notice.until.is_some_and(|until| now >= until))
            .filter_map(|notice| match &notice.kind {
                NoticeKind::Editor { request_id, .. } => Some((notice.source, request_id.clone())),
                _ => None,
            })
            .collect();
        for (source, request_id) in expired {
            self.answer_editor(source, &request_id, EditorOutcome::Expired, cx);
        }
        let before = self.file_notices.len();
        self.file_notices
            .retain(|notice| notice.until.is_none_or(|until| now < until));
        if self.file_notices.len() != before {
            // A drop nobody confirmed sends nothing.
            let shown: Vec<_> = self
                .file_notices
                .iter()
                .filter_map(|notice| match notice.kind {
                    NoticeKind::Drop { request_id } => Some(request_id),
                    _ => None,
                })
                .collect();
            self.file_drops.retain(|id, _| shown.contains(id));
            cx.notify();
        }
    }

    /// A source that lost its connection lost its transfers with it.
    pub(super) fn reconcile_file_transfers(
        &mut self,
        model: &WindowWorkspace,
        cx: &mut Context<Self>,
    ) {
        let gone = |source: SourceToken| model.execution_token(source.source_id()) != Some(source);
        let lost: Vec<_> = self
            .file_stores
            .iter()
            .filter(|(_, (source, _))| gone(*source))
            .map(|(id, (source, _))| (*id, *source))
            .collect();
        for (request_id, source) in lost {
            self.file_stores.remove(&request_id);
            self.file_failure(
                format!("store-{request_id}"),
                source,
                "Nothing was pasted",
                "The connection was lost.".into(),
                cx,
            );
        }
        let stale: Vec<_> = self
            .file_notices
            .iter()
            .filter(|notice| gone(notice.source))
            .map(|notice| (notice.key.clone(), notice.source, notice.kind.clone()))
            .collect();
        for (key, source, kind) in stale {
            match kind {
                NoticeKind::Download { .. } => self.file_failure(
                    key,
                    source,
                    "The file was not saved",
                    "The connection was lost.".into(),
                    cx,
                ),
                NoticeKind::Offer { .. } | NoticeKind::Editor { .. } => {
                    self.drop_file_notice(&key, cx)
                }
                NoticeKind::Drop { request_id } => {
                    self.file_drops.remove(&request_id);
                    self.drop_file_notice(&key, cx);
                }
                NoticeKind::Storing { .. }
                | NoticeKind::Saved { .. }
                | NoticeKind::Failed
                | NoticeKind::Activity { .. } => {}
            }
        }
    }

    fn answer_file_offer(&mut self, source: SourceToken, offer_id: &str, save: bool) -> bool {
        self.commands
            .try_send(WorkerCommand::new(
                source,
                Command::AnswerFileOffer {
                    offer_id: offer_id.to_owned(),
                    save,
                },
            ))
            .is_ok()
    }

    /// Images and files pasted into a terminal.
    pub(super) fn paste_files_into_terminal(
        &mut self,
        route: TerminalToken,
        terminal: Entity<Terminal>,
        files: Vec<PastedFile>,
        cx: &mut Context<Self>,
    ) {
        let source = route.source_token();
        let Some(model) = self.model.as_ref().filter(|model| {
            model.execution_token(source.source_id()) == Some(source)
                && model.accepts_terminal(&route)
        }) else {
            return;
        };
        let local = source.source_id() == model.local_source().source_id();
        if local && files.iter().all(|file| matches!(file, PastedFile::Path(_))) {
            // A file of this computer is already where a local pane runs.
            let paths: Vec<_> = files
                .iter()
                .filter_map(|file| match file {
                    PastedFile::Path(path) => Some(quote_path(&path.to_string_lossy())),
                    PastedFile::Image { .. } => None,
                })
                .collect();
            terminal.update(cx, |terminal, cx| {
                terminal.paste_text(&paths.join(" "));
                cx.notify();
            });
            return;
        }
        self.store_pasted_files(
            source,
            files,
            StoreTarget::Terminal {
                route,
                terminal: terminal.downgrade(),
            },
            cx,
        );
    }

    /// Images and files pasted into a session's scratchpad.
    pub(super) fn paste_files_into_scratchpad(
        &mut self,
        source: SourceToken,
        key: ScratchpadSessionKey,
        files: Vec<PastedFile>,
        cx: &mut Context<Self>,
    ) {
        let Some(input) = self
            .scratchpads
            .editors
            .get(&key)
            .map(|editor| editor.input.downgrade())
        else {
            return;
        };
        if self.execution_token(source.source_id()) != Some(source) {
            return;
        }
        self.store_pasted_files(source, files, StoreTarget::Scratchpad { key, input }, cx);
    }

    /// Files dropped onto a terminal. A local pane takes their paths; a
    /// host's pane gets them only after the user's Send.
    pub(super) fn drop_files_into_terminal(
        &mut self,
        route: TerminalToken,
        terminal: Entity<Terminal>,
        paths: Vec<std::path::PathBuf>,
        cx: &mut Context<Self>,
    ) {
        let source = route.source_token();
        let Some(model) = self.model.as_ref().filter(|model| {
            model.execution_token(source.source_id()) == Some(source)
                && model.accepts_terminal(&route)
        }) else {
            return;
        };
        let files: Vec<_> = paths.into_iter().map(PastedFile::Path).collect();
        if source.source_id() == model.local_source().source_id() {
            return self.paste_files_into_terminal(route, terminal, files, cx);
        }
        let directory = model
            .source(source.source_id())
            .and_then(|model| super::transfer::pane_cwd(model, route.pane_id()));
        self.offer_dropped_files(
            source,
            files,
            StoreTarget::Terminal {
                route,
                terminal: terminal.downgrade(),
            },
            directory,
            cx,
        );
    }

    /// Files dropped onto a session's scratchpad: stored like a pane's, and
    /// their paths go into the text.
    pub(super) fn drop_files_into_scratchpad(
        &mut self,
        source: SourceToken,
        key: ScratchpadSessionKey,
        pane: &str,
        paths: Vec<std::path::PathBuf>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(model) = self
            .model
            .as_ref()
            .filter(|model| model.execution_token(source.source_id()) == Some(source))
        else {
            return;
        };
        let files: Vec<_> = paths.into_iter().map(PastedFile::Path).collect();
        let Some(input) = self
            .scratchpads
            .editors
            .get(&key)
            .map(|editor| editor.input.downgrade())
        else {
            return;
        };
        if source.source_id() == model.local_source().source_id() {
            // A file of this computer is already where a local pane runs.
            let text = files
                .iter()
                .filter_map(|file| match file {
                    PastedFile::Path(path) => Some(quote_path(&path.to_string_lossy())),
                    PastedFile::Image { .. } => None,
                })
                .collect::<Vec<_>>()
                .join(" ");
            let _ = input.update(cx, |input, cx| input.insert(text, window, cx));
            return;
        }
        let directory = model
            .source(source.source_id())
            .and_then(|model| super::transfer::pane_cwd(model, pane));
        self.offer_dropped_files(
            source,
            files,
            StoreTarget::Scratchpad { key, input },
            directory,
            cx,
        );
    }

    /// Ask before files leave this computer: nothing is sent before Send.
    fn offer_dropped_files(
        &mut self,
        source: SourceToken,
        files: Vec<PastedFile>,
        target: StoreTarget,
        directory: Option<String>,
        cx: &mut Context<Self>,
    ) {
        let total: u64 = files
            .iter()
            .filter_map(|file| match file {
                PastedFile::Path(path) => std::fs::metadata(path).ok().map(|meta| meta.len()),
                PastedFile::Image { .. } => None,
            })
            .sum();
        let request_id = Uuid::new_v4();
        let title = match files.as_slice() {
            [PastedFile::Path(path)] => format!(
                "Send {}?",
                path.file_name()
                    .map(|name| name.to_string_lossy().into_owned())
                    .unwrap_or_else(|| "this file".into())
            ),
            files => format!("Send {} files?", files.len()),
        };
        let detail = match &directory {
            Some(directory) => format!(
                "{} to {} · Send stores temporarily, Keep saves in {}",
                format_bytes(total),
                self.file_origin(source),
                super::hosts::display(directory)
            ),
            None => format!(
                "{} to {} · stored temporarily, path pasted",
                format_bytes(total),
                self.file_origin(source)
            ),
        };
        self.file_drops
            .insert(request_id, (files, target, directory));
        self.set_file_notice(
            FileNotice {
                key: format!("store-{request_id}"),
                source,
                title: title.into(),
                detail: detail.into(),
                kind: NoticeKind::Drop { request_id },
                until: Some(Instant::now() + Duration::from_secs(60)),
            },
            cx,
        );
    }

    pub(super) fn store_pasted_files(
        &mut self,
        source: SourceToken,
        files: Vec<PastedFile>,
        target: StoreTarget,
        cx: &mut Context<Self>,
    ) {
        self.store_files(
            source,
            Uuid::new_v4(),
            files,
            target,
            StoreOrigin::Paste,
            cx,
        )
    }

    fn store_files(
        &mut self,
        source: SourceToken,
        request_id: Uuid,
        files: Vec<PastedFile>,
        target: StoreTarget,
        origin: StoreOrigin,
        cx: &mut Context<Self>,
    ) {
        let (dropped, directory) = match origin {
            StoreOrigin::Paste => (false, None),
            StoreOrigin::Drop(directory) => (true, directory),
        };
        let items = files
            .into_iter()
            .map(|file| match file {
                PastedFile::Image { extension, bytes } => StoreItem::Image { extension, bytes },
                PastedFile::Path(path) => StoreItem::File(path),
            })
            .collect();
        match self.commands.try_send(WorkerCommand::new(
            source,
            Command::StoreFiles {
                request_id,
                items,
                dropped,
                directory,
            },
        )) {
            Ok(()) => {
                self.file_stores.insert(request_id, (source, target));
            }
            Err(error) => self.file_failure(
                format!("store-{request_id}"),
                source,
                "Nothing was pasted",
                format!("The paste was not sent: {error}"),
                cx,
            ),
        }
    }

    pub(super) fn file_update(
        &mut self,
        update: FileUpdate,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match update {
            FileUpdate::Offered {
                source,
                offer_id,
                name,
                size,
                expires_in,
            } => {
                let shown = self
                    .file_notices
                    .iter()
                    .filter(|notice| {
                        matches!(
                            notice.kind,
                            NoticeKind::Offer { .. } | NoticeKind::Download { .. }
                        )
                    })
                    .count();
                if self.execution_token(source.source_id()) != Some(source)
                    || shown >= MAX_SHOWN_OFFERS
                {
                    self.answer_file_offer(source, &offer_id, false);
                    return;
                }
                let detail = format!("{} from {}", format_bytes(size), self.file_origin(source));
                self.set_file_notice(
                    FileNotice {
                        key: format!("offer-{offer_id}"),
                        source,
                        title: format!("Save {}?", super::hosts::display(&name)).into(),
                        detail: detail.into(),
                        kind: NoticeKind::Offer { offer_id },
                        until: Some(Instant::now() + expires_in),
                    },
                    cx,
                );
            }
            FileUpdate::OfferClosed { offer_id, .. } => {
                self.drop_file_notice(&format!("offer-{offer_id}"), cx)
            }
            FileUpdate::DownloadProgress {
                source,
                offer_id,
                received,
                size,
            } => {
                let key = format!("offer-{offer_id}");
                let title = self
                    .file_notices
                    .iter()
                    .find(|notice| notice.key == key)
                    .map(|notice| {
                        let name = notice
                            .title
                            .strip_prefix("Save ")
                            .and_then(|title| title.strip_suffix('?'))
                            .or_else(|| notice.title.strip_prefix("Saving "))
                            .unwrap_or(&notice.title);
                        format!("Saving {name}")
                    })
                    .unwrap_or_else(|| "Saving file".into());
                self.set_file_notice(
                    FileNotice {
                        key,
                        source,
                        title: title.into(),
                        detail: format!(
                            "{} % of {} from {}",
                            percent(received, size),
                            format_bytes(size),
                            self.file_origin(source)
                        )
                        .into(),
                        kind: NoticeKind::Download { offer_id },
                        until: None,
                    },
                    cx,
                );
            }
            FileUpdate::Downloaded {
                source,
                offer_id,
                path,
            } => {
                let name = path
                    .file_name()
                    .map(|name| name.to_string_lossy().into_owned())
                    .unwrap_or_default();
                let folder = path
                    .parent()
                    .map(|dir| dir.to_string_lossy().into_owned())
                    .unwrap_or_default();
                self.set_file_notice(
                    FileNotice {
                        key: format!("offer-{offer_id}"),
                        source,
                        title: format!("Saved {name}").into(),
                        detail: folder.into(),
                        kind: NoticeKind::Saved { path },
                        until: Some(Instant::now() + SAVED_NOTICE),
                    },
                    cx,
                );
            }
            FileUpdate::DownloadFailed {
                source,
                offer_id,
                reason,
            } => self.file_failure(
                format!("offer-{offer_id}"),
                source,
                "The file was not saved",
                reason,
                cx,
            ),
            FileUpdate::StoreProgress {
                source,
                request_id,
                stored,
                total,
            } => {
                if total >= PROGRESS_MIN_BYTES && self.file_stores.contains_key(&request_id) {
                    self.set_file_notice(
                        FileNotice {
                            key: format!("store-{request_id}"),
                            source,
                            title: "Sending pasted files".into(),
                            detail: format!(
                                "{} % of {} to {}",
                                percent(stored, total),
                                format_bytes(total),
                                self.file_origin(source)
                            )
                            .into(),
                            kind: NoticeKind::Storing { request_id },
                            until: None,
                        },
                        cx,
                    );
                }
            }
            FileUpdate::Stored {
                source,
                request_id,
                paths,
            } => {
                self.drop_file_notice(&format!("store-{request_id}"), cx);
                let Some((_, target)) = self.file_stores.remove(&request_id) else {
                    return;
                };
                if paths.is_empty() || self.execution_token(source.source_id()) != Some(source) {
                    return;
                }
                match target {
                    StoreTarget::Terminal { route, terminal } => {
                        if !self
                            .model
                            .as_ref()
                            .is_some_and(|model| model.accepts_terminal(&route))
                        {
                            return;
                        }
                        let text = paths
                            .iter()
                            .map(|path| quote_path(path))
                            .collect::<Vec<_>>()
                            .join(" ");
                        let _ = terminal.update(cx, |terminal, cx| {
                            terminal.paste_text(&text);
                            cx.notify();
                        });
                    }
                    StoreTarget::Scratchpad { key, input } => {
                        if !self.scratchpads.editors.contains_key(&key) {
                            return;
                        }
                        let text = paths.join(" ");
                        let _ = input.update(cx, |input, cx| input.insert(text, window, cx));
                    }
                }
            }
            FileUpdate::StoreFailed {
                source,
                request_id,
                reason,
                ..
            } => {
                if self.file_stores.remove(&request_id).is_some() {
                    self.file_failure(
                        format!("store-{request_id}"),
                        source,
                        "Nothing was pasted",
                        reason,
                        cx,
                    );
                }
            }
        }
    }

    fn notice_button(
        &self,
        id: SharedString,
        label: &'static str,
        primary: bool,
        cx: &mut Context<Self>,
        run: impl Fn(&mut Self, &mut Window, &mut Context<Self>) + 'static,
    ) -> impl IntoElement {
        let colors = &self.appearance;
        let hover: Hsla = rgba(colors.hover_overlay).into();
        div()
            .id(id)
            .debug_selector(move || format!("file-notice-{label}"))
            .flex_none()
            .h(chrome_rems(NOTICE_BUTTON))
            .px(chrome_rems(10.))
            .flex()
            .items_center()
            .rounded(chrome_rems(5.))
            .border_1()
            .border_color(rgb(if primary {
                colors.accent
            } else {
                colors.subtle_border
            }))
            .text_size(chrome_rems(11.5))
            .font_weight(FontWeight::MEDIUM)
            .text_color(rgb(if primary {
                colors.accent
            } else {
                colors.secondary
            }))
            .hover(move |button| button.bg(hover))
            .child(label)
            .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                cx.stop_propagation();
                run(this, window, cx);
            }))
    }

    /// The window's file notices, stacked in its lower right corner.
    pub(super) fn render_file_notices(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        if self.file_notices.is_empty() {
            return None;
        }
        let colors = &self.appearance;
        let mut stack = div()
            .id("file-notices")
            .absolute()
            .right(chrome_rems(12.))
            .bottom(chrome_rems(12.))
            .w(chrome_rems(NOTICE_WIDTH))
            .flex()
            .flex_col()
            .gap(chrome_rems(6.));
        for notice in &self.file_notices {
            let key = notice.key.clone();
            let source = notice.source;
            let mut actions = div()
                .flex()
                .items_center()
                .justify_end()
                .gap(chrome_rems(6.));
            match &notice.kind {
                NoticeKind::Offer { offer_id } => {
                    let (dismiss_id, dismiss_key) = (offer_id.clone(), key.clone());
                    let save_id = offer_id.clone();
                    actions = actions
                        .child(self.notice_button(
                            format!("{key}-dismiss").into(),
                            "Dismiss",
                            false,
                            cx,
                            move |this, _, cx| {
                                this.answer_file_offer(source, &dismiss_id, false);
                                this.drop_file_notice(&dismiss_key, cx);
                            },
                        ))
                        .child(self.notice_button(
                            format!("{key}-save").into(),
                            "Save",
                            true,
                            cx,
                            move |this, _, cx| {
                                if !this.answer_file_offer(source, &save_id, true) {
                                    this.status = "Save was not sent; try again".into();
                                    cx.notify();
                                }
                            },
                        ));
                }
                NoticeKind::Download { offer_id } => {
                    let (offer_id, cancel_key) = (offer_id.clone(), key.clone());
                    actions = actions.child(self.notice_button(
                        format!("{key}-cancel").into(),
                        "Cancel",
                        false,
                        cx,
                        move |this, _, cx| {
                            this.answer_file_offer(source, &offer_id, false);
                            this.drop_file_notice(&cancel_key, cx);
                        },
                    ));
                }
                NoticeKind::Saved { path } => {
                    let (path, close_key) = (path.clone(), key.clone());
                    actions = actions.child(self.notice_button(
                        format!("{key}-show").into(),
                        "Show in folder",
                        false,
                        cx,
                        move |this, _, cx| {
                            cx.reveal_path(&path);
                            this.drop_file_notice(&close_key, cx);
                        },
                    ));
                }
                NoticeKind::Drop { request_id } => {
                    let request_id = *request_id;
                    let cancel_key = key.clone();
                    actions = actions.child(self.notice_button(
                        format!("{key}-cancel").into(),
                        "Cancel",
                        false,
                        cx,
                        move |this, _, cx| {
                            this.file_drops.remove(&request_id);
                            this.drop_file_notice(&cancel_key, cx);
                        },
                    ));
                    // The pane's directory can keep the files for good.
                    if self
                        .file_drops
                        .get(&request_id)
                        .is_some_and(|(_, _, directory)| directory.is_some())
                    {
                        actions = actions.child(self.notice_button(
                            format!("{key}-keep").into(),
                            "Keep in folder",
                            false,
                            cx,
                            move |this, _, cx| {
                                this.drop_file_notice(&format!("store-{request_id}"), cx);
                                if let Some((files, target, directory)) =
                                    this.file_drops.remove(&request_id)
                                {
                                    this.store_files(
                                        source,
                                        request_id,
                                        files,
                                        target,
                                        StoreOrigin::Drop(directory),
                                        cx,
                                    );
                                }
                            },
                        ));
                    }
                    actions = actions.child(self.notice_button(
                        format!("{key}-send").into(),
                        "Send",
                        true,
                        cx,
                        move |this, _, cx| {
                            this.drop_file_notice(&format!("store-{request_id}"), cx);
                            if let Some((files, target, _)) = this.file_drops.remove(&request_id) {
                                this.store_files(
                                    source,
                                    request_id,
                                    files,
                                    target,
                                    StoreOrigin::Drop(None),
                                    cx,
                                );
                            }
                        },
                    ));
                }
                NoticeKind::Storing { request_id } => {
                    let request_id = *request_id;
                    let cancel_key = key.clone();
                    actions = actions.child(self.notice_button(
                        format!("{key}-cancel").into(),
                        "Cancel",
                        false,
                        cx,
                        move |this, _, cx| {
                            if this.file_stores.remove(&request_id).is_some() {
                                let _ = this.commands.try_send(WorkerCommand::new(
                                    source,
                                    Command::CancelFileStore { request_id },
                                ));
                            }
                            this.drop_file_notice(&cancel_key, cx);
                        },
                    ));
                }
                NoticeKind::Activity { screen, pane } => {
                    let (close_key, show_key) = (key.clone(), key.clone());
                    actions = actions.child(self.notice_button(
                        format!("{key}-close").into(),
                        "Dismiss",
                        false,
                        cx,
                        move |this, _, cx| this.drop_file_notice(&close_key, cx),
                    ));
                    if let Some(screen) = screen.clone() {
                        let pane = pane.clone();
                        actions = actions.child(self.notice_button(
                            format!("{key}-show").into(),
                            "Show",
                            true,
                            cx,
                            move |this, window, cx| {
                                this.drop_file_notice(&show_key, cx);
                                this.show_notified(screen.clone(), pane.clone(), window, cx);
                            },
                        ));
                    }
                }
                NoticeKind::Editor {
                    request_id,
                    choice,
                    uri,
                    path,
                    remote,
                } => {
                    let (decline_id, decline_key) = (request_id.clone(), key.clone());
                    let (open_id, open_key) = (request_id.clone(), key.clone());
                    let (choice, uri, path, remote) = (*choice, uri.clone(), path.clone(), *remote);
                    actions = actions
                        .child(self.notice_button(
                            format!("{key}-decline").into(),
                            "Decline",
                            false,
                            cx,
                            move |this, _, cx| {
                                this.answer_editor(source, &decline_id, EditorOutcome::Denied, cx);
                                this.drop_file_notice(&decline_key, cx);
                            },
                        ))
                        .child(self.notice_button(
                            format!("{key}-open").into(),
                            "Open",
                            true,
                            cx,
                            move |this, _, cx| {
                                // Taken away first: a second press opens nothing.
                                this.drop_file_notice(&open_key, cx);
                                Self::open_editor(choice, &uri, &path, remote, cx);
                                this.answer_editor(source, &open_id, EditorOutcome::Dispatched, cx);
                            },
                        ));
                }
                NoticeKind::Failed => {
                    let close_key = key.clone();
                    actions = actions.child(self.notice_button(
                        format!("{key}-close").into(),
                        "Dismiss",
                        false,
                        cx,
                        move |this, _, cx| this.drop_file_notice(&close_key, cx),
                    ));
                }
            }
            let failed = notice.kind == NoticeKind::Failed;
            stack = stack.child(
                menu_style::panel(colors)
                    .id(SharedString::from(format!("file-notice-{key}")))
                    .debug_selector(|| "file-notice".into())
                    .occlude()
                    .p(chrome_rems(10.))
                    .gap(chrome_rems(8.))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(chrome_rems(3.))
                            .child(
                                div()
                                    .text_size(chrome_rems(NOTICE_TEXT))
                                    .text_color(rgb(if failed {
                                        colors.danger
                                    } else {
                                        colors.text
                                    }))
                                    .overflow_hidden()
                                    .whitespace_nowrap()
                                    .text_ellipsis()
                                    .child(notice.title.clone()),
                            )
                            .child(
                                div()
                                    .text_size(chrome_rems(NOTICE_DETAIL))
                                    .font_weight(FontWeight::NORMAL)
                                    .text_color(rgb(colors.muted))
                                    .child(notice.detail.clone()),
                            ),
                    )
                    .child(actions),
            );
        }
        Some(stack.into_any_element())
    }
}

#[cfg(test)]
mod tests {
    use super::{format_bytes, percent};

    #[test]
    fn sizes_and_progress_read_as_people_say_them() {
        assert_eq!(format_bytes(0), "0 B");
        assert_eq!(format_bytes(1023), "1023 B");
        assert_eq!(format_bytes(1536), "1.5 KB");
        assert_eq!(format_bytes(12 * 1024 * 1024), "12 MB");
        assert_eq!(percent(0, 0), 100);
        assert_eq!(percent(1, 3), 33);
        assert_eq!(percent(9, 3), 100);
    }
}

#[cfg(all(test, feature = "ui-test-support"))]
mod view_tests {
    use super::{FileUpdate, StoreItem};
    use crate::workspace::chrome::view_tests::with_view;
    use crate::workspace::Command;
    use gpui_kit::{px, size, Modifiers, MouseButton, VisualTestContext};
    use splitty_client_core::workspace::WindowWorkspace;
    use std::time::Duration;
    use tokio::sync::mpsc;
    use uuid::Uuid;

    fn click(window: &mut VisualTestContext, selector: &'static str) {
        window.run_until_parked();
        window.update(|window, cx| window.draw(cx).clear(cx));
        let center = window.debug_bounds(selector).expect(selector).center();
        window.simulate_mouse_down(center, MouseButton::Left, Modifiers::default());
        window.simulate_mouse_up(center, MouseButton::Left, Modifiers::default());
        window.run_until_parked();
        window.update(|window, cx| window.draw(cx).clear(cx));
    }

    #[test]
    fn dropped_files_and_offers_send_nothing_before_the_user_confirms() {
        with_view(|window, view| {
            let mut model = WindowWorkspace::new(Uuid::new_v4());
            let (_, route) = model.ui_test_remote_terminal_fixture("vm.test").unwrap();
            let source = route.source_token();
            let (commands, mut receiver) = mpsc::channel(32);
            window.update(|window, cx| {
                cx.set_reduce_motion(true);
                window.resize(size(px(900.), px(620.)));
                view.update(cx, |workspace, cx| {
                    workspace.commands = commands;
                    workspace.reconcile(model, window, cx);
                    workspace.status.clear();
                    cx.notify();
                });
                window.draw(cx).clear(cx);
            });
            while receiver.try_recv().is_ok() {}
            let dropped = std::env::temp_dir().join(format!("splitty-drop-{}", Uuid::new_v4()));
            std::fs::write(&dropped, b"dropped").unwrap();

            // A drop on a host's pane waits for Send; Cancel sends nothing.
            for confirm in [false, true] {
                window.update(|window, cx| {
                    view.update(cx, |workspace, cx| {
                        workspace.ensure_pane(
                            route.clone(),
                            workspace.current_screen_key().expect("shown remote Screen"),
                            0,
                            crate::terminal::LinkOrigin::Remote,
                            window,
                            cx,
                        );
                        let terminal = workspace.panes[&route.pane_key()].terminal.clone();
                        workspace.drop_files_into_terminal(
                            route.clone(),
                            terminal,
                            vec![dropped.clone()],
                            cx,
                        );
                    })
                });
                assert!(receiver.try_recv().is_err(), "a drop alone sends nothing");
                click(
                    window,
                    if confirm {
                        "file-notice-Send"
                    } else {
                        "file-notice-Cancel"
                    },
                );
                if !confirm {
                    assert!(receiver.try_recv().is_err(), "Cancel sends nothing");
                    assert!(window.debug_bounds("file-notice").is_none());
                }
            }
            let sent = receiver.try_recv().expect("Send stores the files");
            assert_eq!(sent.source_token(), Some(source));
            assert!(matches!(
                sent.kind(),
                Command::StoreFiles { items, dropped: true, .. }
                    if matches!(items.as_slice(), [StoreItem::File(path)] if path == &dropped)
            ));
            assert!(receiver.try_recv().is_err());
            std::fs::remove_file(&dropped).unwrap();

            // An offer reads nothing before Save; Dismiss declines it.
            let offer = |id: &str| FileUpdate::Offered {
                source,
                offer_id: id.to_owned(),
                name: "report.pdf".into(),
                size: 2048,
                expires_in: Duration::from_secs(60),
            };
            window.update(|window, cx| {
                view.update(cx, |workspace, cx| {
                    workspace.file_update(offer("a".repeat(32).as_str()), window, cx)
                })
            });
            assert!(
                receiver.try_recv().is_err(),
                "showing an offer sends nothing"
            );
            click(window, "file-notice-Dismiss");
            assert!(matches!(
                receiver.try_recv().expect("Dismiss declines").kind(),
                Command::AnswerFileOffer { save: false, .. }
            ));
            assert!(window.debug_bounds("file-notice").is_none());

            let second = "b".repeat(32);
            window.update(|window, cx| {
                view.update(cx, |workspace, cx| {
                    workspace.file_update(offer(&second), window, cx)
                })
            });
            click(window, "file-notice-Save");
            let saved = receiver.try_recv().expect("Save accepts");
            assert_eq!(saved.source_token(), Some(source));
            assert!(matches!(
                saved.kind(),
                Command::AnswerFileOffer { offer_id, save: true } if offer_id == &second
            ));
            assert!(receiver.try_recv().is_err());

            // Losing the host's connection ends what it was sending.
            window.update(|window, cx| {
                view.update(cx, |workspace, cx| {
                    workspace.file_update(
                        FileUpdate::DownloadProgress {
                            source,
                            offer_id: second.clone(),
                            received: 1024,
                            size: 2048,
                        },
                        window,
                        cx,
                    );
                    let mut lost = workspace.model.clone().unwrap();
                    lost.disconnect(source.source_id()).unwrap();
                    workspace.reconcile(lost, window, cx);
                })
            });
            window.update(|window, cx| window.draw(cx).clear(cx));
            assert!(
                window.debug_bounds("file-notice-Cancel").is_none(),
                "a download of a lost connection cannot be cancelled into it"
            );
            assert!(window.debug_bounds("file-notice-Dismiss").is_some());
        });
    }
    #[test]
    fn dropped_files_can_be_kept_in_the_folder_of_the_pane() {
        with_view(|window, view| {
            let mut model = WindowWorkspace::new(Uuid::new_v4());
            let (_, route) = model.ui_test_remote_terminal_fixture("vm.test").unwrap();
            let source = route.source_token();
            // The pane's program runs in a folder the host reports.
            model
                .source_mut(source.source_id())
                .unwrap()
                .apply_snapshot(
                    source.connection_id(),
                    &serde_json::json!({"type":"mux.screen_snapshot","screen":{
                        "id":"work","revision":2,
                        "root":{"type":"slot","id":"slot","paneIds":["p0"],"activePaneId":"p0"},
                        "panes":{"p0":{"id":"p0","sessionId":"s-p0"}},
                        "sessions":{"s-p0":{"id":"s-p0","status":"running","cwd":"/srv/app"}}
                    }}),
                )
                .unwrap();
            let (commands, mut receiver) = mpsc::channel(32);
            window.update(|window, cx| {
                cx.set_reduce_motion(true);
                window.resize(size(px(900.), px(620.)));
                view.update(cx, |workspace, cx| {
                    workspace.commands = commands;
                    workspace.reconcile(model, window, cx);
                    workspace.status.clear();
                    cx.notify();
                });
                window.draw(cx).clear(cx);
            });
            while receiver.try_recv().is_ok() {}
            let dropped = std::env::temp_dir().join(format!("splitty-drop-{}", Uuid::new_v4()));
            std::fs::write(&dropped, b"dropped").unwrap();
            window.update(|window, cx| {
                view.update(cx, |workspace, cx| {
                    workspace.ensure_pane(
                        route.clone(),
                        workspace.current_screen_key().expect("shown remote Screen"),
                        0,
                        crate::terminal::LinkOrigin::Remote,
                        window,
                        cx,
                    );
                    let terminal = workspace.panes[&route.pane_key()].terminal.clone();
                    workspace.drop_files_into_terminal(
                        route.clone(),
                        terminal,
                        vec![dropped.clone()],
                        cx,
                    );
                })
            });
            assert!(receiver.try_recv().is_err(), "a drop alone sends nothing");
            click(window, "file-notice-Keep in folder");
            let sent = receiver.try_recv().expect("the files are stored");
            assert_eq!(sent.source_token(), Some(source));
            assert!(matches!(
                sent.kind(),
                Command::StoreFiles { items, dropped: true, directory: Some(directory), .. }
                    if directory == "/srv/app"
                        && matches!(items.as_slice(), [StoreItem::File(path)] if path == &dropped)
            ));
            assert!(receiver.try_recv().is_err());
            std::fs::remove_file(&dropped).unwrap();
        });
    }
    #[test]
    fn an_editor_request_opens_nothing_before_the_user_confirms() {
        use splitty_client_core::native_workspace::{
            EditorOutcome, EditorRequest, EditorUpdate, PathKind,
        };
        with_view(|window, view| {
            let mut model = WindowWorkspace::new(Uuid::new_v4());
            let (_, route) = model.ui_test_remote_terminal_fixture("vm.test").unwrap();
            let source = route.source_token();
            let (commands, mut receiver) = mpsc::channel(32);
            window.update(|window, cx| {
                cx.set_reduce_motion(true);
                window.resize(size(px(900.), px(620.)));
                view.update(cx, |workspace, cx| {
                    workspace.commands = commands;
                    workspace.reconcile(model, window, cx);
                    workspace.status.clear();
                    cx.notify();
                });
                window.draw(cx).clear(cx);
            });
            while receiver.try_recv().is_ok() {}
            let request = |id: &str| {
                EditorUpdate::Requested(EditorRequest {
                    source,
                    request_id: id.to_owned(),
                    path: "/srv/app/main.rs".into(),
                    kind: PathKind::File,
                    editor: None,
                    expires_in: Duration::from_secs(20),
                })
            };
            let answered = |receiver: &mut mpsc::Receiver<_>| {
                let sent: crate::workspace::WorkerCommand =
                    receiver.try_recv().expect("the request is answered");
                assert_eq!(sent.source_token(), Some(source));
                match sent.kind() {
                    Command::AnswerEditorRequest {
                        request_id,
                        outcome,
                    } => (request_id.clone(), *outcome),
                    _ => panic!("an editor answer"),
                }
            };
            for (id, button, outcome) in [
                ("a".repeat(32), "file-notice-Decline", EditorOutcome::Denied),
                (
                    "b".repeat(32),
                    "file-notice-Open",
                    EditorOutcome::Dispatched,
                ),
            ] {
                window.update(|_, cx| {
                    view.update(cx, |workspace, cx| {
                        workspace.editor_update(request(&id), cx)
                    })
                });
                assert!(
                    receiver.try_recv().is_err(),
                    "showing a request answers nothing"
                );
                click(window, button);
                assert_eq!(answered(&mut receiver), (id, outcome));
                assert!(window.debug_bounds("file-notice").is_none());
                assert!(receiver.try_recv().is_err());
            }
            // The service withdraws a request: its notice goes without an answer.
            let withdrawn = "c".repeat(32);
            window.update(|_, cx| {
                view.update(cx, |workspace, cx| {
                    workspace.editor_update(request(&withdrawn), cx);
                    workspace.editor_update(
                        EditorUpdate::Cancelled {
                            source,
                            request_id: withdrawn.clone(),
                        },
                        cx,
                    );
                })
            });
            window.update(|window, cx| window.draw(cx).clear(cx));
            assert!(window.debug_bounds("file-notice").is_none());
            assert!(receiver.try_recv().is_err());
        });
    }
}
