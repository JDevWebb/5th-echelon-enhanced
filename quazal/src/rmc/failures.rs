//! Requests the server couldn't make sense of or failed to answer, passed to a hook the
//! server sets (it keeps them with the player's session events). Refusals (`AccessDenied`)
//! and calls the server doesn't implement (see [`super::unhandled`]) aren't failures here:
//! those are answers the server meant to give.

use std::sync::OnceLock;

/// A request that failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Failure {
    /// The signed-in player whose request it was.
    pub user_id: Option<u32>,
    /// The protocol and method, when the request got that far.
    pub protocol: Option<u16>,
    pub method: Option<u32>,
    /// "Protocol.Method", when known.
    pub call: Option<String>,
    pub error: String,
}

type Hook = Box<dyn Fn(Failure) + Send + Sync>;

static HOOK: OnceLock<Hook> = OnceLock::new();

/// Sets what is called with each failure (once; later calls are ignored).
pub fn set_hook(hook: impl Fn(Failure) + Send + Sync + 'static) {
    let _ = HOOK.set(Box::new(hook));
}

pub(crate) fn report(failure: Failure) {
    if let Some(hook) = HOOK.get() {
        hook(failure);
    }
}

/// Whether a handler's error is a failure rather than a refusal or an unimplemented call.
pub(crate) fn is_failure(e: &super::Error) -> bool {
    !matches!(
        e,
        super::Error::AccessDenied | super::Error::UnknownMethod | super::Error::UnimplementedMethod | super::Error::UnknownProtocol
    )
}
