//! Panic capture: turn an unwinding thread into one structured critical record.
//!
//! The default hook writes a plain-text line to stderr. `docker logs` keeps it,
//! but VictoriaLogs and Tempo never see it: no severity, no `service.name`, no
//! `trace_id`, no stack they can index. This hook emits the same failure through
//! the structured pipeline first, with the trace context of the panicking
//! thread still entered, and keeps the default output as the fallback for the
//! case where that emission itself fails.

use std::any::Any;
use std::backtrace::Backtrace;
use std::cell::Cell;
use std::panic::{Location, PanicHookInfo};

use super::logging::{MAX_ERROR_STACK_CHARS, limit_chars};
use super::{EventFields, LogLevel, log_event};

/// Event name for a captured panic. Stable: alerts key on it.
pub(super) const PANIC_EVENT: &str = "system.panic";

thread_local! {
    /// Re-entrancy guard: a panic raised while emitting the record must not
    /// re-enter this hook.
    static EMITTING: Cell<bool> = const { Cell::new(false) };
}

/// Install the structured panic hook, keeping the previously installed hook as
/// the fallback for failures inside this one.
pub(super) fn install() {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        if EMITTING.with(|flag| flag.replace(true)) {
            previous(info);
            return;
        }
        let emitted = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            log_event(LogLevel::Critical, PANIC_EVENT, panic_fields(info));
        }))
        .is_ok();
        EMITTING.with(|flag| flag.set(false));
        if !emitted {
            previous(info);
        }
    }));
}

fn panic_fields(info: &PanicHookInfo<'_>) -> EventFields {
    fields_for_panic(info.payload(), info.location(), thread_name())
}

/// Structured fields for one panic: type, message, thread, source location and
/// a forced backtrace. Forced on purpose — the operator should not have to set
/// `RUST_BACKTRACE` to learn why the process died.
fn fields_for_panic(
    payload: &(dyn Any + Send),
    location: Option<&Location<'_>>,
    thread: String,
) -> EventFields {
    let (error_type, message) = panic_payload(payload);
    let mut fields = EventFields::default()
        .with_operation("panic")
        .with_status("failed")
        .with_context("thread", thread);
    if let Some(location) = location {
        fields = fields.with_context(
            "location",
            format!(
                "{}:{}:{}",
                location.file(),
                location.line(),
                location.column()
            ),
        );
    }
    fields
        .with_error(error_type, message)
        .with_error_stack(limit_chars(
            Backtrace::force_capture().to_string(),
            MAX_ERROR_STACK_CHARS,
        ))
}

fn panic_payload(payload: &(dyn Any + Send)) -> (&'static str, String) {
    if let Some(message) = payload.downcast_ref::<&'static str>() {
        return ("panic", (*message).to_string());
    }
    if let Some(message) = payload.downcast_ref::<String>() {
        return ("panic", message.clone());
    }
    // `panic_any` with a non-string payload: keep the event, say only that.
    ("panic", "non-string panic payload".to_string())
}

fn thread_name() -> String {
    std::thread::current()
        .name()
        .unwrap_or("unnamed")
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::{fields_for_panic, panic_payload, thread_name};

    #[test]
    fn a_panic_payload_keeps_its_message_and_type() {
        assert_eq!(panic_payload(&"boom"), ("panic", "boom".to_string()));
        assert_eq!(
            panic_payload(&String::from("owned boom")),
            ("panic", "owned boom".to_string())
        );
        assert_eq!(
            panic_payload(&42_u32),
            ("panic", "non-string panic payload".to_string())
        );
    }

    #[test]
    fn panic_fields_carry_a_forced_stack_and_the_panicking_thread() {
        let fields = fields_for_panic(&"boom", None, "worker-1".to_string());

        assert_eq!(fields.error_type.as_deref(), Some("panic"));
        assert_eq!(fields.error_message.as_deref(), Some("boom"));
        assert_eq!(fields.operation.as_deref(), Some("panic"));
        assert_eq!(fields.status.as_deref(), Some("failed"));
        assert_eq!(
            fields.context.get("thread").map(String::as_str),
            Some("worker-1")
        );
        assert!(
            fields
                .error_stack
                .as_deref()
                .is_some_and(|stack| stack.contains("panics.rs")),
            "the stack must point at the panicking frame: {:?}",
            fields.error_stack
        );
    }

    #[test]
    fn unnamed_threads_still_get_a_name() {
        assert!(!thread_name().is_empty());
    }
}
