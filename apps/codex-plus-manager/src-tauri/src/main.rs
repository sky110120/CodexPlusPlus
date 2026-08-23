#![cfg_attr(windows, windows_subsystem = "windows")]

fn main() {
    for arg in std::env::args() {
        if arg.starts_with("dreamskin://") {
            if codex_plus_manager_lib::handle_dream_skin_url(&arg) {
                codex_plus_manager_lib::focus_existing_manager_window();
            }
        } else if arg.starts_with("codexplusplus://session") {
            if codex_plus_manager_lib::handle_session_share_url(&arg) {
                codex_plus_manager_lib::focus_existing_manager_window();
            }
        } else if arg.starts_with("codexplusplus://") {
            if codex_plus_manager_lib::handle_provider_import_url(&arg) {
                codex_plus_manager_lib::focus_existing_manager_window();
            }
        }
    }
    if std::env::args().any(|arg| arg == "--show-update") {
        unsafe {
            std::env::set_var("CODEX_PLUS_SHOW_UPDATE", "1");
        }
    }
    codex_plus_manager_lib::run();
}
