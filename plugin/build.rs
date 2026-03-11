fn main() {
    lib_plugin_web_build::PluginWebBuild::new()
        .tsp_path("../cocoon.tsp")
        .run();
}
