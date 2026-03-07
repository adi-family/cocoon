use std::path::PathBuf;
use typespec_api::codegen::{
    protocol::RustProtocolConfig,
    rust::RustAdiServiceConfig,
    Generator, Language, Side,
};

fn main() {
    println!("cargo:rerun-if-changed=../cocoon.tsp");
    println!("cargo:rerun-if-changed=../../credentials/api.tsp");
    println!("cargo:rerun-if-changed=build.rs");

    let out_dir = std::env::var("OUT_DIR").unwrap();

    // ── Cocoon protocol (signaling messages) ──
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

    let src_dir = proto_dir.join("src");
    let glue = format!(
        "#[path = \"{dir}/types.rs\"]\npub mod types;\n\n#[path = \"{dir}/messages.rs\"]\npub mod messages;\n",
        dir = src_dir.display()
    );
    std::fs::write(format!("{out_dir}/generated_protocol.rs"), glue).unwrap();

    // ── Credentials AdiService (types + handler trait + dispatch) ──
    let cred_source = std::fs::read_to_string("../../credentials/api.tsp").expect("read credentials/api.tsp");
    let cred_file = typespec_api::parse(&cred_source).expect("parse credentials/api.tsp");

    let cred_types_dir = PathBuf::from(&out_dir).join("credentials_types");
    Generator::new(&cred_file, &cred_types_dir, "credentials")
        .generate(Language::Rust, Side::Types)
        .expect("credentials types codegen failed");

    let cred_adi_dir = PathBuf::from(&out_dir).join("credentials_adi");
    let adi_config = RustAdiServiceConfig {
        types_crate: "credentials_types".to_string(),
        service_id: "credentials".to_string(),
        service_name: "Credentials".to_string(),
        service_version: env!("CARGO_PKG_VERSION").to_string(),
    };

    Generator::new(&cred_file, &cred_adi_dir, "credentials")
        .with_rust_adi_config(adi_config)
        .generate(Language::Rust, Side::AdiService)
        .expect("credentials adi codegen failed");

    let cred_types_src = cred_types_dir.join("src");
    let cred_adi_src = cred_adi_dir.join("src");

    // Patch imports for embedding (standalone crate paths → sibling module paths)
    let models_path = cred_types_src.join("models.rs");
    let models_content = std::fs::read_to_string(&models_path).unwrap();
    std::fs::write(&models_path, models_content.replace("use crate::enums", "use super::enums")).unwrap();

    let adi_path = cred_adi_src.join("adi_service.rs");
    let adi_content = std::fs::read_to_string(&adi_path).unwrap();
    let adi_content = adi_content
        .replace(
            "use credentials_types::models::*;",
            "use super::models::*;\nuse chrono::{DateTime, Utc};\nuse std::collections::HashMap;\nuse uuid::Uuid;",
        )
        .replace("use credentials_types::enums::*;", "use super::enums::*;");
    std::fs::write(&adi_path, adi_content).unwrap();

    let cred_glue = format!(
        concat!(
            "#[path = \"{types_dir}/enums.rs\"]\npub mod enums;\n\n",
            "#[path = \"{types_dir}/models.rs\"]\npub mod models;\n\n",
            "#[path = \"{adi_dir}/adi_service.rs\"]\npub mod adi_service;\n",
        ),
        types_dir = cred_types_src.display(),
        adi_dir = cred_adi_src.display(),
    );
    std::fs::write(format!("{out_dir}/generated_credentials.rs"), cred_glue).unwrap();
}
