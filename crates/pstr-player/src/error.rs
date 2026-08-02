use thiserror::Error;

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, Error)]
pub enum Error {
    /// Anything libmpv itself refused: a bad option, an unknown property, a
    /// file it could not load.
    #[error("mpv: {0}")]
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
