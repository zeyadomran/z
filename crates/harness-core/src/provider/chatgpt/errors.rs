use super::*;
pub(super) fn reason(status: u16, v: &Value) -> StopReason {
    let code = v["error"]["code"].as_str();
    match code {
        Some("subscription_sharing_usage_limit_exceeded") => StopReason::SubscriptionLimit,
        Some("subscription_sharing_user_not_eligible") => StopReason::AccessBlocked,
        Some(
            "subscription_sharing_usage_unavailable" | "subscription_sharing_user_unavailable",
        ) => StopReason::Unavailable,
        Some(
            "subscription_sharing_unsupported_capability"
            | "subscription_sharing_route_not_supported"
            | "chatpass_v2_scope_not_authorized"
            | "chatpass_v2_invalid_authorization_context",
        ) => StopReason::InvalidRequest,
        Some("context_length_exceeded") => StopReason::ContextLimit,
        Some("rate_limit_exceeded") => StopReason::Throttled,
        Some("subscription_sharing_invalid_user") => StopReason::Authentication,
        _ if status == 401 => StopReason::Authentication,
        None if status == 503 => StopReason::Unavailable,
        None if status == 403 => StopReason::AccessBlocked,
        _ => StopReason::Protocol,
    }
}
fn safe(v: Option<&str>) -> Option<String> {
    v.filter(|s| {
        !s.is_empty()
            && s.len() <= 128
            && s.bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"_-.[]".contains(&b))
    })
    .map(str::to_owned)
}
pub(super) fn diagnostic(
    p: &watch::Sender<Progress>,
    status: u16,
    v: &Value,
    id: Option<&str>,
    phase: &str,
) {
    p.send_modify(|p|p.diagnostic=Some(json!({"status":status,"code":safe(v["error"]["code"].as_str()),"param":safe(v["error"]["param"].as_str()),"request_id":safe(id),"phase":phase})));
}
