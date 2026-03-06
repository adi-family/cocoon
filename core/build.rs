use std::path::PathBuf;
use typespec_api::codegen::{protocol::RustProtocolConfig, Generator, Language, Side};

fn main() {
    println!("cargo:rerun-if-changed=../cocoon.tsp");
    println!("cargo:rerun-if-changed=build.rs");

    let out_dir = std::env::var("OUT_DIR").unwrap();
    let source = std::fs::read_to_string("../cocoon.tsp").expect("read cocoon.tsp");
    let file = typespec_api::parse(&source).expect("parse cocoon.tsp");

    let proto_dir = PathBuf::from(&out_dir).join("protocol");
    let config = RustProtocolConfig {
        tag: "type".to_string(),
        rename: "snake_case".to_string(),
        enum_name: "CocoonMessage".to_string(),
    };

    Generator::new(&file, &proto_dir, "cocoon")
        .with_rust_protocol_config(config)
        .generate(Language::Rust, Side::Protocol)
        .expect("protocol codegen failed");

    // Glue file with absolute #[path] for module resolution
    let src_dir = proto_dir.join("src");
    let glue = format!(
        "#[path = \"{dir}/types.rs\"]\npub mod types;\n\n#[path = \"{dir}/messages.rs\"]\npub mod messages;\n",
        dir = src_dir.display()
    );
    std::fs::write(format!("{out_dir}/generated_protocol.rs"), glue).unwrap();
}
