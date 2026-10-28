//! Run using `python scripts/test_jupyter.py` with Jupyter Server and ipykernel installed.
use ganymede_core::jupyter::{
    JupyterRestClient, KernelClient, KernelEvent, KernelEvents, ServerConfig,
    messaging::{ExecuteRequest, ExecutionTracker, KernelMessage},
    models::CreateSession,
};
use serde_json::json;
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
