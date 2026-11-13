//! Bounded terminal interpretation only: URL policy and platform opening live elsewhere.
use super::{check, ffi, Engine, LinkHit, LinkKind, LinkRun, LinkSpan};
use anyhow::{ensure, Result};
use linkify::{LinkFinder, LinkKind as DetectedKind};
use std::{mem, ops::Range, ptr};

const MAX_BYTES: usize = 16 * 1024;
const MAX_ROWS: u32 = 128;
const MAX_CELLS: usize = 80_000;
/// Cells an explicit target's visible extent may cover. The extent is only
/// feedback; a longer label is drawn up to this bound.
const MAX_EXTENT_CELLS: usize = 4_096;

impl Engine {
    /// Resolve a current viewport cell. No native ref, selection or parser state
    /// escapes; every lookup is synchronous and leaves the terminal unchanged.
    pub fn link_at(&mut self, col: u16, row: u16) -> Result<Option<LinkHit>> {
        Ok(self.resolve(col, row, false)?.map(|span| span.hit))
    }

    /// The link at a current viewport cell with the viewport cells it covers.
    pub fn link_span_at(&mut self, col: u16, row: u16) -> Result<Option<LinkSpan>> {
        self.resolve(col, row, true)
    }

    fn resolve(&mut self, col: u16, row: u16, extent: bool) -> Result<Option<LinkSpan>> {
        if col >= self.cols || row >= self.height {
            return Ok(None);
        }
        let mut grid = Grid {
            engine: self,
            visited: 0,
        };
        let Some(reference) = grid.at(col, u32::from(row), 1)? else {
            return Ok(None);
        };
        let clicked_cell = cell(&reference)?;
        let mut explicit = false;
        // SAFETY: the cell value is copied; the documented output for key 7 is bool.
        check(unsafe {
            ffi::ghostty_cell_get(clicked_cell, 7, ptr::from_mut(&mut explicit).cast())
        })?;
        if explicit {
            // Even an empty, malformed or excessive explicit URI suppresses
            // detection in its label. No policy decision occurs here.
            let Some(hit) = explicit_uri(&reference)? else {
                return Ok(None);
            };
            let runs = if extent {
                explicit_extent(self, col, row, &hit.uri)?
            } else {
                Vec::new()
            };
            return Ok(Some(LinkSpan { hit, runs }));
        }
        let mut clicked = ffi::Coordinate { x: 0, y: 0 };
        // SAFETY: reference belongs to this unchanged terminal; SCREEN is tag 2.
        check(unsafe {
            ffi::ghostty_terminal_point_from_grid_ref(self.terminal, &reference, 2, &mut clicked)
        })?;
        let mut owner = (clicked.x, clicked.y);
        match wide(clicked_cell)? {
            2 => {
                let Some(x) = owner.0.checked_sub(1) else {
                    return Ok(None);
                };
                owner.0 = x;
            }
            3 => {
                let Some(y) = owner.1.checked_add(1) else {
                    return Ok(None);
                };
                owner = (0, y);
            }
            0 | 1 => {}
            _ => return Ok(None),
        }
        if owner != (clicked.x, clicked.y) {
            let Some(reference) = grid.at(owner.0, owner.1, 2)? else {
                return Ok(None);
            };
            if wide(cell(&reference)?)? != 1 {
                return Ok(None);
            }
        }
        let Some((start, end)) = grid.logical_rows(clicked.y)? else {
            return Ok(None);
        };
        let Some(LineText { text, hit, cells }) = grid.text(start, end, owner)? else {
            return Ok(None);
        };
        let Some((hit, range)) = detect(&text, hit) else {
            return Ok(None);
        };
        let runs = if extent {
            // Screen rows above the viewport's first row; the clicked cell is
            // inside the viewport, so this never underflows.
            let top = clicked.y.saturating_sub(u32::from(row));
            let mut runs = Vec::new();
            for cell in cells.iter().filter(|cell| range.contains(&cell.byte)) {
                let Some(row) = cell
                    .y
                    .checked_sub(top)
                    .and_then(|row| u16::try_from(row).ok())
                    .filter(|row| *row < self.height)
                else {
                    continue;
                };
                push_run(&mut runs, row, cell.x, cell.x.saturating_add(cell.width));
            }
            runs
        } else {
            Vec::new()
        };
        Ok(Some(LinkSpan { hit, runs }))
    }
}

/// Extend the last run when `cols` continues it on the same row.
fn push_run(runs: &mut Vec<LinkRun>, row: u16, start: u16, end: u16) {
    if let Some(last) = runs.last_mut() {
        if last.row == row && last.cols.end == start {
            last.cols.end = end;
            return;
        }
    }
    runs.push(LinkRun {
        row,
        cols: start..end,
    });
}

/// The viewport cells around `(col, row)`, in reading order, that carry the
/// same explicit target. A wide character's spacer cells belong to it: a
/// spacer tail to the cell before, a spacer head at a row's end to the first
/// cell of the next row.
fn explicit_extent(engine: &Engine, col: u16, row: u16, uri: &str) -> Result<Vec<LinkRun>> {
    #[derive(PartialEq)]
    enum Kind {
        Same,
        Tail,
        Head,
        Other,
    }
    let mut grid = Grid { engine, visited: 0 };
    let cols = u32::from(engine.cols);
    let last = cols * u32::from(engine.height);
    let clicked = u32::from(row) * cols + u32::from(col);
    // Indices stay below cols * height, so both parts fit their u16 axes.
    let position = |index: u32| ((index % cols) as u16, index / cols);
    let mut kind = |index: u32| -> Result<Kind> {
        if grid.visited >= MAX_EXTENT_CELLS {
            return Ok(Kind::Other);
        }
        let (x, y) = position(index);
        let Some(reference) = grid.at(x, y, 1)? else {
            return Ok(Kind::Other);
        };
        let cell = cell(&reference)?;
        match wide(cell)? {
            2 => return Ok(Kind::Tail),
            3 => return Ok(Kind::Head),
            _ => {}
        }
        let mut explicit = false;
        // SAFETY: as in Engine::link_at, key 7 writes a bool.
        check(unsafe { ffi::ghostty_cell_get(cell, 7, ptr::from_mut(&mut explicit).cast()) })?;
        Ok(
            if explicit && explicit_uri(&reference)?.is_some_and(|hit| hit.uri == uri) {
                Kind::Same
            } else {
                Kind::Other
            },
        )
    };
    let mut first = clicked;
    while first > 0 {
        first -= match kind(first - 1)? {
            Kind::Same | Kind::Head => 1,
            Kind::Tail if first >= 2 && kind(first - 2)? == Kind::Same => 2,
            _ => break,
        };
    }
    let mut end = clicked + 1;
    while end < last {
        end += match kind(end)? {
            Kind::Same | Kind::Tail => 1,
            Kind::Head if end + 1 < last && kind(end + 1)? == Kind::Same => 2,
            _ => break,
        };
    }
    let mut runs = Vec::new();
    let mut index = first;
    while index < end {
        let (x, y) = position(index);
        let row_end = end.min((y + 1) * cols);
        runs.push(LinkRun {
            row: y as u16,
            cols: x..(row_end - y * cols) as u16,
        });
        index = row_end;
    }
    Ok(runs)
}

fn explicit_uri(reference: &ffi::GridRef) -> Result<Option<LinkHit>> {
    let mut len = 0;
    // SAFETY: the ref is local to Engine::link_at's exclusive borrow. Null/zero
    // is the documented size probe and returns no borrowed URI pointer.
    let result =
        unsafe { ffi::ghostty_grid_ref_hyperlink_uri(reference, ptr::null_mut(), 0, &mut len) };
    if result == ffi::SUCCESS && len == 0 {
        return Ok(None);
    }
    if result != ffi::OUT_OF_SPACE {
        check(result)?;
        return Ok(None);
    }
    if len == 0 || len > MAX_BYTES {
        return Ok(None);
    }
    let mut bytes = vec![0; len];
    check(unsafe {
        ffi::ghostty_grid_ref_hyperlink_uri(reference, bytes.as_mut_ptr(), bytes.len(), &mut len)
    })?;
    ensure!(len <= bytes.len(), "Invalid native hyperlink length");
    bytes.truncate(len);
    let Ok(uri) = String::from_utf8(bytes) else {
        return Ok(None);
    };
    Ok(Some(LinkHit {
        uri,
        kind: LinkKind::Osc8,
    }))
}

/// This immutable borrow is nested inside Engine::link_at's exclusive borrow.
/// All refs are consumed before it ends; no terminal mutation occurs in this module.
struct Grid<'a> {
    engine: &'a Engine,
    visited: usize,
}
impl Grid<'_> {
    fn at(&mut self, x: u16, y: u32, tag: i32) -> Result<Option<ffi::GridRef>> {
        if self.visited == MAX_CELLS {
            return Ok(None);
        }
        self.visited += 1;
        let point = ffi::Point {
            tag,
            value: ffi::PointValue {
                coordinate: ffi::Coordinate { x, y },
            },
        };
        // SAFETY: zero is the documented invalid ref; native fills it with a
        // current pin. A missing row/narrow incomplete-reflow page is no hit.
        let mut reference: ffi::GridRef = unsafe { mem::zeroed() };
        let status =
            unsafe { ffi::ghostty_terminal_grid_ref(self.engine.terminal, point, &mut reference) };
        if status == ffi::INVALID_VALUE {
            return Ok(None);
        }
        check(status)?;
        Ok(Some(reference))
    }
    fn row(&mut self, y: u32) -> Result<Option<(bool, bool)>> {
        let Some(reference) = self.at(0, y, 2)? else {
            return Ok(None);
        };
        let mut row = 0;
        let mut wrap = false;
        let mut continuation = false;
        // SAFETY: row is an opaque u64 value. Both row keys write bool values.
        unsafe {
            check(ffi::ghostty_grid_ref_row(&reference, &mut row))?;
            check(ffi::ghostty_row_get(
                row,
                1,
                ptr::from_mut(&mut wrap).cast(),
            ))?;
            check(ffi::ghostty_row_get(
                row,
                2,
                ptr::from_mut(&mut continuation).cast(),
            ))?;
        }
        Ok(Some((wrap, continuation)))
    }
    fn logical_rows(&mut self, clicked: u32) -> Result<Option<(u32, u32)>> {
        let Some((mut wraps, mut continuation)) = self.row(clicked)? else {
            return Ok(None);
        };
        let mut start = clicked;
        let mut end = clicked;
        while continuation {
            let Some(previous) = start.checked_sub(1) else {
                return Ok(None);
            };
            if end - previous >= MAX_ROWS {
                return Ok(None);
            }
            let Some((previous_wrap, previous_continuation)) = self.row(previous)? else {
                return Ok(None);
            };
            if !previous_wrap {
                return Ok(None);
            }
            start = previous;
            continuation = previous_continuation;
        }
        while wraps {
            let Some(next) = end.checked_add(1) else {
                return Ok(None);
            };
            if next - start >= MAX_ROWS {
                return Ok(None);
            }
            let Some((next_wrap, next_continuation)) = self.row(next)? else {
                return Ok(None);
            };
            if !next_continuation {
                return Ok(None);
            }
            end = next;
            wraps = next_wrap;
        }
        Ok(Some((start, end)))
    }
    fn text(&mut self, start: u32, end: u32, owner: (u16, u32)) -> Result<Option<LineText>> {
        let mut text = String::new();
        let mut hit = None;
        let mut cells = Vec::new();
        for y in start..=end {
            for x in 0..self.engine.cols {
                let Some(reference) = self.at(x, y, 2)? else {
                    return Ok(None);
                };
                let width = match wide(cell(&reference)?)? {
                    2 | 3 => continue,
                    0 => 1,
                    1 => 2,
                    _ => return Ok(None),
                };
                let start = text.len();
                if !append_grapheme(&reference, &mut text)? {
                    return Ok(None);
                }
                cells.push(TextCell {
                    byte: start,
                    x,
                    y,
                    width,
                });
                if (x, y) == owner {
                    hit = Some(start..text.len());
                }
            }
        }
        Ok(hit.map(|hit| LineText { text, hit, cells }))
    }
}

/// A logical line's text, the clicked cell's part of it and where each
/// cell's grapheme starts in it.
struct LineText {
    text: String,
    hit: Range<usize>,
    cells: Vec<TextCell>,
}

/// Where one cell's grapheme starts in a logical line's text, and the screen
/// cells it occupies.
struct TextCell {
    byte: usize,
    x: u16,
    y: u32,
    width: u16,
}
fn cell(reference: &ffi::GridRef) -> Result<u64> {
    let mut cell = 0;
    // SAFETY: callers use a current ref under the uninterrupted engine borrow.
    check(unsafe { ffi::ghostty_grid_ref_cell(reference, &mut cell) })?;
    Ok(cell)
}
fn wide(cell: u64) -> Result<i32> {
    let mut wide = 0i32;
    // SAFETY: key 3 writes GhosttyCellWide, a C enum/int.
    check(unsafe { ffi::ghostty_cell_get(cell, 3, ptr::from_mut(&mut wide).cast()) })?;
    Ok(wide)
}
fn append_grapheme(reference: &ffi::GridRef, text: &mut String) -> Result<bool> {
    let mut stack = [0u32; 16];
    let mut count = 0;
    // SAFETY: native writes at most capacity elements; OUT_OF_SPACE leaves the
    // buffer unchanged and reports required codepoint count, not byte count.
    let status = unsafe {
        ffi::ghostty_grid_ref_graphemes(reference, stack.as_mut_ptr(), stack.len(), &mut count)
    };
    let heap;
    let values = if status == ffi::OUT_OF_SPACE {
        if count > MAX_BYTES - text.len() {
            return Ok(false);
        }
        heap = {
            let mut heap = vec![0; count];
            check(unsafe {
                ffi::ghostty_grid_ref_graphemes(
                    reference,
                    heap.as_mut_ptr(),
                    heap.len(),
                    &mut count,
                )
            })?;
            ensure!(count <= heap.len(), "Invalid native grapheme length");
            heap
        };
        &heap[..count]
    } else {
        check(status)?;
        ensure!(count <= stack.len(), "Invalid native grapheme length");
        &stack[..count]
    };
    if values.is_empty() {
        if text.len() == MAX_BYTES {
            return Ok(false);
        }
        text.push(' ');
    } else {
        for cp in values {
            let Some(ch) = char::from_u32(*cp) else {
                return Ok(false);
            };
            if ch.len_utf8() > MAX_BYTES - text.len() {
                return Ok(false);
            }
            text.push(ch);
        }
    }
    Ok(true)
}
fn detect(text: &str, hit: Range<usize>) -> Option<(LinkHit, Range<usize>)> {
    let mut finder = LinkFinder::new();
    finder.kinds(&[DetectedKind::Url]);
    finder.url_must_have_scheme(true);
    finder.links(text).find_map(|link| {
        let uri = link.as_str();
        let http = uri
            .get(..7)
            .is_some_and(|prefix| prefix.eq_ignore_ascii_case("http://"))
            || uri
                .get(..8)
                .is_some_and(|prefix| prefix.eq_ignore_ascii_case("https://"));
        (http && hit.start < link.end() && hit.end > link.start()).then(|| {
            (
                LinkHit {
                    uri: uri.into(),
                    kind: LinkKind::Detected,
                },
                link.start()..link.end(),
            )
        })
    })
}

#[cfg(test)]
mod tests;
