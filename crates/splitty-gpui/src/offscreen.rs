//! Optional fixed-fixture CEF probe. It has no client, service handle or command bridge.

/// Frame storage has a separate fixed ceiling; it is not transport data.
pub const WIDTH: u32 = 640;
pub const HEIGHT: u32 = 240;
#[cfg(any(feature = "web-spike", test))]
const MAX_PIXELS: usize = 1024 * 768;

pub struct Frame {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
    pub sequence: u64,
}

#[cfg(any(feature = "web-spike", test))]
fn frame_len(width: i32, height: i32) -> Option<usize> {
    let width = usize::try_from(width).ok().filter(|n| *n > 0)?;
    let height = usize::try_from(height).ok().filter(|n| *n > 0)?;
    width
        .checked_mul(height)
        .filter(|pixels| *pixels <= MAX_PIXELS)?
        .checked_mul(4)
}

/// CEF's child entry must run before starting an application runtime.
pub fn subprocess_exit() -> Option<i32> {
    #[cfg(feature = "web-spike")]
    {
        engine::subprocess_exit()
    }
    #[cfg(not(feature = "web-spike"))]
    {
        None
    }
}

pub struct OffscreenProbe {
    status: String,
    script_ticks: u64,
    #[cfg(feature = "web-spike")]
    engine: Option<engine::Engine>,
}

impl OffscreenProbe {
    pub fn new(enabled: bool) -> Self {
        let mut probe = Self {
            script_ticks: 0,
            status: if enabled {
                "Offscreen engine unavailable: build with --features web-spike".into()
            } else {
                "Offscreen engine disabled; use --web-spike for the fixed fixture".into()
            },
            #[cfg(feature = "web-spike")]
            engine: None,
        };
        if enabled {
            probe.start();
        }
        probe
    }

    fn start(&mut self) {
        #[cfg(feature = "web-spike")]
        match engine::Engine::new() {
            Ok(engine) => {
                self.status = "CEF started; waiting for its first real frame".into();
                self.engine = Some(engine);
            }
            Err(error) => self.status = format!("Offscreen engine unavailable: {error}"),
        }
    }

    pub fn status(&self) -> &str {
        &self.status
    }

    pub fn script_ticks(&self) -> u64 {
        self.script_ticks
    }

    pub fn poll(&mut self) -> Option<Frame> {
        #[cfg(feature = "web-spike")]
        if let Some(engine) = self.engine.as_mut() {
            let frame = engine.poll();
            self.script_ticks = engine
                .script_ticks
                .load(std::sync::atomic::Ordering::Acquire);
            if let Some(frame) = frame {
                self.status = format!("CEF offscreen · frame {}", frame.sequence);
                return Some(frame);
            }
        }
        None
    }

    pub fn click(&mut self, x: i32, y: i32) {
        #[cfg(feature = "web-spike")]
        if let Some(engine) = self.engine.as_mut() {
            engine.click(x, y);
        }
        #[cfg(not(feature = "web-spike"))]
        let _ = (x, y);
    }

    /// Finish the optional engine before reporting successful probe teardown.
    pub fn close(&mut self) -> Result<(), String> {
        #[cfg(feature = "web-spike")]
        if let Some(mut engine) = self.engine.take() {
            if let Err(error) = engine.close() {
                self.status = error.clone();
                return Err(error);
            }
            self.status = "CEF offscreen engine closed".into();
        }
        Ok(())
    }
}

#[cfg(feature = "web-spike")]
mod engine {
    use super::{frame_len, Frame as PixelFrame, HEIGHT, WIDTH};
    use cef::{args::Args, rc::Rc, *};
    use std::sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc, Mutex,
    };
    use std::time::{Duration, Instant};

    const FIXTURE: &str = include_str!("../assets/offscreen.html");

    wrap_app! {
        struct ProbeApp {}
        impl App {
            fn on_before_command_line_processing(
                &self,
                _process_type: Option<&CefString>,
                command_line: Option<&mut CommandLine>,
            ) {
                if let Some(line) = command_line {
                    // CPU OSR deliberately avoids cross-platform texture sharing.
                    line.append_switch(Some(&"disable-gpu".into()));
                    line.append_switch(Some(&"disable-background-networking".into()));
                    line.append_switch(Some(&"disable-sync".into()));
                    line.append_switch(Some(&"no-first-run".into()));
                    #[cfg(target_os = "linux")]
                    if std::env::var_os("WAYLAND_DISPLAY").is_some() {
                        // Follow GPUI's display choice, including Wayland-only sessions.
                        line.append_switch_with_value(Some(&"ozone-platform".into()), Some(&"wayland".into()));
                    }
                }
            }
        }
    }

    pub fn subprocess_exit() -> Option<i32> {
        if !std::env::args().any(|arg| arg.starts_with("--type=")) {
            return None;
        }
        let _ = api_hash(sys::CEF_API_VERSION_LAST, 0);
        let args = Args::new();
        let mut app = ProbeApp::new();
        let code = execute_process(
            Some(args.as_main_args()),
            Some(&mut app),
            std::ptr::null_mut(),
        );
        Some(code.max(0))
    }

    wrap_render_handler! {
        struct Pixels {
            latest: Arc<Mutex<Option<PixelFrame>>>,
            sequence: Arc<AtomicU64>,
        }
        impl RenderHandler {
            fn view_rect(&self, _browser: Option<&mut Browser>, rect: Option<&mut Rect>) {
                if let Some(rect) = rect {
                    rect.width = i32::try_from(WIDTH).expect("fixed width fits CEF");
                    rect.height = i32::try_from(HEIGHT).expect("fixed height fits CEF");
                }
            }

            fn on_paint(
                &self,
                _browser: Option<&mut Browser>,
                kind: PaintElementType,
                _dirty_rects: Option<&[Rect]>,
                buffer: *const u8,
                width: i32,
                height: i32,
            ) {
                if buffer.is_null() || kind != PaintElementType::VIEW {
                    return;
                }
                let Some(len) = frame_len(width, height) else { return; };
                let Ok(mut latest) = self.latest.lock() else { return; };
                // Drop the superseded pending frame before allocating its replacement.
                latest.take();
                // SAFETY: CEF guarantees width*height BGRA pixels valid for this
                // callback. Positive dimensions and the fixed ceiling were checked.
                let bytes = unsafe { std::slice::from_raw_parts(buffer, len) };
                let mut rgba = bytes.to_vec();
                for pixel in rgba.chunks_exact_mut(4) {
                    pixel.swap(0, 2);
                }
                *latest = Some(PixelFrame {
                    width: u32::try_from(width).expect("positive validated width"),
                    height: u32::try_from(height).expect("positive validated height"),
                    rgba,
                    sequence: self.sequence.fetch_add(1, Ordering::Relaxed) + 1,
                });
            }
        }
    }

    wrap_request_handler! {
        struct FixtureOnly { fixture_url: String }
        impl RequestHandler {
            fn on_before_browse(
                &self, _browser: Option<&mut Browser>, _frame: Option<&mut cef::Frame>,
                request: Option<&mut Request>, _user_gesture: i32, _is_redirect: i32,
            ) -> i32 {
                let allowed = request.is_some_and(|request| CefString::from(&request.url()).to_string() == self.fixture_url);
                i32::from(!allowed)
            }
            fn on_open_urlfrom_tab(
                &self, _browser: Option<&mut Browser>, _frame: Option<&mut cef::Frame>,
                _target_url: Option<&CefString>, _target_disposition: WindowOpenDisposition,
                _user_gesture: i32,
            ) -> i32 { 1 }
        }
    }

    wrap_life_span_handler! {
        struct Lifetime { closed: Arc<AtomicBool> }
        impl LifeSpanHandler {
            fn on_before_close(&self, _browser: Option<&mut Browser>) {
                self.closed.store(true, Ordering::Release);
            }
            fn on_before_popup(
                &self, _browser: Option<&mut Browser>, _frame: Option<&mut cef::Frame>,
                _popup_id: i32, _target_url: Option<&CefString>, _target_frame_name: Option<&CefString>,
                _target_disposition: WindowOpenDisposition, _user_gesture: i32,
                _popup_features: Option<&PopupFeatures>, _window_info: Option<&mut WindowInfo>,
                _client: Option<&mut Option<Client>>, _settings: Option<&mut BrowserSettings>,
                _extra_info: Option<&mut Option<DictionaryValue>>, _no_javascript_access: Option<&mut i32>,
            ) -> i32 { 1 }
        }
    }

    wrap_client! {
        struct ProbeClient {
            pixels: RenderHandler,
            requests: RequestHandler,
            lifetime: LifeSpanHandler,
            progress: DisplayHandler,
        }
        impl Client {
            fn render_handler(&self) -> Option<RenderHandler> { Some(self.pixels.clone()) }
            fn request_handler(&self) -> Option<RequestHandler> { Some(self.requests.clone()) }
            fn life_span_handler(&self) -> Option<LifeSpanHandler> { Some(self.lifetime.clone()) }
            fn display_handler(&self) -> Option<DisplayHandler> { Some(self.progress.clone()) }
        }
    }

    wrap_display_handler! {
        struct Progress { ticks: Arc<AtomicU64> }
        impl DisplayHandler {
            fn on_title_change(&self, _browser: Option<&mut Browser>, title: Option<&CefString>) {
                // One-way proof that this fixed fixture executes its timer. No
                // script message can address the service or change application state.
                let Some(title) = title else { return; };
                let title = title.to_string();
                let Some(ticks) = title.strip_prefix("splitty-fixture:")
                    .filter(|value| value.len() <= 6)
                    .and_then(|value| value.parse::<u64>().ok()) else { return; };
                self.ticks.fetch_max(ticks.min(999_999), Ordering::Release);
            }
        }
    }

    pub struct Engine {
        browser: Option<Browser>,
        latest: Arc<Mutex<Option<PixelFrame>>>,
        closed: Arc<AtomicBool>,
        cache: Option<tempfile::TempDir>,
        close_attempted: bool,
        pub script_ticks: Arc<AtomicU64>,
    }

    impl Engine {
        pub fn new() -> Result<Self, String> {
            let cache = tempfile::tempdir().map_err(|e| e.to_string())?;
            let _ = api_hash(sys::CEF_API_VERSION_LAST, 0);
            let args = Args::new();
            let mut app = ProbeApp::new();
            let settings = Settings {
                windowless_rendering_enabled: 1,
                external_message_pump: 1,
                command_line_args_disabled: 1,
                root_cache_path: cache.path().to_string_lossy().as_ref().into(),
                ..Default::default()
            };
            if initialize(
                Some(args.as_main_args()),
                Some(&settings),
                Some(&mut app),
                std::ptr::null_mut(),
            ) != 1
            {
                return Err("CEF initialization failed with sandbox enabled".into());
            }
            let latest = Arc::new(Mutex::new(None));
            let closed = Arc::new(AtomicBool::new(false));
            let script_ticks = Arc::new(AtomicU64::new(0));
            let fixture_url = format!(
                "data:text/html;charset=utf-8,{}",
                percent_encoding::utf8_percent_encode(FIXTURE, percent_encoding::NON_ALPHANUMERIC)
            );
            let mut client = ProbeClient::new(
                Pixels::new(latest.clone(), Arc::new(AtomicU64::new(0))),
                FixtureOnly::new(fixture_url.clone()),
                Lifetime::new(closed.clone()),
                Progress::new(script_ticks.clone()),
            );
            let browser = browser_host_create_browser_sync(
                Some(&WindowInfo {
                    windowless_rendering_enabled: 1,
                    ..Default::default()
                }),
                Some(&mut client),
                Some(&fixture_url.as_str().into()),
                Some(&BrowserSettings {
                    windowless_frame_rate: 15,
                    javascript_access_clipboard: State::DISABLED,
                    javascript_dom_paste: State::DISABLED,
                    remote_fonts: State::DISABLED,
                    image_loading: State::DISABLED,
                    local_storage: State::DISABLED,
                    webgl: State::DISABLED,
                    ..Default::default()
                }),
                None,
                None,
            );
            if browser.is_none() {
                shutdown();
                return Err("CEF did not create the offscreen fixture".into());
            }
            Ok(Self {
                browser,
                latest,
                closed,
                cache: Some(cache),
                close_attempted: false,
                script_ticks,
            })
        }

        pub fn poll(&mut self) -> Option<PixelFrame> {
            do_message_loop_work();
            self.latest.lock().ok()?.take()
        }

        pub fn click(&mut self, x: i32, y: i32) {
            if x < 0
                || y < 0
                || x >= i32::try_from(WIDTH).expect("width")
                || y >= i32::try_from(HEIGHT).expect("height")
            {
                return;
            }
            if let Some(host) = self.browser.as_mut().and_then(|browser| browser.host()) {
                let event = MouseEvent {
                    x,
                    y,
                    ..Default::default()
                };
                host.send_mouse_click_event(Some(&event), MouseButtonType::LEFT, 0, 1);
                host.send_mouse_click_event(Some(&event), MouseButtonType::LEFT, 1, 1);
            }
        }

        pub fn close(&mut self) -> Result<(), String> {
            self.close_attempted = true;
            if let Some(host) = self.browser.as_mut().and_then(|browser| browser.host()) {
                host.close_browser(1);
            }
            self.browser.take();
            let deadline = Instant::now() + Duration::from_secs(2);
            while !self.closed.load(Ordering::Acquire) && Instant::now() < deadline {
                do_message_loop_work();
                std::thread::sleep(Duration::from_millis(5));
            }
            if self.closed.load(Ordering::Acquire) {
                shutdown();
                self.cache.take();
                Ok(())
            } else {
                // Do not remove a cache that may still be used by live CEF
                // workers. This is failed cleanup, never passing smoke evidence.
                if let Some(cache) = self.cache.take() {
                    let _ = cache.keep();
                }
                Err("CEF probe cleanup timed out before browser closure".into())
            }
        }
    }

    impl Drop for Engine {
        fn drop(&mut self) {
            if !self.close_attempted {
                if let Err(error) = self.close() {
                    eprintln!("{error}");
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_empty_negative_overflowing_and_oversized_frames_before_copying() {
        for (width, height) in [(0, 2), (-1, 2), (2, -1), (i32::MAX, i32::MAX), (1025, 768)] {
            assert_eq!(frame_len(width, height), None);
        }
        assert_eq!(frame_len(640, 240), Some(640 * 240 * 4));
        assert_eq!(frame_len(1024, 768), Some(MAX_PIXELS * 4));
    }

    #[test]
    fn disabled_engine_never_fabricates_a_rendered_frame() {
        let mut probe = OffscreenProbe::new(false);
        assert!(probe.poll().is_none());
        assert!(probe.status().contains("disabled"));
        probe.click(10, 10);
        assert!(probe.poll().is_none());
    }

    #[cfg(not(feature = "web-spike"))]
    #[test]
    fn missing_feature_reports_unavailable_without_a_frame() {
        let mut probe = OffscreenProbe::new(true);
        assert!(probe.status().contains("unavailable"));
        assert!(probe.poll().is_none());
    }
}
