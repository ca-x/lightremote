//! Host adapter shared by the CLI and application acceptance tests.
use anyhow::{Context, Result};
use lightremote_core::IncomingStream;
use lightremote_services::HostServices;
use serde_json::json;

mod approval;
pub mod host;
pub mod update;

/// Bind host-owned stream identity and cancellation to one application service.
/// RESET cancels a service even after its input FIN, while successful completion
/// alone authorizes the runtime to emit the response's final FIN.
pub async fn serve_stream(services: HostServices, mut incoming: IncomingStream) -> Result<()> {
    if incoming.route == "shell" {
        incoming
            .metadata
            .as_object_mut()
            .context("metadata must be object")?
            .insert("stream_id".into(), json!(incoming.stream.id));
    }
    let mut cancelled = incoming.stream.cancellation();
    let result = tokio::select! {
        result=services.serve(&incoming.route,incoming.metadata,incoming.stream)=>result,
        _=cancelled.cancelled()=>Err(anyhow::anyhow!("stream cancelled")),
    };
    incoming.completion.finish(result.is_ok());
    result
}
