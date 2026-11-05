//! Build-time gates for unfinished UI. Keep release defaults production-safe.

/// Ordinary development builds show prototypes; release builds require opt-in.
pub const UNFINISHED: bool = cfg!(any(debug_assertions, feature = "unfinished"));
