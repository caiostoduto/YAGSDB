#[path = "src/config.rs"]
mod config;

use std::fs::File;
use std::io::Write;
use std::path::PathBuf;

fn main() {
    println!("cargo:rerun-if-changed=src/config.rs");

    // Generate schema from Config struct
    let schema = schemars::schema_for!(config::Config);
    let schema_json = serde_json::to_string_pretty(&schema).expect("Failed to serialize schema");

    // Get target directory
    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR not set");
    let target_dir = PathBuf::from(manifest_dir).join("target");

    // Create target directory if it doesn't exist
    if !target_dir.exists() {
        std::fs::create_dir_all(&target_dir).expect("Failed to create target directory");
    }

    // Create schema file
    let schema_path = target_dir.join("config-schema.json");
    let mut file = File::create(&schema_path).expect("Failed to create schema file");
    file.write_all(schema_json.as_bytes())
        .expect("Failed to write schema");
}
