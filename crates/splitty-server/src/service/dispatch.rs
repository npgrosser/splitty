//! Native entry point: fixed operation and trusted role, then the same domain
//! implementations used by browser/control adapters. No URL routing or auth.
use super::catalog::{ArgumentType, ErrorStyle, Operation, Rejection, Request, Role};
use super::operations as op;
use super::reply::{IntoReply, Payload, Reply};
use super::{jobs, review, State};
use http::StatusCode;
use serde::de::DeserializeOwned;
use serde_json::{json, Value};
use std::sync::Arc;

/// The answer of an operation of `style` to an argument or body it cannot
/// decode.
fn undecodable(style: ErrorStyle, error: impl std::fmt::Display) -> Reply {
    match style {
        ErrorStyle::Plain => (
            StatusCode::UNPROCESSABLE_ENTITY,
            Payload(json!({"error": error.to_string()})),
        )
            .into_reply(),
        ErrorStyle::Job => {
            crate::jobs::JobError::new(400, "invalid_request", error.to_string()).into_reply()
        }
        ErrorStyle::Viewer => review::invalid_body(error),
    }
}

fn decode<T: DeserializeOwned>(body: Value, style: ErrorStyle) -> Result<T, Reply> {
    serde_json::from_value(body).map_err(|error| undecodable(style, error))
}

impl State {
    pub async fn execute(&self, mut request: Request, role: Role) -> Reply {
        if let Err(error) = request.validate(role) {
            let status = match error {
                Rejection::Forbidden => StatusCode::FORBIDDEN,
                Rejection::TooLarge => StatusCode::PAYLOAD_TOO_LARGE,
                Rejection::Invalid(_) => StatusCode::BAD_REQUEST,
            };
            return (status, Payload(json!({"error":error.to_string()}))).into_reply();
        }
        let description = request.operation.description();
        let style = description.errors;
        for argument in description.arguments {
            if argument.kind != ArgumentType::Count {
                continue;
            }
            if let Some(value) = request.args.get_mut(argument.name) {
                let parsed = value.as_str().and_then(|value| value.parse::<u64>().ok());
                let Some(number) = parsed else {
                    return undecodable(style, format!("invalid {}", argument.name));
                };
                *value = json!(number);
            }
        }
        let body = request.args.remove("body").unwrap_or(Value::Null);
        let id = request
            .args
            .remove("id")
            .and_then(|value| value.as_str().map(str::to_owned))
            .unwrap_or_default();
        let slug = request
            .args
            .remove("slug")
            .and_then(|value| value.as_str().map(str::to_owned))
            .unwrap_or_default();
        let query = || {
            request
                .args
                .iter()
                .filter_map(|(name, value)| {
                    value.as_str().map(|value| (name.clone(), value.to_owned()))
                })
                .collect()
        };
        macro_rules! body {
            () => {
                match decode(body, style) {
                    Ok(body) => body,
                    Err(error) => return error,
                }
            };
        }
        let hub = &self.hub;
        let jobs = jobs::Context {
            jobs: Arc::downgrade(&self.client_state.jobs),
            actor: crate::jobs::Actor::User,
        };
        macro_rules! job {
            ($call:expr) => {
                match $call.await {
                    Ok(reply) => reply,
                    Err(error) => error.into_reply(),
                }
            };
        }
        match request.operation {
            Operation::HomeGet => op::get_home_settings(self).await.into_reply(),
            Operation::HomeMetadata => op::get_home_metadata().into_reply(),
            Operation::HomeUpdate => op::patch_home_settings(self, body).await,
            Operation::ThemeGet => op::get_resolved_theme(self, query()).await.into_reply(),
            Operation::ThemeList => op::list_available_themes(self).await.into_reply(),
            Operation::ShellProfilesGet => op::get_shell_profiles(self).await.into_reply(),
            Operation::ShellProfilesUpdate => op::put_shell_profiles(self, body!()).await,
            Operation::TemplateExtract => op::extract_template(self, body!()).await,
            Operation::TemplateList => op::list_templates(self).await,
            Operation::TemplateGet => op::get_template(self, slug).await,
            Operation::TemplateSave => op::save_template(self, slug, body).await,
            Operation::TemplateCreate => op::create_template(self, body).await,
            Operation::TemplateDelete => op::delete_template(self, slug).await,
            Operation::BuildGet => op::get_build(self).await.into_reply(),
            Operation::CommandSend => op::post_command(self, body!()).await.into_reply(),
            Operation::MuxState => op::get_mux_state(self).await.into_reply(),
            Operation::MuxScreens => op::get_mux_screens(self).await.into_reply(),
            Operation::MuxScreen => op::get_mux_screen(self, id).await,
            Operation::MuxOp => op::post_mux_op(self, body!()).await,
            Operation::MuxNeighbor => op::get_mux_neighbor(self, query()).await,
            Operation::MuxStatus => op::get_mux_status(self).await.into_reply(),
            Operation::SessionSend => op::post_session_send(self, id, body!()).await,
            Operation::EditorOpen => op::post_editor_open(self, body!()).await,
            Operation::FileOffer => op::post_file_offer(self, body!()).await,
            Operation::ViewerComments => review::get_viewer_comments(self, query()).await,
            Operation::ViewerCommentReply => review::post_comment_reply(self, body!()).await,
            Operation::ViewerReviewWait => review::post_review_wait(self, body!()).await,
            Operation::JobList => job!(jobs.list()),
            Operation::JobLaunch => job!(jobs.launch(body!())),
            Operation::JobInspect => job!(jobs.inspect(id)),
            Operation::JobInput => job!(jobs.input(id, body!())),
            Operation::JobStop => job!(jobs.stop(id)),
            Operation::JobForget => job!(jobs.forget(id)),
            Operation::JobLogs => {
                let args = match decode(Value::Object(request.args), style) {
                    Ok(args) => args,
                    Err(error) => return error,
                };
                job!(jobs.logs(id, args))
            }
            Operation::HostList => Payload(hub.known_hosts()).into_reply(),
            Operation::HostForget => {
                op::host_preference_response(hub.forget_host(body).map(|()| None))
            }
            Operation::HostPreferencesGet => Payload(hub.host_preferences()).into_reply(),
            Operation::HostPreferencesUpdate => {
                op::host_preference_response(hub.update_host_preference(body).map(Some))
            }
            Operation::HostOrderGet => Payload(hub.host_order()).into_reply(),
            Operation::HostOrderUpdate => match hub.set_host_order(body) {
                Ok(order) => Payload(order).into_reply(),
                Err(error) => op::host_preference_response(Err(error)),
            },
            Operation::RemoteScreensGet => Payload(hub.remembered_screens()).into_reply(),
            Operation::RemoteScreensUpdate => op::remember_screens(hub, body),
            Operation::RemoteScreensUpdateHost => op::remember_host_screens(hub, body),
            Operation::HostDirectory => Payload(hub.directory()).into_reply(),
            Operation::SshPrompts => Payload(splitty_remote::askpass::prompts()).into_reply(),
            Operation::SshAnswer => Payload(
                serde_json::json!({"accepted": splitty_remote::askpass::respond_document(&body)}),
            )
            .into_reply(),
            Operation::CatalogGet => Payload(hub.catalog()).into_reply(),
            Operation::CatalogUpdate => op::update_catalog(hub, body),
            Operation::ForwardList => Payload(hub.forwards()).into_reply(),
            Operation::ForwardOpen => op::open_forward(hub, body).await,
            Operation::ForwardStop => op::stop_forward(hub, body).await,
            Operation::ForwardPreferencesGet => Payload(hub.forward_preferences()).into_reply(),
            Operation::ForwardPreferencesSave => op::save_forward(hub, body).await,
            Operation::ForwardPreferencesForget => op::forget_forward(hub, body).await,
            Operation::ReverseList => Payload(hub.reverses()).into_reply(),
            Operation::ReverseOpen => op::open_reverse(hub, body).await,
            Operation::ReverseStop => op::stop_reverse(hub, body).await,
            Operation::ReversePreferencesGet => Payload(hub.reverse_preferences()).into_reply(),
            Operation::ReversePreferencesSave => op::save_reverse(hub, body).await,
            Operation::ReversePreferencesForget => op::forget_reverse(hub, body).await,
        }
    }
}
