use super::*;
pub(crate) enum Command {
    NewNotebook,
    NewText,
    NewMarkdown,
    NewPython,
    NewFolder,
    Upload,
    SaveAll,
    SaveAs,
    Rename,
    NextTab,
    PreviousTab,
    ShutdownKernel,
    ShutdownAll,
    ShutdownUnused,
}
impl Workspace {
    pub fn active_path(&self) -> Option<String> {
        self.documents
            .iter()
            .find(|d| d.id == self.tabs.active)
            .and_then(|d| d.path.clone())
    }
    pub fn active_notebook(&self, _: &App) -> Option<Entity<Notebook>> {
        self.documents
            .iter()
            .find(|d| d.id == self.tabs.active)
            .and_then(|d| match &d.content {
                Content::Notebook(n) => Some(n.clone()),
                _ => None,
            })
    }
    fn cycle_tab(&mut self, backwards: bool, window: &mut Window, cx: &mut Context<Self>) {
        let len = self.tabs.open.len();
        if len < 2 {
            return;
        }
        let current = self
            .tabs
            .open
            .iter()
            .position(|id| *id == self.tabs.active)
            .unwrap_or(0);
        let index = if backwards {
            (current + len - 1) % len
        } else {
            (current + 1) % len
        };
        self.activate(self.tabs.open[index], window, cx);
    }
    pub(crate) fn menu_command(
        &mut self,
        command: Command,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match command {
            Command::NewNotebook => self.create(DocumentKind::Notebook, window, cx),
            Command::NewText => self.create(DocumentKind::Text, window, cx),
            Command::NewMarkdown => self.create(DocumentKind::Markdown, window, cx),
            Command::NewPython => self.create(DocumentKind::Python, window, cx),
            Command::NewFolder => self.prompt_new_folder(window, cx),
            Command::Upload => self.upload(window, cx),
            Command::SaveAll => {
                for doc in &self.documents {
                    doc.content.save(window, cx);
                }
            }
            Command::SaveAs => {
                if let Some(path) = self.active_path() {
                    self.prompt_save_as(path, window, cx);
                }
            }
            Command::Rename => {
                if let Some(path) = self.active_path() {
                    self.prompt_rename(path, false, window, cx);
                }
            }
            Command::NextTab => self.cycle_tab(false, window, cx),
            Command::PreviousTab => self.cycle_tab(true, window, cx),
            Command::ShutdownKernel => {
                if let Some(id) = self
                    .active_notebook(cx)
                    .and_then(|n| n.read(cx).kernel_id())
                {
                    self.confirm_kernel_shutdown(id, window, cx);
                }
            }
            Command::ShutdownAll => self.kernel_toolbar(kernels::KernelAction::All, window, cx),
            Command::ShutdownUnused => {
                self.kernel_toolbar(kernels::KernelAction::Unused, window, cx)
            }
        }
        cx.notify();
    }
}
