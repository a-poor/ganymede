use super::*;
use crate::toast::{Toast, ToastKind};
use ganymede_core::jupyter::{Error, ExportFormat};
use std::{io::Write, path::Path};

impl Workspace {
    pub(crate) fn export_notebook(&mut self, format: ExportFormat, cx: &mut Context<Self>) {
        if self.exporting {
            return;
        }
        let Some(notebook) = self.active_notebook(cx) else {
            return;
        };
        // Flush editor buffers before taking an immutable snapshot, without saving it.
        let controller = notebook.update(cx, |n, cx| n.file_controller(cx));
        let snapshot = controller.snapshot();
        let name = snapshot
            .path
            .rsplit('/')
            .next()
            .unwrap_or("notebook.ipynb")
            .to_owned();
        let content = snapshot.document.to_json();
        let mut config = controller.server_config();
        config.request_timeout = Duration::from_secs(120);
        self.exporting = true;
        let toast = self.toasts.update(cx, |host, cx| {
            host.push(
                Toast::new("Exporting notebook…")
                    .description("Using current edits and outputs. Cells will not be run.")
                    .kind(ToastKind::Loading),
                cx,
            )
        });
        let task = self.runtime.handle.spawn(async move {
            JupyterRestClient::new(config)?
                .export_notebook(&name, content, format)
                .await
        });
        let stem = Path::new(&snapshot.path)
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("notebook")
            .to_owned();
        cx.spawn(async move |this, cx| {
            let result = task
                .await
                .unwrap_or_else(|_| Err(Error::Protocol("Export task failed".into())));
            let export = match result {
                Ok(export) => export,
                Err(error) => {
                    this.update(cx, |this, cx| {
                        this.exporting = false;
                        this.toasts.update(cx, |host, cx| {
                            host.resolve(
                                toast,
                                Toast::new("Could not export notebook")
                                    .description(export_error(&error, format))
                                    .kind(ToastKind::Error)
                                    .duration(Duration::from_secs(20)),
                                cx,
                            )
                        });
                    })
                    .ok();
                    return;
                }
            };
            let picker = this.update(cx, |this, cx| {
                this.toasts.update(cx, |host, cx| {
                    host.resolve(
                        toast,
                        Toast::new("Export ready")
                            .kind(ToastKind::Loading)
                            .description(if export.extension == "zip" {
                                "Save the ZIP containing Markdown and its images."
                            } else {
                                "Choose where to save the exported notebook."
                            }),
                        cx,
                    )
                });
                let directory = std::env::var_os("HOME")
                    .map(std::path::PathBuf::from)
                    .unwrap_or_default();
                cx.prompt_for_new_path(&directory, Some(&format!("{stem}.{}", export.extension)))
            });
            let Ok(picker) = picker else {
                return;
            };
            let result = match picker.await {
                Ok(Ok(Some(path))) => {
                    cx.background_spawn(async move {
                        write_export(&path, &export.bytes, export.extension)?;
                        Ok(Some(path))
                    })
                    .await
                }
                Ok(Ok(None)) => Ok(None),
                _ => Err("Could not open the save dialog.".to_owned()),
            };
            this.update(cx, |this, cx| {
                this.exporting = false;
                let message = match result {
                    Ok(Some(path)) => Toast::new("Notebook exported")
                        .kind(ToastKind::Success)
                        .action("Show file", move |_, cx| cx.reveal_path(&path)),
                    Ok(None) => Toast::new("Export canceled"),
                    Err(error) => Toast::new("Could not save export")
                        .description(error)
                        .kind(ToastKind::Error),
                };
                this.toasts
                    .update(cx, |host, cx| host.resolve(toast, message, cx));
            })
            .ok();
        })
        .detach();
    }
}

fn export_error(error: &Error, format: ExportFormat) -> String {
    match error {
        Error::Http(e) if e.is_timeout() => "Conversion timed out after two minutes. Try again or export HTML.".into(),
        Error::Http(_) | Error::Disconnected => "Could not reach the server. Reconnect and try again.".into(),
        Error::Api { status, .. } if matches!(status.as_u16(), 401 | 403) => "The server denied export access. Check your connection credentials and server permissions.".into(),
        Error::Api { .. } => if format == ExportFormat::Pdf {
            "PDF conversion failed. In the server’s Python environment, install nbconvert[webpdf] and run python -m playwright install chromium. The server logs contain conversion details."
        } else {
            "Conversion failed. Install nbconvert in the server’s Python environment. If it is already installed, check the server logs for unsupported content or configuration errors."
        }.into(),
        Error::Protocol(message) => message.clone(),
        _ => "Could not convert this notebook. Check the server’s nbconvert installation and logs.".into(),
    }
}

/// Keep the destination untouched if writing fails; never follow a destination symlink.
fn write_export(path: &Path, bytes: &[u8], extension: &str) -> Result<(), String> {
    if path.extension().and_then(|s| s.to_str()) != Some(extension) {
        return Err(format!("Use a .{extension} filename for this export."));
    }
    let parent = path.parent().ok_or("Choose a destination folder.")?;
    let temporary = parent.join(format!(".ganymede-export-{}", uuid::Uuid::new_v4()));
    let result = (|| -> std::io::Result<()> {
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        std::fs::rename(&temporary, path)
    })();
    let _ = std::fs::remove_file(&temporary);
    result.map_err(|_| {
        "Could not write the export. Check the destination’s permissions and available space."
            .into()
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn export_write_preserves_notebooks_and_replaces_output_atomically() {
        let dir =
            std::env::temp_dir().join(format!("ganymede-export-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&dir).unwrap();
        let notebook = dir.join("demo.ipynb");
        std::fs::write(&notebook, b"original").unwrap();
        assert!(write_export(&notebook, b"export", "html").is_err());
        assert_eq!(std::fs::read(&notebook).unwrap(), b"original");
        let output = dir.join("demo.html");
        std::fs::write(&output, b"old").unwrap();
        write_export(&output, b"new", "html").unwrap();
        assert_eq!(std::fs::read(&output).unwrap(), b"new");
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 2);
        std::fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    fn export_failures_give_setup_help_without_showing_server_details() {
        let error = Error::Api {
            status: reqwest::StatusCode::INTERNAL_SERVER_ERROR,
            message: "private notebook contents".into(),
        };
        assert!(export_error(&error, ExportFormat::Pdf).contains("playwright install chromium"));
        assert!(export_error(&error, ExportFormat::Html).contains("Install nbconvert"));
        assert!(!export_error(&error, ExportFormat::Html).contains("private notebook"));
    }
}
