use thiserror::Error;

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, Error)]
pub enum Error {
    /// Anything libmpv itself refused: a bad option, an unknown property, a
    /// file it could not load.
    #[error("mpv: {}", describe(.0))]
    Mpv(#[from] libmpv2::Error),

    /// A string on its way into mpv contained an interior NUL. Only reachable
    /// from a caller-supplied option or property name.
    #[error("{0} contains a NUL byte")]
    NulByte(&'static str),

    /// `mpv_render_context_create` reported success and handed back nothing.
    /// Not reachable through any documented path — it exists so the pointer
    /// can be a `NonNull` rather than something later code has to re-check.
    #[error("mpv returned no render context")]
    RenderContext,

    /// The block layer failed. Read errors reach the player as a short read
    /// instead; this is for the paths that surface them directly.
    #[error(transparent)]
    Stream(#[from] pstr_stream::Error),
}

/// What mpv calls an error, rather than libmpv2's `Raw(-8)`.
///
/// libmpv2 formats its errors with `Debug`, so a refused property reaches the
/// viewer as a bare number. mpv has a name for every one of them.
fn describe(error: &libmpv2::Error) -> String {
    match error {
        libmpv2::Error::Raw(code) => libmpv2_sys::mpv_error_str(*code).to_owned(),
        other => other.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_mpv_error_is_shown_by_name_rather_than_by_number() {
        let error = Error::from(libmpv2::Error::Raw(libmpv2::mpv_error::PropertyNotFound));
        assert_eq!(error.to_string(), "mpv: property not found");
    }
}
