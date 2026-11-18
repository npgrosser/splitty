use gpui::{
    Bounds, ContentMask, Corners, DevicePixels, Quad, ScaledPixels, Scene, Shadow, point, rgb, size,
};
use gpui_wgpu::WgpuHeadlessRenderer;

// Real GPU readback: rounding only the window's background would leave
// the second opaque child quad visible in the corners.
#[test]
fn client_frame_clips_children_preserves_shadows_and_tracks_tiling() {
    let mut renderer = WgpuHeadlessRenderer::new().unwrap();
    let dimensions = size(DevicePixels(64), DevicePixels(64));
    let frame = Bounds::new(
        point(ScaledPixels(8.), ScaledPixels(8.)),
        size(ScaledPixels(48.), ScaledPixels(48.)),
    );
    let viewport = Bounds::new(
        point(ScaledPixels(0.), ScaledPixels(0.)),
        size(ScaledPixels(64.), ScaledPixels(64.)),
    );
    let mut scene = Scene::default();
    for color in [0xff0000, 0x0000ff] {
        scene.insert_primitive(Quad {
            bounds: viewport,
            content_mask: ContentMask { bounds: viewport },
            background: rgb(color).into(),
            ..Default::default()
        });
    }
    scene.finish();
    let draw = |renderer: &mut WgpuHeadlessRenderer, scene: &Scene| {
        renderer
            .render_scene_to_transparent_image(scene, dimensions)
            .unwrap()
    };
    renderer.set_client_frame(frame, Corners::all(ScaledPixels(12.)));
    let rounded = draw(&mut renderer, &scene);
    assert_eq!(rounded.get_pixel(8, 8).0[3], 0);
    assert_eq!(rounded.get_pixel(55, 55).0[3], 0);
    assert_eq!(rounded.get_pixel(32, 32).0, [0, 0, 255, 255]);
    assert_eq!(rounded.get_pixel(32, 8).0[3], 255);
    assert!(
        rounded.pixels().any(|p| p.0[3] > 0 && p.0[3] < 255),
        "antialiased silhouette"
    );

    // Paths always use premultiplied composition, including on a straight-
    // alpha surface. The mask must remove RGB as well as alpha there.
    let mut builder = gpui::PathBuilder::fill();
    builder.move_to(point(gpui::px(0.), gpui::px(0.)));
    builder.line_to(point(gpui::px(64.), gpui::px(0.)));
    builder.line_to(point(gpui::px(64.), gpui::px(64.)));
    builder.line_to(point(gpui::px(0.), gpui::px(64.)));
    builder.close();
    let mut path = builder.build().unwrap().scale(1.);
    path.content_mask = ContentMask { bounds: viewport };
    path.color = rgb(0xffffff).into();
    let mut path_scene = Scene::default();
    path_scene.insert_primitive(path);
    path_scene.finish();
    let path_image = draw(&mut renderer, &path_scene);
    assert_eq!(path_image.get_pixel(8, 8).0, [0, 0, 0, 0]);
    assert_eq!(path_image.get_pixel(32, 32).0, [255; 4]);

    renderer.set_client_frame(
        frame,
        Corners {
            top_left: ScaledPixels(0.),
            top_right: ScaledPixels(0.),
            bottom_left: ScaledPixels(12.),
            bottom_right: ScaledPixels(12.),
        },
    );
    let tiled = draw(&mut renderer, &scene);
    assert_eq!(tiled.get_pixel(8, 8).0[3], 255);
    assert_eq!(tiled.get_pixel(8, 55).0[3], 0);
    renderer.set_client_frame(frame, Corners::default());
    assert_eq!(draw(&mut renderer, &scene).get_pixel(8, 8).0[3], 255);

    renderer.set_client_frame(frame, Corners::all(ScaledPixels(12.)));
    let frame = Bounds::new(
        point(ScaledPixels(8.), ScaledPixels(8.)),
        size(ScaledPixels(48.), ScaledPixels(48.)),
    );
    let mut shadow_scene = Scene::default();
    shadow_scene.insert_primitive(Shadow {
        order: Default::default(),
        blur_radius: ScaledPixels(3.),
        bounds: frame,
        corner_radii: Corners::all(ScaledPixels(12.)),
        content_mask: ContentMask { bounds: viewport },
        color: rgb(0xffffff).into(),
        element_bounds: frame,
        element_corner_radii: Corners::all(ScaledPixels(12.)),
        inset: 0,
        pad: 0,
    });
    shadow_scene.finish();
    assert!(
        draw(&mut renderer, &shadow_scene).get_pixel(32, 6).0[3] > 0,
        "drop shadow outside the frame is retained"
    );
}

#[test]
fn window_clipping_shaders_validate_for_all_transports() {
    let common = include_str!("../src/shaders.wgsl");
    let storage = include_str!("../src/shaders_storage.wgsl");
    for (source, caps) in [
        (
            format!("{common}\n{storage}"),
            naga::valid::Capabilities::empty(),
        ),
        (
            format!("{common}\n{}", include_str!("../src/shaders_webgl.wgsl")),
            naga::valid::Capabilities::empty(),
        ),
        (
            format!(
                "enable dual_source_blending;\n{common}\n{storage}\n{}",
                include_str!("../src/shaders_subpixel.wgsl")
            ),
            naga::valid::Capabilities::DUAL_SOURCE_BLENDING,
        ),
    ] {
        let module = naga::front::wgsl::parse_str(&source).unwrap();
        naga::valid::Validator::new(naga::valid::ValidationFlags::all(), caps)
            .validate(&module)
            .unwrap();
    }
}
