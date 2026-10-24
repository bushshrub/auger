use provider::{LlmError, LlmErrorKind};
use reqwest::{header::HeaderMap, StatusCode};
use serde_json::Value;
use std::time::Duration;

pub(crate) fn from_transport(error: reqwest::Error) -> LlmError {
    let status = error.status();
    LlmError {
        kind: classify_status(status, None),
        message: error.to_string(),
        status: status.map(|value| value.as_u16()),
        request_id: None,
    }
}

pub(crate) fn from_response(status: StatusCode, headers: &HeaderMap, body: String) -> LlmError {
    let body_kind = body_type(&body).as_deref().and_then(classify_body_type);
    let kind = with_retry_after(classify_status(Some(status), body_kind), headers);
    LlmError {
        kind,
        message: format!("HTTP {}: {}", status, body),
        status: Some(status.as_u16()),
        request_id: headers
            .get("request-id")
            .and_then(|value| value.to_str().ok())
            .map(str::to_string),
    }
}

pub(crate) fn parse_error(message: String) -> LlmError {
    LlmError {
        kind: LlmErrorKind::Fatal,
        message,
        status: None,
        request_id: None,
    }
}

pub(crate) fn stream_error(message: String, request_id: Option<String>, body_type: Option<&str>) -> LlmError {
    LlmError {
        kind: classify_status(None, body_type.and_then(classify_body_type)),
        message,
        status: None,
        request_id,
    }
}

pub(crate) fn body_type(body: &str) -> Option<String> {
    let value: Value = serde_json::from_str(body).ok()?;
    value["error"]["type"]
        .as_str()
        .or_else(|| value["type"].as_str())
        .map(str::to_string)
}

fn classify_status(status: Option<StatusCode>, body_kind: Option<LlmErrorKind>) -> LlmErrorKind {
    if let Some(kind) = body_kind {
        return kind;
    }
    let transient = status
        .map(|value| value.is_server_error() || matches!(value.as_u16(), 408 | 409 | 429 | 529))
        .unwrap_or(true);
    if transient {
        LlmErrorKind::Transient { retry_after: None }
    } else {
        LlmErrorKind::Fatal
    }
}

fn classify_body_type(value: &str) -> Option<LlmErrorKind> {
    match value {
        "billing_error" | "insufficient_quota" | "quota_exceeded" | "exceeded_current_quota_error" | "payment_required" => Some(LlmErrorKind::Fatal),
        "overloaded_error" | "rate_limit_exceeded" | "rate_limit_reached_error" | "engine_overloaded_error" => Some(LlmErrorKind::Transient { retry_after: None }),
        _ => None,
    }
}

pub(crate) fn retry_after(headers: &HeaderMap) -> Option<Duration> {
    let seconds = headers.get("retry-after")?.to_str().ok()?.parse::<u64>().ok()?;
    (seconds <= 300).then(|| Duration::from_secs(seconds))
}

pub(crate) fn with_retry_after(kind: LlmErrorKind, headers: &HeaderMap) -> LlmErrorKind {
    match kind {
        LlmErrorKind::Transient { .. } => LlmErrorKind::Transient {
            retry_after: retry_after(headers),
        },
        fatal => fatal,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_status_and_body() {
        assert!(matches!(classify_status(Some(StatusCode::TOO_MANY_REQUESTS), classify_body_type("rate_limit_exceeded")), LlmErrorKind::Transient { .. }));
        assert!(matches!(classify_status(Some(StatusCode::TOO_MANY_REQUESTS), classify_body_type("insufficient_quota")), LlmErrorKind::Fatal));
        assert!(matches!(classify_status(Some(StatusCode::TOO_MANY_REQUESTS), classify_body_type("exceeded_current_quota_error")), LlmErrorKind::Fatal));
        assert!(matches!(classify_status(Some(StatusCode::TOO_MANY_REQUESTS), classify_body_type("billing_error")), LlmErrorKind::Fatal));
        assert!(matches!(classify_status(Some(StatusCode::UNAUTHORIZED), None), LlmErrorKind::Fatal));
        assert!(matches!(classify_status(Some(StatusCode::SERVICE_UNAVAILABLE), None), LlmErrorKind::Transient { .. }));
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
