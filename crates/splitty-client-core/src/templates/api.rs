use super::*;
use crate::api_stream::ApiStream as Client;
use crate::local_service::decode;
use serde_json::{json, Map, Value};
use splitty_core::native_api::Operation;
use tokio::sync::Notify;

pub(crate) async fn execute(
    client: &mut Client,
    owner: Uuid,
    id: Uuid,
    request: TemplateApiRequest,
    cancel: &Notify,
) -> TemplateApiResult {
    let outcome = inner(client, owner, id, request, cancel)
        .await
        .unwrap_or_else(|error| TemplateApiOutcome::Rejected {
            reason: error.to_string(),
        });
    TemplateApiResult {
        connection_id: owner,
        request_id: id,
        outcome,
    }
}
fn args(value: Value) -> Map<String, Value> {
    value.as_object().expect("object literal").clone()
}
async fn read(
    client: &mut Client,
    op: Operation,
    args: Map<String, Value>,
    cancel: &Notify,
) -> io::Result<Charged<splitty_transport::wire::Reply>> {
    tokio::select! {biased; _=cancel.notified()=>Err(invalid("Template request cancelled")),result=client.request(op,args)=>{let reply=result?;if reply.uncertain{Err(invalid("Template read outcome unavailable"))}else{Ok(reply)}}}
}
async fn mutation(
    client: &mut Client,
    op: Operation,
    args: Map<String, Value>,
    cancel: &Notify,
) -> io::Result<bool> {
    let result = tokio::select! {biased; _=cancel.notified()=>return Ok(false),result=client.request(op,args)=>result};
    match result {
        Err(_) => Ok(false),
        Ok(reply) if reply.uncertain => Ok(false),
        Ok(reply) if (200..300).contains(&reply.status) => Ok(true),
        Ok(reply) => Err(io::Error::other(format!(
            "Template mutation rejected (status {})",
            reply.status
        ))),
    }
}
async fn inner(
    client: &mut Client,
    owner: Uuid,
    id: Uuid,
    request: TemplateApiRequest,
    cancel: &Notify,
) -> io::Result<TemplateApiOutcome> {
    request.validate(owner)?;
    match request {
        TemplateApiRequest::List => {
            let reply = read(client, Operation::TemplateList, Map::new(), cancel).await?;
            let mut entries = decode::<Vec<TemplateSummary>>(client, &reply)?;
            if entries.len() > 256
                || entries
                    .iter()
                    .any(|s| slug(&s.slug).is_err() || s.display_name.len() > 512)
            {
                return Err(invalid("Template catalog exceeds native limits"));
            }
            entries.value.sort_by(|a, b| {
                a.display_name
                    .cmp(&b.display_name)
                    .then(a.slug.cmp(&b.slug))
            });
            Ok(TemplateApiOutcome::Catalog(Arc::new(
                TemplateCatalogSnapshot {
                    connection_id: owner,
                    request_id: id,
                    entries,
                },
            )))
        }
        TemplateApiRequest::Load { slug } => {
            let reply = read(
                client,
                Operation::TemplateGet,
                args(json!({"slug":slug})),
                cancel,
            )
            .await?;
            draft(client, owner, id, &reply)
        }
        TemplateApiRequest::Extract {
            screen_id,
            name,
            root,
        } => {
            // Empty extraction names are UI drafts, while shared applicability
            // requires a name. Supply a stable draft label without changing storage.
            let name = if name.trim().is_empty() {
                "Untitled".to_owned()
            } else {
                name
            };
            let reply = read(
                client,
                Operation::TemplateExtract,
                args(json!({"body":{"screenId":screen_id,"name":name,"root":root}})),
                cancel,
            )
            .await?;
            draft(client, owner, id, &reply)
        }
        TemplateApiRequest::Save {
            draft,
            slug,
            name,
            included_commands,
            overwrite_confirmed,
        } => {
            let existing = read(
                client,
                Operation::TemplateGet,
                args(json!({"slug":slug})),
                cancel,
            )
            .await?;
            if (200..300).contains(&existing.status) && !overwrite_confirmed {
                return Ok(TemplateApiOutcome::OverwriteRequired { slug });
            }
            if existing.status != 404 && !(200..300).contains(&existing.status) {
                return Err(invalid("Could not check existing template; nothing saved"));
            }
            let _scratch = draft.reserve_copy()?;
            let mut template = draft.template().clone();
            template.name = name.trim().to_owned();
            template.retain_commands(|pane| included_commands.iter().any(|name| name == pane));
            validate(&template)?;
            if serialized_size(&template)? > MAX_TEMPLATE_BYTES {
                return Err(invalid("Saved template exceeds byte limit"));
            }
            let confirmed = mutation(
                client,
                Operation::TemplateSave,
                args(json!({"slug":slug,"body":template})),
                cancel,
            )
            .await?;
            Ok(if confirmed {
                TemplateApiOutcome::Saved { slug }
            } else {
                TemplateApiOutcome::Uncertain {
                    reason: "Template save outcome is unknown; reload before deciding again".into(),
                }
            })
        }
        TemplateApiRequest::Delete { slug } => {
            let confirmed = mutation(
                client,
                Operation::TemplateDelete,
                args(json!({"slug":slug})),
                cancel,
            )
            .await?;
            Ok(if confirmed {
                TemplateApiOutcome::Deleted { slug }
            } else {
                TemplateApiOutcome::Uncertain {
                    reason: "Template deletion outcome is unknown; reload before deciding again"
                        .into(),
                }
            })
        }
    }
}
fn draft(
    client: &Client,
    owner: Uuid,
    id: Uuid,
    reply: &splitty_transport::wire::Reply,
) -> io::Result<TemplateApiOutcome> {
    Ok(TemplateApiOutcome::Draft(Arc::new(TemplateDraft::new(
        owner,
        id,
        decode(client, reply)?,
        client.sender().context().budget.clone(),
    )?)))
}
