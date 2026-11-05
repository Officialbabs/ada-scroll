use std::{io::Write, path::Path, time::Duration};

use smithay::{
    backend::{
        allocator::Fourcc,
        egl::context::GlAttributes,
        renderer::{
            damage::OutputDamageTracker,
            element::surface::WaylandSurfaceRenderElement,
            gles::{GlesRenderbuffer, GlesRenderer},
            Bind, ExportMem, ImportDma, Offscreen,
        },
        winit::{self, WinitEvent},
    },
    desktop::{Space, Window},
    output::{Mode, Output, PhysicalProperties, Scale, Subpixel},
    reexports::{
        calloop::EventLoop,
        wayland_server::DisplayHandle,
        winit::{dpi::LogicalSize, window::Window as WinitWindow},
    },
    utils::{Buffer, Rectangle, Size, Transform},
};

use crate::{CalloopData, Scrollvil};

pub fn init_winit(
    event_loop: &mut EventLoop<CalloopData>,
    data: &mut CalloopData,
) -> Result<(), Box<dyn std::error::Error>> {
    let display_handle = &mut data.display_handle;
    let state = &mut data.state;

    // No vsync: the frame clock is our own (Scrollvil::request_redraw), so
    // that apps keep drawing while the desktop isn't showing this window and
    // sends it no frame callbacks.
    let (mut backend, winit) = winit::init_from_attributes_with_gl_attr::<GlesRenderer>(
        WinitWindow::default_attributes()
            .with_inner_size(LogicalSize::new(1600.0, 900.0))
            .with_title("Scrollvil")
            .with_visible(true),
        GlAttributes { version: (3, 0), profile: None, debug: false, vsync: false },
    )?;

    let formats = backend.renderer().dmabuf_formats();
    state.dmabuf_state.create_global::<Scrollvil>(display_handle, formats);

    let mode = Mode { size: backend.window_size(), refresh: 60_000 };
    let output = Output::new(
        "winit".to_string(),
        PhysicalProperties { size: (0, 0).into(), subpixel: Subpixel::Unknown, make: "Smithay".into(), model: "Winit".into() },
    );
    let _global = output.create_global::<Scrollvil>(display_handle);
    // The desktop's own scale, so apps here are the size they are there.
    let scale = backend.window().scale_factor();
    output.change_current_state(
        Some(mode),
        Some(Transform::Flipped180),
        Some(Scale::Fractional(scale)),
        Some((0, 0).into()),
    );
    output.set_preferred(mode);
    state.space.map_output(&output, (0, 0));
    state.damage_tracker = Some(OutputDamageTracker::from_output(&output));
    state.output = Some(output);
    state.backend = Some(backend);
    let area = mode.size.to_f64().to_logical(scale).to_i32_round();
    state.set_area(area.w, area.h);

    std::env::set_var("WAYLAND_DISPLAY", &state.socket_name);

    event_loop.handle().insert_source(winit, move |event, _, data| {
        let state = &mut data.state;
        match event {
            WinitEvent::Resized { size, scale_factor } => {
                let output = state.output.clone().unwrap();
                let mode = Mode { size, refresh: 60_000 };
                output.change_current_state(Some(mode), None, Some(Scale::Fractional(scale_factor)), None);
                state.damage_tracker = Some(OutputDamageTracker::from_output(&output));
                let area = size.to_f64().to_logical(scale_factor).to_i32_round();
                state.set_area(area.w, area.h);
            }
            WinitEvent::Input(event) => state.process_input_event(event),
            WinitEvent::Redraw => render_frame(state, &mut data.display_handle, true),
            WinitEvent::CloseRequested => state.loop_signal.stop(),
            _ => (),
        };
    })?;

    Ok(())
}

/// One beat of the frame clock: windows move a step and apps get their frame
/// callbacks. The frame goes on the screen only when the desktop asks for one
/// (`present`, from winit's redraw); a desktop that isn't showing this window
/// stops releasing its buffers, and drawing into them would block.
pub fn render_frame(state: &mut Scrollvil, display: &mut DisplayHandle, present: bool) {
    let Some(output) = state.output.clone() else {
        return;
    };
    if state.backend.is_none() || state.damage_tracker.is_none() {
        return;
    }
    state.dirty = false;
    state.last_frame = std::time::Instant::now();
    let moving = state.tick();

    if let Some((path, mut asker)) = state.shot.take() {
        let backend = state.backend.as_mut().unwrap();
        let size = backend.window_size();
        let saved = draw_offscreen(backend.renderer(), &output, &state.space, (size.w, size.h).into())
            .and_then(|pixels| save_png(&path, size.w as u32, size.h as u32, &pixels));
        let reply = match saved {
            Ok(()) => format!("ok {}\n", path.display()),
            Err(err) => format!("error {err}\n"),
        };
        let _ = asker.write_all(reply.as_bytes());
    }

    let backend = state.backend.as_mut().unwrap();
    if present {
        state.frames += 1;
        let size = backend.window_size();
        let damage_tracker = state.damage_tracker.as_mut().unwrap();
        match backend.bind() {
            Ok((renderer, mut framebuffer)) => {
                let rendered =
                    smithay::desktop::space::render_output::<_, WaylandSurfaceRenderElement<GlesRenderer>, _, _>(
                        &output,
                        renderer,
                        &mut framebuffer,
                        1.0,
                        0,
                        [&state.space],
                        &[],
                        damage_tracker,
                        BACKGROUND,
                    );
                if let Err(err) = rendered {
                    tracing::warn!("could not draw: {err:?}");
                }
            }
            Err(err) => tracing::warn!("could not bind the window: {err}"),
        }
        if let Err(err) = backend.submit(Some(&[Rectangle::from_size(size)])) {
            tracing::warn!("could not show the frame: {err}");
        }
    } else {
        // Shown when the desktop next wants a frame from this window.
        backend.window().request_redraw();
    }

    state.space.elements().for_each(|window| {
        window.send_frame(&output, state.start_time.elapsed(), Some(Duration::ZERO), |_, _| Some(output.clone()))
    });
    state.space.refresh();
    state.popups.cleanup();
    let _ = display.flush_clients();

    if moving {
        state.request_redraw();
    }
}

const BACKGROUND: [f32; 4] = [0.08, 0.09, 0.11, 1.0];

/// The space drawn into a buffer of its own, for a screenshot: it doesn't
/// need the desktop to be showing this window.
fn draw_offscreen(
    renderer: &mut GlesRenderer,
    output: &Output,
    space: &Space<Window>,
    size: Size<i32, Buffer>,
) -> Result<Vec<u8>, String> {
    let mut buffer: GlesRenderbuffer = renderer.create_buffer(Fourcc::Abgr8888, size).map_err(|e| e.to_string())?;
    let mut target = renderer.bind(&mut buffer).map_err(|e| e.to_string())?;
    let mut damage_tracker = OutputDamageTracker::from_output(output);
    smithay::desktop::space::render_output::<_, WaylandSurfaceRenderElement<GlesRenderer>, _, _>(
        output,
        renderer,
        &mut target,
        1.0,
        0,
        [space],
        &[],
        &mut damage_tracker,
        BACKGROUND,
    )
    .map_err(|e| format!("{e:?}"))?;
    let mapping = renderer
        .copy_framebuffer(&target, Rectangle::from_size(size), Fourcc::Abgr8888)
        .map_err(|e| e.to_string())?;
    drop(target);
    renderer.map_texture(&mapping).map(<[u8]>::to_vec).map_err(|e| e.to_string())
}

/// The framebuffer's rows run bottom to top.
fn save_png(path: &Path, width: u32, height: u32, pixels: &[u8]) -> Result<(), String> {
    let row = width as usize * 4;
    let mut flipped = Vec::with_capacity(pixels.len());
    for line in pixels.chunks_exact(row).rev() {
        flipped.extend_from_slice(line);
    }
    for alpha in flipped.iter_mut().skip(3).step_by(4) {
        *alpha = 255;
    }
    let file = std::fs::File::create(path).map_err(|e| e.to_string())?;
    let mut encoder = png::Encoder::new(std::io::BufWriter::new(file), width, height);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    let mut writer = encoder.write_header().map_err(|e| e.to_string())?;
    writer.write_image_data(&flipped).map_err(|e| e.to_string())
}
