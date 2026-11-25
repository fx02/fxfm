// Prevents an additional console window on Windows in release.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod audio;
mod clipboard;
mod commands;
mod dragdrop;
mod editwatch;
mod fnkey;
#[cfg(target_os = "macos")]
mod fsevents;
mod icons;
mod jobs;
mod keychain;
mod launch;
mod menu;
mod remote;
mod search;
mod smb;
mod state;
mod translocation;
mod volwatch;

use std::time::Duration;
use tauri::{Manager, RunEvent, WindowEvent};

fn main() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .manage(state::AppState::load())
        .setup(|app| {
            let state = app.state::<state::AppState>();
            let _ = state.handle.set(app.handle().clone());
            let home = state.home.clone();
            volwatch::start(app.handle().clone(), home);
            fnkey::start(app.handle().clone());
            let search = state::lock(&state.settings).search.clone();
            state.search.start_live();
            state.search.start(&search);
            state.search.start_ticker();
            let keymap = state::lock(&app.state::<state::AppState>().keymap).clone();
            let mods = state::lock(&app.state::<state::AppState>().settings).modifiers;
            app.set_menu(menu::build(app.handle(), &keymap, &mods)?)?;
            Ok(())
        })
        .on_menu_event(menu::on_event)
        .invoke_handler(tauri::generate_handler![
            commands::bootstrap,
            commands::list_dir,
            commands::list_volumes,
            commands::nearest_existing,
            commands::paths_exist,
            commands::dir_size,
            commands::set_session,
            commands::save_session,
            commands::save_settings,
            commands::save_hotlist,
            commands::save_keymap,
            commands::reset_keymap,
            commands::set_storage_mode,
            commands::start_job,
            commands::answer_conflict,
            commands::answer_error,
            commands::cancel_job,
            commands::mkdir,
            commands::rename,
            commands::multi_rename,
            smb::smb_browse,
            smb::smb_list_shares,
            commands::view_content,
            commands::view_text_as,
            commands::quick_look,
            commands::set_archive_password,
            commands::location_in_archive,
            commands::open_viewer,
            commands::open_find_files,
            commands::edit_file,
            commands::open_remote_archive,
            commands::cancel_download,
            commands::edit_new,
            commands::upload_back,
            commands::edit_confirm,
            commands::discard_pending_edits,
            commands::reveal_file,
            commands::open_default,
            commands::run_in_terminal,
            commands::open_fda_settings,
            commands::open_keyboard_settings,
            dragdrop::start_drag,
            dragdrop::drag_modifiers,
            remote::chmod,
            remote::servers_load,
            remote::servers_save,
            remote::server_secret_set,
            remote::connect,
            remote::disconnect,
            remote::list_connections,
            remote::smb_shares,
            remote::smb_unmount,
            remote::connect_cancel,
            remote::master_unlock,
            remote::master_set,
            remote::master_status,
            remote::import_key,
            translocation::fix_quarantine,
            audio::audio_play,
            audio::audio_stop,
            audio::audio_seek,
            audio::audio_pause,
            audio::audio_volume,
            audio::audio_outputs,
            icons::file_icons,
            clipboard::clipboard_write,
            clipboard::clipboard_files,
            search::commands::search_status,
            search::commands::search_build,
            search::commands::search_start,
            search::commands::search_cancel,
            search::commands::search_forget,
            search::commands::search_rebuild,
            search::commands::search_refresh,
        ])
        .on_window_event(|window, event| {
            // Closing the main window (Exit, Alt+F4, red button) quits fxfm,
            // also with F3 viewer windows still open. `exit` runs ExitRequested
            // and Exit, so jobs are cancelled and the session is flushed.
            if window.label() == "main" && matches!(event, WindowEvent::CloseRequested { .. }) {
                window.app_handle().exit(0);
            }
        })
        .build(tauri::generate_context!())
        .expect("error while building fxfm")
        .run(|app, event| {
            if let RunEvent::Exit = event {
                // Cmd+Q (NSApp terminate) only delivers Exit, never ExitRequested.
                commands::flush_session(&app.state::<state::AppState>());
                app.state::<state::AppState>().search.shutdown();
            }
            if let RunEvent::ExitRequested { .. } | RunEvent::Exit = event {
                // Cancelled operations remove their partial output; give them
                // a moment before the process exits.
                let jobs = &app.state::<state::AppState>().jobs;
                if !jobs.cancel_all(Duration::from_secs(3)) {
                    eprintln!("fxfm: exiting with jobs still running");
                }
            }
            if let RunEvent::Exit = event {
                // Server connections close. View/edit copies of archive and
                // server files go; edited copies that were not uploaded stay
                // (listed in pending-edits.json).
                let state = app.state::<state::AppState>();
                state.audio.shutdown();
                state.remote.close_all(&state.vfs);
                state.edits.exit_cleanup();
            }
        });
}
