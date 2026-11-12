//! The terminal's scrollbar and its "scroll to bottom" control: where the
//! thumb lies for a viewport position, and which position a thumb place means.
use crate::terminal_engine::Scrollback;

/// The track's width; the thumb is drawn narrower inside it.
pub(super) const TRACK_WIDTH: f32 = 10.;
const MIN_THUMB: f32 = 24.;

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Thumb {
    pub top: f32,
    pub height: f32,
}

/// The thumb in a track of `track` pixels, or none when every row is visible.
pub(super) fn thumb(track: f32, scrollback: Scrollback) -> Option<Thumb> {
    let hidden = scrollback.total.checked_sub(scrollback.rows)?;
    if hidden == 0 || track <= MIN_THUMB {
        return None;
    }
    let height = (track * scrollback.rows as f32 / scrollback.total as f32).clamp(MIN_THUMB, track);
    let position = scrollback.offset.min(hidden) as f32 / hidden as f32;
    Some(Thumb {
        top: (track - height) * position,
        height,
    })
}

/// The viewport's first row when the thumb's top is put at `top`.
pub(super) fn offset_at(track: f32, scrollback: Scrollback, top: f32) -> u64 {
    let hidden = scrollback.total.saturating_sub(scrollback.rows);
    let Some(thumb) = thumb(track, scrollback) else {
        return hidden;
    };
    let travel = track - thumb.height;
    if travel <= 0. {
        return hidden;
    }
    let position = (top / travel).clamp(0., 1.);
    (position * hidden as f32).round() as u64
}

/// The count on the control, shortened from a thousand rows on.
pub(super) fn count_label(rows: u64) -> String {
    if rows >= 1000 {
        format!("{}k", rows / 1000)
    } else {
        rows.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(total: u64, offset: u64, rows: u64) -> Scrollback {
        Scrollback {
            total,
            offset,
            rows,
        }
    }

    #[test]
    fn no_thumb_without_hidden_rows_or_room() {
        assert_eq!(thumb(400., at(24, 0, 24)), None);
        assert_eq!(thumb(400., at(10, 0, 24)), None);
        assert_eq!(thumb(20., at(100, 0, 24)), None);
    }

    #[test]
    fn thumb_spans_the_visible_share_and_reaches_both_ends() {
        let top = thumb(400., at(100, 0, 25)).expect("thumb");
        assert_eq!((top.top, top.height), (0., 100.));
        let bottom = thumb(400., at(100, 75, 25)).expect("thumb");
        assert_eq!(bottom.top + bottom.height, 400.);
        // A long history keeps a thumb that can be grabbed.
        let long = thumb(400., at(100_000, 99_976, 24)).expect("thumb");
        assert_eq!((long.top, long.height), (376., 24.));
    }

    #[test]
    fn a_thumb_place_names_the_row_it_was_drawn_for() {
        for offset in [0, 1, 37, 74, 75] {
            let scrollback = at(100, offset, 25);
            let drawn = thumb(400., scrollback).expect("thumb");
            assert_eq!(offset_at(400., scrollback, drawn.top), offset);
        }
        let scrollback = at(100, 40, 25);
        assert_eq!(offset_at(400., scrollback, -50.), 0);
        assert_eq!(offset_at(400., scrollback, 900.), 75);
    }

    #[test]
    fn count_is_shortened_from_a_thousand() {
        assert_eq!(count_label(7), "7");
        assert_eq!(count_label(999), "999");
        assert_eq!(count_label(1000), "1k");
        assert_eq!(count_label(12_900), "12k");
    }
}
