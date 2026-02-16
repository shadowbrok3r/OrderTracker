use std::{collections::HashMap, env, path::PathBuf};

fn main() {
    let manifest_dir = env::var("CARGO_MANIFEST_DIR").unwrap();
    let env_path = PathBuf::from(&manifest_dir).join(".env");

    println!("cargo:rerun-if-changed={}", env_path.display());

    let mut vars: HashMap<String, String> = HashMap::new();
    if env_path.exists() {
        for item in dotenvy::from_path_iter(&env_path).expect("Failed to read .env file") {
            let (key, val) = item.expect("Failed to parse .env entry");
            vars.insert(key, val);
        }
    } else {
        eprintln!("Warning: .env file not found at {}", env_path.display());
    }
    // Etsy: ETSY_SECRET = app shared secret for x-api-key header; optional when using only OAuth refresh token
    vars.entry("ETSY_SECRET".to_string()).or_insert_with(String::new);

    for (key, val) in vars {
        println!("cargo:rustc-env={}={}", key, val);
    }
}
