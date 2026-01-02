//! Cocoon Plugin
//!
//! Remote containerized worker with PTY support and signaling server connectivity.

mod core;

pub use core::run;

use abi_stable::std_types::{ROption, RResult, RStr, RString, RVec};
use lib_plugin_abi::{
    PluginContext, PluginInfo, PluginVTable, ServiceDescriptor, ServiceError,
    ServiceHandle, ServiceMethod, ServiceVTable, ServiceVersion,
};
use std::ffi::c_void;

/// Plugin-specific CLI service ID
const SERVICE_CLI: &str = "adi.cocoon.cli";

// === CLI Service Implementation ===

extern "C" fn cli_invoke(
    _handle: *const c_void,
    method: RStr<'_>,
    args: RStr<'_>,
) -> RResult<RString, ServiceError> {
    match method.as_str() {
        "run_command" => {
            // Parse the args JSON (context from CLI host)
            let context: serde_json::Value = match serde_json::from_str(args.as_str()) {
                Ok(v) => v,
                Err(e) => {
                    return RResult::RErr(ServiceError::new(1, format!("Invalid args: {}", e)))
                }
            };

            let cmd_args: Vec<String> = context
                .get("args")
                .and_then(|v| v.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|v| v.as_str().map(String::from))
                        .collect()
                })
                .unwrap_or_default();

            let subcommand = cmd_args.first().map(|s| s.as_str()).unwrap_or("run");

            match subcommand {
                "start" => {
                    let mode = cmd_args.get(1).map(|s| s.as_str()).unwrap_or("native");

                    match mode {
                        "docker" => {
                            // Get environment variables
                            let signaling_url = std::env::var("SIGNALING_SERVER_URL")
                                .unwrap_or_else(|_| "ws://localhost:8080/ws".to_string());
                            let cocoon_secret = std::env::var("COCOON_SECRET").ok();
                            let setup_token = std::env::var("COCOON_SETUP_TOKEN").ok();

                            // Build docker run command
                            let mut docker_cmd = std::process::Command::new("docker");
                            docker_cmd.arg("run")
                                .arg("--rm")
                                .arg("-it")
                                .arg("-e").arg(format!("SIGNALING_SERVER_URL={}", signaling_url))
                                .arg("-v").arg("cocoon-data:/cocoon");

                            if let Some(secret) = cocoon_secret {
                                docker_cmd.arg("-e").arg(format!("COCOON_SECRET={}", secret));
                            }

                            if let Some(token) = setup_token {
                                docker_cmd.arg("-e").arg(format!("COCOON_SETUP_TOKEN={}", token));
                            }

                            docker_cmd.arg("ghcr.io/adi-family/cocoon:latest");

                            println!("Starting cocoon in Docker container...");
                            println!("Command: docker run --rm -it -e SIGNALING_SERVER_URL={} -v cocoon-data:/cocoon ghcr.io/adi-family/cocoon:latest", signaling_url);

                            match docker_cmd.status() {
                                Ok(status) if status.success() => {
                                    RResult::ROk(RString::from("Cocoon container exited"))
                                }
                                Ok(status) => {
                                    RResult::RErr(ServiceError::new(1, format!("Docker exited with code: {}", status)))
                                }
                                Err(e) => {
                                    RResult::RErr(ServiceError::new(1, format!("Failed to start Docker: {}. Make sure Docker is installed and running.", e)))
                                }
                            }
                        }
                        "native" | _ => {
                            // Start cocoon worker in a background task
                            std::thread::spawn(|| {
                                let rt = match tokio::runtime::Runtime::new() {
                                    Ok(rt) => rt,
                                    Err(e) => {
                                        eprintln!("Failed to create runtime: {}", e);
                                        return;
                                    }
                                };

                                rt.block_on(async {
                                    if let Err(e) = core::run().await {
                                        eprintln!("Cocoon error: {}", e);
                                    }
                                });
                            });

                            RResult::ROk(RString::from("Cocoon worker started in background"))
                        }
                    }
                }
                "run" => {
                    // Alias for "start native"
                    std::thread::spawn(|| {
                        let rt = match tokio::runtime::Runtime::new() {
                            Ok(rt) => rt,
                            Err(e) => {
                                eprintln!("Failed to create runtime: {}", e);
                                return;
                            }
                        };

                        rt.block_on(async {
                            if let Err(e) = core::run().await {
                                eprintln!("Cocoon error: {}", e);
                            }
                        });
                    });

                    RResult::ROk(RString::from("Cocoon worker started in background"))
                }
                "help" | _ => {
                    let help_text = r#"Cocoon - Remote containerized worker

USAGE:
    adi cocoon [COMMAND]

COMMANDS:
    run                 Start the cocoon worker natively (default)
    start [MODE]        Start cocoon worker
        native          Start natively (default)
        docker          Start in Docker container
    help                Show this help message

ENVIRONMENT VARIABLES:
    SIGNALING_SERVER_URL    WebSocket URL (default: ws://localhost:8080/ws)
    COCOON_SECRET           Pre-generated secret for persistent device ID
    COCOON_SETUP_TOKEN      Setup token for auto-claim
    COCOON_NAME             Optional name for this cocoon instance

EXAMPLES:
    adi cocoon run
    adi cocoon start native
    adi cocoon start docker
    SIGNALING_SERVER_URL=wss://example.com/ws adi cocoon start docker
    COCOON_SETUP_TOKEN=<token> adi cocoon start docker
"#;
                    RResult::ROk(RString::from(help_text))
                }
            }
        }
        "list_commands" => {
            let commands = serde_json::json!([
                {"name": "run", "description": "Start the cocoon worker natively", "usage": "run"},
                {"name": "start", "description": "Start cocoon worker (native or docker)", "usage": "start [native|docker]"},
                {"name": "help", "description": "Show help message", "usage": "help"}
            ]);
            RResult::ROk(RString::from(
                serde_json::to_string(&commands).unwrap_or_default(),
            ))
        }
        _ => RResult::RErr(ServiceError::method_not_found(method.as_str())),
    }
}

extern "C" fn cli_list_methods(_handle: *const c_void) -> RVec<ServiceMethod> {
    vec![
        ServiceMethod::new("run_command").with_description("Run a CLI command"),
        ServiceMethod::new("list_commands").with_description("List available commands"),
    ]
    .into_iter()
    .collect()
}

static CLI_SERVICE_VTABLE: ServiceVTable = ServiceVTable {
    invoke: cli_invoke,
    list_methods: cli_list_methods,
};

// === Plugin VTable Implementation ===

extern "C" fn plugin_info() -> PluginInfo {
    PluginInfo::new("adi.cocoon", "Cocoon", env!("CARGO_PKG_VERSION"), "core")
        .with_author("ADI Team")
        .with_description("Remote containerized worker with PTY support")
        .with_min_host_version("0.8.0")
}

extern "C" fn plugin_init(ctx: *mut PluginContext) -> i32 {
    unsafe {
        let host = (*ctx).host();

        // Register CLI commands service
        let cli_descriptor = ServiceDescriptor::new(
            SERVICE_CLI,
            ServiceVersion::new(1, 0, 0),
            "adi.cocoon",
        )
        .with_description("CLI commands for cocoon worker");

        let cli_handle = ServiceHandle::new(
            SERVICE_CLI,
            ctx as *const c_void,
            &CLI_SERVICE_VTABLE as *const ServiceVTable,
        );

        if let Err(code) = host.register_svc(cli_descriptor, cli_handle) {
            host.error(&format!(
                "Failed to register CLI commands service: {}",
                code
            ));
            return code;
        }

        host.info("Cocoon plugin initialized");
    }

    0
}

extern "C" fn plugin_cleanup(_ctx: *mut PluginContext) {}

// === Plugin Entry Point ===

static PLUGIN_VTABLE: PluginVTable = PluginVTable {
    info: plugin_info,
    init: plugin_init,
    update: ROption::RNone,
    cleanup: plugin_cleanup,
    handle_message: ROption::RNone,
};

#[no_mangle]
pub extern "C" fn plugin_entry() -> *const PluginVTable {
    &PLUGIN_VTABLE
}
