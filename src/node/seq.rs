use chrono::Utc;
use reqwest::Client;
use serde_json::json;
use std::collections::HashMap;
use std::error::Error;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tracing::{Event, Subscriber};
use tracing_subscriber::layer::{Context, Layer};

/// Cap on how long a single log shipment may take. Without it an unreachable Seq host leaves every
/// spawned shipment hanging on reqwest's default (which is no timeout at all), so one task per log
/// line accumulates for as long as the outage lasts.
const SEND_TIMEOUT: Duration = Duration::from_secs(5);

/// Whether the sink is currently believed to be down. Used only to decide whether to *print* about
/// it, so one outage produces one line instead of one per log event.
///
/// A process-wide static rather than state on the layer: there is exactly one logging pipeline, and
/// the detached shipment tasks must be able to read it without holding anything.
static SINK_DOWN: AtomicBool = AtomicBool::new(false);

pub struct SeqLogger {
    seq_url: String,
    api_key: String,
    client: Client,
}

impl SeqLogger {
    pub fn new(seq_url: &str, api_key: &str) -> Self {
        SeqLogger {
            seq_url: seq_url.to_string(),
            api_key: api_key.to_string(),
            client: Client::builder()
                .timeout(SEND_TIMEOUT)
                .build()
                .unwrap_or_else(|_| Client::new()),
        }
    }

    pub async fn log_to_seq(
        &self,
        message: &str,
        level: &str,
        fields: &serde_json::Value,
    ) -> Result<(), Box<dyn Error>> {
        let mut event = json!({
            "@t": Utc::now().to_rfc3339(),  // Timestamp
            "@mt": message,   // Message template
            "@l": level,      // Log level
        });

        if let Some(fields_map) = fields.as_object() {
            for (key, value) in fields_map {
                event[key] = value.clone();
            }
        }

        let seq_address = format!("{}/ingest/clef", self.seq_url);
        let payload = format!("{}\n", event);
        let mut request = self
            .client
            .post(&seq_address)
            .header("Content-Type", "application/vnd.serilog.clef");

        request = request.header("X-Seq-ApiKey", self.api_key.to_string());

        let response = request.body(payload).send().await?;

        if response.status().is_success() {
            Ok(())
        } else {
            let error_message = response.text().await?;
            Err(format!("Failed to send log: {}", error_message).into())
        }
    }
}

/// Report a shipment outcome WITHOUT going through `tracing`.
///
/// That is the whole reason these are `eprintln!` and not `error!`: this code runs inside the
/// tracing pipeline. Reporting a failed shipment through `tracing` re-enters `on_event`, which
/// spawns another shipment, which fails, which reports again — a self-amplifying storm triggered by
/// nothing worse than the log host being down. Going straight to stderr breaks the cycle.
///
/// Edge-triggered on the down/up transition, so an outage costs one line rather than one per log
/// event, while still never failing silently.
fn report_transition(now_down: bool, detail: &str) {
    let was_down = SINK_DOWN.swap(now_down, Ordering::Relaxed);
    if now_down && !was_down {
        eprintln!("seq: log sink unreachable, dropping log shipments until it recovers ({detail})");
    } else if !now_down && was_down {
        eprintln!("seq: log sink recovered, resuming log shipments");
    }
}

pub struct SeqLayer {
    logger: Arc<SeqLogger>,
}

impl SeqLayer {
    pub fn new(logger: Arc<SeqLogger>) -> Self {
        Self { logger }
    }
}

impl<S> Layer<S> for SeqLayer
where
    S: Subscriber,
{
    fn on_event(&self, event: &Event<'_>, _ctx: Context<'_, S>) {
        let logger = self.logger.clone();

        // Create a JSON object to hold the fields
        let mut fields_map = HashMap::new();

        // Record the fields from the event using the correct closure signature
        event.record(
            &mut |field: &tracing::field::Field, value: &dyn std::fmt::Debug| {
                fields_map.insert(field.name().to_string(), format!("{:?}", value));
            },
        );

        // A String->String map always serializes; fall back rather than unwrap, so that no path
        // through the logger can panic the process.
        let fields_json = serde_json::to_value(fields_map).unwrap_or_else(|_| json!({}));

        let message = format!("Log event: {}", event.metadata().target());
        let level = event.metadata().level().as_str();

        // Ship asynchronously and detached: getting logs out must never block or fail the caller's
        // actual work.
        //
        // `spawn` needs a runtime, and `on_event` can fire from a non-async context (startup, or a
        // plain `std::thread`) where `tokio::spawn` PANICS — panicking inside the logger is
        // precisely what must not happen. Ship only when a runtime is present; otherwise drop the
        // shipment, since the `fmt` layer has already put the event on stdout.
        if tokio::runtime::Handle::try_current().is_err() {
            return;
        }
        tokio::spawn(async move {
            // Never `unwrap` here. It used to, so one unreachable log host panicked a task per log
            // line — and the panic output itself re-entered the logger.
            match logger.log_to_seq(&message, level, &fields_json).await {
                Ok(()) => report_transition(false, ""),
                Err(e) => report_transition(true, &e.to_string()),
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The property that matters: a failing shipment returns an error instead of panicking. Points
    /// at a closed port, exercising the real unreachable-host path with no fake server needed.
    #[tokio::test]
    async fn unreachable_sink_returns_err_and_never_panics() {
        let logger = SeqLogger::new("http://127.0.0.1:1", "no-key");
        let result = logger.log_to_seq("msg", "INFO", &json!({"k": "v"})).await;
        assert!(result.is_err(), "an unreachable sink must surface an error, not panic");
    }

    /// Only the down->up and up->down transitions print — this is what bounds an outage to one
    /// line instead of one per log event.
    #[test]
    fn report_transition_is_edge_triggered() {
        SINK_DOWN.store(false, Ordering::Relaxed);

        report_transition(true, "first failure");
        assert!(SINK_DOWN.load(Ordering::Relaxed), "first failure must latch the down state");

        // Still down: no transition, so nothing further is printed.
        report_transition(true, "same outage");
        assert!(SINK_DOWN.load(Ordering::Relaxed));

        report_transition(false, "");
        assert!(!SINK_DOWN.load(Ordering::Relaxed), "a success must clear the down state");

        // Leave the static as other tests expect to find it.
        SINK_DOWN.store(false, Ordering::Relaxed);
    }

    /// `on_event` must not panic when no tokio runtime is present. That is the startup path, and a
    /// panic in the logger takes the node down before it can report why.
    #[test]
    fn layer_does_not_panic_without_a_runtime() {
        use tracing_subscriber::layer::SubscriberExt;

        let layer = SeqLayer::new(Arc::new(SeqLogger::new("http://127.0.0.1:1", "k")));
        let subscriber = tracing_subscriber::registry().with(layer);
        tracing::subscriber::with_default(subscriber, || {
            tracing::info!(field = "value", "emitted with no runtime running");
        });
    }
}
