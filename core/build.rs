use std::path::PathBuf;
use typespec_api::codegen::{
    protocol::RustProtocolConfig,
    rust::RustAdiServiceConfig,
    Generator, Language, Side,
};

fn main() {
    println!("cargo:rerun-if-changed=../cocoon.tsp");
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

    // ── Credentials AdiService handler ──
    let cred_tsp = "../../../plugins/adi.credentials/api.tsp";
    println!("cargo:rerun-if-changed={cred_tsp}");

    if let Ok(cred_source) = std::fs::read_to_string(cred_tsp) {
        let cred_file = typespec_api::parse(&cred_source).expect("parse credentials api.tsp");

        let cred_dir = PathBuf::from(&out_dir).join("credentials_adi");
        let adi_config = RustAdiServiceConfig {
            types_crate: "credentials_core".into(),
            cocoon_crate: "crate".into(),
            service_name: "Credentials".into(),
            ..Default::default()
        };

        Generator::new(&cred_file, &cred_dir, "credentials")
            .with_rust_adi_config(adi_config)
            .generate(Language::Rust, Side::AdiService)
            .expect("credentials adi codegen failed");

        // Concatenate generated files into a single include!-able file
        let adi_src = cred_dir.join("src/adi_service.rs");
        if adi_src.exists() {
            let content = std::fs::read_to_string(&adi_src).unwrap();
            std::fs::write(format!("{out_dir}/credentials_adi_service.rs"), content).unwrap();
        }
    }
}
