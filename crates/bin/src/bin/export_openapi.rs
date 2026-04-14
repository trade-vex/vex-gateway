fn main() {
    let document = lib_server::openapi_document();
    println!(
        "{}",
        serde_json::to_string_pretty(&document).expect("failed to serialize OpenAPI document")
    );
}
