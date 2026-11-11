use tracing::Span;

/// Set request_id attribute on the current span
pub fn set_request_id(request_id: &str) {
    Span::current().record("request_id", request_id);
}

/// Set user_id attribute on the current span
pub fn set_user_id(user_id: &str) {
    Span::current().record("user_id", user_id);
}

/// Set session_id attribute on the current span
pub fn set_session_id(session_id: &str) {
    Span::current().record("session_id", session_id);
}

/// Set source_id attribute on the current span.
pub fn set_source_id(source_id: &str) {
    Span::current().record("source_id", source_id);
}

/// Set webhook event id on the current span.
pub fn set_webhook_event_id(event_id: &str) {
    Span::current().record("webhook_event_id", event_id);
}

/// Set outbound event id on the current span.
pub fn set_outbound_event_id(event_id: &str) {
    Span::current().record("outbound_event_id", event_id);
}

/// Set sync flow outcome on the current span.
pub fn set_sync_outcome(outcome: &str) {
    Span::current().record("sync_outcome", outcome);
}
