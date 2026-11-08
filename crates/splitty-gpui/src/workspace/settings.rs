//! Retained local Settings presentation. The worker remains the only write owner.

use std::{collections::BTreeMap, sync::Arc};

use gpui_kit::base::Selectable as _;
use gpui_kit::component::{
    button::{Button, ButtonVariants},
    form::{Field, Form},
    select::{SearchableVec, Select, SelectEvent, SelectState},
    switch::Switch,
    ActiveTheme as _, Disableable, Sizable,
};
use gpui_kit::{prelude::*, *};
use splitty_client_core::appearance::AppearanceSnapshot;
use splitty_client_core::local_service::{
    NotificationSettingsRead, SettingsReconciled, SettingsWriteOutcome, SettingsWriteResult,
};
use splitty_client_core::settings::{SettingsDomain, SettingsEdit, SettingsField, SettingsValue};
use uuid::Uuid;

const APPEARANCE: &[SettingsField] = &[
    SettingsField::ThemeMode,
    SettingsField::Theme,
    SettingsField::ThemeDark,
    SettingsField::ThemeLight,
    SettingsField::FontSize,
    SettingsField::FontFamily,
    SettingsField::Zoom,
    SettingsField::ZoomScope,
];
const TERMINAL: &[SettingsField] = &[SettingsField::TerminalTheme, SettingsField::CloseOnExit];
const WORKSPACE: &[SettingsField] = &[SettingsField::DrawerPinned, SettingsField::DrawerWidth];
const NOTIFICATIONS: &[SettingsField] = &[
    SettingsField::NotificationEnabled,
    SettingsField::NotificationProcessFinished,
    SettingsField::NotificationProcessExited,
    SettingsField::NotificationProcessCrashed,
];

fn is_switch(field: SettingsField) -> bool {
    field == SettingsField::DrawerPinned || field.domain() == SettingsDomain::Notifications
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Section {
    Appearance,
    Terminal,
    Workspace,
    Notifications,
}

impl Section {
    const ALL: [Self; 4] = [
        Self::Appearance,
        Self::Terminal,
        Self::Workspace,
        Self::Notifications,
    ];

    fn id(self) -> &'static str {
        match self {
            Self::Appearance => "appearance",
            Self::Terminal => "terminal",
            Self::Workspace => "workspace",
            Self::Notifications => "notifications",
        }
    }

    fn title(self) -> &'static str {
        match self {
            Self::Appearance => "Appearance",
            Self::Terminal => "Terminal",
            Self::Workspace => "Workspace",
            Self::Notifications => "Notifications",
        }
    }

    fn fields(self) -> &'static [SettingsField] {
        match self {
            Self::Appearance => APPEARANCE,
            Self::Terminal => TERMINAL,
            Self::Workspace => WORKSPACE,
            Self::Notifications => NOTIFICATIONS,
        }
    }

    fn description(self) -> &'static str {
        match self {
            Self::Appearance => "Interface colors, type and display scale",
            Self::Terminal => "Terminal colors and exit behavior",
            Self::Workspace => "Drawer placement and size",
            Self::Notifications => "Activity from other terminals",
        }
    }
}

#[derive(Clone, Debug)]
enum WriteState {
    Pending {
        request_id: Uuid,
        value: SettingsValue,
    },
    Confirmed {
        generation: u64,
    },
    Rejected {
        reason: String,
    },
    Uncertain {
        request_id: Uuid,
        reason: String,
        blocked: bool,
    },
}

/// Events carry the fixed local owner shown by the editor. The workspace
/// checks that owner again before routing an intent to its worker.
pub(super) enum SettingsEvent {
    Update {
        connection_id: Uuid,
        request_id: Uuid,
        edit: SettingsEdit,
    },
    Close,
}

pub(super) struct SettingsView {
    owner: Option<Uuid>,
    snapshot: Option<Arc<AppearanceSnapshot>>,
    notification: Option<NotificationSettingsRead>,
    notification_generation: u64,
    notification_error: Option<String>,
    section: Section,
    focus: FocusHandle,
    scroll: ScrollHandle,
    available_fonts: Vec<String>,
    selects: BTreeMap<SettingsField, Entity<SelectState<SearchableVec<String>>>>,
    choices: BTreeMap<SettingsField, Vec<(String, SettingsValue)>>,
    writes: BTreeMap<SettingsField, WriteState>,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<SettingsEvent> for SettingsView {}

impl SettingsView {
    pub(super) fn new(
        mut available_fonts: Vec<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        available_fonts.sort_unstable();
        available_fonts.dedup();
        available_fonts.retain(|name| {
            !name.is_empty() && name.len() <= 256 && !name.chars().any(char::is_control)
        });
        let mut selects = BTreeMap::new();
        let mut subscriptions = Vec::new();
        for field in SettingsField::ALL {
            if is_switch(field) {
                continue;
            }
            let select = cx.new(|cx| {
                SelectState::new(SearchableVec::new(Vec::<String>::new()), None, window, cx)
                    .searchable(matches!(
                        field,
                        SettingsField::Theme
                            | SettingsField::ThemeDark
                            | SettingsField::ThemeLight
                            | SettingsField::TerminalTheme
                            | SettingsField::FontFamily
                    ))
            });
            subscriptions.push(cx.subscribe_in(
                &select,
                window,
                move |this, _, event: &SelectEvent<SearchableVec<String>>, window, cx| {
                    let SelectEvent::Confirm(Some(label)) = event else {
                        return;
                    };
                    let label = label.clone();
                    let owner = this.owner;
                    // Select emits while updating its own state. Defer the
                    // controlled-value restoration until its borrow ends.
                    cx.defer_in(window, move |this, window, cx| {
                        if this.owner != owner {
                            return;
                        }
                        let Some(value) = this
                            .choices
                            .get(&field)
                            .and_then(|choices| {
                                choices.iter().find(|(candidate, _)| candidate == &label)
                            })
                            .map(|(_, value)| value.clone())
                        else {
                            return;
                        };
                        this.request(field, value, cx);
                        this.restore_select(field, window, cx);
                    });
                },
            ));
            selects.insert(field, select);
        }
        Self {
            owner: None,
            snapshot: None,
            notification: None,
            notification_generation: 0,
            notification_error: None,
            section: Section::Appearance,
            focus: cx.focus_handle(),
            scroll: ScrollHandle::new(),
            available_fonts,
            selects,
            choices: BTreeMap::new(),
            writes: BTreeMap::new(),
            _subscriptions: subscriptions,
        }
    }

    pub(super) fn set_snapshot(
        &mut self,
        snapshot: Arc<AppearanceSnapshot>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self
            .owner
            .is_some_and(|owner| owner != snapshot.connection_id())
        {
            return;
        }
        self.owner = Some(snapshot.connection_id());
        if self.snapshot.as_ref().is_some_and(|previous| {
            previous.connection_id() == snapshot.connection_id()
                && previous.generation() >= snapshot.generation()
        }) {
            return;
        }
        if self
            .snapshot
            .as_ref()
            .is_some_and(|previous| previous.connection_id() != snapshot.connection_id())
        {
            self.writes.clear();
        }
        self.writes.retain(|field, state| {
            if field.domain() != SettingsDomain::Appearance {
                return true;
            }
            match state {
                WriteState::Pending { .. } => true,
                WriteState::Confirmed { generation } => snapshot.generation() < *generation,
                WriteState::Rejected { .. } | WriteState::Uncertain { .. } => true,
            }
        });
        self.snapshot = Some(snapshot);
        for field in SettingsField::ALL {
            if is_switch(field) {
                continue;
            }
            let choices = self.options(field);
            let labels: Vec<String> = choices.iter().map(|(label, _)| label.clone()).collect();
            self.choices.insert(field, choices);
            if let Some(select) = self.selects.get(&field) {
                select.update(cx, |select, cx| {
                    select.set_items(SearchableVec::new(labels), window, cx)
                });
                self.restore_select(field, window, cx);
            }
        }
        cx.notify();
    }

    pub(super) fn set_notification_read(
        &mut self,
        read: NotificationSettingsRead,
        cx: &mut Context<Self>,
    ) {
        if self
            .owner
            .is_some_and(|owner| owner != read.connection_id())
            || read.generation() <= self.notification_generation
        {
            return;
        }
        self.owner = Some(read.connection_id());
        self.notification_generation = read.generation();
        match read.result() {
            Ok(_) => {
                self.writes.retain(|field, state| {
                    field.domain() != SettingsDomain::Notifications
                        || !matches!(state, WriteState::Confirmed { generation } if read.generation() >= *generation)
                });
                self.notification_error = None;
                self.notification = Some(read);
            }
            Err(error) => self.notification_error = Some(error.to_owned()),
        }
        cx.notify();
    }

    fn generation(&self, domain: SettingsDomain) -> Option<u64> {
        match domain {
            SettingsDomain::Appearance => {
                self.snapshot.as_ref().map(|snapshot| snapshot.generation())
            }
            SettingsDomain::Notifications => self
                .notification
                .as_ref()
                .map(|snapshot| snapshot.generation()),
        }
    }

    fn value(&self, field: SettingsField) -> Option<SettingsValue> {
        match field.domain() {
            SettingsDomain::Appearance => self.snapshot.as_ref()?.value(field),
            SettingsDomain::Notifications => self.notification.as_ref()?.value(field),
        }
    }

    fn default_value(&self, field: SettingsField) -> Option<SettingsValue> {
        match field.domain() {
            SettingsDomain::Appearance => self.snapshot.as_ref()?.metadata()?.default_value(field),
            SettingsDomain::Notifications => self.notification.as_ref()?.default_value(field),
        }
    }

    fn request_captured(
        &mut self,
        owner: Option<Uuid>,
        field: SettingsField,
        value: SettingsValue,
        cx: &mut Context<Self>,
    ) {
        if owner.is_some() && self.owner == owner {
            self.request(field, value, cx);
        }
    }

    fn reset_captured(
        &mut self,
        owner: Option<Uuid>,
        field: SettingsField,
        expected: SettingsValue,
        cx: &mut Context<Self>,
    ) {
        if self.default_value(field).as_ref() == Some(&expected) {
            self.request_captured(owner, field, expected, cx);
        }
    }

    fn notification_notice(&self) -> Option<String> {
        if let Some(error) = &self.notification_error {
            return Some(format!("Couldn’t refresh notifications: {error}"));
        }
        let snapshot = self.notification.as_ref()?;
        if let Some(error) = snapshot.metadata_error() {
            return Some(format!("Reset defaults unavailable: {error}"));
        }
        NOTIFICATIONS
            .iter()
            .any(|field| snapshot.default_value(*field).is_none())
            .then(|| "Some reset defaults are unavailable".into())
    }

    pub(super) fn set_owner(
        &mut self,
        connection: Uuid,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.owner == Some(connection) {
            return;
        }
        self.clear(window, cx);
        self.owner = Some(connection);
    }

    pub(super) fn clear(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.owner = None;
        self.snapshot = None;
        self.notification = None;
        self.notification_generation = 0;
        self.notification_error = None;
        self.writes.clear();
        self.choices.clear();
        for select in self.selects.values() {
            select.update(cx, |select, cx| {
                select.set_items(SearchableVec::new(Vec::<String>::new()), window, cx);
                select.set_selected_index(None, window, cx);
            });
        }
        cx.notify();
    }

    pub(super) fn apply_write_result(
        &mut self,
        result: &SettingsWriteResult,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.owner != Some(result.connection_id()) {
            return;
        }
        let field = result.field();
        if !matches!(self.writes.get(&field), Some(WriteState::Pending { request_id, .. })
            if *request_id == result.request_id())
        {
            return;
        }
        let state = match result.outcome() {
            SettingsWriteOutcome::Confirmed => {
                if self
                    .generation(field.domain())
                    .is_some_and(|generation| generation >= result.generation())
                {
                    self.writes.remove(&field);
                    cx.notify();
                    return;
                }
                WriteState::Confirmed {
                    generation: result.generation(),
                }
            }
            SettingsWriteOutcome::Rejected { reason } => WriteState::Rejected {
                reason: reason.clone(),
            },
            SettingsWriteOutcome::Uncertain { reason } => WriteState::Uncertain {
                request_id: result.request_id(),
                reason: reason.clone(),
                blocked: true,
            },
        };
        self.writes.insert(field, state);
        cx.notify();
    }

    pub(super) fn apply_reconciled(
        &mut self,
        reconciliation: &SettingsReconciled,
        cx: &mut Context<Self>,
    ) {
        if self.owner != Some(reconciliation.connection_id()) {
            return;
        }
        for (field, request_id) in reconciliation.requests() {
            if field.domain() != reconciliation.domain() {
                continue;
            }
            if let Some(WriteState::Uncertain {
                request_id: pending,
                blocked,
                ..
            }) = self.writes.get_mut(field)
            {
                if pending == request_id {
                    *blocked = false;
                }
            }
        }
        cx.notify();
    }

    pub(super) fn reject_update(
        &mut self,
        request_id: Uuid,
        reason: impl Into<String>,
        cx: &mut Context<Self>,
    ) {
        if let Some((field, _)) = self.writes.iter().find(|(_, state)| {
            matches!(state, WriteState::Pending { request_id: pending, .. }
                if *pending == request_id)
        }) {
            let field = *field;
            self.writes.insert(
                field,
                WriteState::Rejected {
                    reason: reason.into(),
                },
            );
            cx.notify();
        }
    }

    pub(super) fn focus(&self, window: &mut Window, cx: &mut App) {
        self.focus.focus(window, cx);
    }

    fn restore_select(&self, field: SettingsField, window: &mut Window, cx: &mut Context<Self>) {
        let Some(snapshot) = &self.snapshot else {
            return;
        };
        let Some(value) = snapshot.value(field) else {
            return;
        };
        let label = self
            .choices
            .get(&field)
            .and_then(|choices| choices.iter().find(|(_, candidate)| candidate == &value))
            .map(|(label, _)| label.clone());
        if let Some(select) = self.selects.get(&field) {
            select.update(cx, |select, cx| {
                if let Some(label) = label {
                    select.set_selected_value(&label, window, cx);
                } else {
                    select.set_selected_index(None, window, cx);
                }
            });
        }
    }

    fn options(&self, field: SettingsField) -> Vec<(String, SettingsValue)> {
        let Some(snapshot) = &self.snapshot else {
            return Vec::new();
        };
        let metadata = snapshot.metadata();
        let values: Option<Vec<SettingsValue>> = match field {
            SettingsField::Theme | SettingsField::ThemeDark | SettingsField::ThemeLight => snapshot
                .themes()
                .map(|names| names.iter().cloned().map(SettingsValue::Text).collect()),
            SettingsField::TerminalTheme => snapshot.themes().map(|names| {
                metadata
                    .and_then(|metadata| metadata.default_value(field))
                    .filter(|value| value == &SettingsValue::Text("default".into()))
                    .into_iter()
                    .chain(
                        names
                            .iter()
                            .filter(|name| name.as_str() != "default")
                            .cloned()
                            .map(SettingsValue::Text),
                    )
                    .collect()
            }),
            SettingsField::FontFamily => {
                let mut fonts = self
                    .available_fonts
                    .iter()
                    .take(4096)
                    .cloned()
                    .collect::<Vec<_>>();
                if let Some(SettingsValue::Text(current)) = snapshot.value(field) {
                    if !current.is_empty()
                        && self.available_fonts.binary_search(&current).is_ok()
                        && !fonts.contains(&current)
                    {
                        fonts.push(current);
                        fonts.sort_unstable();
                    }
                }
                let choices = std::iter::once(SettingsValue::Text(String::new()))
                    .chain(fonts.into_iter().map(SettingsValue::Text))
                    .collect();
                Some(choices)
            }
            SettingsField::DrawerPinned
            | SettingsField::NotificationEnabled
            | SettingsField::NotificationProcessFinished
            | SettingsField::NotificationProcessExited
            | SettingsField::NotificationProcessCrashed => None,
            SettingsField::ThemeMode | SettingsField::ZoomScope | SettingsField::CloseOnExit => {
                metadata
                    .and_then(|metadata| metadata.choices(field))
                    .map(|choices| choices.iter().cloned().map(SettingsValue::Text).collect())
            }
            SettingsField::FontSize | SettingsField::Zoom | SettingsField::DrawerWidth => metadata
                .and_then(|metadata| metadata.limits(field))
                .and_then(|limits| {
                    if !limits.values().is_empty() {
                        return Some(
                            limits
                                .values()
                                .iter()
                                .copied()
                                .map(SettingsValue::Number)
                                .collect(),
                        );
                    }
                    let (min, max, step) = (limits.min()?, limits.max()?, limits.step()?);
                    if !min.is_finite()
                        || !max.is_finite()
                        || !step.is_finite()
                        || step <= 0.
                        || max < min
                        || (max - min) / step > 100.
                    {
                        return None;
                    }
                    let count = ((max - min) / step).round() as usize;
                    Some(
                        (0..=count)
                            .map(|index| {
                                SettingsValue::Number(
                                    ((min + step * index as f64) * 1000.).round() / 1000.,
                                )
                            })
                            .collect(),
                    )
                }),
        };
        values
            .unwrap_or_default()
            .into_iter()
            .filter(|value| SettingsEdit::new(field, value.clone()).is_ok())
            .map(|value| (display_value(field, &value), value))
            .collect()
    }

    fn request(&mut self, field: SettingsField, value: SettingsValue, cx: &mut Context<Self>) {
        if !self.enabled(field) {
            return;
        }
        let Some(connection_id) = self.owner else {
            return;
        };
        if self.value(field).as_ref() == Some(&value) {
            return;
        }
        let Ok(edit) = SettingsEdit::new(field, value.clone()) else {
            return;
        };
        let request_id = Uuid::new_v4();
        self.writes
            .insert(field, WriteState::Pending { request_id, value });
        cx.emit(SettingsEvent::Update {
            connection_id,
            request_id,
            edit,
        });
        cx.notify();
    }

    fn enabled(&self, field: SettingsField) -> bool {
        if matches!(
            self.writes.get(&field),
            Some(
                WriteState::Pending { .. }
                    | WriteState::Confirmed { .. }
                    | WriteState::Uncertain { blocked: true, .. }
            )
        ) {
            return false;
        }
        if field.domain() == SettingsDomain::Notifications {
            return self.value(field).is_some();
        }
        self.snapshot.as_ref().is_some_and(|snapshot| {
            snapshot.metadata().is_some_and(|metadata| {
                !matches!(
                    field,
                    SettingsField::FontFamily | SettingsField::DrawerPinned
                ) || metadata.default_value(field).is_some()
            })
        }) && (is_switch(field)
            || self
                .choices
                .get(&field)
                .is_some_and(|choices| !choices.is_empty()))
    }

    fn status(&self, field: SettingsField) -> Option<String> {
        match self.writes.get(&field) {
            Some(WriteState::Pending { value, .. }) => {
                Some(format!("Applying {}…", display_value(field, value)))
            }
            Some(WriteState::Confirmed { .. }) => Some("Checking saved value…".into()),
            Some(WriteState::Rejected { reason, .. }) => Some(format!("Couldn’t save: {reason}")),
            Some(WriteState::Uncertain { reason, .. }) => {
                Some(format!("Save status unknown: {reason}"))
            }
            None => {
                if field.domain() == SettingsDomain::Notifications {
                    return None;
                }
                let snapshot = self.snapshot.as_ref()?;
                if let Some(error) = snapshot.metadata_error() {
                    return Some(format!("Settings options unavailable: {error}"));
                }
                if matches!(
                    field,
                    SettingsField::Theme
                        | SettingsField::ThemeDark
                        | SettingsField::ThemeLight
                        | SettingsField::TerminalTheme
                ) {
                    snapshot
                        .themes_error()
                        .map(|error| format!("Theme choices unavailable: {error}"))
                } else {
                    None
                }
            }
        }
    }

    fn render_field(&self, field: SettingsField, cx: &mut Context<Self>) -> Field {
        let owner = self.owner;
        let current = self.value(field);
        let default = self.default_value(field);
        let enabled = self.enabled(field);
        let disabled_reason = if current.is_none() {
            Some("Connecting to local settings")
        } else if self.writes.contains_key(&field) {
            None
        } else if !enabled {
            Some("Choices unavailable")
        } else {
            None
        };
        let control: AnyElement = if is_switch(field) {
            let checked = matches!(current.as_ref(), Some(SettingsValue::Bool(true)));
            let weak = cx.entity().downgrade();
            Switch::new(field.path())
                .accessibility_label(field_label(field))
                .checked(checked)
                .disabled(!enabled)
                .on_change(move |next, _, cx| {
                    let next = *next;
                    let _ = weak.update(cx, |this, cx| {
                        this.request_captured(owner, field, SettingsValue::Bool(next), cx);
                    });
                })
                .into_any_element()
        } else {
            let select = self.selects.get(&field).expect("known select field");
            Select::new(select)
                .id(field.path())
                .accessibility_label(field_label(field))
                .placeholder(
                    current
                        .as_ref()
                        .map(|value| display_value(field, value))
                        .unwrap_or_else(|| "Loading…".into()),
                )
                .menu_max_h(rems(18.))
                .disabled(!enabled)
                .w_full()
                .into_any_element()
        };
        let weak = cx.entity().downgrade();
        let can_reset = enabled
            && default.as_ref().is_some_and(|default| {
                Some(default) != current.as_ref()
                    && (is_switch(field)
                        || self.choices.get(&field).is_some_and(|choices| {
                            choices.iter().any(|(_, value)| value == default)
                        }))
            });
        let reset = Button::new(SharedString::from(format!(
            "settings-reset-{}",
            field.path()
        )))
        .label("Reset")
        .accessibility_label(format!("Reset {}", field_label(field)))
        .when(cfg!(all(test, feature = "ui-test-support")), |button| {
            button.debug_selector(|| format!("settings-reset-{}", field.path()))
        })
        .ghost()
        .small()
        .disabled(!can_reset)
        .on_click(move |_, _, cx| {
            if let Some(default) = default.clone() {
                let _ = weak.update(cx, |this, cx| {
                    this.reset_captured(owner, field, default, cx)
                });
            }
        });
        let mut field_view = Field::new()
            .label(field_label(field))
            .description(field_description(field))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .w_full()
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .when(cfg!(all(test, feature = "ui-test-support")), |control| {
                                control
                                    .debug_selector(|| format!("settings-control-{}", field.path()))
                            })
                            .child(control),
                    )
                    .child(reset),
            );
        if let Some(status) = self.status(field) {
            field_view = field_view.child(
                div()
                    .text_sm()
                    .text_color(cx.theme().muted_foreground)
                    .child(status),
            );
        } else if let Some(reason) = disabled_reason {
            field_view = field_view.child(
                div()
                    .text_sm()
                    .text_color(cx.theme().muted_foreground)
                    .child(reason),
            );
        }
        field_view
    }
}

impl Render for SettingsView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let mut nav = div().flex().flex_col().gap_1();
        for section in Section::ALL {
            let selected = self.section == section;
            nav = nav.child(
                Button::new(SharedString::from(format!("settings-nav-{}", section.id())))
                    .label(section.title())
                    .when(cfg!(all(test, feature = "ui-test-support")), |button| {
                        button.debug_selector(|| format!("settings-nav-{}", section.id()))
                    })
                    .selected(selected)
                    .ghost()
                    .small()
                    .w_full()
                    .justify_start()
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if this.section != section {
                            this.section = section;
                            this.scroll.set_offset(point(px(0.), px(0.)));
                            cx.notify();
                        }
                    })),
            );
        }
        let notification_notice = (self.section == Section::Notifications)
            .then(|| self.notification_notice())
            .flatten();
        let mut form = Form::new();
        for field in self.section.fields() {
            form = form.child(self.render_field(*field, cx));
        }
        div()
            .id("settings-view")
            .track_focus(&self.focus)
            .flex()
            .size_full()
            .min_w_0()
            .min_h_0()
            .bg(cx.theme().background)
            .text_color(cx.theme().foreground)
            .child(
                div()
                    .w(rems(11.))
                    .flex_none()
                    .border_r_1()
                    .border_color(cx.theme().border)
                    .bg(cx.theme().sidebar)
                    .text_color(cx.theme().sidebar_foreground)
                    .px_3()
                    .py_4()
                    .child(nav),
            )
            .child(
                div()
                    .id("settings-content")
                    .flex_1()
                    .min_w_0()
                    .min_h_0()
                    .overflow_y_scroll()
                    .track_scroll(&self.scroll)
                    .child(
                        div()
                            .w_full()
                            .max_w(rems(42.))
                            .px_6()
                            .py_5()
                            .flex()
                            .flex_col()
                            .gap_5()
                            .child(
                                div()
                                    .flex()
                                    .items_start()
                                    .justify_between()
                                    .gap_3()
                                    .child(
                                        div()
                                            .flex()
                                            .flex_col()
                                            .gap_1()
                                            .child(
                                                div()
                                                    .text_2xl()
                                                    .font_weight(FontWeight::SEMIBOLD)
                                                    .child(self.section.title()),
                                            )
                                            .child(
                                                div()
                                                    .text_sm()
                                                    .text_color(cx.theme().muted_foreground)
                                                    .child(self.section.description()),
                                            ),
                                    )
                                    .child(
                                        Button::new("settings-close")
                                            .label("Close")
                                            .ghost()
                                            .small()
                                            .on_click(cx.listener(|_, _, _, cx| {
                                                cx.emit(SettingsEvent::Close)
                                            })),
                                    ),
                            )
                            .when_some(notification_notice, |view, notice| {
                                view.child(
                                    div()
                                        .text_sm()
                                        .text_color(cx.theme().muted_foreground)
                                        .child(notice),
                                )
                            })
                            .child(form),
                    ),
            )
    }
}

fn field_label(field: SettingsField) -> &'static str {
    match field {
        SettingsField::ThemeMode => "Theme mode",
        SettingsField::Theme => "Theme",
        SettingsField::ThemeDark => "Dark theme",
        SettingsField::ThemeLight => "Light theme",
        SettingsField::FontSize => "Interface font size",
        SettingsField::FontFamily => "Interface font",
        SettingsField::Zoom => "Display zoom",
        SettingsField::ZoomScope => "Zoom applies to",
        SettingsField::TerminalTheme => "Terminal colors",
        SettingsField::CloseOnExit => "Close on exit",
        SettingsField::DrawerPinned => "Show drawer",
        SettingsField::DrawerWidth => "Drawer width",
        SettingsField::NotificationEnabled => "Enable notifications",
        SettingsField::NotificationProcessFinished => "Process finished",
        SettingsField::NotificationProcessExited => "Process exited",
        SettingsField::NotificationProcessCrashed => "Process crashed",
    }
}

fn field_description(field: SettingsField) -> &'static str {
    match field {
        SettingsField::ThemeMode => "Follow the system or use one theme",
        SettingsField::Theme => "Used when theme mode is fixed",
        SettingsField::ThemeDark => "Used when the system is dark",
        SettingsField::ThemeLight => "Used when the system is light",
        SettingsField::FontSize => "Controls the interface, not terminal text",
        SettingsField::FontFamily => "Choose an installed font or the theme default",
        SettingsField::Zoom => "Scales the interface or terminals according to scope",
        SettingsField::ZoomScope => "Choose which part of this window scales",
        SettingsField::TerminalTheme => "Default follows the interface theme",
        SettingsField::CloseOnExit => "When a terminal session ends",
        SettingsField::DrawerPinned => "Keep the Screen drawer visible",
        SettingsField::DrawerWidth => "Width of the Screen drawer",
        SettingsField::NotificationEnabled => "Show activity from other terminals in the drawer",
        SettingsField::NotificationProcessFinished => "When a process finishes",
        SettingsField::NotificationProcessExited => "When a terminal session ends normally",
        SettingsField::NotificationProcessCrashed => "When a terminal session ends with an error",
    }
}

fn display_value(field: SettingsField, value: &SettingsValue) -> String {
    match value {
        SettingsValue::Bool(true) => "On".into(),
        SettingsValue::Bool(false) => "Off".into(),
        SettingsValue::Text(text) if field == SettingsField::FontFamily && text.is_empty() => {
            "Theme default".into()
        }
        SettingsValue::Text(text) if field == SettingsField::TerminalTheme && text == "default" => {
            "Follow interface theme".into()
        }
        SettingsValue::Text(text) => match (field, text.as_str()) {
            (SettingsField::ThemeMode, "auto") => "Follow system".into(),
            (SettingsField::ThemeMode, "fixed") => "Fixed".into(),
            (SettingsField::ZoomScope, "window") => "Whole window".into(),
            (SettingsField::ZoomScope, "terminals") => "Terminals".into(),
            (SettingsField::CloseOnExit, "always") => "Always".into(),
            (SettingsField::CloseOnExit, "never") => "Never".into(),
            (SettingsField::CloseOnExit, "graceful") => "When successful".into(),
            _ => text.clone(),
        },
        SettingsValue::Number(number) if field == SettingsField::FontSize && *number == 0. => {
            "Theme default".into()
        }
        SettingsValue::Number(number) if field == SettingsField::FontSize => {
            format!("{number:.0} px")
        }
        SettingsValue::Number(number) if field == SettingsField::DrawerWidth => {
            format!("{number:.0} px")
        }
        SettingsValue::Number(number) if field == SettingsField::Zoom => {
            format!("{:.0}%", number * 100.)
        }
        SettingsValue::Number(number) => number.to_string(),
    }
}

#[cfg(all(test, feature = "ui-test-support"))]
mod tests {
    use super::{SettingsView, WriteState};
    use gpui_kit::{Entity, TestAppContext};
    use splitty_client_core::appearance::test_support::{settings_snapshot, Availability};
    use splitty_client_core::appearance::AppearanceSnapshot;
    use splitty_client_core::local_service::{
        SettingsReconciled, SettingsWriteOutcome, SettingsWriteResult,
    };
    use splitty_client_core::settings::{SettingsDomain, SettingsField, SettingsValue};
    use std::sync::Arc;
    use uuid::Uuid;

    fn show(
        view: &Entity<SettingsView>,
        window: &mut gpui_kit::VisualTestContext,
        snapshot: Arc<AppearanceSnapshot>,
    ) {
        window.update(|window, cx| {
            view.update(cx, |view, cx| view.set_snapshot(snapshot, window, cx));
            window.draw(cx).clear(cx);
        });
    }

    #[test]
    fn missing_metadata_or_catalog_keeps_confirmed_values_and_disables_only_dependent_fields() {
        let mut cx = TestAppContext::single();
        cx.update(gpui_kit::init);
        let (view, window) = cx
            .add_window_view(|window, cx| SettingsView::new(vec!["Test Mono".into()], window, cx));
        let owner = Uuid::new_v4();
        window.update(|window, cx| {
            view.update(cx, |view, cx| view.set_owner(owner, window, cx));
        });
        show(
            &view,
            window,
            settings_snapshot(owner, 1, Availability::MissingMetadata),
        );
        window.update(|_, cx| {
            let state = view.read(cx);
            assert_eq!(
                state.snapshot.as_ref().unwrap().value(SettingsField::Theme),
                Some(SettingsValue::Text("splitty-dark".into()))
            );
            assert!(SettingsField::ALL
                .iter()
                .all(|field| !state.enabled(*field)));
            assert!(state
                .status(SettingsField::Theme)
                .unwrap()
                .contains("Metadata unavailable"));
            let selected = state.selects[&SettingsField::Theme].read(cx);
            assert_eq!(
                selected.selected_value().map(String::as_str),
                Some("splitty-dark")
            );
        });
        show(
            &view,
            window,
            settings_snapshot(owner, 2, Availability::MissingThemes),
        );
        window.update(|_, cx| {
            let state = view.read(cx);
            assert!(!state.enabled(SettingsField::Theme));
            assert!(!state.enabled(SettingsField::TerminalTheme));
            assert!(state.enabled(SettingsField::Zoom));
            assert!(state.enabled(SettingsField::DrawerPinned));
            assert_eq!(
                state.snapshot.as_ref().unwrap().value(SettingsField::Theme),
                Some(SettingsValue::Text("splitty-dark".into()))
            );
            assert!(state
                .status(SettingsField::Theme)
                .unwrap()
                .contains("Theme catalog unavailable"));
        });
    }

    #[test]
    fn rejected_and_uncertain_results_keep_confirmed_value_and_ignore_stale_identity() {
        let mut cx = TestAppContext::single();
        cx.update(gpui_kit::init);
        let (view, window) =
            cx.add_window_view(|window, cx| SettingsView::new(Vec::new(), window, cx));
        let owner = Uuid::new_v4();
        window.update(|window, cx| {
            view.update(cx, |view, cx| view.set_owner(owner, window, cx));
        });
        show(
            &view,
            window,
            settings_snapshot(owner, 1, Availability::Full),
        );
        let field = SettingsField::ThemeMode;
        let value = SettingsValue::Text("auto".into());
        let request = window.update(|_, cx| {
            view.update(cx, |view, cx| view.request(field, value.clone(), cx));
            match view.read(cx).writes.get(&field).unwrap() {
                WriteState::Pending { request_id, .. } => *request_id,
                _ => panic!("expected pending write"),
            }
        });
        let result = |connection, request_id, outcome| {
            SettingsWriteResult::ui_test_fixture(connection, request_id, field, 2, outcome)
        };
        window.update(|window, cx| {
            view.update(cx, |view, cx| {
                view.apply_write_result(
                    &result(
                        owner,
                        Uuid::new_v4(),
                        SettingsWriteOutcome::Rejected {
                            reason: "old request".into(),
                        },
                    ),
                    window,
                    cx,
                )
            });
            view.update(cx, |view, cx| {
                view.apply_write_result(
                    &result(
                        Uuid::new_v4(),
                        request,
                        SettingsWriteOutcome::Rejected {
                            reason: "old owner".into(),
                        },
                    ),
                    window,
                    cx,
                )
            });
            assert!(matches!(
                view.read(cx).writes.get(&field),
                Some(WriteState::Pending { .. })
            ));
            view.update(cx, |view, cx| {
                view.apply_write_result(
                    &result(
                        owner,
                        request,
                        SettingsWriteOutcome::Rejected {
                            reason: "Service rejected value".into(),
                        },
                    ),
                    window,
                    cx,
                )
            });
            window.draw(cx).clear(cx);
            let state = view.read(cx);
            assert!(state.enabled(field));
            assert!(state
                .status(field)
                .unwrap()
                .contains("Service rejected value"));
            assert_eq!(
                state.snapshot.as_ref().unwrap().value(field),
                Some(SettingsValue::Text("fixed".into()))
            );
        });
        let uncertain_request = window.update(|_, cx| {
            view.update(cx, |view, cx| view.request(field, value, cx));
            match view.read(cx).writes.get(&field).unwrap() {
                WriteState::Pending { request_id, .. } => *request_id,
                _ => panic!("expected second pending write"),
            }
        });
        window.update(|window, cx| {
            view.update(cx, |view, cx| {
                view.apply_write_result(
                    &result(
                        owner,
                        uncertain_request,
                        SettingsWriteOutcome::Uncertain {
                            reason: "Reply lost".into(),
                        },
                    ),
                    window,
                    cx,
                )
            });
            window.draw(cx).clear(cx);
            let state = view.read(cx);
            assert!(!state.enabled(field));
            assert!(state.status(field).unwrap().contains("Reply lost"));
            assert_eq!(
                state.snapshot.as_ref().unwrap().value(field),
                Some(SettingsValue::Text("fixed".into()))
            );
        });
        let wrong = SettingsReconciled::ui_test_fixture(
            owner,
            2,
            SettingsDomain::Appearance,
            vec![(field, request)],
        );
        window.update(|_, cx| view.update(cx, |view, cx| view.apply_reconciled(&wrong, cx)));
        window.update(|_, cx| assert!(!view.read(cx).enabled(field)));
        let correct = SettingsReconciled::ui_test_fixture(
            owner,
            2,
            SettingsDomain::Appearance,
            vec![(field, uncertain_request)],
        );
        window.update(|_, cx| view.update(cx, |view, cx| view.apply_reconciled(&correct, cx)));
        window.update(|_, cx| {
            assert!(view.read(cx).enabled(field));
            assert!(view.read(cx).status(field).unwrap().contains("Reply lost"));
        });
    }
}

#[cfg(all(test, feature = "ui-test-support"))]
mod notification_tests;
