//! Print the generated OpenAPI 3.1 YAML to stdout.
//!
//! Run `cargo run -p piping-backend --bin dump-openapi` and redirect the output
//! to regenerate `apps/api/contracts/piping.openapi.yaml`.

use std::error::Error;
use std::io::{self, Write};

use piping_backend::api::openapi::ApiDoc;
use utoipa::OpenApi;

fn main() -> Result<(), Box<dyn Error>> {
    let doc = ApiDoc::openapi();
    // JSON-valued numeric defaults must not leak serde_json's private number tag.
    let json = serde_json::to_string(&doc)?;
    let value: serde_yml::Value = serde_yml::from_str(&json)?;
    let yaml = serde_yml::to_string(&value)?;
    io::stdout().lock().write_all(yaml.as_bytes())?;
    Ok(())
}
