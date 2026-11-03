//! Plain-text file view; file paths retain the extension for future language support.
use crate::{
    autosave::{AutosaveClock, AutosavePreference},
    code_cell::{CellEvent, CodeCell},
    execution::Runtime,
    toast::{Toast, ToastHost, ToastKind},
};
use ganymede_core::text_file::{Error, TextFileController};
use gpui::{Context, Entity, Focusable, Window, div, prelude::*};
use std::time::{Duration, Instant};

#[derive(Clone, Copy)]
enum Operation {
    Save,
    Overwrite,
    Reload,
    Copy,
}
enum Outcome {
    Saved,
    Reloaded,
    Copied(String),
}

pub struct TextFile {
    editor: Entity<CodeCell>,
    controller: TextFileController,
    toasts: Entity<ToastHost>,
    saving: bool,
    file_busy: bool,
    conflict: bool,
    save_error: bool,
    autosave: AutosaveClock,
    suspended: bool,
}
impl TextFile {
    pub fn view_offset(&self, cx: &gpui::App) -> (f32, f32) {
        self.editor.read(cx).view_offset()
    }

    pub fn restore_offset(&self, x: f32, y: f32, cx: &mut gpui::App) {
        self.editor
            .update(cx, |editor, _| editor.restore_offset(x, y));
    }

    pub fn new(
        controller: TextFileController,
        toasts: Entity<ToastHost>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let editor = cx.new(|cx| {
            let mut editor = CodeCell::new_document(window, cx);
            editor.set_text(controller.snapshot().text, cx);
            editor
        });
        cx.subscribe(&editor, |this, editor, event, cx| {
            if matches!(event, CellEvent::Changed) {
                this.controller
                    .update_text(editor.read(cx).text().to_owned());
                cx.notify();
            }
        })
        .detach();
        cx.observe_global::<AutosavePreference>(|this, cx| {
            this.autosave.resume();
            cx.notify();
        })
        .detach();
        cx.spawn_in(window, async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(250))
                    .await;
                if this
                    .update_in(cx, |this, window, cx| {
                        let (revision, dirty) = this.controller.status();
                        let enabled = cx.global::<AutosavePreference>().0
                            && !this.suspended
                            && !this.file_busy
                            && !this.conflict;
                        if this
                            .autosave
                            .due(revision, dirty, enabled, this.saving, Instant::now())
                        {
                            this.save(window, cx);
                        }
                    })
                    .is_err()
                {
                    break;
                }
            }
        })
        .detach();
        Self {
            editor,
            controller,
            toasts,
            saving: false,
            file_busy: false,
            conflict: false,
            save_error: false,
            autosave: Default::default(),
            suspended: false,
        }
    }
    pub fn focus(&self, window: &mut Window, cx: &mut Context<Self>) {
        self.editor.read(cx).focus_handle(cx).focus(window, cx);
    }
    pub fn is_dirty(&self) -> bool {
        self.controller.status().1
    }
    pub fn is_saving(&self) -> bool {
        self.saving
    }
    pub fn save_failed(&self) -> bool {
        self.save_error
    }
    pub fn set_file_busy(&mut self, busy: bool) {
        self.file_busy = busy;
    }
    pub fn file_controller(&self) -> TextFileController {
        self.controller.clone()
    }
    pub fn file_saved_as(&mut self, cx: &mut Context<Self>) {
        self.conflict = false;
        self.save_error = false;
        self.autosave.resume();
        cx.notify();
    }
    pub fn suspend_autosave(&mut self, suspended: bool) {
        self.suspended = suspended;
    }
    pub fn save(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.saving || self.file_busy {
            return;
        }
        self.start(Operation::Save, window, cx);
    }
    fn start(&mut self, operation: Operation, window: &mut Window, cx: &mut Context<Self>) {
        self.saving = true;
        self.save_error = false;
        self.controller
            .update_text(self.editor.read(cx).text().to_owned());
        let controller = self.controller.clone();
        let task = cx.global::<Runtime>().handle.spawn(async move {
            match operation {
                Operation::Save | Operation::Overwrite => {
                    controller
                        .save(matches!(operation, Operation::Overwrite))
                        .await?;
                    Ok(Outcome::Saved)
                }
                Operation::Reload => {
                    controller.reload().await?;
                    Ok(Outcome::Reloaded)
                }
                Operation::Copy => controller.save_copy().await.map(Outcome::Copied),
            }
        });
        cx.spawn_in(window, async move |this, cx| {
            let result = task.await.unwrap_or(Err(Error::Interrupted));
            if matches!(&result, Err(Error::Conflict)) {
                this.update(cx, |this, cx| {
                    this.conflict = true;
                    this.autosave.failed();
                    cx.notify();
                }).ok();
                loop {
                    let response = this.update_in(cx, |this, window, cx| {
                        if window.has_active_prompt() { return None; }
                        Some(crate::confirm::file_save_conflict(&this.controller.path(), window, cx))
                    });
                    match response {
                        Ok(Some(response)) => {
                            let choice = response.await;
                            this.update_in(cx, |this, window, cx| {
                                match choice {
                                    1 => this.start(Operation::Reload, window, cx),
                                    2 => this.start(Operation::Copy, window, cx),
                                    3 => this.start(Operation::Overwrite, window, cx),
                                    _ => { this.saving = false; this.save_error = true; cx.notify(); }
                                }
                            }).ok();
                            break;
                        }
                        Ok(None) => cx.background_executor().timer(Duration::from_millis(100)).await,
                        Err(_) => break,
                    }
                }
                return;
            }
            this.update_in(cx, |this, window, cx| {
                this.saving = false;
                match result {
                    Ok(Outcome::Copied(path)) => {
                        // A copy never acknowledges the original's unsaved changes.
                        this.save_error = true;
                        this.toasts.update(cx, |host, cx| { host.push(Toast::new("File copy saved")
                            .description(format!("Saved as {path}. The original is still open; autosave remains paused.")), cx); });
                    }
                    Ok(outcome) => {
                        if matches!(outcome, Outcome::Reloaded) {
                            let text = this.controller.snapshot().text;
                            this.editor.update(cx, |editor, cx| editor.set_text(text, cx));
                            this.focus(window, cx);
                        }
                        this.conflict = false;
                        this.save_error = false;
                        this.autosave.resume();
                    }
                    Err(error) => {
                        this.save_error = true;
                        this.autosave.failed();
                        this.toasts.update(cx, |host, cx| { host.push(Toast::new("Could not save file")
                            .description(error.to_string()).kind(ToastKind::Error), cx); });
                    }
                }
                cx.notify();
            }).ok();
        }).detach();
        cx.notify();
    }
}
impl Render for TextFile {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .min_h_0()
            .flex()
            .flex_col()
            .on_action(cx.listener(|this, _: &crate::Save, window, cx| this.save(window, cx)))
            .child(div().flex_1().min_h_0().p_3().child(self.editor.clone()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ganymede_core::jupyter::JupyterRestClient;
    use std::sync::{Arc, Mutex};

    #[tokio::test]
    #[ignore = "requires the Jupyter server configured in .env"]
    async fn live_text_files_round_trip_and_detect_external_edits() {
        let runtime = Runtime::new(tokio::runtime::Handle::current());
        let config = runtime.config.unwrap();
        let rest = JupyterRestClient::new(config.clone()).unwrap();
        let paths = Arc::new(Mutex::new(Vec::new()));
        let cleanup = paths.clone();
        let client = rest.clone();
        let task = tokio::spawn(async move {
            for (extension, text) in [
                (".md", "# Hello 🪐\n\nMarkdown text\n"),
                (".py", "print('hello')\n"),
                (".toml", "[project]\nname = 'example'\n"),
                (".lock", "version = 1\nrevision = 3\n"),
                ("", "An extensionless text file\n"),
            ] {
                let created = client.create_text_file("", extension).await.unwrap();
                paths.lock().unwrap().push(created.path.clone());
                let file = TextFileController::open(config.clone(), &created.path)
                    .await
                    .unwrap();
                assert!(!file.snapshot().is_dirty());
                file.update_text(text.into());
                file.save(false).await.unwrap();
                assert!(!file.snapshot().is_dirty());
                let other = TextFileController::open(config.clone(), &created.path)
                    .await
                    .unwrap();
                assert_eq!(other.snapshot().text, text);
                file.update_text("saved elsewhere\n".into());
                file.save(false).await.unwrap();
                other.update_text("keep my edits\n".into());
                assert!(matches!(other.save(false).await, Err(Error::Conflict)));
                let copy = other.save_copy().await.unwrap();
                paths.lock().unwrap().push(copy.clone());
                assert_eq!(
                    TextFileController::open(config.clone(), &copy)
                        .await
                        .unwrap()
                        .snapshot()
                        .text,
                    "keep my edits\n"
                );
                assert!(other.snapshot().is_dirty());
                other.reload().await.unwrap();
                assert_eq!(other.snapshot().text, "saved elsewhere\n");
                other.update_text(String::new());
                other.save(false).await.unwrap();
                assert_eq!(
                    TextFileController::open(config.clone(), &created.path)
                        .await
                        .unwrap()
                        .snapshot()
                        .text,
                    ""
                );
            }
            let binary = client.create_text_file("", ".bin").await.unwrap();
            paths.lock().unwrap().push(binary.path.clone());
            client
                .save_text_file(&binary.path, "binary\0payload")
                .await
                .unwrap();
            assert!(matches!(
                TextFileController::open(config, &binary.path).await,
                Err(Error::NotText)
            ));
        });
        let result = task.await;
        let paths = cleanup.lock().unwrap().clone();
        for path in paths {
            rest.delete_contents(&path).await.unwrap();
        }
        result.unwrap();
    }
}
