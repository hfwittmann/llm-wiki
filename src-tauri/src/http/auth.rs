//! Session cookie middleware + `AuthUser` extractor.

use axum::extract::{FromRequestParts, State};
use axum::http::request::Parts;
use axum::middleware::Next;
use axum::response::Response;
use tower_cookies::Cookies;

use crate::auth::users::User;
use crate::http::error::ApiError;
use crate::http::AppState;

/// Extractor that yields the authenticated user, or 401 if missing.
///
/// `session_middleware` is responsible for placing the `User` into request
/// extensions. Routes mounted under the authed router get the middleware
/// automatically; legacy 127.0.0.1:19828 routes do not.
#[derive(Debug, Clone)]
pub struct AuthUser(pub User);

impl<S> FromRequestParts<S> for AuthUser
where
    S: Send + Sync,
{
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        parts
            .extensions
            .get::<User>()
            .cloned()
            .map(AuthUser)
            .ok_or_else(ApiError::unauthenticated)
    }
}

/// Middleware: read the session cookie, look up the session, look up the
/// user. On hit, inject `User` into request extensions. On miss, do nothing
/// (the request proceeds; only routes that extract `AuthUser` will reject).
pub async fn session_middleware(
    State(state): State<AppState>,
    cookies: Cookies,
    mut request: axum::extract::Request,
    next: Next,
) -> Response {
    if let Some(cookie) = cookies.get(&state.config.session_cookie_name) {
        if let Some(user_id) = state.sessions.lookup(cookie.value()) {
            if let Some(user) = state.users.lookup_user(&user_id) {
                request.extensions_mut().insert(user);
            }
        }
    }
    next.run(request).await
}
