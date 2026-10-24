use provider::{LlmError, LlmErrorKind};
use reqwest::{header::HeaderMap, StatusCode};
use serde_json::Value;
use std::time::Duration;

pub(crate) fn from_transport(error: reqwest::Error) -> LlmError {
    let status = error.status();
    LlmError { kind: classify_status(status, None, None), message: error.to_string(), status: status.map(|s| s.as_u16()), request_id: None }
}

pub(crate) fn from_response(status: StatusCode, headers: &HeaderMap, body: String) -> LlmError {
    let kind = classify_status(Some(status), body_kind(&body), retry_after(headers));
    LlmError {
        kind,
        message: format!("HTTP {}: {}", status, body),
        status: Some(status.as_u16()),
        request_id: headers.get("x-request-id").and_then(|v| v.to_str().ok()).map(str::to_string),
    }
}

pub(crate) fn parse_error(message: String) -> LlmError {
    LlmError { kind: LlmErrorKind::Fatal, message, status: None, request_id: None }
}

pub(crate) fn stream_error(message: String, request_id: Option<String>) -> LlmError {
    LlmError { kind: LlmErrorKind::Transient { retry_after: None }, message, status: None, request_id }
}

fn body_kind(body: &str) -> Option<LlmErrorKind> {
    let value: Value = serde_json::from_str(body).ok()?;
    [value["error"]["type"].as_str(), value["error"]["code"].as_str()]
        .into_iter()
        .flatten()
        .find_map(|value| classify_body_type(Some(value)))
}

fn classify_status(status: Option<StatusCode>, body: Option<LlmErrorKind>, retry: Option<Duration>) -> LlmErrorKind {
    if let Some(kind) = body {
        return match kind {
            LlmErrorKind::Transient { .. } => LlmErrorKind::Transient { retry_after: retry },
            fatal => fatal,
        };
    }
    let transient = status.map(|s| s.is_server_error() || matches!(s.as_u16(), 408 | 409 | 429 | 529)).unwrap_or(true);
    if transient { LlmErrorKind::Transient { retry_after: retry } } else { LlmErrorKind::Fatal }
}

pub(crate) fn classify_body_type(value: Option<&str>) -> Option<LlmErrorKind> {
    match value? {
        "insufficient_quota" | "quota_exceeded" | "exceeded_current_quota_error" | "payment_required" => Some(LlmErrorKind::Fatal),
        "rate_limit_exceeded" | "rate_limit_reached_error" | "engine_overloaded_error" => Some(LlmErrorKind::Transient { retry_after: None }),
        _ => None,
    }
}

fn retry_after(headers: &HeaderMap) -> Option<Duration> {
    let seconds = headers.get("retry-after")?.to_str().ok()?.parse::<u64>().ok()?;
    (seconds <= 300).then(|| Duration::from_secs(seconds))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_status_and_provider_codes() {
        assert!(matches!(classify_status(Some(StatusCode::TOO_MANY_REQUESTS), classify_body_type(Some("rate_limit_exceeded")), None), LlmErrorKind::Transient { .. }));
        assert!(matches!(classify_status(Some(StatusCode::TOO_MANY_REQUESTS), classify_body_type(Some("insufficient_quota")), None), LlmErrorKind::Fatal));
        assert!(matches!(classify_status(Some(StatusCode::TOO_MANY_REQUESTS), classify_body_type(Some("exceeded_current_quota_error")), None), LlmErrorKind::Fatal));
        assert!(matches!(classify_status(Some(StatusCode::UNAUTHORIZED), None, None), LlmErrorKind::Fatal));
        assert!(matches!(classify_status(Some(StatusCode::SERVICE_UNAVAILABLE), None, None), LlmErrorKind::Transient { .. }));
        let transport = reqwest::Client::new().get("http://").build().unwrap_err();
        assert!(matches!(from_transport(transport).kind, LlmErrorKind::Transient { .. }));
    }

    #[test]
    fn parses_retry_after_with_limit() {
        let mut headers = HeaderMap::new();
        headers.insert("retry-after", "30".parse().unwrap());
        assert_eq!(retry_after(&headers), Some(Duration::from_secs(30)));
        headers.insert("retry-after", "99999".parse().unwrap());
        assert_eq!(retry_after(&headers), None);
    }
}
