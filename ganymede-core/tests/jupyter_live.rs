//! Run using `python scripts/test_jupyter.py` with Jupyter Server and ipykernel installed.
use ganymede_core::jupyter::{
    JupyterRestClient, KernelClient, KernelEvent, KernelEvents, ServerConfig,
    messaging::{ExecuteRequest, ExecutionTracker, KernelMessage},
    models::CreateSession,
};
use ganymede_core::notebook::{
    CellId, CellKind, ConnectionState, ExecutionPhase, NotebookController, NotebookDocument,
    NotebookSnapshot,
};
use serde_json::json;

async fn wait_notebook(
    controller: &NotebookController,
    predicate: impl Fn(&NotebookSnapshot) -> bool,
) {
    let mut snapshots = controller.watch();
    tokio::time::timeout(Duration::from_secs(30), async {
        loop {
            if predicate(&snapshots.borrow_and_update()) {
                return;
            }
            snapshots.changed().await.unwrap();
        }
    })
    .await
    .unwrap_or_else(|_| panic!("notebook wait timed out: {:?}", controller.snapshot()));
}
async fn run_notebook_cell(controller: &NotebookController, cell: &CellId, code: &str) {
    controller.update_source(cell, code).unwrap();
    let request = controller.run_cell(cell, false).await.unwrap();
    wait_notebook(controller, |snapshot| {
        snapshot
            .executions
            .get(cell)
            .is_some_and(|run| run.request_id == request && !run.phase.is_active())
    })
    .await;
}

#[tokio::test]
#[ignore = "requires disposable Jupyter server; run scripts/test_jupyter.py"]
async fn application_notebook_lifecycle() {
    let config = ServerConfig::new(&std::env::var("GANYMEDE_JUPYTER_URL").unwrap())
        .unwrap()
        .with_token(&std::env::var("GANYMEDE_JUPYTER_TOKEN").unwrap())
        .unwrap();
    let rest = JupyterRestClient::new(config.clone()).unwrap();
    let mut document = NotebookDocument::new();
    let markdown = document
        .insert_cell(0, CellKind::Markdown, "# Application test")
        .unwrap();
    let cell = document
        .insert_cell(1, CellKind::Code, "print('hello app'); 42")
        .unwrap();
    let controller =
        NotebookController::new(config.clone(), "application.ipynb", document).unwrap();
    controller.save().await.unwrap();
    controller.connect("python3").await.unwrap();
    let session = controller.snapshot().session.clone().unwrap();
    assert!(controller.run_cell(&markdown, false).await.is_err());
    run_notebook_cell(&controller, &cell, "print('hello app'); 42").await;
    assert_eq!(
        controller.snapshot().executions[&cell].phase,
        ExecutionPhase::Succeeded
    );
    let output = controller.snapshot().document.to_json()["cells"][1]["outputs"].clone();
    assert!(
        output
            .as_array()
            .unwrap()
            .iter()
            .any(|o| o["output_type"] == "execute_result" && o["data"]["text/plain"] == "42")
    );
    assert!(controller.snapshot().document.is_dirty());

    run_notebook_cell(&controller, &cell, "from IPython.display import display, update_display, clear_output\ndisplay('old', display_id='test-display')\nupdate_display('updated', display_id='test-display')").await;
    assert_eq!(
        controller.snapshot().document.to_json()["cells"][1]["outputs"][0]["data"]["text/plain"],
        "'updated'"
    );
    run_notebook_cell(
        &controller,
        &cell,
        "print('old')\nclear_output(wait=True)\nprint('replacement')",
    )
    .await;
    let output = controller.snapshot().document.to_json()["cells"][1]["outputs"].clone();
    assert_eq!(output.as_array().unwrap().len(), 1);
    assert_eq!(output[0]["text"], "replacement\n");
    run_notebook_cell(&controller, &cell, "raise ValueError('application error')").await;
    assert_eq!(
        controller.snapshot().executions[&cell].phase,
        ExecutionPhase::Failed
    );

    controller
        .update_source(&cell, "print(input('App input: '))")
        .unwrap();
    controller.run_cell(&cell, true).await.unwrap();
    wait_notebook(&controller, |snapshot| !snapshot.pending_inputs.is_empty()).await;
    let input = controller
        .snapshot()
        .pending_inputs
        .values()
        .next()
        .unwrap()
        .clone();
    assert_eq!(input.cell_id, cell);
    controller
        .reply_input(&input.id, "app answer")
        .await
        .unwrap();
    wait_notebook(&controller, |snapshot| {
        snapshot.executions[&cell].phase == ExecutionPhase::Succeeded
    })
    .await;
    assert!(controller.snapshot().pending_inputs.is_empty());

    controller
        .update_source(
            &cell,
            "import time\nprint('running', flush=True)\ntime.sleep(60)",
        )
        .unwrap();
    controller.run_cell(&cell, false).await.unwrap();
    wait_notebook(&controller, |snapshot| {
        !snapshot.document.cell(&cell).unwrap().outputs().is_empty()
    })
    .await;
    controller
        .update_source(&cell, "edited during execution")
        .unwrap();
    controller.interrupt().await.unwrap();
    wait_notebook(&controller, |snapshot| {
        snapshot.executions[&cell].phase == ExecutionPhase::Failed
    })
    .await;
    assert_eq!(
        controller.snapshot().document.cell(&cell).unwrap().source(),
        "edited during execution"
    );
    controller.restart_kernel().await.unwrap();
    assert_eq!(
        controller.snapshot().document.cell(&cell).unwrap().source(),
        "edited during execution"
    );
    run_notebook_cell(&controller, &cell, "print('after restart')").await;
    assert_eq!(
        controller.snapshot().executions[&cell].phase,
        ExecutionPhase::Succeeded
    );
    assert!(
        !controller
            .snapshot()
            .document
            .cell(&cell)
            .unwrap()
            .outputs()
            .is_empty()
    );
    controller.save().await.unwrap();
    let reopened = NotebookController::open(config.clone(), "application.ipynb")
        .await
        .unwrap();
    let mut reloaded = reopened.snapshot().document.to_json();
    // Jupyter adds its server-side trust assessment when loading code cells.
    assert!(reloaded["cells"][1]["metadata"]["trusted"].is_boolean());
    reloaded["cells"][1]["metadata"]
        .as_object_mut()
        .unwrap()
        .remove("trusted");
    assert_eq!(reloaded, controller.snapshot().document.to_json());
    reopened.attach_session(session.id.clone()).await.unwrap();
    reopened.disconnect().await.unwrap();
    assert!(rest.get_kernel(&session.kernel.id).await.is_ok());
    drop(reopened);

    controller
        .update_source(
            &cell,
            "import time\nprint('disconnect-ready', flush=True)\ntime.sleep(60)",
        )
        .unwrap();
    controller.run_cell(&cell, false).await.unwrap();
    wait_notebook(&controller, |snapshot| {
        !snapshot.document.cell(&cell).unwrap().outputs().is_empty()
    })
    .await;
    assert!(controller.remove_cell(&cell).is_err());
    controller.disconnect().await.unwrap();
    // Kernel deletion need not immediately close Jupyter's WebSocket.
    rest.delete_session(&session.id).await.unwrap();
    wait_notebook(&controller, |snapshot| {
        matches!(snapshot.connection, ConnectionState::Disconnected { .. })
    })
    .await;
    assert_eq!(
        controller.snapshot().executions[&cell].phase,
        ExecutionPhase::Unknown
    );
    assert!(controller.snapshot().document.is_dirty());
    assert!(
        controller
            .snapshot()
            .document
            .cell(&cell)
            .unwrap()
            .source()
            .contains("disconnect-ready")
    );

    controller.detach_session().await.unwrap();
    assert!(controller.snapshot().document.is_dirty());
    controller.connect("python3").await.unwrap();
    let kernel_id = controller
        .snapshot()
        .session
        .as_ref()
        .unwrap()
        .kernel
        .id
        .clone();
    controller.shutdown_session().await.unwrap();
    assert!(controller.snapshot().session.is_none());
    assert!(rest.get_kernel(&kernel_id).await.is_err());
}
use std::time::Duration;

async fn next(events: &mut KernelEvents) -> KernelMessage {
    match tokio::time::timeout(Duration::from_secs(30), events.recv())
        .await
        .expect("event timeout")
    {
        Some(KernelEvent::Message(message)) => *message,
        event => panic!("unexpected terminal event: {event:?}"),
    }
}

async fn finish(events: &mut KernelEvents, id: &str) -> Vec<KernelMessage> {
    let mut tracker = ExecutionTracker::new(id);
    let mut output = Vec::new();
    while !tracker.is_complete() {
        let message = next(events).await;
        tracker.observe(&message);
        if message.parent_id() == Some(id) {
            output.push(message);
        }
    }
    output
}

#[tokio::test]
#[ignore = "requires disposable Jupyter server; run scripts/test_jupyter.py"]
async fn notebook_lifecycle() {
    let config = ServerConfig::new(&std::env::var("GANYMEDE_JUPYTER_URL").unwrap())
        .unwrap()
        .with_token(&std::env::var("GANYMEDE_JUPYTER_TOKEN").unwrap())
        .unwrap();
    let rest = JupyterRestClient::new(config.clone()).unwrap();
    assert!(!rest.server_version().await.unwrap().version.is_empty());
    assert!(
        rest.list_kernel_specs()
            .await
            .unwrap()
            .kernelspecs
            .contains_key("python3")
    );
    let notebook =
        json!({"nbformat":4,"nbformat_minor":5,"metadata":{"custom":{"preserve":true}},"cells":[]});
    rest.save_notebook("test notebook.ipynb", &notebook)
        .await
        .unwrap();
    assert_eq!(
        rest.get_contents("test notebook.ipynb", true)
            .await
            .unwrap()
            .content,
        notebook
    );
    let session = rest
        .create_session(&CreateSession::notebook("test notebook.ipynb", "python3"))
        .await
        .unwrap();
    assert_eq!(
        rest.get_session(&session.id).await.unwrap().kernel.id,
        session.kernel.id
    );
    assert!(
        rest.list_sessions()
            .await
            .unwrap()
            .iter()
            .any(|s| s.id == session.id)
    );
    assert!(
        rest.list_kernels()
            .await
            .unwrap()
            .iter()
            .any(|k| k.id == session.kernel.id)
    );
    let (kernel, mut events) = KernelClient::connect(config.clone(), &session.kernel.id)
        .await
        .unwrap();
    let info = kernel.kernel_info().await.unwrap();
    loop {
        let message = next(&mut events).await;
        if message.parent_id() == Some(&info) && message.header.msg_type == "kernel_info_reply" {
            break;
        }
    }
    let id = kernel
        .execute(ExecuteRequest::new("print('hello from ganymede')\n6 * 7"))
        .await
        .unwrap();
    let output = finish(&mut events, &id).await;
    assert!(output.iter().any(|m| {
        m.header.msg_type == "stream"
            && m.content["text"]
                .as_str()
                .unwrap()
                .contains("hello from ganymede")
    }));
    assert!(
        output
            .iter()
            .any(|m| m.header.msg_type == "execute_result"
                && m.content["data"]["text/plain"] == "42")
    );
    let id = kernel
        .execute(ExecuteRequest::new(
            "raise ValueError('expected test error')",
        ))
        .await
        .unwrap();
    assert!(
        finish(&mut events, &id)
            .await
            .iter()
            .any(|m| m.header.msg_type == "error" && m.content["ename"] == "ValueError")
    );

    let mut request = ExecuteRequest::new("answer = input('Your answer? ')\nprint(answer)");
    request.allow_stdin = true;
    let id = kernel.execute(request).await.unwrap();
    loop {
        let message = next(&mut events).await;
        if message.header.msg_type == "input_request" {
            kernel.input_reply(&message, "from client").await.unwrap();
            break;
        }
    }
    assert!(
        finish(&mut events, &id)
            .await
            .iter()
            .any(|m| m.header.msg_type == "stream"
                && m.content["text"].as_str().unwrap().contains("from client"))
    );

    let id = kernel
        .execute(ExecuteRequest::new(
            "import time\nprint('interrupt-ready', flush=True)\ntime.sleep(60)",
        ))
        .await
        .unwrap();
    loop {
        let message = next(&mut events).await;
        if message.parent_id() == Some(&id) && message.header.msg_type == "stream" {
            break;
        }
    }
    rest.interrupt_kernel(&session.kernel.id).await.unwrap();
    assert!(
        finish(&mut events, &id)
            .await
            .iter()
            .any(|m| m.header.msg_type == "error" && m.content["ename"] == "KeyboardInterrupt")
    );
    let frontend_session = kernel.session_id().to_owned();
    kernel.close().await.unwrap();
    rest.restart_kernel(&session.kernel.id).await.unwrap();
    let (kernel, mut events) =
        KernelClient::connect_with_session(config, &session.kernel.id, frontend_session)
            .await
            .unwrap();
    let info = kernel.kernel_info().await.unwrap();
    loop {
        let message = next(&mut events).await;
        if message.parent_id() == Some(&info) && message.header.msg_type == "kernel_info_reply" {
            break;
        }
    }
    let id = kernel
        .execute(ExecuteRequest::new("print('after restart')"))
        .await
        .unwrap();
    assert!(
        finish(&mut events, &id)
            .await
            .iter()
            .any(|m| m.header.msg_type == "stream")
    );
    kernel.close().await.unwrap();
    rest.delete_session(&session.id).await.unwrap();
    assert!(
        !rest
            .list_kernels()
            .await
            .unwrap()
            .iter()
            .any(|k| k.id == session.kernel.id)
    );
    let standalone = rest.start_kernel("python3", None).await.unwrap();
    assert_eq!(
        rest.get_kernel(&standalone.id).await.unwrap().id,
        standalone.id
    );
    rest.shutdown_kernel(&standalone.id).await.unwrap();
}

#[tokio::test]
#[ignore = "requires disposable Jupyter server; run scripts/test_jupyter.py"]
async fn switching_kernels_preserves_notebooks_and_isolates_sessions() {
    let config = ServerConfig::new(&std::env::var("GANYMEDE_JUPYTER_URL").unwrap())
        .unwrap()
        .with_token(&std::env::var("GANYMEDE_JUPYTER_TOKEN").unwrap())
        .unwrap();
    let rest = JupyterRestClient::new(config.clone()).unwrap();
    let mut document = NotebookDocument::new();
    let cell = document
        .insert_cell(0, CellKind::Code, "answer = 42\nanswer")
        .unwrap();
    document.set_metadata(
        serde_json::from_value(json!({"vendor": {"keep": true}, "kernelspec": {"custom": 17}}))
            .unwrap(),
    );
    let first = NotebookController::new(config.clone(), "switch-first.ipynb", document).unwrap();
    let second =
        NotebookController::new(config, "switch-second.ipynb", NotebookDocument::new()).unwrap();
    first.connect("python3").await.unwrap();
    second.connect("python3").await.unwrap();
    let original = first.snapshot().session.clone().unwrap();
    let other = second.snapshot().session.clone().unwrap();
    assert_ne!(original.kernel.id, other.kernel.id);
    run_notebook_cell(&first, &cell, "answer = 42\nanswer").await;
    let before = first.snapshot().document.to_json();
    assert!(
        first
            .change_kernel("missing-ganymede-kernel".into())
            .await
            .is_err()
    );
    assert_eq!(first.snapshot().connection, ConnectionState::Connected);
    assert_eq!(
        first.snapshot().session.as_ref().unwrap().kernel.id,
        original.kernel.id
    );
    assert_eq!(first.snapshot().document.to_json(), before);
    first.change_kernel("python3".into()).await.unwrap();
    let replacement = first.snapshot().session.clone().unwrap();
    assert_eq!(replacement.id, original.id);
    assert_ne!(replacement.kernel.id, original.kernel.id);
    assert!(rest.get_kernel(&original.kernel.id).await.is_err());
    assert!(rest.get_kernel(&other.kernel.id).await.is_ok());
    let after = first.snapshot().document.to_json();
    assert_eq!(after["cells"], before["cells"]);
    assert_eq!(after["metadata"]["vendor"], before["metadata"]["vendor"]);
    assert_eq!(after["metadata"]["kernelspec"]["custom"], 17);
    assert_eq!(after["metadata"]["kernelspec"]["name"], "python3");
    run_notebook_cell(&first, &cell, "'answer' not in globals()").await;
    assert_eq!(
        first.snapshot().document.to_json()["cells"][0]["outputs"][0]["data"]["text/plain"],
        "True"
    );
    // A server-side shutdown must be recoverable by an explicit reconnect.
    rest.shutdown_kernel(&replacement.kernel.id).await.unwrap();
    first.disconnect().await.unwrap();
    first.connect("python3").await.unwrap();
    assert_ne!(
        first.snapshot().session.as_ref().unwrap().kernel.id,
        replacement.kernel.id
    );
    first.shutdown_session().await.unwrap();
    second.shutdown_session().await.unwrap();
}
