/// Canonical backend event names used by structured logging.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CanonicalEvent {
    SystemBootstrapStarted,
    SystemBootstrapCompleted,
    HttpRequestReceived,
    HttpRequestCompleted,
    HttpRequestFailed,
    RestRouteReceived,
    RestRouteCompleted,
    RestRouteFailed,
    ConnectionSessionRequested,
    ConnectionSessionCreated,
    ConnectionSessionFailed,
    WsSessionConnected,
    WsSessionDisconnected,
    WsSessionFailed,
    NoiseSummary,
}

impl CanonicalEvent {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::SystemBootstrapStarted => "system.bootstrap.started",
            Self::SystemBootstrapCompleted => "system.bootstrap.completed",
            Self::HttpRequestReceived => "http.request.received",
            Self::HttpRequestCompleted => "http.request.completed",
            Self::HttpRequestFailed => "http.request.failed",
            Self::RestRouteReceived => "rest.route.received",
            Self::RestRouteCompleted => "rest.route.completed",
            Self::RestRouteFailed => "rest.route.failed",
            Self::ConnectionSessionRequested => "connection.session.requested",
            Self::ConnectionSessionCreated => "connection.session.created",
            Self::ConnectionSessionFailed => "connection.session.failed",
            Self::WsSessionConnected => "ws.session.connected",
            Self::WsSessionDisconnected => "ws.session.disconnected",
            Self::WsSessionFailed => "ws.session.failed",
            Self::NoiseSummary => "observability.noise.suppressed_summary",
        }
    }
}

/// Compose an event name in `<domain>.<action>.<outcome>` format.
pub fn compose_event_name(domain: &str, action: &str, outcome: &str) -> String {
    let sanitize = |value: &str| {
        value
            .trim()
            .to_ascii_lowercase()
            .replace([' ', '/', '-'], "_")
    };

    format!(
        "{}.{}.{}",
        sanitize(domain),
        sanitize(action),
        sanitize(outcome)
    )
}

#[cfg(test)]
mod tests {
    use super::{CanonicalEvent, compose_event_name};

    #[test]
    fn canonical_event_names_are_stable() {
        assert_eq!(
            CanonicalEvent::RestRouteCompleted.as_str(),
            "rest.route.completed"
        );
        assert_eq!(
            CanonicalEvent::WsSessionFailed.as_str(),
            "ws.session.failed"
        );
    }

    #[test]
    fn compose_event_name_normalizes_tokens() {
        assert_eq!(
            compose_event_name("REST", "Inventory Hosts", "Completed"),
            "rest.inventory_hosts.completed"
        );
    }
}
