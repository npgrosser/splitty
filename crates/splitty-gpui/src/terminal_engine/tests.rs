use super::*;

fn screen(engine: &mut Engine) -> Result<Vec<String>> {
    let mut frame = engine.frame()?;
    let mut rows = Vec::new();
    while let Some(mut row) = frame.next_row()? {
        let mut text = String::new();
        while let Some(cell) = row.next_cell()? {
            if !cell.skip {
                text.push_str(if cell.text.is_empty() { " " } else { cell.text });
            }
        }
        rows.push(text.trim_end().to_owned());
    }
    frame.finish()?;
    Ok(rows)
}

#[test]
fn split_utf8_replay_replies_and_reset_are_isolated() -> Result<()> {
    let mut engine = Engine::new(20, 3)?;
    engine.feed(&[0xc3], true)?;
    assert!(engine.feed(&[0xa9], true)?.is_empty());
    assert!(engine.feed(b"\x1b[6n", true)?.is_empty());
    assert_eq!(screen(&mut engine)?[0], "é");
    assert!(engine.feed(b"", false)?.is_empty());
    assert_eq!(engine.feed(b"\x1b[6n", false)?, b"\x1b[1;2R");
    engine.feed(b"\x1b[", true)?;
    engine.reset()?;
    engine.feed(b"ok", false)?;
    assert_eq!(screen(&mut engine)?[0], "ok");
    assert_eq!(engine.feed(b"\x1b[6n", false)?, b"\x1b[1;3R");
    Ok(())
}

#[test]
fn reply_overflow_discards_whole_operation_and_recovers() -> Result<()> {
    let mut engine = Engine::new(20, 3)?;
    assert!(engine.feed(&b"\x1b[6n".repeat(MAX_INPUT), false).is_err());
    assert_eq!(engine.feed(b"\x1b[6n", false)?, b"\x1b[1;1R");
    assert!(engine.feed(&b"\x1b[6n".repeat(MAX_INPUT), true)?.is_empty());
    assert!(engine.feed(b"", false)?.is_empty());
    Ok(())
}

#[test]
fn resize_reflows_and_collects_size_reports_without_leaking() -> Result<()> {
    let mut engine = Engine::new(20, 3)?;
    engine.feed(b"abcdefghijkl\x1b[?2048h", true)?;
    let reply = engine.resize(10, 4, 8, 16)?;
    assert!(!reply.is_empty(), "resize must return mode 2048's report");
    assert!(engine.feed(b"", false)?.is_empty());
    let rows = screen(&mut engine)?;
    assert_eq!(rows[0], "abcdefghij");
    assert_eq!(rows[1], "kl");
    engine.reset()?;
    assert!(
        engine.resize(20, 3, 8, 16)?.is_empty(),
        "reset clears report mode"
    );
    Ok(())
}

#[test]
fn keys_follow_modes_and_leave_printable_commits_to_platform() -> Result<()> {
    let mut engine = Engine::new(20, 3)?;
    assert_eq!(engine.key("c", true, false, false)?, Some(b"\x03".to_vec()));
    assert_eq!(
        engine.key("up", false, false, false)?,
        Some(b"\x1b[A".to_vec())
    );
    engine.feed(b"\x1b[?1h", false)?;
    assert_eq!(
        engine.key("up", false, false, false)?,
        Some(b"\x1bOA".to_vec())
    );
    assert_eq!(
        engine.key("tab", false, false, true)?,
        Some(b"\x1b[Z".to_vec())
    );
    assert_eq!(engine.key("é", false, false, false)?, None);
    assert_eq!(
        engine.key("x", false, true, false)?,
        Some(b"\x1bx".to_vec())
    );
    for (key, byte) in [
        ("@", 0),
        ("i", 9),
        ("m", 13),
        ("[", 27),
        ("\\", 28),
        ("]", 29),
        ("^", 30),
        ("_", 31),
    ] {
        assert_eq!(
            engine.key(key, true, false, false)?,
            Some(vec![byte]),
            "Ctrl+{key}"
        );
    }
    // Consecutive temporary UTF-8 storage must not be retained by native events.
    for text in [String::from("a"), String::from("z")] {
        assert!(engine.key(&text, true, false, false)?.is_some());
    }
    engine.feed(b"\x1b[>1u", false)?;
    assert_eq!(
        engine.key("i", true, false, false)?,
        Some(b"\x1b[105;5u".to_vec())
    );
    engine.reset()?;
    engine.feed(b"\x1b[>4;2m", false)?;
    assert_eq!(
        engine.key("i", true, false, false)?,
        Some(b"\x1b[27;5;105~".to_vec())
    );
    Ok(())
}

#[test]
fn active_screen_tracks_vt_switch_and_arrow_encoding_mode() -> Result<()> {
    let mut engine = Engine::new(40, 8)?;
    assert!(!engine.is_alternate_screen()?);
    engine.feed(b"\x1b[?1049h", false)?;
    assert!(engine.is_alternate_screen()?);
    assert_eq!(
        engine.key("up", false, false, false)?,
        Some(b"\x1b[A".to_vec())
    );
    engine.feed(b"\x1b[?1h", false)?;
    assert_eq!(
        engine.key("up", false, false, false)?,
        Some(b"\x1bOA".to_vec())
    );
    engine.feed(b"\x1b[?1049l", false)?;
    assert!(!engine.is_alternate_screen()?);
    Ok(())
}

#[test]
fn paste_preserves_policy_and_submits_complete_brackets() -> Result<()> {
    let mut engine = Engine::new(20, 3)?;
    assert_eq!(engine.paste("a\r\nb\nc")?, b"a\rb\rc");
    engine.feed(b"\x1b[?2004h", false)?;
    assert_eq!(
        engine.paste("a\x1b[201~\nb")?,
        b"\x1b[200~a[201~\rb\x1b[201~"
    );
    assert_eq!(engine.paste(&"x".repeat(MAX_INPUT - 12))?.len(), MAX_INPUT);
    assert!(engine.paste(&"x".repeat(MAX_INPUT - 11)).is_err());
    assert!(engine.feed(b"", false)?.is_empty());
    assert_eq!(engine.paste("ok")?, b"\x1b[200~ok\x1b[201~");
    Ok(())
}

#[test]
fn selection_tracks_content_and_copy_never_uses_stale_references() -> Result<()> {
    let mut engine = Engine::new(20, 3)?;
    engine.feed(b"hello world", false)?;
    engine.select_start(2, 0, true)?;
    assert_eq!(engine.selection_text()?.as_deref(), Some("hello"));
    engine.select_end()?;
    engine.feed(b"\r\nsecond\r\nthird\r\nfourth", false)?;
    assert_eq!(engine.selection_text()?.as_deref(), Some("hello"));
    engine.scroll(2)?;
    assert!(screen(&mut engine)?
        .iter()
        .any(|line| line == "hello world"));
    engine.scroll_bottom()?;
    engine.select_start(0, 0, false)?;
    engine.select_update(5, 0)?;
    engine.select_end()?;
    assert_eq!(engine.selection_text()?.as_deref(), Some("second"));
    engine.resize(10, 4, 8, 16)?;
    assert_eq!(engine.selection_text()?.as_deref(), Some("second"));
    engine.reset()?;
    assert_eq!(engine.selection_text()?, None);
    Ok(())
}

#[test]
fn wide_graphemes_styles_cursor_and_frame_damage_are_available() -> Result<()> {
    let mut engine = Engine::new(20, 3)?;
    engine.feed("\x1b[1;3;4;31m界e\u{301}\x1b[0m".as_bytes(), false)?;
    let mut frame = engine.frame()?;
    assert!(frame.full_dirty);
    assert!(frame.cursor.visible);
    assert_eq!(frame.cursor.col, 3);
    assert_eq!(frame.background, 0x10151c);
    let id;
    {
        let mut row = frame.next_row()?.expect("first row");
        id = row.id;
        assert_eq!(row.index, 0);
        let cell = row.next_cell()?.expect("wide cell");
        assert_eq!((cell.col, cell.text, cell.width), (0, "界", 2));
        assert!(cell.bold && cell.italic && cell.underline && !cell.hidden);
        assert!(row.next_cell()?.expect("wide spacer").skip);
        let cell = row.next_cell()?.expect("combining grapheme");
        assert_eq!((cell.col, cell.text, cell.width), (2, "e\u{301}", 1));
    }
    // Reading cells is optional: cached rows can be skipped safely.
    while frame.next_row()?.is_some() {}
    frame.finish()?;
    let mut frame = engine.frame()?;
    assert!(!frame.full_dirty);
    let row = frame.next_row()?.expect("first row");
    assert_eq!(row.id, id);
    assert!(!row.dirty);
    frame.finish()?;
    engine.feed(b"x", false)?;
    let mut frame = engine.frame()?;
    assert!(frame.next_row()?.expect("changed row").dirty);
    frame.finish()?;
    Ok(())
}

#[test]
fn selection_damage_and_scrolled_cursor_are_observable() -> Result<()> {
    let mut engine = Engine::new(20, 3)?;
    engine.feed(b"hello world", false)?;
    screen(&mut engine)?;
    engine.select_start(1, 0, true)?;
    let mut frame = engine.frame()?;
    let full_dirty = frame.full_dirty;
    let mut row = frame.next_row()?.expect("selected row");
    assert!(row.dirty || full_dirty);
    assert!(row.next_cell()?.expect("selected cell").selected);
    frame.finish()?;
    engine.feed(b"\r\na\r\nb\r\nc\r\nd", false)?;
    engine.scroll(3)?;
    assert!(!engine.cursor()?.visible);
    engine.scroll_bottom()?;
    assert!(engine.cursor()?.visible);
    Ok(())
}

#[test]
fn clipboard_and_image_effects_stay_disabled() -> Result<()> {
    let mut engine = Engine::new(20, 3)?;
    assert!(engine
        .feed(b"\x1b]52;c;aGVsbG8=\x07\x1b]52;c;?\x07", false)?
        .is_empty());
    // No clipboard-read callback means mode 5522 still pastes literal text.
    engine.feed(b"\x1b[?5522h", false)?;
    assert_eq!(engine.paste("safe")?, b"safe");
    Ok(())
}

#[test]
fn erased_cells_keep_their_background_and_palette_changes_invalidate() -> Result<()> {
    let mut engine = Engine::new(20, 3)?;
    engine.feed(b"\x1b[41m\x1b[2J\x1b[0m", false)?;
    let mut frame = engine.frame()?;
    let mut row = frame.next_row()?.expect("erased row");
    let cell = row.next_cell()?.expect("erased cell");
    assert!(cell.text.is_empty());
    assert_eq!(cell.background, 0xe06c75);
    frame.finish()?;
    engine.feed(b"\x1b]4;1;rgb:11/22/33\x07", false)?;
    let mut frame = engine.frame()?;
    let full_dirty = frame.full_dirty;
    let mut row = frame.next_row()?.expect("recolored row");
    assert!(full_dirty || row.dirty);
    assert_eq!(
        row.next_cell()?.expect("recolored cell").background,
        0x112233
    );
    frame.finish()?;
    Ok(())
}

#[test]
fn applied_colors_repaint_retained_content_and_survive_engine_reset() -> Result<()> {
    let mut engine = Engine::new(20, 3)?;
    engine.feed(b"keep \x1b[31mred", false)?;
    assert_eq!(screen(&mut engine)?[0], "keep red");
    let extended_before = {
        let frame = engine.frame()?;
        let color = packed(frame.colors.palette[16]);
        frame.finish()?;
        color
    };
    let mut colors = ColorConfig {
        foreground: 0x112233,
        background: 0x223344,
        cursor: 0x334455,
        ..ColorConfig::default()
    };
    colors.ansi[1] = 0x445566;
    engine.apply_colors(colors)?;
    let mut frame = engine.frame()?;
    assert_eq!(frame.background, colors.background);
    assert_eq!(frame.cursor.color, colors.cursor);
    assert_eq!(packed(frame.colors.palette[16]), extended_before);
    assert!(frame.full_dirty, "a retained row must be recolored");
    let mut row = frame.next_row()?.expect("content row");
    let mut foregrounds = Vec::new();
    while let Some(cell) = row.next_cell()? {
        if !cell.text.is_empty() && cell.text != " " {
            foregrounds.push(cell.foreground);
        }
    }
    assert!(foregrounds.contains(&colors.foreground));
    assert!(foregrounds.contains(&colors.ansi[1]));
    frame.finish()?;
    assert_eq!(screen(&mut engine)?[0], "keep red");
    engine.reset()?;
    assert_eq!(engine.colors, colors);
    let frame = engine.frame()?;
    assert_eq!(frame.background, colors.background);
    assert_eq!(frame.cursor.color, colors.cursor);
    frame.finish()?;
    Ok(())
}

#[test]
fn pinned_c_abi_layouts_match_64_bit_headers() {
    // Linux/macOS/Windows desktop targets have 64-bit pointers. Sizes/alignments
    // were extracted with sizeof/_Alignof from the matching 1.0.0 C headers.
    if mem::size_of::<usize>() != 8 {
        return;
    }
    macro_rules! layout {
        ($ty:ty, $size:expr, $align:expr) => {
            assert_eq!(
                (mem::size_of::<$ty>(), mem::align_of::<$ty>()),
                ($size, $align),
                stringify!($ty)
            );
        };
    }
    layout!(ffi::Allocator, 16, 8);
    layout!(ffi::AllocatorVtable, 64, 8);
    layout!(ffi::SelectionBuffer, 24, 8);
    assert_eq!(mem::offset_of!(ffi::Allocator, vtable), 8);
    assert_eq!(mem::offset_of!(ffi::AllocatorVtable, free), 24);
    assert_eq!(mem::offset_of!(ffi::AllocatorVtable, reserved), 32);
    layout!(ffi::Rgb, 3, 1);
    layout!(ffi::StringView, 16, 8);
    layout!(ffi::Buffer, 24, 8);
    layout!(ffi::StyleColorValue, 3, 1);
    layout!(ffi::StyleColor, 4, 1);
    layout!(ffi::Style, 16, 2);
    layout!(ffi::Coordinate, 8, 4);
    layout!(ffi::PointValue, 16, 8);
    layout!(ffi::Point, 24, 8);
    layout!(ffi::GridRef, 16, 8);
    layout!(ffi::Selection, 40, 8);
    layout!(ffi::Position, 16, 8);
    layout!(ffi::MousePosition, 8, 4);
    layout!(ffi::MouseSize, 40, 8);
    assert_eq!(mem::offset_of!(ffi::MouseSize, screen_width), 8);
    assert_eq!(mem::offset_of!(ffi::MouseSize, padding_left), 36);
    layout!(ffi::GestureGeometry, 16, 4);
    layout!(ffi::GestureBehaviors, 12, 4);
    layout!(ffi::ScrollValue, 16, 8);
    layout!(ffi::Scroll, 24, 8);
    layout!(ffi::Scrollbar, 24, 8);
    layout!(ffi::Writer, 16, 8);
    layout!(ffi::MimeReader, 16, 8);
    layout!(ffi::Paste, 56, 8);
    layout!(ffi::RenderCursor, 24, 8);
    layout!(ffi::RenderColors, 792, 8);
    assert_eq!(mem::offset_of!(ffi::RenderCursor, visual_style), 20);
    assert_eq!(mem::offset_of!(ffi::RenderColors, palette), 18);
    assert_eq!(mem::offset_of!(ffi::Paste, reader), 32);
    assert_eq!(mem::offset_of!(ffi::Style, flags), 12);
}

fn pointer(action: MouseAction, button: Option<MouseButton>, x: f32, y: f32) -> MouseInput {
    MouseInput {
        action,
        button,
        x,
        y,
        modifiers: MouseModifiers::default(),
        any_button_pressed: button.is_some(),
    }
}

#[test]
fn mouse_tracking_and_wheel_policy_follow_canonical_encoder() -> Result<()> {
    for (mode, tracking) in [
        (0, MouseTracking::None),
        (9, MouseTracking::X10),
        (1000, MouseTracking::Normal),
        (1002, MouseTracking::Button),
        (1003, MouseTracking::Any),
    ] {
        let mut engine = Engine::new(100, 30)?;
        engine.set_mouse_geometry(1000.0, 600.0, 10.0, 20.0)?;
        engine.feed(format!("\x1b[?{mode}h\x1b[?1006h").as_bytes(), false)?;
        let policy = engine.mouse_policy()?;
        assert_eq!(policy.tracking, tracking);
        assert_eq!(policy.reports_buttons(), mode != 0);
        assert_eq!(policy.reports_wheel(), matches!(mode, 1000 | 1002 | 1003));
        let press = pointer(MouseAction::Press, Some(MouseButton::Left), 22.0, 45.0);
        assert_eq!(!engine.mouse(press)?.is_empty(), mode != 0);
        assert_eq!(
            !engine
                .mouse(MouseInput {
                    action: MouseAction::Release,
                    ..press
                })?
                .is_empty(),
            policy.reports_wheel()
        );
        assert_eq!(
            !engine
                .mouse(pointer(
                    MouseAction::Motion,
                    Some(MouseButton::Left),
                    32.0,
                    45.0
                ))?
                .is_empty(),
            matches!(mode, 1002 | 1003)
        );
        assert_eq!(
            !engine
                .mouse(pointer(MouseAction::Motion, None, 42.0, 45.0))?
                .is_empty(),
            mode == 1003
        );
        assert_eq!(
            !engine
                .mouse(pointer(
                    MouseAction::Press,
                    Some(MouseButton::WheelUp),
                    22.0,
                    45.0
                ))?
                .is_empty(),
            policy.reports_wheel()
        );
    }
    let mut engine = Engine::new(100, 30)?;
    engine.feed(b"\x1b[?1000h\x1b[?1003h\x1b[?1000l", false)?;
    assert_eq!(
        engine.mouse_policy()?.tracking,
        MouseTracking::None,
        "mode bits retain 1003, actual native encoder flag is None"
    );
    engine.feed(b"\x1b[?1002h", false)?;
    assert_eq!(engine.mouse_policy()?.tracking, MouseTracking::Button);
    engine.feed(b"\x1b[?1002l", false)?;
    assert!(engine
        .mouse(pointer(
            MouseAction::Release,
            Some(MouseButton::Left),
            0.0,
            0.0
        ))?
        .is_empty());
    Ok(())
}

#[test]
fn mouse_all_formats_preserve_bytes_and_coordinates() -> Result<()> {
    for (format, expected) in [
        (0, vec![0x1b, b'[', b'M', 32, 133, 35]),
        (1005, b"\x1b[M \xc2\x85#".to_vec()),
        (1006, b"\x1b[<0;101;3M".to_vec()),
        (1015, b"\x1b[32;101;3M".to_vec()),
        (1016, b"\x1b[<0;1002;45M".to_vec()),
    ] {
        let mut engine = Engine::new(200, 30)?;
        engine.set_mouse_geometry(2000.0, 600.0, 10.0, 20.0)?;
        engine.feed(format!("\x1b[?1000h\x1b[?{format}h").as_bytes(), false)?;
        assert_eq!(
            engine.mouse(pointer(
                MouseAction::Press,
                Some(MouseButton::Left),
                1002.0,
                45.0
            ))?,
            expected,
            "format {format}"
        );
    }
    let mut engine = Engine::new(400, 30)?;
    engine.set_mouse_geometry(4000.0, 600.0, 10.0, 20.0)?;
    engine.feed(b"\x1b[?1000h", false)?;
    assert!(engine
        .mouse(pointer(
            MouseAction::Press,
            Some(MouseButton::Left),
            2240.0,
            20.0
        ))?
        .is_empty());
    assert!(
        engine.mouse_policy()?.reports_wheel(),
        "legacy coordinate overflow must not trigger local wheel fallback"
    );
    engine.feed(b"\x1b[?1006h\x1b[?1016h\x1b[?1016l", false)?;
    assert_eq!(
        engine.mouse(pointer(
            MouseAction::Press,
            Some(MouseButton::Left),
            1002.0,
            45.0
        ))?,
        b"\x1b[M \x85#",
        "format flag resets to legacy even though SGR mode bit remains set"
    );
    engine.feed(b"\x1b[?1006h", false)?;
    for (button, code) in [
        (MouseButton::Left, 0),
        (MouseButton::Middle, 1),
        (MouseButton::Right, 2),
        (MouseButton::WheelUp, 64),
        (MouseButton::WheelDown, 65),
        (MouseButton::WheelLeft, 66),
        (MouseButton::WheelRight, 67),
        (MouseButton::Back, 128),
        (MouseButton::Forward, 129),
    ] {
        assert_eq!(
            engine.mouse(pointer(MouseAction::Press, Some(button), 22.0, 45.0))?,
            format!("\x1b[<{code};3;3M").as_bytes()
        );
    }
    Ok(())
}

#[test]
fn mouse_motion_dedup_and_pixel_positions_survive_geometry_changes() -> Result<()> {
    let mut engine = Engine::new(300, 30)?;
    let cell = 9.37;
    engine.set_mouse_geometry(300.0 * cell, 630.0, cell, 21.0)?;
    engine.feed(b"\x1b[?1003h\x1b[?1006h", false)?;
    let event = pointer(MouseAction::Motion, None, 250.25 * cell, 2.25 * 21.0);
    assert_eq!(engine.mouse(event)?, b"\x1b[<35;251;3M");
    assert!(engine.mouse(event)?.is_empty());
    engine.feed(b"ordinary application repaint", false)?;
    assert!(
        engine.mouse(event)?.is_empty(),
        "ordinary output must not reset motion suppression"
    );
    assert!(engine
        .mouse(MouseInput {
            x: 250.75 * cell,
            ..event
        })?
        .is_empty());
    engine.set_mouse_geometry(300.0 * cell, 630.0, cell, 21.0)?;
    assert!(
        engine.mouse(event)?.is_empty(),
        "unchanged geometry retains dedup"
    );
    engine.reset_mouse()?;
    assert!(!engine.mouse(event)?.is_empty());
    engine.set_mouse_geometry(600.0 * cell, 1260.0, 2.0 * cell, 42.0)?;
    let scaled = MouseInput {
        x: event.x * 2.0,
        y: event.y * 2.0,
        ..event
    };
    assert_eq!(engine.mouse(scaled)?, b"\x1b[<35;251;3M");
    engine.feed(b"\x1b[?1016h", false)?;
    assert_eq!(
        engine.mouse(pointer(MouseAction::Motion, None, 33.2, 45.2))?,
        b"\x1b[<35;33;45M"
    );
    assert_eq!(
        engine.mouse(pointer(MouseAction::Motion, None, 34.2, 45.2))?,
        b"\x1b[<35;34;45M"
    );
    engine.feed(b"\x1b[?1016l\x1b[?1006h", false)?;
    assert_eq!(engine.mouse(scaled)?, b"\x1b[<35;251;3M");
    engine.reset()?;
    assert_eq!(engine.mouse_policy()?.tracking, MouseTracking::None);
    engine.feed(b"\x1b[?1000h\x1b[?1006h", false)?;
    assert_eq!(
        engine.mouse(MouseInput {
            action: MouseAction::Press,
            button: Some(MouseButton::Left),
            ..scaled
        })?,
        b"\x1b[<0;251;3M"
    );
    Ok(())
}

#[test]
fn mouse_outside_release_modifiers_and_invalid_geometry_are_bounded() -> Result<()> {
    assert_eq!(
        MouseModifiers {
            shift: true,
            control: true,
            alt: true,
            super_key: true,
        }
        .native_bits(),
        0b1111
    );
    let mut engine = Engine::new(100, 30)?;
    engine.set_mouse_geometry(1000.0, 600.0, 10.0, 20.0)?;
    engine.feed(b"\x1b[?1002h\x1b[?1006h", false)?;
    let press = MouseInput {
        modifiers: MouseModifiers {
            shift: true,
            control: true,
            alt: true,
            super_key: false,
        },
        ..pointer(MouseAction::Press, Some(MouseButton::Right), 22.0, 45.0)
    };
    assert_eq!(engine.mouse(press)?, b"\x1b[<30;3;3M");
    assert_eq!(
        engine.mouse(MouseInput {
            action: MouseAction::Motion,
            x: -50.0,
            y: 620.0,
            ..press
        })?,
        b"\x1b[<62;1;30M"
    );
    assert_eq!(
        engine.mouse(MouseInput {
            action: MouseAction::Release,
            x: -50.0,
            y: 620.0,
            ..press
        })?,
        b"\x1b[<30;1;30m"
    );
    assert!(engine
        .mouse(pointer(MouseAction::Motion, None, 1050.0, 100.0))?
        .is_empty());
    assert!(engine
        .mouse(MouseInput {
            x: f32::NAN,
            ..press
        })
        .is_err());
    assert!(engine
        .mouse(MouseInput {
            x: f32::MAX,
            ..press
        })
        .is_err());
    assert!(engine
        .mouse(MouseInput {
            x: 1_000_000.0,
            ..press
        })
        .is_err());
    for bad in [0.0, -1.0, f32::NAN, f32::INFINITY] {
        assert!(engine.set_mouse_geometry(1000.0, 600.0, bad, 20.0).is_err());
    }
    assert!(engine
        .set_mouse_geometry(1_000_000.0, 600.0, 0.01, 20.0)
        .is_err());
    assert_eq!(
        engine.mouse(press)?,
        b"\x1b[<30;3;3M",
        "invalid geometry leaves previous geometry usable"
    );
    engine.feed(b"\x1b[?9h", false)?;
    assert_eq!(
        engine.mouse(press)?,
        b"\x1b[<2;3;3M",
        "X10 tracking omits modifiers even in SGR format"
    );
    Ok(())
}
