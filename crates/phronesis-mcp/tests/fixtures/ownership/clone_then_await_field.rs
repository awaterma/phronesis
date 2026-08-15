// Core case 4: an async fn that clones a struct field and later awaits.

struct Config {
    value: String,
}

async fn clone_field_then_await(cfg: &Config) -> String {
    let cloned = cfg.value.clone();
    let _result = heavy_computation(&cloned).await;
    cloned
}

async fn heavy_computation(_s: &str) -> String {
    String::new()
}
