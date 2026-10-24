use async_openai::error::{ApiErrorResponse, OpenAIError};
use provider::{LlmError, LlmErrorKind};
use reqwest::StatusCode;
#[cfg(test)]
use reqwest::header::HeaderMap;
#[cfg(test)]
use std::time::Duration;

pub(crate) fn from_error(error: OpenAIError) -> LlmError {
    match error {
        OpenAIError::ApiError(ApiErrorResponse { status_code, api_error }) => {
            let code = api_error.code.as_deref().or(api_error.r#type.as_deref());
            LlmError {
                kind: classify_status(Some(status_code), code),
                message: api_error.to_string(),
                status: Some(status_code.as_u16()),
                request_id: None,
            }
        }
        OpenAIError::Reqwest(error) => {
            let status = error.status();
            LlmError { kind: classify_status(status, None), message: error.to_string(), status: status.map(|s| s.as_u16()), request_id: None }
        }
        OpenAIError::JSONDeserialize(error, body) => LlmError {
            kind: LlmErrorKind::Fatal,
            message: format!("failed to deserialize api response: error:{error} content:{body}"),
            status: None,
            request_id: None,
        },
        OpenAIError::InvalidArgument(message) => LlmError { kind: LlmErrorKind::Fatal, message: format!("invalid args: {message}"), status: None, request_id: None },
        other => LlmError { kind: LlmErrorKind::Transient { retry_after: None }, message: other.to_string(), status: None, request_id: None },
    }
}

pub(crate) fn in_band(message: String, body_type: Option<&str>) -> LlmError {
    LlmError { kind: classify_body_type(body_type).unwrap_or(LlmErrorKind::Transient { retry_after: None }), message, status: None, request_id: None }
}

pub(crate) fn in_band_fields(message: String, error_type: Option<&str>, code: Option<&str>) -> LlmError {
    let kind = [error_type, code]
        .into_iter()
        .flatten()
        .find_map(|value| classify_body_type(Some(value)))
        .unwrap_or(LlmErrorKind::Transient { retry_after: None });
    LlmError { kind, message, status: None, request_id: None }
}

fn classify_status(status: Option<StatusCode>, body: Option<&str>) -> LlmErrorKind {
    if let Some(kind) = classify_body_type(body) { return kind; }
    let transient = status.map(|s| s.is_server_error() || matches!(s.as_u16(), 408 | 409 | 429 | 529)).unwrap_or(true);
    if transient { LlmErrorKind::Transient { retry_after: None } } else { LlmErrorKind::Fatal }
}

pub(crate) fn classify_body_type(value: Option<&str>) -> Option<LlmErrorKind> {
    match value? {
        "insufficient_quota" | "quota_exceeded" | "exceeded_current_quota_error" | "payment_required" => Some(LlmErrorKind::Fatal),
        "rate_limit_exceeded" | "rate_limit_reached_error" | "engine_overloaded_error" => Some(LlmErrorKind::Transient { retry_after: None }),
        _ => None,
    }
}

#[cfg(test)]
fn retry_after(headers: &HeaderMap) -> Option<Duration> {
    let seconds = headers.get("retry-after")?.to_str().ok()?.parse::<u64>().ok()?;
    (seconds <= 300).then(|| Duration::from_secs(seconds))
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_openai::error::StreamError;

    #[test]
    fn classifies_openai_compatible_codes() {
        assert!(matches!(classify_status(Some(StatusCode::TOO_MANY_REQUESTS), Some("rate_limit_exceeded")), LlmErrorKind::Transient { .. }));
        assert!(matches!(classify_status(Some(StatusCode::TOO_MANY_REQUESTS), Some("insufficient_quota")), LlmErrorKind::Fatal));
        assert!(matches!(classify_status(Some(StatusCode::TOO_MANY_REQUESTS), Some("exceeded_current_quota_error")), LlmErrorKind::Fatal));
        assert!(matches!(classify_status(Some(StatusCode::UNAUTHORIZED), None), LlmErrorKind::Fatal));
        assert!(matches!(classify_status(Some(StatusCode::SERVICE_UNAVAILABLE), None), LlmErrorKind::Transient { .. }));
        assert!(matches!(from_error(OpenAIError::StreamError(Box::new(StreamError::EventStream("dropped".to_string())))).kind, LlmErrorKind::Transient { .. }));
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
