//! Immutable saved-template data and identified local library operations.
pub use splitty_core::template::{Pane as TemplatePane, Summary as TemplateSummary, Template};
use splitty_transport::budget::{Charged, ConnectionBudget, Lease};
use std::{io, sync::Arc};
use uuid::Uuid;

pub(crate) const MAX_TEMPLATE_BYTES: usize = 1024 * 1024;
#[derive(Debug)]
pub struct TemplateDraft {
    connection_id: Uuid,
    request_id: Uuid,
    value: Charged<Template>,
    budget: ConnectionBudget,
    copy_bytes: usize,
}
impl TemplateDraft {
    pub fn connection_id(&self) -> Uuid {
        self.connection_id
    }
    pub fn request_id(&self) -> Uuid {
        self.request_id
    }
    pub fn template(&self) -> &Template {
        &self.value
    }
    pub fn name(&self) -> &str {
        &self.value.name
    }
    pub(crate) fn reserve_copy(&self) -> io::Result<Lease> {
        self.budget
            .reserve(self.copy_bytes, false)
            .map_err(io::Error::other)
    }
    pub(crate) fn new(
        connection_id: Uuid,
        request_id: Uuid,
        value: Charged<Template>,
        budget: ConnectionBudget,
    ) -> io::Result<Self> {
        let size = serialized_size(&*value)?;
        if size > MAX_TEMPLATE_BYTES {
            return Err(invalid("Template exceeds native byte limit"));
        }
        let copy_bytes = size.saturating_mul(8).saturating_add(65536);
        let _scratch = budget
            .reserve(copy_bytes, false)
            .map_err(io::Error::other)?;
        validate(&value)?;
        Ok(Self {
            connection_id,
            request_id,
            value,
            budget,
            copy_bytes,
        })
    }
}
#[derive(Debug)]
pub struct TemplateCatalogSnapshot {
    connection_id: Uuid,
    request_id: Uuid,
    entries: Charged<Vec<TemplateSummary>>,
}
impl TemplateCatalogSnapshot {
    pub fn connection_id(&self) -> Uuid {
        self.connection_id
    }
    pub fn request_id(&self) -> Uuid {
        self.request_id
    }
    pub fn entries(&self) -> &[TemplateSummary] {
        &self.entries
    }
}
#[derive(Clone, Debug)]
pub enum TemplateApiRequest {
    List,
    Load {
        slug: String,
    },
    Extract {
        screen_id: String,
        name: String,
        root: String,
    },
    /// Save under `name`; the service names the file (`templates`, Saved
    /// Template Names) and, without `overwrite_confirmed`, answers
    /// [`TemplateApiOutcome::OverwriteRequired`] when that file exists.
    Save {
        draft: Arc<TemplateDraft>,
        name: String,
        included_commands: Vec<String>,
        overwrite_confirmed: bool,
    },
    Delete {
        slug: String,
    },
}
#[derive(Debug)]
pub struct TemplateApiResult {
    pub connection_id: Uuid,
    pub request_id: Uuid,
    pub outcome: TemplateApiOutcome,
}
#[derive(Debug)]
pub enum TemplateApiOutcome {
    Catalog(Arc<TemplateCatalogSnapshot>),
    Draft(Arc<TemplateDraft>),
    Saved { slug: String },
    Deleted { slug: String },
    OverwriteRequired { slug: String },
    Rejected { reason: String },
    Uncertain { reason: String },
}
impl TemplateApiResult {
    pub(crate) fn rejected(
        connection_id: Uuid,
        request_id: Uuid,
        reason: impl Into<String>,
    ) -> Self {
        Self {
            connection_id,
            request_id,
            outcome: TemplateApiOutcome::Rejected {
                reason: reason.into(),
            },
        }
    }
}
fn invalid(reason: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, reason)
}
pub(crate) fn slug(value: &str) -> io::Result<()> {
    if value.is_empty()
        || value.len() > 128
        || value.contains('/')
        || value.contains('\\')
        || value.contains("..")
    {
        return Err(invalid("Invalid or oversized template slug"));
    }
    Ok(())
}
pub(crate) fn validate(template: &Template) -> io::Result<()> {
    if template.name.len() > 256
        || template.panes.len() > 256
        || template.panes.iter().any(|(name, p)| {
            name.len() > 256
                || p.command.len() > 65536
                || p.dir.len() > 4096
                || p.shell_profile.len() > 256
        })
    {
        return Err(invalid("Template exceeds native presentation limits"));
    }
    // Use the shared projector for complete applicability, including nested floats
    // and pruned legacy web panes. Validation IDs are never installed as routes.
    let pane_ids = template
        .panes
        .iter()
        .filter(|(_, p)| p.pane_type != "webview")
        .enumerate()
        .map(|(i, (name, _))| {
            (
                name.clone(),
                splitty_core::ids::PaneId::from(format!("validate-{i}")),
            )
        })
        .collect();
    let request = splitty_core::mux::ApplyTemplate {
        template: template.clone(),
        pane_ids,
        root: String::new(),
        create_screen: true,
        replace_confirmed: false,
    };
    splitty_core::mux::project_template(&request).map_err(io::Error::other)?;
    Ok(())
}
impl TemplateApiRequest {
    pub(crate) fn validate(&self, owner: Uuid) -> io::Result<()> {
        match self {
            Self::List => {}
            Self::Load { slug: value } | Self::Delete { slug: value } => slug(value)?,
            Self::Extract {
                screen_id,
                name,
                root,
            } => {
                crate::connection::identifier(screen_id)?;
                if name.len() > 256 || root.len() > 4096 {
                    return Err(invalid("Template extraction input exceeds limit"));
                }
            }
            Self::Save {
                draft,
                name,
                included_commands,
                ..
            } => {
                if draft.connection_id() != owner {
                    return Err(invalid("Template belongs to a previous connection"));
                }
                if name.trim().is_empty()
                    || name.len() > 256
                    || included_commands.len() > 256
                    || included_commands
                        .iter()
                        .any(|id| !draft.template().panes.contains_key(id))
                {
                    return Err(invalid("Invalid template save selection"));
                }
            }
        }
        Ok(())
    }
}
mod api;
pub(crate) use api::execute;

pub(crate) fn serialized_size(value: &impl serde::Serialize) -> io::Result<usize> {
    struct Count(usize);
    impl io::Write for Count {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            self.0 = self
                .0
                .checked_add(bytes.len())
                .ok_or_else(|| invalid("Template size overflow"))?;
            Ok(bytes.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }
    let mut count = Count(0);
    serde_json::to_writer(&mut count, value).map_err(io::Error::other)?;
    Ok(count.0)
}

#[cfg(test)]
pub(crate) mod tests;
