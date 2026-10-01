//! Task-local immutable metadata; never accepts free-form messages or secrets.
use crate::{
    log_settings::LogLevel,
    system_log::{
        EventKind, EventReason, EventStage, SystemEvent, SystemLogGeneration, SystemLogSink,
    },
};
use std::time::Instant;

#[derive(Clone)]
pub struct Context {
    sink: SystemLogSink,
    generation: SystemLogGeneration,
    operation_id: Option<String>,
    account_id: Option<String>,
    started: Instant,
    pub diagnostics: bool,
}
tokio::task_local! { pub static CONTEXT: Context; }
impl Context {
    pub fn new(
        sink: SystemLogSink,
        operation_id: Option<String>,
        account_id: Option<String>,
        diagnostics: bool,
    ) -> Self {
        let generation = sink.capture_generation();
        Self {
            sink,
            generation,
            operation_id,
            account_id,
            started: Instant::now(),
            diagnostics,
        }
    }
    pub fn emit(
        &self,
        level: LogLevel,
        kind: EventKind,
        stage: Option<EventStage>,
        reason: Option<EventReason>,
        status: Option<u16>,
        diagnostics: Option<crate::diagnostics::LoginDiagnostics>,
    ) {
        let mut event = SystemEvent::new(level, kind);
        event.operation_id = self.operation_id.clone();
        event.account_id = self.account_id.clone();
        event.stage = stage;
        event.reason = reason;
        event.http_status = status;
        event.elapsed_ms = Some(self.started.elapsed().as_millis().min(u64::MAX as u128) as u64);
        event.diagnostics = diagnostics;
        self.sink.try_enqueue_captured(event, self.generation);
        report_failure(self.sink.failed());
    }
}
pub fn emit(
    level: LogLevel,
    kind: EventKind,
    stage: EventStage,
    reason: Option<EventReason>,
    status: Option<u16>,
) {
    let _ =
        CONTEXT.try_with(|context| context.emit(level, kind, Some(stage), reason, status, None));
}
pub fn diagnostics_enabled() -> bool {
    CONTEXT.try_with(|c| c.diagnostics).unwrap_or(false)
}
pub fn helper_snapshot(value: Option<crate::diagnostics::LoginDiagnostics>) {
    if value.is_none() {
        return;
    }
    let _ = CONTEXT.try_with(|context| {
        context.emit(
            LogLevel::Debug,
            EventKind::HelperResult,
            Some(EventStage::HelperResult),
            None,
            None,
            value,
        )
    });
}

pub struct Completion(bool);
impl Completion {
    pub fn new() -> Self {
        Self(false)
    }
    pub fn finish(&mut self) {
        self.0 = true;
    }
}
impl Drop for Completion {
    fn drop(&mut self) {
        if !self.0 {
            emit(
                LogLevel::Warn,
                EventKind::OperationCancelled,
                EventStage::Cleanup,
                Some(EventReason::Cancelled),
                None,
            );
        }
    }
}
pub fn report_failure(failed: bool) {
    static LAST: std::sync::Mutex<Option<Instant>> = std::sync::Mutex::new(None);
    if failed
        && let Ok(mut last) = LAST.lock()
        && last.is_none_or(|t| t.elapsed().as_secs() >= 60)
    {
        *last = Some(Instant::now());
        eprintln!("system_logging_unavailable");
    }
}
