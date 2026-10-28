use std::{fs, path::Path};

fn main() -> std::io::Result<()> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("broker API crate must be inside the workspace crates directory");
    let directory = root.join("web").join("src").join("api").join("generated");
    fs::create_dir_all(&directory)?;
    fs::write(
        directory.join("index.ts"),
        azbridge_broker_api::typescript_bindings(),
    )
}
