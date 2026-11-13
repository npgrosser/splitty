use super::*;

fn explicit(engine: &mut Engine, uri: &[u8], label: &str) -> Result<()> {
    let mut bytes = b"\x1b]8;;".to_vec();
    bytes.extend_from_slice(uri);
    bytes.extend_from_slice(b"\x1b\\");
    bytes.extend_from_slice(label.as_bytes());
    bytes.extend_from_slice(b"\x1b]8;;\x1b\\");
    engine.feed(&bytes, false)?;
    Ok(())
}
fn hit(uri: &str, kind: LinkKind) -> Option<LinkHit> {
    Some(LinkHit {
        uri: uri.into(),
        kind,
    })
}

#[test]
fn explicit_targets_override_labels_without_applying_url_policy() -> Result<()> {
    let mut engine = Engine::new(80, 4)?;
    explicit(
        &mut engine,
        b"https://actual.test/target",
        "https://label.test/",
    )?;
    assert_eq!(
        engine.link_at(3, 0)?,
        hit("https://actual.test/target", LinkKind::Osc8)
    );
    engine.feed(b"\r\n", false)?;
    explicit(
        &mut engine,
        b"file:///private/notes",
        "https://safe-looking.test/",
    )?;
    assert_eq!(
        engine.link_at(3, 1)?,
        hit("file:///private/notes", LinkKind::Osc8)
    );
    // The engine returns a raw explicit URI; policy rejection cannot cause a
    // second plain-label lookup inside it.
    assert_eq!(engine.link_at(80, 1)?, None);
    assert_eq!(engine.link_at(0, 4)?, None);
    engine.feed(b"\x1b[1;1H\x1b[2K", false)?;
    assert_eq!(
        engine.link_at(3, 0)?,
        None,
        "fresh hit observes overwritten output"
    );
    Ok(())
}

#[test]
fn malformed_retained_uri_is_rejected_and_parser_overflow_is_not_truncated() -> Result<()> {
    let mut engine = Engine::new(80, 4)?;
    explicit(&mut engine, b"https://bad.test/\xff", "https://label.test/")?;
    assert_eq!(engine.link_at(2, 0)?, None);
    engine.feed(b"\r\n", false)?;
    let excessive = format!("https://large.test/{}", "a".repeat(MAX_BYTES));
    // Pinned OSC8 uses a fixed 2048-byte capture. Overflow invalidates the
    // command before grid metadata exists; it does not retain a shortened URI.
    explicit(&mut engine, excessive.as_bytes(), "plain label")?;
    assert_eq!(engine.link_at(2, 1)?, None);
    engine.feed(b"\r\n", false)?;
    explicit(&mut engine, excessive.as_bytes(), "https://label.test/")?;
    assert_eq!(
        engine.link_at(2, 2)?,
        hit("https://label.test/", LinkKind::Detected),
        "without retained OSC8 metadata the complete visible URL is ordinary text"
    );
    assert!(
        engine.feed(b"\x1b[6n", false)?.starts_with(b"\x1b["),
        "terminal remains usable"
    );
    Ok(())
}

#[test]
fn plain_urls_use_unicode_cell_offsets_and_matcher_punctuation() -> Result<()> {
    let mut engine = Engine::new(80, 4)?;
    engine.feed(
        "e\u{301} (https://example.test/a_(b)), next".as_bytes(),
        false,
    )?;
    assert_eq!(
        engine.link_at(5, 0)?,
        hit("https://example.test/a_(b)", LinkKind::Detected)
    );
    assert_eq!(engine.link_at(0, 0)?, None);
    assert_eq!(
        engine.link_at(2, 0)?,
        None,
        "opening punctuation is outside URL"
    );
    assert_eq!(
        engine.link_at(32, 0)?,
        None,
        "unwritten/adjacent text is outside URL"
    );
    engine.feed("\r\nhttps://e.test/界x".as_bytes(), false)?;
    let column = u16::try_from("https://e.test/".len())?;
    for col in [column, column + 1] {
        assert_eq!(
            engine.link_at(col, 1)?,
            hit("https://e.test/界x", LinkKind::Detected),
            "both wide cells map to the same byte interval"
        );
    }
    engine.feed(
        b"\r\nexample.test ftp://example.test http://localhost:5173/",
        false,
    )?;
    assert_eq!(engine.link_at(2, 2)?, None, "scheme required");
    assert_eq!(
        engine.link_at(16, 2)?,
        None,
        "detected non-web schemes excluded"
    );
    assert_eq!(
        engine.link_at(40, 2)?,
        hit("http://localhost:5173/", LinkKind::Detected)
    );
    Ok(())
}

#[test]
fn wrapped_history_reflow_and_alternate_screen_resolve_current_grid() -> Result<()> {
    let mut engine = Engine::new(9, 2)?;
    let uri = "https://example.test/long/path";
    engine.feed(uri.as_bytes(), false)?;
    engine.scroll(100)?;
    assert_eq!(engine.link_at(3, 0)?, hit(uri, LinkKind::Detected));
    assert_eq!(
        engine.link_at(3, 1)?,
        hit(uri, LinkKind::Detected),
        "logical line crosses viewport into retained rows"
    );
    engine.resize(12, 3, 8, 16)?;
    engine.scroll(100)?;
    assert_eq!(engine.link_at(4, 0)?, hit(uri, LinkKind::Detected));
    engine.feed(b"\x1b[?1049h\x1b[H", false)?;
    explicit(&mut engine, b"https://alternate.test/", "alternate")?;
    assert_eq!(
        engine.link_at(2, 0)?,
        hit("https://alternate.test/", LinkKind::Osc8)
    );
    engine.feed(b"\x1b[?1049l", false)?;
    engine.scroll(100)?;
    assert_eq!(engine.link_at(3, 0)?, hit(uri, LinkKind::Detected));
    engine.reset()?;
    assert_eq!(engine.link_at(3, 0)?, None);
    Ok(())
}

#[test]
fn wrap_spacers_preserve_uri_and_hard_lines_do_not_join() -> Result<()> {
    let prefix = "https://e.test/";
    let mut engine = Engine::new(u16::try_from(prefix.len() + 1)?, 4)?;
    let uri = format!("{prefix}界z");
    engine.feed(uri.as_bytes(), false)?;
    for (col, row) in [(u16::try_from(prefix.len())?, 0), (0, 1), (1, 1)] {
        assert_eq!(engine.link_at(col, row)?, hit(&uri, LinkKind::Detected));
    }
    engine.reset()?;
    explicit(
        &mut engine,
        b"https://wide.test/",
        &format!("{}界", "x".repeat(prefix.len())),
    )?;
    for (col, row) in [(u16::try_from(prefix.len())?, 0), (0, 1), (1, 1)] {
        assert_eq!(
            engine.link_at(col, row)?,
            hit("https://wide.test/", LinkKind::Osc8)
        );
    }
    let mut hard = Engine::new(80, 3)?;
    hard.feed(b"https://example.test/a\r\n/continuation", false)?;
    assert_eq!(
        hard.link_at(2, 0)?,
        hit("https://example.test/a", LinkKind::Detected)
    );
    assert_eq!(hard.link_at(2, 1)?, None);
    Ok(())
}

#[test]
fn bounded_complete_lines_reject_prefixes_and_preserve_selection() -> Result<()> {
    let mut engine = Engine::new(400, 50)?;
    let uri = format!("https://example.test/{}", "a".repeat(15_000));
    engine.feed(uri.as_bytes(), false)?;
    engine.select_start(0, 0, false)?;
    engine.select_update(4, 0)?;
    engine.select_end()?;
    let selected = engine.selection_text()?;
    let started = std::time::Instant::now();
    assert_eq!(engine.link_at(2, 0)?, hit(&uri, LinkKind::Detected));
    eprintln!(
        "accepted 15KiB logical-line lookup: {:?}",
        started.elapsed()
    );
    assert_eq!(
        engine.selection_text()?,
        selected,
        "link lookup has no selection effect"
    );
    engine.reset()?;
    engine.feed(
        format!("https://example.test/{}", "a".repeat(MAX_BYTES)).as_bytes(),
        false,
    )?;
    let started = std::time::Instant::now();
    assert_eq!(
        engine.link_at(2, 0)?,
        None,
        "incomplete byte-limited extraction never returns prefix"
    );
    eprintln!(
        "rejected oversized logical-line lookup: {:?}",
        started.elapsed()
    );
    let mut rows = Engine::new(8, 3)?;
    rows.feed(
        format!("https://e.test/{}", "a".repeat(8 * 128)).as_bytes(),
        false,
    )?;
    rows.scroll(1000)?;
    assert_eq!(
        rows.link_at(2, 0)?,
        None,
        "row-limited extraction never returns prefix"
    );
    let mut grid = Grid {
        engine: &engine,
        visited: MAX_CELLS,
    };
    assert!(
        grid.at(0, 0, 1)?.is_none(),
        "cell inspection limit is checked before native traversal"
    );
    Ok(())
}

#[test]
fn spans_cover_a_links_visible_cells_and_nothing_beside_it() -> Result<()> {
    let run = |row, cols: Range<u16>| LinkRun { row, cols };
    let span = |engine: &mut Engine, col, row| -> Result<Option<Vec<LinkRun>>> {
        Ok(engine.link_span_at(col, row)?.map(|span| span.runs))
    };
    // A detected link wraps; its wide character covers both its cells.
    let mut engine = Engine::new(12, 3)?;
    engine.feed("ab https://e.test/界x c".as_bytes(), false)?;
    let wrapped = vec![run(0, 3..12), run(1, 0..9)];
    for (col, row) in [(4, 0), (6, 1), (7, 1), (8, 1)] {
        assert_eq!(span(&mut engine, col, row)?, Some(wrapped.clone()));
    }
    assert_eq!(
        engine.link_at(4, 0)?,
        hit("https://e.test/界x", LinkKind::Detected),
        "the span's link is the one activation resolves"
    );
    assert_eq!(span(&mut engine, 1, 0)?, None);

    // Only the rows in the viewport are covered: the link takes four rows,
    // two of them in view, at the bottom and scrolled to the top.
    let mut scrolled = Engine::new(9, 2)?;
    scrolled.feed(b"https://example.test/long/path", false)?;
    assert_eq!(
        span(&mut scrolled, 1, 1)?,
        Some(vec![run(0, 0..9), run(1, 0..3)])
    );
    scrolled.scroll(100)?;
    assert_eq!(
        span(&mut scrolled, 3, 1)?,
        Some(vec![run(0, 0..9), run(1, 0..9)])
    );

    // An explicit target covers its label, not the text around it.
    let mut explicit_engine = Engine::new(20, 3)?;
    explicit_engine.feed(b"ab ", false)?;
    explicit(&mut explicit_engine, b"https://x.test/", "label")?;
    explicit_engine.feed(b" z", false)?;
    explicit(&mut explicit_engine, b"https://y.test/", "next")?;
    assert_eq!(span(&mut explicit_engine, 5, 0)?, Some(vec![run(0, 3..8)]));
    assert_eq!(
        span(&mut explicit_engine, 11, 0)?,
        Some(vec![run(0, 10..14)]),
        "a different target next to it is its own link"
    );

    // A wide character that wraps keeps the label in one span.
    let mut wide_engine = Engine::new(16, 3)?;
    explicit(
        &mut wide_engine,
        b"https://wide.test/",
        &format!("{}界", "x".repeat(15)),
    )?;
    for (col, row) in [(2, 0), (0, 1), (1, 1)] {
        assert_eq!(
            span(&mut wide_engine, col, row)?,
            Some(vec![run(0, 0..16), run(1, 0..2)])
        );
    }
    Ok(())
}
