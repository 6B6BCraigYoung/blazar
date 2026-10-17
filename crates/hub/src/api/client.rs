use super::*;

const MAX_REPORT_CHARS: usize = 4000;

pub async fn report_panic(body: String) -> StatusCode {
    let text: String = body.chars().take(MAX_REPORT_CHARS).collect();
    tracing::error!(target: "blazar::web", "前端崩溃：{text}");
    StatusCode::NO_CONTENT
}
