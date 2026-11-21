//! The settings editor's choices of the window's look that wait for the
//! local service. The window draws them over its last confirmed settings
//! until the write fails or a read shows what the service saved; they never
//! count as confirmed (settings, Native Appearance Choices Before
//! Confirmation).
use crate::appearance::{Choices, NativeAppearance};
use splitty_client_core::local_service::{SettingsWriteOutcome, SettingsWriteResult};
use splitty_client_core::settings::{SettingsEdit, SettingsField, SettingsValue};
use std::collections::BTreeMap;
use uuid::Uuid;

#[derive(Debug)]
struct Choice {
    request_id: Uuid,
    value: SettingsValue,
    /// Once the service acknowledged the write, the read generation that
    /// shows the saved value: the editor's rule for a saved change.
    readback: Option<u64>,
}

#[derive(Debug, Default)]
pub(super) struct AppearanceChoices {
    choices: BTreeMap<SettingsField, Choice>,
}

impl AppearanceChoices {
    /// Show `edit`, sent as `request_id`, from now on; false for a field
    /// the window shows only once confirmed.
    pub(super) fn submit(&mut self, request_id: Uuid, edit: &SettingsEdit) -> bool {
        if !NativeAppearance::shows_choice(edit.field()) {
            return false;
        }
        self.choices.insert(
            edit.field(),
            Choice {
                request_id,
                value: edit.value().clone(),
                readback: None,
            },
        );
        true
    }

    /// The outcome of a shown choice's write; `read` is the generation of
    /// the last confirmed read. An acknowledged write keeps its choice until
    /// a read at its required generation; a rejected or uncertain one drops
    /// it. True when a choice was dropped.
    pub(super) fn settle(&mut self, result: &SettingsWriteResult, read: u64) -> bool {
        let field = result.field();
        let Some(choice) = self.choices.get_mut(&field) else {
            return false;
        };
        if choice.request_id != result.request_id() {
            return false;
        }
        match result.outcome() {
            SettingsWriteOutcome::Confirmed if read < result.generation() => {
                choice.readback = Some(result.generation());
                false
            }
            SettingsWriteOutcome::Confirmed
            | SettingsWriteOutcome::Rejected { .. }
            | SettingsWriteOutcome::Uncertain { .. } => {
                self.choices.remove(&field);
                true
            }
        }
    }

    /// A confirmed read at `generation` arrived: the choices it shows saved
    /// give way to its values.
    pub(super) fn read(&mut self, generation: u64) {
        self.choices
            .retain(|_, choice| choice.readback.is_none_or(|readback| generation < readback));
    }

    /// Forget every choice, as when the local connection changes.
    pub(super) fn clear(&mut self) {
        self.choices.clear();
    }

    /// The shown values by field.
    pub(super) fn values(&self) -> Choices {
        self.choices
            .iter()
            .map(|(field, choice)| (*field, choice.value.clone()))
            .collect()
    }
}

// The write results of a test come from the client core's UI test fixture.
#[cfg(all(test, feature = "ui-test-support"))]
mod tests {
    use super::*;

    fn theme(name: &str) -> SettingsEdit {
        SettingsEdit::new(SettingsField::Theme, SettingsValue::Text(name.into())).unwrap()
    }

    fn result(
        request_id: Uuid,
        generation: u64,
        outcome: SettingsWriteOutcome,
    ) -> SettingsWriteResult {
        SettingsWriteResult::ui_test_fixture(
            Uuid::nil(),
            request_id,
            SettingsField::Theme,
            generation,
            outcome,
        )
    }

    fn shown(choices: &AppearanceChoices) -> Option<SettingsValue> {
        choices.values().remove(&SettingsField::Theme)
    }

    /// settings, Native Appearance Choices Before Confirmation: a choice
    /// stays through reads below its readback and gives way to the read
    /// that shows the saved value.
    #[test]
    fn an_acknowledged_choice_stays_until_its_readback() {
        let mut choices = AppearanceChoices::default();
        let request = Uuid::new_v4();
        assert!(choices.submit(request, &theme("splitty-light")));
        choices.read(3);
        assert_eq!(
            shown(&choices),
            Some(SettingsValue::Text("splitty-light".into()))
        );
        assert!(!choices.settle(&result(request, 5, SettingsWriteOutcome::Confirmed), 3));
        choices.read(4);
        assert!(
            shown(&choices).is_some(),
            "a read below the readback keeps it"
        );
        choices.read(5);
        assert_eq!(shown(&choices), None);
    }

    /// An acknowledgement whose readback the window already has drops the
    /// choice at once; a rejected or uncertain write drops it too.
    #[test]
    fn a_settled_or_failed_write_drops_its_choice() {
        for outcome in [
            SettingsWriteOutcome::Confirmed,
            SettingsWriteOutcome::Rejected {
                reason: "refused".into(),
            },
            SettingsWriteOutcome::Uncertain {
                reason: "reply lost".into(),
            },
        ] {
            let mut choices = AppearanceChoices::default();
            let request = Uuid::new_v4();
            choices.submit(request, &theme("splitty-light"));
            assert!(choices.settle(&result(request, 4, outcome), 4));
            assert_eq!(shown(&choices), None);
        }
    }

    /// A result of another request leaves the shown choice alone, and a
    /// field the window shows only once confirmed is never a choice.
    #[test]
    fn other_requests_and_fields_leave_the_choices_alone() {
        let mut choices = AppearanceChoices::default();
        let request = Uuid::new_v4();
        choices.submit(request, &theme("splitty-light"));
        let other = result(
            Uuid::new_v4(),
            0,
            SettingsWriteOutcome::Rejected {
                reason: "refused".into(),
            },
        );
        assert!(!choices.settle(&other, 0));
        assert!(shown(&choices).is_some());
        for edit in [
            SettingsEdit::new(SettingsField::Zoom, SettingsValue::Number(1.5)).unwrap(),
            SettingsEdit::new(SettingsField::DrawerPinned, SettingsValue::Bool(false)).unwrap(),
            SettingsEdit::new(
                SettingsField::TitleBarStyle,
                SettingsValue::Text("system".into()),
            )
            .unwrap(),
            SettingsEdit::new(
                SettingsField::InterfaceStyle,
                SettingsValue::Text("system".into()),
            )
            .unwrap(),
        ] {
            assert!(!choices.submit(Uuid::new_v4(), &edit));
        }
        assert_eq!(choices.values().len(), 1);
        choices.clear();
        assert!(choices.values().is_empty());
    }
}
