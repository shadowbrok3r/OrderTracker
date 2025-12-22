use std::{env, path::PathBuf};

fn main() {
    let manifest_dir = env::var("CARGO_MANIFEST_DIR").unwrap();
    let env_path = PathBuf::from(&manifest_dir).join(".env");

    println!("cargo:rerun-if-changed={}", env_path.display());

    if env_path.exists() {
        for item in dotenvy::from_path_iter(&env_path).expect("Failed to read .env file") {
            let (key, val) = item.expect("Failed to parse .env entry");
            println!("cargo:rustc-env={}={}", key, val);
        }
    } else {
        eprintln!("Warning: .env file not found at {}", env_path.display());
    }
}
