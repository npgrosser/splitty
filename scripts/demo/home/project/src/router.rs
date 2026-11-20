pub struct Router {
    routes: Vec<(&'static str, fn() -> String)>,
}

pub fn routes() -> Router {
    Router {
        routes: vec![("/api/items", items), ("/healthz", health)],
    }
}

fn items() -> String {
    "[]".into()
}

fn health() -> String {
    "ok".into()
}
