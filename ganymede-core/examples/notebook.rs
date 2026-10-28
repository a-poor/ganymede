//! Create, run, and save a notebook through the application-facing API.
use ganymede_core::{
    jupyter::ServerConfig,
    notebook::{CellKind, NotebookController, NotebookDocument},
};
use std::time::Duration;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut config = ServerConfig::new(&std::env::var("GANYMEDE_JUPYTER_URL")?)?;
    if let Ok(token) = std::env::var("GANYMEDE_JUPYTER_TOKEN") {
        config = config.with_token(&token)?;
    }
    let path = std::env::args()
        .nth(1)
        .unwrap_or_else(|| format!("ganymede-{}.ipynb", uuid::Uuid::new_v4()));
    let mut document = NotebookDocument::new();
    document.insert_cell(0, CellKind::Markdown, "# Ganymede notebook")?;
    let cell = document.insert_cell(
        1,
        CellKind::Code,
        "print('Hello from the notebook controller')\n6 * 7",
    )?;
    let notebook = NotebookController::new(config, path, document)?;
    notebook.save().await?;
    notebook.connect("python3").await?;
    let work = async {
        let mut snapshots = notebook.watch();
        notebook.run_cell(&cell, false).await?;
        loop {
            let complete = {
                let snapshot = snapshots.borrow_and_update();
                snapshot
                    .executions
                    .get(&cell)
                    .is_some_and(|run| !run.phase.is_active())
            };
            if complete {
                break;
            }
            snapshots.changed().await?;
        }
        notebook.save().await?;
        let snapshot = notebook.snapshot();
        println!(
            "Saved {} (dirty: {})",
            snapshot.path,
            snapshot.document.is_dirty()
        );
        println!("Outputs: {:#?}", snapshot.document.cell(&cell)?.outputs());
        Ok::<(), Box<dyn std::error::Error>>(())
    };
    let result = tokio::time::timeout(Duration::from_secs(60), work).await;
    let cleanup = notebook.shutdown_session().await;
    result??;
    cleanup?;
    Ok(())
}
