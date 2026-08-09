//! Reading the chapter list out of the player.
//!
//! The chapters themselves — what a name claims, what the rest of the file
//! settles, where the tail begins — live in [`pstr_core::chapters`], because
//! Android reads its chapter list through its own JNI adapter and must reach
//! the same verdict as this crate does. Only the mpv read is here.
//!
//! Read the same way as the track list — `chapter-list/N/title`, one
//! sub-property at a time — for the same reason: those are plain strings and
//! doubles, and a node walk would buy nothing.

use libmpv2::Mpv;

pub use pstr_core::chapters::{
    Chapter, ChapterRole, chapter_at, chapter_end, credits_start, roles,
};

/// Every chapter of the file mpv currently has open.
pub(crate) fn read(mpv: &Mpv) -> Vec<Chapter> {
    let count: i64 = mpv.get_property("chapter-list/count").unwrap_or(0);
    (0..count)
        .map(|index| Chapter {
            index,
            title: mpv
                .get_property::<String>(&format!("chapter-list/{index}/title"))
                .ok()
                .map(|title| title.trim().to_string())
                .filter(|title| !title.is_empty()),
            start: mpv
                .get_property(&format!("chapter-list/{index}/time"))
                .unwrap_or(0.0),
        })
        .collect()
}
