//! Native macOS menu-bar integration. All native objects stay on GPUI's main thread.
use gpui::{App, Global};
use std::{collections::HashMap, time::Duration};
use tray_icon::{
    Icon, TrayIcon, TrayIconBuilder,
    menu::{Menu, MenuEvent, MenuId, MenuItem, PredefinedMenuItem, Submenu},
};

type Entries = Vec<(String, String)>;
#[derive(Clone)]
enum Command {
    Open,
    New,
    Recent(String),
    Server(String, bool),
    Settings,
    Quit,
}
#[derive(Default)]
struct Tray {
    icon: Option<TrayIcon>,
    snapshot: Option<(Entries, Entries)>,
    commands: HashMap<MenuId, Command>,
}
impl Global for Tray {}

pub fn init(cx: &mut App) {
    cx.set_global(Tray::default());
    let (send, mut receive) = tokio::sync::mpsc::unbounded_channel();
    MenuEvent::set_event_handler(Some(move |event: MenuEvent| {
        let _ = send.send(event.id);
    }));
    cx.spawn(async move |cx| {
        while let Some(id) = receive.recv().await {
            cx.update(|cx| {
                let command = cx.global::<Tray>().commands.get(&id).cloned();
                match command {
                    Some(Command::Open) => crate::connection::reopen(cx),
                    Some(Command::New) => {
                        crate::connection::open(cx);
                        cx.activate(true);
                    }
                    Some(Command::Recent(key)) => crate::connection::open_recent(&key, cx),
                    Some(Command::Server(address, stop)) => {
                        crate::connection::tray_server(&address, stop, cx)
                    }
                    Some(Command::Settings) => {
                        crate::open_settings(cx);
                        cx.activate(true);
                    }
                    Some(Command::Quit) => cx.dispatch_action(&crate::Quit),
                    None => {}
                }
            });
        }
    })
    .detach();
    // Yield until AppKit's event loop is running before creating an NSStatusItem.
    // Managed server state is shared with worker tasks; compare cheap in-memory
    // snapshots so those transitions update the menu without network polling.
    cx.spawn(async move |cx| {
        loop {
            cx.background_executor().timer(Duration::from_secs(1)).await;
            cx.update(refresh);
        }
    })
    .detach();
}

pub fn refresh(cx: &mut App) {
    if !cx.global::<crate::preferences::MenuBarPreference>().0 {
        *cx.global_mut::<Tray>() = Tray::default();
        return;
    }
    let snapshot = crate::connection::tray_connections(cx);
    if cx.global::<Tray>().snapshot.as_ref() == Some(&snapshot) {
        return;
    }
    match build_menu(&snapshot) {
        Ok((menu, commands)) => {
            let tray = cx.global_mut::<Tray>();
            if let Some(icon) = &tray.icon {
                icon.set_menu(Some(Box::new(menu)));
            } else {
                match TrayIconBuilder::new()
                    .with_icon_templated(planet_icon())
                    .with_tooltip("Ganymede")
                    .with_autosave_name("GanymedeMenuBar")
                    .with_menu(Box::new(menu))
                    .build()
                {
                    Ok(icon) => tray.icon = Some(icon),
                    Err(error) => {
                        eprintln!("Could not create menu bar icon: {error}");
                        return;
                    }
                }
            }
            tray.commands = commands;
            tray.snapshot = Some(snapshot);
        }
        Err(error) => eprintln!("Could not update menu bar menu: {error}"),
    }
}

fn label(value: &str) -> String {
    let clean: String = value.chars().filter(|c| !c.is_control()).take(72).collect();
    if value.chars().count() > 72 {
        format!("{clean}…")
    } else {
        clean
    }
}
fn build_menu(snapshot: &(Entries, Entries)) -> anyhow::Result<(Menu, HashMap<MenuId, Command>)> {
    let menu = Menu::new();
    let mut commands = HashMap::new();
    let mut item = |text: &str, command: Command| {
        let item = MenuItem::new(label(text), true, None);
        commands.insert(item.id().clone(), command);
        item
    };
    menu.append(&item("Open Ganymede", Command::Open))?;
    menu.append(&item("New Connection…", Command::New))?;
    let recent = Submenu::new("Recent Connections", true);
    if snapshot.0.is_empty() {
        recent.append(&MenuItem::new("Nothing yet", false, None))?;
    }
    for (key, text) in &snapshot.0 {
        recent.append(&item(text, Command::Recent(key.clone())))?;
    }
    menu.append(&recent)?;
    let servers = Submenu::new("Running Local Servers", true);
    if snapshot.1.is_empty() {
        servers.append(&MenuItem::new("None running", false, None))?;
    }
    for (address, text) in &snapshot.1 {
        let server = Submenu::new(label(text), true);
        server.append(&item("Open", Command::Server(address.clone(), false)))?;
        server.append(&item("Stop…", Command::Server(address.clone(), true)))?;
        servers.append(&server)?;
    }
    menu.append(&servers)?;
    menu.append(&PredefinedMenuItem::separator())?;
    menu.append(&item("Settings…", Command::Settings))?;
    menu.append(&item("Quit Ganymede", Command::Quit))?;
    Ok((menu, commands))
}

fn planet_icon() -> Icon {
    // Template artwork: AppKit adapts the dotted sphere to the menu-bar appearance.
    let image = image::load_from_memory(include_bytes!("../branding/icon-menu-bar.png"))
        .expect("bundled menu-bar icon is valid PNG")
        .into_rgba8();
    let (width, height) = image.dimensions();
    Icon::from_rgba(image.into_raw(), width, height).expect("valid menu icon dimensions")
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn menu_labels_are_single_line_and_bounded() {
        assert_eq!(label("Project\nname\t"), "Projectname");
        assert_eq!(label(&"é".repeat(80)).chars().count(), 73);
    }
}
