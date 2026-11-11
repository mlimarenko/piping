use super::current_trace_and_span_ids;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LogContext {
    pub request_id: Option<String>,
    pub correlation_id: Option<String>,
    pub trace_id: Option<String>,
    pub span_id: Option<String>,
    pub user_id: Option<String>,
    pub session_id: Option<String>,
    pub client_ip: Option<String>,
}

impl LogContext {
    pub fn from_current_span() -> Self {
        let (trace_id, span_id) = current_trace_and_span_ids();
        Self {
            trace_id,
            span_id,
            ..Self::default()
        }
    }

    pub fn with_correlation_id(mut self, correlation_id: impl Into<String>) -> Self {
        let correlation_id = correlation_id.into();
        self.request_id = Some(correlation_id.clone());
        self.correlation_id = Some(correlation_id);
        self
    }

    pub fn with_user_id(mut self, user_id: impl Into<String>) -> Self {
        self.user_id = Some(user_id.into());
        self
    }

    pub fn with_session_id(mut self, session_id: impl Into<String>) -> Self {
        self.session_id = Some(session_id.into());
        self
    }

    pub fn with_client_ip(mut self, client_ip: impl Into<String>) -> Self {
        self.client_ip = Some(client_ip.into());
        self
    }

    pub fn merge_missing(mut self, fallback: &Self) -> Self {
        if self.request_id.is_none() {
            self.request_id = fallback.request_id.clone();
        }
        if self.correlation_id.is_none() {
            self.correlation_id = fallback.correlation_id.clone();
        }
        if self.trace_id.is_none() {
            self.trace_id = fallback.trace_id.clone();
        }
        if self.span_id.is_none() {
            self.span_id = fallback.span_id.clone();
        }
        if self.user_id.is_none() {
            self.user_id = fallback.user_id.clone();
        }
        if self.session_id.is_none() {
            self.session_id = fallback.session_id.clone();
        }
        if self.client_ip.is_none() {
            self.client_ip = fallback.client_ip.clone();
        }
        self
    }

    pub fn correlation_id_or_unknown(&self) -> &str {
        self.correlation_id.as_deref().unwrap_or("unknown")
    }

    pub fn request_id_or_unknown(&self) -> &str {
        self.request_id.as_deref().unwrap_or("unknown")
    }
}

#[cfg(test)]
mod tests {
    use super::LogContext;

    #[test]
    fn with_correlation_id_sets_request_and_correlation() {
        let ctx = LogContext::default().with_correlation_id("req-1");
        assert_eq!(ctx.request_id.as_deref(), Some("req-1"));
        assert_eq!(ctx.correlation_id.as_deref(), Some("req-1"));
    }

    #[test]
    fn unknown_helpers_fallback_when_missing() {
        let ctx = LogContext::default();
        assert_eq!(ctx.request_id_or_unknown(), "unknown");
        assert_eq!(ctx.correlation_id_or_unknown(), "unknown");
    }

    #[test]
    fn merge_missing_prefers_existing_values() {
        let primary = LogContext::default()
            .with_correlation_id("primary-correlation")
            .with_user_id("primary-user");
        let secondary = LogContext::default()
            .with_correlation_id("secondary-correlation")
            .with_user_id("secondary-user")
            .with_client_ip("127.0.0.1");

        let merged = primary.merge_missing(&secondary);
        assert_eq!(
            merged.correlation_id.as_deref(),
            Some("primary-correlation")
        );
        assert_eq!(merged.user_id.as_deref(), Some("primary-user"));
        assert_eq!(merged.client_ip.as_deref(), Some("127.0.0.1"));
    }
}
