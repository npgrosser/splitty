//! The HTTP form of a catalog operation, shared by the browser gateway and the
//! owner control socket: query and path arguments, the JSON body and the
//! catalog's validation turn one request into a native request. Both
//! transports therefore answer a malformed request alike.

use std::collections::HashMap;

use axum::{
    body::{Body, Bytes},
    extract::{FromRequestParts, Path, Query},
    http::{header::CONTENT_TYPE, request::Parts, HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use serde_json::{json, Value};
use splitty_transport::{
    budget::{Charged, ConnectionBudget},
    wire,
};

use crate::service::catalog::{self, Description, Operation, Rejection, Role};

pub(super) fn rejected(
    status: StatusCode,
    message: impl std::fmt::Display,
    uncertain: bool,
) -> Response {
    (
        status,
        Json(json!({"error":message.to_string(),"uncertain":uncertain})),
    )
        .into_response()
}

/// A body operation takes only a JSON media type: `application/json` or an
/// `application/*+json` type, with or without parameters. No API body can then
/// be sent as a request a browser makes across origins without a preflight.
/// Checked before the body is read.
pub(super) fn refuse_content_type(
    description: &Description,
    headers: &HeaderMap,
) -> Option<Response> {
    if !description.takes_body() || json_content_type(headers) {
        return None;
    }
    Some(rejected(
        StatusCode::UNSUPPORTED_MEDIA_TYPE,
        "expected a request body with Content-Type: application/json",
        false,
    ))
}

fn json_content_type(headers: &HeaderMap) -> bool {
    let Some(value) = headers
        .get(CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
    else {
        return false;
    };
    let essence = value
        .split(';')
        .next()
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase();
    let Some(subtype) = essence.strip_prefix("application/") else {
        return false;
    };
    subtype == "json" || (subtype.ends_with("+json") && subtype.len() > "+json".len())
}

/// Admit URI decoding before Axum/serde can allocate maps and decoded strings.
/// Route argument counts are fixed by the catalog, never by query field names.
pub(super) fn parse_query(
    uri: &axum::http::Uri,
    allowed: &[&str],
    budget: &ConnectionBudget,
) -> Result<Charged<HashMap<String, String>>, Box<Response>> {
    let query = uri.query().unwrap_or_default();
    let count = query.split('&').filter(|field| !field.is_empty()).count();
    if count > allowed.len() {
        return Err(Box::new(rejected(
            StatusCode::BAD_REQUEST,
            "too many query arguments",
            false,
        )));
    }
    // The real HTTP listener already has a 64 KiB header limit. Keep that bound
    // here too for in-process adapters before any URI-dependent allocation.
    let length = uri.path().len().saturating_add(query.len());
    if length > 64 * 1024 {
        return Err(Box::new(rejected(
            StatusCode::URI_TOO_LONG,
            "URI exceeds parser limit",
            false,
        )));
    }
    // Covers percent-decoder scratch, owned path/query strings, Vec/HashMap
    // growth and the later JSON object nodes while the original maps move out.
    let bytes = 4 * length + 4096 * (allowed.len() + 1);
    let lease = budget.reserve(bytes, false).map_err(|_| {
        Box::new(rejected(
            StatusCode::SERVICE_UNAVAILABLE,
            "parameter capacity exhausted",
            false,
        ))
    })?;
    let Query(fields) = Query::<Vec<(String, String)>>::try_from_uri(uri).map_err(|_| {
        Box::new(rejected(
            StatusCode::BAD_REQUEST,
            "invalid query arguments",
            false,
        ))
    })?;
    let mut params = HashMap::with_capacity(fields.len());
    for (key, value) in fields {
        if key == "body" || !allowed.contains(&key.as_str()) || params.insert(key, value).is_some()
        {
            return Err(Box::new(rejected(
                StatusCode::BAD_REQUEST,
                "unknown or duplicate query argument",
                false,
            )));
        }
    }
    Ok(lease.charge(params))
}

/// The query arguments of `description`'s request.
pub(super) fn query_arguments(
    description: &Description,
    uri: &axum::http::Uri,
    budget: &ConnectionBudget,
) -> Result<Charged<HashMap<String, String>>, Box<Response>> {
    let names: Vec<&str> = description
        .arguments
        .iter()
        .map(|argument| argument.name)
        .collect();
    parse_query(uri, &names, budget)
}

/// The path arguments of the matched catalog route.
pub(super) async fn path_arguments(
    parts: &mut Parts,
) -> Result<HashMap<String, String>, Box<Response>> {
    match Path::<HashMap<String, String>>::from_request_parts(parts, &()).await {
        Ok(Path(path)) => Ok(path),
        Err(_) => Err(Box::new(rejected(
            StatusCode::BAD_REQUEST,
            "invalid path arguments",
            false,
        ))),
    }
}

/// Bodyless catalog operations need no maximum-sized JSON allocation.
pub(super) fn body_limit(description: &Description) -> usize {
    if description.takes_body() {
        description.body_limit
    } else {
        0
    }
}

/// Read at most `limit` bytes of body within the frame timeout.
pub(super) async fn read_body(body: Body, limit: usize) -> Result<Bytes, Box<Response>> {
    match tokio::time::timeout(wire::FRAME_TIMEOUT, axum::body::to_bytes(body, limit)).await {
        Err(_) => Err(Box::new(rejected(
            StatusCode::REQUEST_TIMEOUT,
            "request body timed out",
            false,
        ))),
        Ok(Ok(bytes)) => Ok(bytes),
        Ok(Err(error)) => Err(Box::new(rejected(
            if limit == 0 {
                StatusCode::BAD_REQUEST
            } else {
                StatusCode::PAYLOAD_TOO_LARGE
            },
            error,
            false,
        ))),
    }
}

/// The native request of `operation` from its path and query arguments and
/// body, validated for `role`.
pub(super) fn native_request(
    operation: Operation,
    path: HashMap<String, String>,
    query: HashMap<String, String>,
    bytes: &[u8],
    role: Role,
) -> Result<catalog::Request, Box<Response>> {
    let description = operation.description();
    let mut args = serde_json::Map::new();
    for (name, value) in path.into_iter().chain(query) {
        if args.insert(name, Value::String(value)).is_some() {
            return Err(Box::new(rejected(
                StatusCode::BAD_REQUEST,
                "duplicate argument",
                false,
            )));
        }
    }
    if description.takes_body() {
        let body = serde_json::from_slice(bytes)
            .map_err(|error| Box::new(rejected(StatusCode::UNPROCESSABLE_ENTITY, error, false)))?;
        args.insert("body".into(), body);
    } else if !bytes.is_empty() {
        return Err(Box::new(rejected(
            StatusCode::BAD_REQUEST,
            "unexpected request body",
            false,
        )));
    }
    let request = catalog::Request { operation, args };
    if let Err(error) = request.validate(role) {
        let status = match error {
            Rejection::Forbidden => StatusCode::FORBIDDEN,
            Rejection::TooLarge => StatusCode::PAYLOAD_TOO_LARGE,
            Rejection::Invalid(_) => StatusCode::BAD_REQUEST,
        };
        return Err(Box::new(rejected(status, error, false)));
    }
    Ok(request)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_json_media_types_carry_a_body() {
        let accepted = |value: Option<&str>| {
            let mut headers = HeaderMap::new();
            if let Some(value) = value {
                headers.insert(CONTENT_TYPE, value.parse().expect("header"));
            }
            json_content_type(&headers)
        };
        for value in [
            "application/json",
            "application/json; charset=utf-8",
            "Application/JSON",
            "application/merge-patch+json",
        ] {
            assert!(accepted(Some(value)), "{value}");
        }
        for value in [
            "text/plain",
            "text/plain; charset=utf-8",
            "application/x-www-form-urlencoded",
            "multipart/form-data; boundary=x",
            "application/jsonp",
            "application/+json",
            "text/json",
            "",
        ] {
            assert!(!accepted(Some(value)), "{value}");
        }
        assert!(!accepted(None));
    }
}
