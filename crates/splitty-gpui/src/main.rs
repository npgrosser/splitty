//! Explicitly invoked native UI experiment; released desktop clients are unchanged.
mod app;
mod document;
mod offscreen;
mod terminal;

use gpui_kit::*;
use std::{
    cell::RefCell,
    rc::Rc,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
};

fn main() -> anyhow::Result<()> {
    if let Some(code) = offscreen::subprocess_exit() {
        std::process::exit(code);
    }
    let mut smoke = false;
    let mut web = false;
    let mut screenshot = None;
    let mut arguments = std::env::args().skip(1);
    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "--smoke" => smoke = true,
            "--web-spike" => web = true,
            "--screenshot" => {
                screenshot =
                    Some(std::path::PathBuf::from(arguments.next().ok_or_else(
                        || anyhow::anyhow!("--screenshot requires a path"),
                    )?))
            }
            "--help" | "-h" => {
                println!("splitty-gpui [--smoke] [--web-spike] [--screenshot PATH]\nExperimental isolated native terminal/Markdown probe. --smoke reports automated evidence and exits; manual IME and clipboard remain unverified.\n--screenshot requires --smoke and the capture feature. The pinned GPUI snapshot does not implement native Linux window capture; use external compositor/window capture there. An explicitly requested screenshot failure fails the smoke.");
                return Ok(());
            }
            other => anyhow::bail!("Unknown probe argument: {other}"),
        }
    }
    if screenshot.is_some() && (!smoke || !cfg!(feature = "capture")) {
        anyhow::bail!("--screenshot requires --smoke and a build with --features capture");
    }
    let worker = Rc::new(RefCell::new(None));
    let launched_worker = worker.clone();
    let outcome = Arc::new(AtomicBool::new(false));
    let reported_outcome = outcome.clone();
    gpui_kit::application().run(move |cx| {
        gpui_kit::init(cx);
        let theme = gpui_kit::base::Theme::global_mut(cx);
        theme.appearance = gpui_kit::base::ThemeAppearance::Dark;
        theme.tokens.colors = gpui_kit::base::ColorTokens::dark();
        let result = gpui_kit::open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::centered(size(px(1200.), px(760.)), cx)),
                titlebar: Some(TitlebarOptions {
                    title: Some("Splitty native UI probe".into()),
                    ..Default::default()
                }),
                ..Default::default()
            },
            cx,
            move |window, cx| {
                cx.new(|cx| {
                    let (probe, owned_worker) =
                        app::Probe::new(smoke, web, reported_outcome, screenshot, window, cx);
                    *launched_worker.borrow_mut() = Some(owned_worker);
                    probe
                })
            },
        );
        if let Err(error) = result {
            eprintln!("Cannot open native GPUI window: {error:#}");
            cx.quit();
        }
        cx.activate(true);
    });
    if let Some(worker) = worker.borrow_mut().take() {
        worker.shutdown()?;
        if smoke {
            eprintln!("{{\"probeShutdown\":\"complete\"}}");
        }
    }
    if smoke && !outcome.load(Ordering::Acquire) {
        anyhow::bail!("Native smoke did not satisfy its evidence checks");
    }
    Ok(())
}
