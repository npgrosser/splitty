//! Search-owned memory and native results remain inside the terminal boundary.
//! Scheduling belongs to the view; no call here drains the whole search.
use super::{check, ffi, Engine};
use anyhow::{anyhow, ensure, Result};
use std::{
    alloc::{alloc, dealloc, Layout},
    cell::Cell,
    ffi::c_void,
    mem, ptr,
};

const MAX_QUERY: usize = 1024;
const MAX_MATCHES: usize = 20_000;
const MAX_MEMORY: usize = 16 * 1024 * 1024;
const MAX_VIEW_CELLS: usize = 80_000;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SearchProgress {
    #[default]
    Idle,
    Searching,
    Complete,
    Limited,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SearchHighlight {
    pub row: u16,
    pub start_col: u16,
    /// Exclusive end; a wide glyph occupies both cells.
    pub end_col: u16,
    pub selected: bool,
}
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SearchSnapshot {
    pub progress: SearchProgress,
    /// Matches found so far, not a final total while Searching.
    pub total: usize,
    pub selected: Option<usize>,
    pub highlights: Vec<SearchHighlight>,
    pub revision: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum AllocationFailure {
    Limit,
    Allocation,
}

/// Both this descriptor and its context must stay at stable addresses until
/// the native search is freed: Ghostty retains a pointer to the descriptor.
struct Budget {
    allocator: ffi::Allocator,
    used: Cell<usize>,
    failure: Cell<Option<AllocationFailure>>,
    limit: usize,
}
impl Budget {
    fn boxed(limit: usize) -> Box<Self> {
        let mut value = Box::new(Self {
            allocator: ffi::Allocator {
                context: ptr::null_mut(),
                vtable: &VTABLE,
            },
            used: Cell::new(0),
            failure: Cell::new(None),
            limit,
        });
        value.allocator.context = ptr::from_mut(value.as_mut()).cast();
        value
    }
    fn fail(&self, failure: AllocationFailure) {
        if self.failure.get().is_none() {
            self.failure.set(Some(failure));
        }
    }
}
static VTABLE: ffi::AllocatorVtable = ffi::AllocatorVtable {
    alloc: Some(search_alloc),
    resize: None,
    remap: None,
    free: Some(search_free),
    reserved: [0; 4],
};
fn allocation_layout(len: usize, alignment: u8) -> Option<Layout> {
    Layout::from_size_align(len.max(1), 1_usize.checked_shl(u32::from(alignment))?).ok()
}
unsafe extern "C" fn search_alloc(
    context: *mut c_void,
    len: usize,
    alignment: u8,
    _: usize,
) -> *mut c_void {
    // SAFETY: the boxed context outlives every allocation and is only accessed
    // on Engine's owning thread. Callbacks never borrow Engine or unwind.
    let budget = unsafe { &*context.cast::<Budget>() };
    if budget.failure.get().is_some() {
        return ptr::null_mut();
    }
    let Some(layout) = allocation_layout(len, alignment) else {
        budget.fail(AllocationFailure::Allocation);
        return ptr::null_mut();
    };
    let Some(next) = budget
        .used
        .get()
        .checked_add(layout.size())
        .filter(|n| *n <= budget.limit)
    else {
        budget.fail(AllocationFailure::Limit);
        return ptr::null_mut();
    };
    // SAFETY: checked nonzero layout. Allocation failure is returned through C.
    let memory = unsafe { alloc(layout) };
    if memory.is_null() {
        budget.fail(AllocationFailure::Allocation);
    } else {
        budget.used.set(next);
    }
    memory.cast()
}
unsafe extern "C" fn search_free(
    context: *mut c_void,
    memory: *mut c_void,
    len: usize,
    alignment: u8,
    _: usize,
) {
    // SAFETY: native free supplies the original length/alignment; resize/remap
    // are disabled, so Rust always receives exactly its allocation layout.
    let budget = unsafe { &*context.cast::<Budget>() };
    if memory.is_null() {
        return;
    }
    let Some(layout) = allocation_layout(len, alignment) else {
        budget.fail(AllocationFailure::Allocation);
        return;
    };
    unsafe {
        dealloc(memory.cast(), layout);
    }
    if let Some(remaining) = budget.used.get().checked_sub(layout.size()) {
        budget.used.set(remaining);
    } else {
        budget.fail(AllocationFailure::Allocation);
    }
}
struct Index {
    raw: ffi::Handle,
    budget: Box<Budget>,
}
impl Index {
    fn close(&mut self) {
        // SAFETY: uniquely owned search. Its terminal remains alive because
        // Engine explicitly clears this field before freeing/replacing terminal.
        unsafe {
            ffi::ghostty_search_free(self.raw);
        }
        self.raw = ptr::null_mut();
    }
}
impl Drop for Index {
    fn drop(&mut self) {
        self.close();
    }
}

#[derive(Default)]
pub(super) struct SearchState {
    index: Option<Index>,
    query: String,
    progress: SearchProgress,
    dirty: bool,
    pub(super) revision: u64,
}
impl SearchState {
    pub(super) fn invalidate(&mut self) {
        self.revision = self.revision.wrapping_add(1);
        self.dirty |= self.index.is_some();
    }
    /// A query is set: its matches are found in the terminal as it is now.
    pub(super) fn active(&self) -> bool {
        self.index.is_some()
    }
}

impl Engine {
    pub fn set_search_query(&mut self, query: &str) -> Result<()> {
        self.set_search_query_with_limit(query, MAX_MEMORY)
    }
    fn set_search_query_with_limit(&mut self, query: &str, limit: usize) -> Result<()> {
        if query.is_empty() {
            self.clear_search();
            return Ok(());
        }
        if query == self.search.query {
            return Ok(());
        }
        self.clear_search();
        if query.len() > MAX_QUERY || self.search_cell_count().is_none() {
            self.search.progress = SearchProgress::Limited;
            return Ok(());
        }
        self.release_frame();
        self.search.query = query.to_owned();
        let mut index = Index {
            raw: ptr::null_mut(),
            budget: Budget::boxed(limit),
        };
        // SAFETY: the local output slot is disjoint from the boxed allocator.
        // No shared Index borrow exists while C initializes its handle field.
        // Install the local owner before checking either status or sticky OOM,
        // so even a partial nonnull handle is freed with its allocator alive.
        let status = unsafe {
            ffi::ghostty_search_new(&index.budget.allocator, &mut index.raw, self.terminal)
        };
        self.search.index = Some(index);
        if self.search_call(|_| status)?.is_none() {
            return Ok(());
        }
        let needle = ffi::StringView {
            ptr: query.as_ptr(),
            len: query.len(),
        };
        // SAFETY: native setter copies needle bytes synchronously.
        if self
            .search_call(|raw| unsafe {
                ffi::ghostty_search_set(raw, 1, ptr::from_ref(&needle).cast())
            })?
            .is_none()
        {
            return Ok(());
        }
        self.search.progress = SearchProgress::Searching;
        self.search.dirty = true;
        Ok(())
    }

    pub fn clear_search(&mut self) {
        self.search.index = None;
        self.search.query.clear();
        self.search.progress = SearchProgress::Idle;
        self.search.dirty = false;
        self.search.revision = self.search.revision.wrapping_add(1);
    }
    fn search_cell_count(&self) -> Option<usize> {
        usize::from(self.cols)
            .checked_mul(usize::from(self.height))
            .filter(|count| *count <= MAX_VIEW_CELLS)
    }

    pub fn search_revision(&self) -> u64 {
        self.search.revision
    }
    pub fn search_pending(&self) -> bool {
        self.search.index.is_some()
            && (self.search.dirty || self.search.progress == SearchProgress::Searching)
    }
    fn stop_search(&mut self, progress: SearchProgress) {
        self.search.index = None;
        self.search.progress = progress;
        self.search.dirty = false;
        self.search.revision = self.search.revision.wrapping_add(1);
    }
    fn search_empty(&self) -> SearchSnapshot {
        SearchSnapshot {
            progress: self.search.progress,
            revision: self.search.revision,
            ..SearchSnapshot::default()
        }
    }

    /// A cap failure is not an FFI error: it is an explicit stopped UI state.
    /// Native feed/tick can swallow OOM, so inspect the sticky flag even when
    /// they return SUCCESS, and before touching any potentially partial results.
    fn search_call(
        &mut self,
        call: impl FnOnce(ffi::Handle) -> ffi::Status,
    ) -> Result<Option<ffi::Status>> {
        self.search_call_allow(call, &[ffi::SUCCESS])
    }

    fn search_call_allow(
        &mut self,
        call: impl FnOnce(ffi::Handle) -> ffi::Status,
        allowed: &[ffi::Status],
    ) -> Result<Option<ffi::Status>> {
        let Some(index) = self.search.index.as_ref() else {
            return Ok(None);
        };
        let status = call(index.raw);
        let failure = index.budget.failure.get();
        match failure {
            Some(AllocationFailure::Limit) => {
                self.stop_search(SearchProgress::Limited);
                return Ok(None);
            }
            Some(AllocationFailure::Allocation) => {
                self.stop_search(SearchProgress::Idle);
                return Err(anyhow!("Terminal search allocation failed"));
            }
            None => {}
        }
        if !allowed.contains(&status) {
            self.stop_search(SearchProgress::Idle);
            check(status)?;
        }
        Ok(Some(status))
    }
    fn search_get<T>(&mut self, data: i32, output: &mut T) -> Result<Option<ffi::Status>> {
        // SAFETY: private callers supply the exact output type for each key.
        let allowed: &[ffi::Status] = match data {
            4 | 5 => &[ffi::SUCCESS, ffi::NO_VALUE],
            7 => &[ffi::SUCCESS, ffi::OUT_OF_SPACE],
            _ => &[ffi::SUCCESS],
        };
        self.search_call_allow(
            |raw| unsafe { ffi::ghostty_search_get(raw, data, ptr::from_mut(output).cast()) },
            allowed,
        )
    }
    fn search_count(&mut self) -> Result<Option<usize>> {
        let mut total = 0usize;
        if self.search_get(3, &mut total)?.is_none() {
            return Ok(None);
        }
        // Counts describe the active native primary/alternate buffer. The
        // allocation budget covers both buffers and viewport caches together.
        if total > MAX_MATCHES {
            self.stop_search(SearchProgress::Limited);
            return Ok(None);
        }
        Ok(Some(total))
    }

    /// At most one feed and one tick. This is cooperative scheduling, not a
    /// bound on the duration of a native call. The caller must yield between steps.
    pub fn search_step(&mut self) -> Result<SearchSnapshot> {
        let result = self.search_step_inner();
        if result.is_err() {
            self.stop_search(SearchProgress::Idle);
        }
        result
    }

    fn search_step_inner(&mut self) -> Result<SearchSnapshot> {
        if self.search.index.is_none() {
            return Ok(self.search_empty());
        }
        if self.search_cell_count().is_none() {
            self.stop_search(SearchProgress::Limited);
            return Ok(self.search_empty());
        }
        let mut status = 0i32;
        if self.search_get(1, &mut status)?.is_none() {
            return Ok(self.search_empty());
        }
        if self.search.dirty || status == 1 {
            // SAFETY: search and terminal are exclusively owned/borrowed here.
            if self
                .search_call(|raw| unsafe { ffi::ghostty_search_feed(raw) })?
                .is_none()
            {
                return Ok(self.search_empty());
            }
            self.search.dirty = false;
            self.search.revision = self.search.revision.wrapping_add(1);
            if self.search_count()?.is_none() {
                return Ok(self.search_empty());
            }
            if self.search_get(1, &mut status)?.is_none() {
                return Ok(self.search_empty());
            }
        }
        if status == 0 {
            if self
                .search_call(|raw| unsafe { ffi::ghostty_search_tick(raw, &mut status) })?
                .is_none()
            {
                return Ok(self.search_empty());
            }
            self.search.revision = self.search.revision.wrapping_add(1);
        }
        self.search.progress = match status {
            0 | 1 => SearchProgress::Searching,
            2 => SearchProgress::Complete,
            _ => {
                self.stop_search(SearchProgress::Idle);
                return Err(anyhow!("Unknown terminal search progress"));
            }
        };
        self.search_snapshot()
    }

    pub fn search_navigate(&mut self, older: bool) -> Result<SearchSnapshot> {
        let result = self.search_navigate_inner(older);
        if result.is_err() {
            self.stop_search(SearchProgress::Idle);
        }
        result
    }

    fn search_navigate_inner(&mut self, older: bool) -> Result<SearchSnapshot> {
        // Refresh and guard newly accumulated matches BEFORE native selection's
        // pruning pass. Never navigate a stale/limited partial index.
        if self.search_pending() {
            self.search_step()?;
        }
        if self.search_count()?.is_none() {
            return Ok(self.search_empty());
        }
        let selected = self.search_call_allow(
            |raw| unsafe {
                if older {
                    ffi::ghostty_search_select_next(raw)
                } else {
                    ffi::ghostty_search_select_prev(raw)
                }
            },
            &[ffi::SUCCESS, ffi::NO_VALUE],
        )?;
        if selected.is_none() {
            return Ok(self.search_empty());
        }
        self.search.revision = self.search.revision.wrapping_add(1);
        // Selection's built-in feed predates its viewport scroll. Defer a new
        // feed to the next scheduled step; selected highlight is mapped now.
        self.search.dirty = true;
        self.search_snapshot()
    }

    fn search_snapshot(&mut self) -> Result<SearchSnapshot> {
        let Some(total) = self.search_count()? else {
            return Ok(self.search_empty());
        };
        let mut selected_index = 0usize;
        let selected = match self.search_get(4, &mut selected_index)? {
            Some(ffi::SUCCESS) => Some(selected_index),
            Some(ffi::NO_VALUE) => None,
            None => return Ok(self.search_empty()),
            _ => return Err(anyhow!("Invalid selected search index")),
        };
        ensure!(
            selected.is_none_or(|index| index < total),
            "Native search selection exceeds match count"
        );
        let mut buffer = ffi::SelectionBuffer {
            ptr: ptr::null_mut(),
            cap: 0,
            len: 0,
        };
        if self.search_get(7, &mut buffer)?.is_none() {
            return Ok(self.search_empty());
        }
        if buffer.len > MAX_MATCHES {
            self.stop_search(SearchProgress::Limited);
            return Ok(self.search_empty());
        }
        let mut matches = Vec::<ffi::Selection>::with_capacity(buffer.len);
        buffer.ptr = matches.as_mut_ptr();
        buffer.cap = matches.capacity();
        let count = buffer.len;
        if self.search_get(7, &mut buffer)? != Some(ffi::SUCCESS) {
            return if self.search.index.is_none() {
                Ok(self.search_empty())
            } else {
                Err(anyhow!("Invalid search highlight buffer"))
            };
        }
        ensure!(
            buffer.len <= count,
            "Native search highlight length changed without a feed"
        );
        // SAFETY: successful native read initialized exactly len selections.
        unsafe {
            matches.set_len(buffer.len);
        }
        let mut active: ffi::Selection = unsafe { mem::zeroed() };
        let has_active = self.search_get(5, &mut active)? == Some(ffi::SUCCESS);
        if self.search.index.is_none() {
            return Ok(self.search_empty());
        }
        let mut scrollbar: ffi::Scrollbar = unsafe { mem::zeroed() };
        // SAFETY: correct scalar output layout and live terminal.
        check(unsafe {
            ffi::ghostty_terminal_get(
                self.terminal,
                ffi::DATA_SCROLLBAR,
                ptr::from_mut(&mut scrollbar).cast(),
            )
        })?;
        // A transient viewport mask bounds exported storage by cells, not by
        // match_count * rows. It also merges adjacent ordinary highlights and
        // lets the selected match win without overlapping paint rectangles.
        let Some(cell_count) = self.search_cell_count() else {
            self.stop_search(SearchProgress::Limited);
            return Ok(self.search_empty());
        };
        let mut cells = vec![0_u8; cell_count];
        for selection in &matches {
            self.clip_search_match(selection, scrollbar.offset, false, &mut cells)?;
        }
        if has_active {
            self.clip_search_match(&active, scrollbar.offset, true, &mut cells)?;
        }
        let mut highlights = Vec::new();
        for (row, cells) in cells.chunks_exact(usize::from(self.cols)).enumerate() {
            let mut col = 0;
            while col < cells.len() {
                let value = cells[col];
                let start = col;
                while col < cells.len() && cells[col] == value {
                    col += 1;
                }
                if value != 0 {
                    highlights.push(SearchHighlight {
                        row: u16::try_from(row)?,
                        start_col: u16::try_from(start)?,
                        end_col: u16::try_from(col)?,
                        selected: value == 2,
                    });
                }
            }
        }
        Ok(SearchSnapshot {
            progress: self.search.progress,
            total,
            selected,
            highlights,
            revision: self.search.revision,
        })
    }

    fn clip_search_match(
        &self,
        selection: &ffi::Selection,
        offset: u64,
        selected: bool,
        cells: &mut [u8],
    ) -> Result<()> {
        let mut start = ffi::Coordinate { x: 0, y: 0 };
        let mut end = start;
        // SCREEN coordinates preserve highlights crossing viewport boundaries.
        // All untracked refs are consumed in this same terminal borrow, before
        // any feed/resize/scroll mutation. No native reference escapes.
        unsafe {
            check(ffi::ghostty_terminal_point_from_grid_ref(
                self.terminal,
                &selection.start,
                2,
                &mut start,
            ))?;
            check(ffi::ghostty_terminal_point_from_grid_ref(
                self.terminal,
                &selection.end,
                2,
                &mut end,
            ))?;
        }
        let first = u64::from(start.y).max(offset);
        let last = u64::from(end.y).min(offset + u64::from(self.height) - 1);
        if first > last {
            return Ok(());
        }
        let mut cell = 0u64;
        let mut wide = 0i32;
        unsafe {
            check(ffi::ghostty_grid_ref_cell(&selection.end, &mut cell))?;
            check(ffi::ghostty_cell_get(
                cell,
                ffi::CELL_WIDE,
                ptr::from_mut(&mut wide).cast(),
            ))?;
        }
        let end_col = end
            .x
            .saturating_add(if wide == 1 { 2 } else { 1 })
            .min(self.cols);
        for row in first..=last {
            let start_col = if row == u64::from(start.y) {
                start.x.min(self.cols)
            } else {
                0
            };
            let end_col = if row == u64::from(end.y) {
                end_col
            } else {
                self.cols
            };
            if start_col < end_col {
                let base = usize::try_from(row - offset)? * usize::from(self.cols);
                cells[base + usize::from(start_col)..base + usize::from(end_col)]
                    .fill(if selected { 2 } else { 1 });
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn complete(engine: &mut Engine) -> Result<SearchSnapshot> {
        for _ in 0..1000 {
            let view = engine.search_step()?;
            if !engine.search_pending() {
                return Ok(view);
            }
        }
        panic!("search did not settle within bounded fixture steps");
    }
    fn plain_spans(view: &SearchSnapshot) -> Vec<(u16, u16, u16)> {
        view.highlights
            .iter()
            .map(|span| (span.row, span.start_col, span.end_col))
            .collect()
    }

    #[test]
    fn full_retained_history_is_incremental_and_navigation_wraps() -> Result<()> {
        let mut engine = Engine::new(20, 3)?;
        let output: String = (0..120)
            .map(|n| {
                if n % 15 == 0 {
                    format!("needle {n}\r\n")
                } else {
                    format!("row {n}\r\n")
                }
            })
            .collect();
        engine.feed(output.as_bytes(), true)?;
        engine.set_search_query("NEEDLE")?;
        assert!(engine.search_pending());
        let view = complete(&mut engine)?;
        assert_eq!(view.progress, SearchProgress::Complete);
        assert_eq!(view.total, 8);
        assert_eq!(view.selected, None);
        assert_eq!(engine.search_navigate(true)?.selected, Some(0));
        assert_eq!(engine.search_navigate(true)?.selected, Some(1));
        assert_eq!(engine.search_navigate(false)?.selected, Some(0));
        assert_eq!(engine.search_navigate(false)?.selected, Some(7));
        assert_eq!(engine.search_navigate(true)?.selected, Some(0));
        let settled = complete(&mut engine)?;
        assert!(settled.highlights.iter().any(|span| span.selected));
        assert_eq!(
            engine.search_step()?,
            settled,
            "complete idle step does not change revision"
        );
        Ok(())
    }

    #[test]
    fn search_spans_follow_unicode_soft_wraps_and_viewport_clipping() -> Result<()> {
        let mut engine = Engine::new(8, 3)?;
        engine.feed("a界e\u{301}Zé".as_bytes(), false)?;
        engine.set_search_query("界")?;
        assert_eq!(plain_spans(&complete(&mut engine)?), [(0, 1, 3)]);
        engine.set_search_query("E\u{301}")?;
        assert_eq!(plain_spans(&complete(&mut engine)?), [(0, 3, 4)]);
        engine.set_search_query("\u{301}")?;
        assert_eq!(plain_spans(&complete(&mut engine)?), [(0, 3, 4)]);
        engine.set_search_query("É")?;
        assert_eq!(complete(&mut engine)?.total, 0, "nonASCII case is exact");
        let mut wrapped = Engine::new(4, 2)?;
        wrapped.feed(b"abcdef\r\nXYZ", false)?;
        wrapped.set_search_query("cdef")?;
        let clipped = complete(&mut wrapped)?;
        assert_eq!(clipped.total, 1);
        assert_eq!(
            plain_spans(&clipped),
            [(0, 0, 2)],
            "match beginning above viewport keeps its visible suffix"
        );
        wrapped.scroll(1)?;
        assert_ne!(wrapped.search_revision(), clipped.revision);
        assert_eq!(
            plain_spans(&complete(&mut wrapped)?),
            [(0, 2, 4), (1, 0, 2)]
        );
        let mut bottom = Engine::new(4, 1)?;
        bottom.feed(b"abcdef\r\nXYZ", false)?;
        bottom.scroll(10)?;
        bottom.set_search_query("cdef")?;
        assert_eq!(
            plain_spans(&complete(&mut bottom)?),
            [(0, 2, 4)],
            "match ending below viewport keeps its visible prefix"
        );
        Ok(())
    }

    #[test]
    fn output_preserves_anchor_and_clipboard_selection_but_resize_reset_invalidate() -> Result<()> {
        let mut engine = Engine::new(20, 4)?;
        engine.feed(b"copy\r\nneedle old\r\nneedle new", false)?;
        engine.select_start(0, 0, false)?;
        engine.select_update(3, 0)?;
        engine.select_end()?;
        let copied = engine.selection_text()?;
        engine.set_search_query("needle")?;
        complete(&mut engine)?;
        let selected = engine.search_navigate(true)?;
        let old = selected
            .highlights
            .iter()
            .find(|span| span.selected)
            .copied()
            .unwrap();
        engine.feed(b"\r\nordinary", false)?;
        assert_ne!(engine.search_revision(), selected.revision);
        let updated = complete(&mut engine)?;
        assert_eq!(updated.selected, Some(0));
        assert!(updated
            .highlights
            .iter()
            .any(|span| span.selected && span.row == old.row));
        assert_eq!(
            engine.selection_text()?,
            copied,
            "search never installs terminal selection"
        );
        engine.resize(10, 4, 10, 20)?;
        let resized = complete(&mut engine)?;
        assert_eq!(resized.total, 2);
        assert_eq!(
            resized.selected, None,
            "native reflow rebuilds navigation index"
        );
        engine.reset()?;
        assert!(!engine.search_pending());
        assert_eq!(engine.search_step()?.progress, SearchProgress::Idle);
        assert_ne!(engine.search_revision(), resized.revision);
        Ok(())
    }

    #[test]
    fn primary_alternate_switches_do_not_reuse_wrong_counts_or_highlights() -> Result<()> {
        let mut engine = Engine::new(20, 3)?;
        engine.feed(b"needle needle", false)?;
        engine.set_search_query("needle")?;
        assert_eq!(complete(&mut engine)?.total, 2);
        engine.feed(b"\x1b[?1049hneedle", false)?;
        assert_eq!(complete(&mut engine)?.total, 1);
        engine.feed(b"\x1b[?1049l", false)?;
        assert_eq!(complete(&mut engine)?.total, 2);
        Ok(())
    }

    #[test]
    fn search_limits_are_sticky_and_refinement_or_reopen_recovers() -> Result<()> {
        let mut engine = Engine::new(80, 24)?;
        engine.feed("a".repeat(79).repeat(600).as_bytes(), false)?;
        engine.set_search_query("a")?;
        let limited = complete(&mut engine)?;
        assert_eq!(limited.progress, SearchProgress::Limited);
        assert_eq!(limited.total, 0);
        assert!(limited.highlights.is_empty());
        assert!(!engine.search_pending());
        engine.feed(b"\r\nneedle", false)?;
        engine.set_search_query("a")?;
        assert!(
            !engine.search_pending(),
            "same query/output never retries a limit"
        );
        assert_eq!(
            engine.search_navigate(true)?.progress,
            SearchProgress::Limited
        );
        engine.set_search_query("needle")?;
        assert_eq!(complete(&mut engine)?.total, 1);
        engine.set_search_query(&"é".repeat(513))?;
        assert_eq!(engine.search_step()?.progress, SearchProgress::Limited);
        engine.set_search_query_with_limit("needle", 1)?;
        assert_eq!(
            engine.search_step()?.progress,
            SearchProgress::Limited,
            "constructor allocation is guarded too"
        );
        engine.clear_search();
        engine.set_search_query("needle")?;
        assert_eq!(complete(&mut engine)?.total, 1);
        Ok(())
    }

    #[test]
    fn swallowed_native_feed_oom_discards_partial_index_and_refinement_recovers() -> Result<()> {
        let mut engine = Engine::new(400, 200)?;
        let line = format!("{}\r\n", "a".repeat(399));
        engine.feed(line.repeat(200).as_bytes(), false)?;
        engine.set_search_query("a")?;
        assert!(
            engine.search.index.is_some(),
            "constructor and needle admission succeed"
        );
        let mut raw_status = ffi::NO_VALUE;
        let result = engine.search_call(|raw| {
            // SAFETY: exercising the same live owned handle as search_step.
            raw_status = unsafe { ffi::ghostty_search_feed(raw) };
            raw_status
        })?;
        assert_eq!(
            raw_status,
            ffi::SUCCESS,
            "pinned feed swallows allocation failure"
        );
        assert_eq!(
            result, None,
            "sticky allocator failure overrides native SUCCESS"
        );
        assert!(engine.search.index.is_none());
        assert_eq!(engine.search_step()?.progress, SearchProgress::Limited);
        engine.feed(b"\r\nneedle", false)?;
        engine.set_search_query("needle")?;
        assert_eq!(complete(&mut engine)?.total, 1);
        Ok(())
    }

    #[test]
    fn oversized_viewport_is_rejected_before_search_or_snapshot_allocation() -> Result<()> {
        let mut engine = Engine::new(401, 200)?;
        engine.set_search_query("needle")?;
        assert!(engine.search.index.is_none());
        let limited = engine.search_step()?;
        assert_eq!(limited.progress, SearchProgress::Limited);
        assert!(limited.highlights.is_empty());
        engine.clear_search();
        assert_eq!(engine.search_step()?.progress, SearchProgress::Idle);
        Ok(())
    }

    #[test]
    fn allocator_honors_alignment_and_releases_all_native_storage_after_move() -> Result<()> {
        let mut budget = Budget::boxed(8192);
        let context = ptr::from_mut(budget.as_mut()).cast();
        let memory = unsafe { search_alloc(context, 4096, 12, 0) };
        assert!(!memory.is_null());
        assert_eq!(memory.addr() % 4096, 0);
        assert_eq!(budget.used.get(), 4096);
        assert!(unsafe { search_alloc(context, 4097, 0, 0) }.is_null());
        assert_eq!(budget.failure.get(), Some(AllocationFailure::Limit));
        unsafe {
            search_free(context, memory, 4096, 12, 0);
        }
        assert_eq!(budget.used.get(), 0);
        assert!(
            unsafe { search_alloc(context, 1, 0, 0) }.is_null(),
            "a failed native call cannot resume allocating during recovery"
        );
        let mut engine = Engine::new(20, 3)?;
        engine.feed(b"needle\r\nneedle", false)?;
        engine.set_search_query("needle")?;
        let mut moved = Box::new(engine);
        assert_eq!(complete(&mut moved)?.total, 2);
        let mut index = moved.search.index.take().unwrap();
        assert!(index.budget.used.get() > 0);
        index.close();
        assert_eq!(index.budget.used.get(), 0);
        Ok(())
    }
}
