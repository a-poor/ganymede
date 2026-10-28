//! Execute code in a temporary kernel on an existing Jupyter server.
use ganymede_core::jupyter::{
    JupyterRestClient, KernelClient, KernelEvent, ServerConfig,
    messaging::{ExecuteRequest, ExecutionTracker},
};
use std::time::Duration;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let url = std::env::var("GANYMEDE_JUPYTER_URL")?;
    let mut config = ServerConfig::new(&url)?;
    if let Ok(token) = std::env::var("GANYMEDE_JUPYTER_TOKEN") {
        config = config.with_token(&token)?;
    }
    let rest = JupyterRestClient::new(config.clone())?;
    let specs = rest.list_kernel_specs().await?;
    let name = std::env::var("GANYMEDE_KERNEL_NAME").unwrap_or(specs.default);
    let model = rest.start_kernel(&name, None).await?;
    let result = tokio::time::timeout(Duration::from_secs(60), async {
        let (kernel, mut events) = KernelClient::connect(config, &model.id).await?;
        let code = std::env::args()
            .nth(1)
            .unwrap_or_else(|| "print('Hello from Ganymede')\n6 * 7".into());
        let id = kernel.execute(ExecuteRequest::new(code)).await?;
        let mut tracker = ExecutionTracker::new(id);
        let result = loop {
            match events.recv().await {
                Some(KernelEvent::Message(message)) => {
                    tracker.observe(&message);
                    println!(
                        "{} {}: {}",
                        message.channel, message.header.msg_type, message.content
                    );
                    if tracker.is_complete() {
                        break Ok(());
                    }
                }
                Some(KernelEvent::Failed(error)) => break Err(error),
                _ => break Err(ganymede_core::jupyter::Error::Disconnected),
            }
        };
        let closed = kernel.close().await;
        result.and(closed)
    })
    .await;
    // Clean up even after connection, protocol, or execution timeout errors.
    let cleanup = rest.shutdown_kernel(&model.id).await;
    result??;
    cleanup?;
    Ok(())
}
