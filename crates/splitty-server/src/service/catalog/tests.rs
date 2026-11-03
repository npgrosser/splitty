use super::*;
use serde_json::json;
use std::collections::BTreeSet;
use syn::visit::{self, Visit};

#[derive(Default)]
struct Routes(BTreeSet<(String, String)>);

fn methods(expression: &syn::Expr, found: &mut Vec<String>) {
    match expression {
        syn::Expr::MethodCall(call) => {
            methods(&call.receiver, found);
            found.push(call.method.to_string().to_uppercase());
        }
        syn::Expr::Call(call) => {
            let syn::Expr::Path(path) = &*call.func else {
                panic!("unexpected routing expression")
            };
            found.push(
                path.path
                    .segments
                    .last()
                    .expect("routing method")
                    .ident
                    .to_string()
                    .to_uppercase(),
            );
        }
        _ => panic!("route methods must be explicitly classified"),
    }
}

impl<'ast> Visit<'ast> for Routes {
    fn visit_expr_method_call(&mut self, call: &'ast syn::ExprMethodCall) {
        if call.method == "route" {
            let syn::Expr::Lit(path) = &call.args[0] else {
                panic!("route path must be classified")
            };
            let syn::Lit::Str(path) = &path.lit else {
                panic!("route path must be a string")
            };
            let mut verbs = Vec::new();
            methods(&call.args[1], &mut verbs);
            for method in verbs {
                self.0.insert((method, path.value()));
            }
        }
        visit::visit_expr_method_call(self, call);
    }
}

#[test]
fn every_registered_method_and_path_has_an_explicit_classification() {
    let mut routes = Routes::default();
    for source in [
        include_str!("../../http/routes.rs"),
        include_str!("../../http/jobs.rs"),
    ] {
        routes.visit_file(&syn::parse_file(source).expect("valid Rust routes"));
    }
    let classified: BTreeSet<_> = OPERATIONS
        .iter()
        .map(|entry| (entry.method.to_owned(), entry.path.to_owned()))
        .chain(
            ADAPTER_ROUTES
                .iter()
                .map(|(method, path)| ((*method).into(), (*path).into())),
        )
        .collect();
    assert_eq!(
        routes.0, classified,
        "a route was added/removed without reviewing native authority"
    );
    assert_eq!(
        classified.len(),
        OPERATIONS.len() + ADAPTER_ROUTES.len(),
        "duplicate mapping"
    );
}

#[test]
fn published_catalog_matches_the_checked_policy() {
    let published: Value =
        serde_json::from_str(include_str!("../../../../../docs/native-operations.json"))
            .expect("catalog fixture");
    let catalog: Vec<_> = OPERATIONS.iter().map(|entry| {
        assert_eq!(entry.name, entry.operation.description().name);
        json!({
            "operation": entry.name, "method": entry.method, "path": entry.path,
            "scope": match entry.scope { Scope::Backend => "backend", Scope::LocalHub => "localHub" },
            "arguments": entry.arguments, "maxBodyBytes": entry.body_limit,
            "cancellation": match entry.cancellation {
                Cancellation::Read => "read", Cancellation::Mutation => "mutation", Cancellation::Registration => "registration",
            },
            "deadlineMs": entry.deadline.duration().map(|duration| duration.as_millis()),
        })
    }).collect();
    assert_eq!(
        published,
        json!({"operations": catalog, "sessionOperations": SESSION_OPERATIONS})
    );
}

#[test]
fn credentials_administration_and_generic_proxy_are_not_native_operations() {
    for name in [
        "fetch",
        "connect",
        "spawn",
        "open",
        "bootstrap",
        "ws-ticket",
        "agent.list",
        "job.owner",
        "file.readLocal",
    ] {
        let request = json!({"operation": name, "args": {"url": "http://127.0.0.1/"}});
        assert!(
            Request::parse(&serde_json::to_vec(&request).expect("JSON"), Role::Main).is_err(),
            "{name}"
        );
    }
    for field in [
        "url",
        "path",
        "method",
        "headers",
        "socket",
        "authorization",
        "role",
        "backend",
        "window",
    ] {
        for in_args in [false, true] {
            let mut request = json!({"operation": "mux.state", "args": {}});
            if in_args {
                request["args"][field] = json!("attacker-chosen");
            } else {
                request[field] = json!("attacker-chosen");
            }
            assert!(
                Request::parse(&serde_json::to_vec(&request).expect("JSON"), Role::Main).is_err(),
                "{field}, in_args={in_args}"
            );
        }
    }
}

#[test]
fn remote_roles_cannot_use_the_assigned_backend_to_obtain_hub_authority() {
    for description in OPERATIONS
        .iter()
        .filter(|entry| entry.scope == Scope::LocalHub)
    {
        let request = Request {
            operation: description.operation,
            args: Map::new(),
        };
        assert_eq!(request.validate(Role::Remote), Err(Rejection::Forbidden));
    }
    let request = br#"{"operation":"session.send","args":{"id":"terminal-1","body":{"data":"printf hello\n"}}}"#;
    assert!(
        Request::parse(request, Role::Remote).is_ok(),
        "intended terminal authority stays available on its assigned service"
    );
}

#[test]
fn argument_shapes_and_existing_smaller_job_limits_are_enforced() {
    assert!(Request::parse(br#"{"operation":"mux.screen"}"#, Role::Main).is_err());
    assert!(Request::parse(
        br#"{"operation":"mux.screen","args":{"id":null}}"#,
        Role::Main
    )
    .is_err());
    assert!(Request::parse(
        br#"{"operation":"theme.get","args":{"name":"dark"}}"#,
        Role::Main
    )
    .is_ok());
    let request = serde_json::to_vec(&json!({"operation":"job.input","args":{"id":"job-1","body":{"data":"x".repeat(MAX_JOB_BODY)}}})).expect("JSON");
    assert!(matches!(
        Request::parse(&request, Role::Main),
        Err(Rejection::TooLarge)
    ));
    assert!(matches!(
        Request::parse(&vec![b' '; MAX_API_BODY + 1], Role::Main),
        Err(Rejection::TooLarge)
    ));
}

#[test]
fn session_catalog_covers_core_requests_and_rejects_authority_injection() {
    let file = syn::parse_file(include_str!(
        "../../../../splitty-core/src/protocol/messages.rs"
    ))
    .expect("core protocol source");
    for item in file.items {
        let syn::Item::Const(item) = item else {
            continue;
        };
        if !item.ident.to_string().starts_with("MSG_") {
            continue;
        }
        let syn::Expr::Lit(value) = *item.expr else {
            panic!("message type must be explicit")
        };
        let syn::Lit::Str(value) = value.lit else {
            panic!("message type must be a string")
        };
        assert!(
            SESSION_OPERATIONS.contains(&value.value().as_str()),
            "missing {}",
            value.value()
        );
    }
    assert!(
        validate_session_test(br#"{"type":"input","id":"terminal-1","data":"\u0000ls\n"}"#).is_ok()
    );
    assert!(validate_session_test(
        br#"{"type":"input","id":"terminal-1","url":"http://127.0.0.1/"}"#
    )
    .is_err());
    assert!(validate_session_test(br#"{"type":"remote.connect","target":"host"}"#).is_err());
}

fn validate_session_test(
    bytes: &[u8],
) -> Result<splitty_transport::budget::Charged<Value>, Rejection> {
    validate_session(bytes, &Default::default())
}
