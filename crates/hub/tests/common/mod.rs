pub fn authenticated_router(st: std::sync::Arc<blazar_hub::AppState>) -> axum::Router {
    let token = format!("Bearer {}", st.auth.get().unwrap().token());
    blazar_hub::build_router(st).layer(axum::middleware::map_request(
        move |mut request: axum::extract::Request| {
            let token = token.clone();
            async move {
                request
                    .headers_mut()
                    .insert(axum::http::header::AUTHORIZATION, token.parse().unwrap());
                if !request.headers().contains_key(axum::http::header::HOST) {
                    request
                        .headers_mut()
                        .insert(axum::http::header::HOST, "127.0.0.1:7777".parse().unwrap());
                }
                request
            }
        },
    ))
}
