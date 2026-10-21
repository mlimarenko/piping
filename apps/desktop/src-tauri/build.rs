fn main() {
    tauri_build::try_build(tauri_build::Attributes::new().app_manifest(
        tauri_build::AppManifest::new().commands(&[
            "get_app_version",
            "get_service_url",
            "get_system_language",
            "set_service_url",
            "change_service_address",
            "show_about",
            "check_for_updates",
            "minimize_window",
            "toggle_maximize_window",
            "close_window",
            "start_window_drag",
            "show_app_menu",
            "open_external_url",
            "open_detached_window",
            "open_app_uri",
        ]),
    ))
    .expect("failed to build Tauri desktop application");
}
