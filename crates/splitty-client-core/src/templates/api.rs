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
        TemplateApiRequest::Save {
            draft,
            name,
            included_commands,
            overwrite_confirmed,
        } => {
            let _scratch = draft.reserve_copy()?;
            let mut template = draft.template().clone();
            template.name = name.trim().to_owned();
            template.retain_commands(|pane| included_commands.iter().any(|name| name == pane));
            validate(&template)?;
            if serialized_size(&template)? > MAX_TEMPLATE_BYTES {
                return Err(invalid("Saved template exceeds byte limit"));
            }
            // The service names the file and checks and saves in one step.
            let request = client.request(
                Operation::TemplateCreate,
                args(json!({"body":{"template":template,"overwrite":overwrite_confirmed}})),
            );
            let reply =
                tokio::select! {biased; _=cancel.notified()=>None, result=request=>result.ok()};
            let uncertain = || TemplateApiOutcome::Uncertain {
                reason: "Template save outcome is unknown; reload before deciding again".into(),
            };
            let Some(reply) = reply.filter(|reply| !reply.uncertain) else {
                return Ok(uncertain());
            };
            let file_name = |reply: &splitty_transport::wire::Reply| {
                serde_json::from_slice::<Value>(reply.body.as_bytes())
                    .ok()
                    .and_then(|body| body["slug"].as_str().map(str::to_owned))
                    .filter(|value| slug(value).is_ok())
            };
            match reply.status {
                200..300 => Ok(file_name(&reply)
                    .map_or_else(uncertain, |slug| TemplateApiOutcome::Saved { slug })),
                409 => Ok(TemplateApiOutcome::OverwriteRequired {
                    slug: file_name(&reply)
                        .ok_or_else(|| invalid("Template exists under an invalid file name"))?,
                }),
                status => Err(io::Error::other(
                    serde_json::from_slice::<Value>(reply.body.as_bytes())
                        .ok()
                        .and_then(|body| body["error"].as_str().map(str::to_owned))
                        .filter(|error| error.len() <= 512)
                        .unwrap_or_else(|| format!("Template save rejected (status {status})")),
                )),
            }
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
