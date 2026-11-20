mod router;

fn main() {
    let app = router::routes();
    println!("listening on http://localhost:8080");
    app.serve(8080);
}
