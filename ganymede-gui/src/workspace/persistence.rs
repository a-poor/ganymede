//! Layout only: credentials and document contents never belong in this file.
use super::*;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub(super) struct SavedTab {
    // None is the launcher. Terminal/console placeholders are not restored.
    pub(super) path: Option<String>,
    notebook: bool,
    item: usize,
    offset: f32,
    selected: usize,
    x: f32,
    y: f32,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub(super) struct Layout {
    tabs: Vec<SavedTab>,
    active: usize,
    expanded: Vec<String>,
    pub sidebar_open: bool,
    pub sidebar_tab: String,
}
impl Default for Layout {
    fn default() -> Self {
        Self {
            tabs: vec![],
            active: 0,
            expanded: vec![],
            sidebar_open: true,
            sidebar_tab: "files".into(),
        }
    }
}

pub(super) fn state_path(address: &str) -> PathBuf {
    // Hex is reversible and collision-free, unlike a process-dependent hash.
    // Use a directory component per chunk to stay below filename limits for long base URLs.
    let mut path = crate::connection::history_path().with_extension("workspaces");
    let encoded: String = address.bytes().map(|b| format!("{b:02x}")).collect();
    for chunk in encoded.as_bytes().chunks(120) {
        path.push(std::str::from_utf8(chunk).unwrap());
    }
    path.join("layout.json")
}
fn read(path: &Path) -> Option<Layout> {
    serde_json::from_slice(&std::fs::read(path).ok()?).ok()
}
fn write(path: &Path, layout: &Layout) -> std::io::Result<()> {
    std::fs::create_dir_all(path.parent().unwrap())?;
    let temporary = path.with_extension(format!("{}.tmp", uuid::Uuid::new_v4()));
    std::fs::write(&temporary, serde_json::to_vec(layout)?)?;
    let result = std::fs::rename(&temporary, path);
    if result.is_err() {
        let _ = std::fs::remove_file(temporary);
    }
    result
}

fn retain_unavailable(
    tabs: &mut Vec<SavedTab>,
    active: &mut usize,
    unavailable: &[(usize, SavedTab)],
) {
    for (index, tab) in unavailable {
        if !tabs.iter().any(|t| t.path == tab.path) {
            let position = (*index).min(tabs.len());
            tabs.insert(position, tab.clone());
            if position <= *active {
                *active += 1;
            }
        }
    }
    *active = (*active).min(tabs.len().saturating_sub(1));
}

impl Workspace {
    pub(super) fn load_layout(runtime: &Runtime) -> Layout {
        runtime
            .config
            .as_ref()
            .ok()
            .and_then(|config| read(&state_path(config.connection_key())))
            .unwrap_or_default()
    }

    pub fn sidebar_state(&self) -> (bool, crate::SidebarTab) {
        let tab = match self.saved_layout.sidebar_tab.as_str() {
            "kernels" => crate::SidebarTab::Kernels,
            "git" => crate::SidebarTab::Git,
            _ => crate::SidebarTab::Files,
        };
        (self.saved_layout.sidebar_open, tab)
    }
    pub fn set_sidebar_state(&mut self, open: bool, tab: crate::SidebarTab) {
        self.saved_layout.sidebar_open = open;
        self.saved_layout.sidebar_tab = match tab {
            crate::SidebarTab::Files => "files",
            crate::SidebarTab::Kernels => "kernels",
            crate::SidebarTab::Git => "git",
        }
        .into();
    }

    pub(super) fn begin_restore(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        for (path, notebook) in self.recovery.paths() {
            if !self
                .saved_layout
                .tabs
                .iter()
                .any(|t| t.path.as_ref() == Some(&path))
            {
                self.saved_layout.tabs.push(SavedTab {
                    path: Some(path),
                    notebook,
                    ..Default::default()
                });
            }
        }
        let saved = self.saved_layout.clone();
        self.expanded.extend(saved.expanded.iter().cloned());
        for path in saved.expanded {
            self.refresh_directory(path, cx);
        }
        self.restoring = !saved.tabs.is_empty();
        for tab in saved.tabs {
            if let Some(path) = tab.path {
                if tab.notebook {
                    self.open_notebook(Some(path), window, cx);
                } else {
                    self.open_text(
                        Some(path.clone()),
                        DocumentKind::for_text_path(&path),
                        window,
                        cx,
                    );
                }
            }
        }
        self.finish_restore(window, cx);
        cx.on_release(|this, cx| {
            this.persist_layout(cx);
            this.persist_recovery(cx);
        })
        .detach();
        cx.spawn_in(window, async move |this, cx| {
            loop {
                cx.background_executor().timer(Duration::from_secs(1)).await;
                if this
                    .update_in(cx, |this, window, cx| {
                        this.persist_layout(cx);
                        this.persist_recovery(cx);
                        this.next_recovery(window, cx);
                    })
                    .is_err()
                {
                    break;
                }
            }
        })
        .detach();
    }

    pub(super) fn finish_restore(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.restoring || !self.opening.is_empty() || self.recovery.waiting() {
            return;
        }
        self.unrestored = self
            .saved_layout
            .tabs
            .iter()
            .enumerate()
            .filter(|(_, tab)| {
                tab.path.as_ref().is_some_and(|path| {
                    !self
                        .documents
                        .iter()
                        .any(|doc| doc.path.as_ref() == Some(path))
                })
            })
            .map(|(index, tab)| (index, tab.clone()))
            .collect();
        let mut ordered = Vec::new();
        let mut active = None;
        for (index, tab) in self.saved_layout.tabs.iter().enumerate() {
            let id = if let Some(path) = &tab.path {
                let Some(doc) = self
                    .documents
                    .iter()
                    .find(|doc| doc.path.as_ref() == Some(path))
                else {
                    continue;
                };
                match &doc.content {
                    Content::Notebook(view) => view.update(cx, |view, cx| {
                        view.restore_position(tab.item, tab.offset, tab.selected, cx)
                    }),
                    Content::Text(view) => {
                        view.update(cx, |view, cx| view.restore_offset(tab.x, tab.y, cx))
                    }
                    _ => (),
                }
                doc.id
            } else {
                LAUNCHER
            };
            if !ordered.contains(&id) {
                ordered.push(id);
            }
            if index == self.saved_layout.active {
                active = Some(id);
            }
        }
        // Retain any tabs the user opened while restoration was in flight.
        for id in &self.tabs.open {
            if *id != LAUNCHER && !ordered.contains(id) {
                ordered.push(*id);
            }
        }
        if ordered.is_empty() {
            ordered.push(LAUNCHER);
        }
        self.tabs.open = ordered;
        self.expanded = self.saved_layout.expanded.iter().cloned().collect();
        self.restoring = false;
        self.activate(active.unwrap_or(self.tabs.open[0]), window, cx);
        cx.notify();
    }

    pub(super) fn persist_layout(&mut self, cx: &App) {
        if self.restoring {
            return;
        }
        let Ok(config) = &self.runtime.config else {
            return;
        };
        let mut tabs = Vec::new();
        let mut active = 0;
        for id in &self.tabs.open {
            let saved = if *id == LAUNCHER {
                SavedTab::default()
            } else {
                let Some(doc) = self.documents.iter().find(|d| d.id == *id) else {
                    continue;
                };
                let Some(path) = &doc.path else {
                    continue;
                };
                let mut saved = SavedTab {
                    path: Some(path.clone()),
                    ..Default::default()
                };
                match &doc.content {
                    Content::Notebook(view) => {
                        saved.notebook = true;
                        (saved.item, saved.offset, saved.selected) = view.read(cx).view_position();
                    }
                    Content::Text(view) => {
                        (saved.x, saved.y) = view.read(cx).view_offset(cx);
                    }
                    _ => continue,
                }
                saved
            };
            if *id == self.tabs.active {
                active = tabs.len();
            }
            tabs.push(saved);
        }
        // A temporarily unavailable file remains in the saved layout for the next connection.
        retain_unavailable(&mut tabs, &mut active, &self.unrestored);
        let mut expanded: Vec<_> = self.expanded.iter().cloned().collect();
        expanded.sort();
        let layout = Layout {
            tabs,
            active,
            expanded,
            sidebar_open: self.saved_layout.sidebar_open,
            sidebar_tab: self.saved_layout.sidebar_tab.clone(),
        };
        if self.last_written.as_ref() == Some(&layout) {
            return;
        }
        if write(&state_path(config.connection_key()), &layout).is_ok() {
            self.last_written = Some(layout);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn layout_round_trips_without_contents_or_credentials() {
        let path = std::env::temp_dir()
            .join(format!("ganymede-layout-{}", uuid::Uuid::new_v4()))
            .join("layout.json");
        let layout = Layout {
            tabs: vec![
                SavedTab {
                    path: Some("folder/notes.ipynb".into()),
                    notebook: true,
                    item: 47,
                    offset: 19.,
                    selected: 45,
                    ..Default::default()
                },
                SavedTab::default(),
            ],
            active: 1,
            expanded: vec!["folder".into()],
            sidebar_open: false,
            sidebar_tab: "kernels".into(),
        };
        write(&path, &layout).unwrap();
        assert_eq!(read(&path), Some(layout));
        std::fs::write(&path, b"invalid").unwrap();
        assert_eq!(read(&path), None);
        std::fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }
    #[test]
    fn unavailable_tabs_keep_order_without_changing_active_document() {
        let tab = |path: &str| SavedTab {
            path: Some(path.into()),
            ..Default::default()
        };
        let mut tabs = vec![tab("a"), tab("c")];
        let mut active = 1;
        retain_unavailable(&mut tabs, &mut active, &[(1, tab("b")), (3, tab("d"))]);
        assert_eq!(
            tabs.iter()
                .map(|t| t.path.as_deref().unwrap())
                .collect::<Vec<_>>(),
            ["a", "b", "c", "d"]
        );
        assert_eq!(tabs[active].path.as_deref(), Some("c"));
        retain_unavailable(&mut tabs, &mut active, &[(1, tab("b"))]);
        assert_eq!(tabs.len(), 4);
        assert_eq!(active, 2);
    }

    #[test]
    fn server_layout_keys_include_base_path() {
        assert_ne!(
            state_path("https://host/user/a/"),
            state_path("https://host/user/b/")
        );
        assert_ne!(state_path("http://host/"), state_path("https://host/"));
    }
}
