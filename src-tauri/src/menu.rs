//! Native menu bar built from the command registry.
//!
//! Command items have no native accelerator: the menu would take the key
//! before the webview sees it. The bound key is shown in the title instead,
//! and a click emits `menu://command {id}` to the main window.

use fxfm_core::keymap::{KeyCombo, Keymap, COMMANDS};
use serde::Serialize;
use tauri::menu::{Menu, MenuBuilder, MenuEvent, MenuItemBuilder, SubmenuBuilder};
use tauri::{AppHandle, Emitter, Runtime};

/// Menus after the app and Edit menus, in menu-bar order.
const MENUS: &[&str] = &[
    "Files",
    "Mark",
    "Commands",
    "Net",
    "Show",
    "Configuration",
    "Start",
    "Help",
];

/// `title<TAB>first key`, or just the title when the command has no key.
pub fn menu_title_with_key<S: AsRef<str>>(title: &str, keys: &[S]) -> String {
    match keys.first() {
        Some(k) => format!("{title}\t{}", k.as_ref()),
        None => title.to_string(),
    }
}

/// Keys bound to `cmd`, its still-bound defaults first (in registry order).
fn display_keys(keymap: &Keymap, cmd: &str, defaults: &[&str]) -> Vec<String> {
    let mut bound: Vec<String> = keymap
        .combos_for(cmd)
        .iter()
        .map(|c| c.to_string())
        .collect();
    let canonical: Vec<String> = defaults
        .iter()
        .filter_map(|d| d.parse::<KeyCombo>().ok())
        .map(|c| c.to_string())
        .collect();
    bound.sort_by_key(|k| canonical.iter().position(|d| d == k).unwrap_or(usize::MAX));
    bound
}

pub fn build<R: Runtime>(app: &AppHandle<R>, keymap: &Keymap) -> tauri::Result<Menu<R>> {
    let app_menu = SubmenuBuilder::new(app, "fxfm")
        .about(None)
        .separator()
        .hide()
        .hide_others()
        .show_all()
        .separator()
        // The predefined Quit (Cmd+Q) sends `terminate:`, which ends in
        // RunEvent::Exit where the session is flushed.
        .quit()
        .build()?;
    let edit = SubmenuBuilder::new(app, "Edit")
        .undo()
        .redo()
        .separator()
        .cut()
        .copy()
        .paste()
        .select_all()
        .build()?;
    let mut menu = MenuBuilder::new(app).item(&app_menu).item(&edit);
    for name in MENUS {
        let mut sub = SubmenuBuilder::new(app, *name);
        let mut empty = true;
        for c in COMMANDS.iter().filter(|c| c.menu == *name) {
            let keys = display_keys(keymap, c.id, c.defaults);
            let item = MenuItemBuilder::with_id(c.id, menu_title_with_key(c.title, &keys))
                .enabled(c.available)
                .build(app)?;
            sub = sub.item(&item);
            empty = false;
        }
        if empty {
            let item = MenuItemBuilder::new("(empty)").enabled(false).build(app)?;
            sub = sub.item(&item);
        }
        menu = menu.item(&sub.build()?);
    }
    menu.build()
}

/// Rebuilds the menu bar after the keymap changed (titles show the keys).
pub fn refresh<R: Runtime>(app: &AppHandle<R>, keymap: &Keymap) {
    if let Err(e) = build(app, keymap).and_then(|m| app.set_menu(m)) {
        eprintln!("fxfm: menu: {e}");
    }
}

#[derive(Serialize, Clone)]
struct MenuCommand<'a> {
    id: &'a str,
}

pub fn on_event<R: Runtime>(app: &AppHandle<R>, event: MenuEvent) {
    let id = event.id().as_ref();
    if let Some(c) = COMMANDS.iter().find(|c| c.id == id) {
        if let Err(e) = app.emit_to("main", "menu://command", MenuCommand { id: c.id }) {
            eprintln!("fxfm: menu event: {e}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn title_carries_key() {
        assert_eq!(menu_title_with_key("Copy", &["F5"]), "Copy\tF5");
        assert_eq!(menu_title_with_key::<&str>("Options…", &[]), "Options…");
    }

    #[test]
    fn display_keys_prefer_defaults() {
        let k = Keymap::defaults();
        assert_eq!(display_keys(&k, "cm_delete", &["F8", "Delete"]), ["F8", "Delete"]);
        let mut k = Keymap::defaults();
        k.unbind(&"F5".parse().unwrap());
        k.bind(&"Ctrl+K".parse().unwrap(), "cm_copy");
        assert_eq!(display_keys(&k, "cm_copy", &["F5"]), ["Ctrl+K"]);
    }

    #[test]
    fn every_command_menu_is_in_the_bar() {
        for c in COMMANDS {
            assert!(MENUS.contains(&c.menu), "{} in unknown menu {}", c.id, c.menu);
        }
    }
}
