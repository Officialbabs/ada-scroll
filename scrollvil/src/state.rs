use std::{
    collections::{HashMap, HashSet},
    ffi::OsString,
    os::unix::net::UnixStream,
    path::PathBuf,
    sync::Arc,
    time::{Duration, Instant},
};

use scroll_layout::{Config, Rect, Strip};
use smithay::{
    backend::{
        renderer::{damage::OutputDamageTracker, gles::GlesRenderer},
        winit::WinitGraphicsBackend,
    },
    desktop::{PopupManager, Space, Window, WindowSurfaceType},
    input::{Seat, SeatState},
    output::Output,
    reexports::{
        calloop::{
            generic::Generic,
            timer::{TimeoutAction, Timer},
            EventLoop, Interest, LoopHandle, LoopSignal, Mode, PostAction,
        },
        wayland_protocols::xdg::shell::server::xdg_toplevel,
        wayland_server::{
            backend::{ClientData, ClientId, DisconnectReason},
            protocol::wl_surface::WlSurface,
            Display, DisplayHandle,
        },
    },
    utils::{Logical, Point, SERIAL_COUNTER},
    wayland::{
        compositor::{CompositorClientState, CompositorState},
        dmabuf::DmabufState,
        output::OutputManagerState,
        selection::data_device::DataDeviceState,
        shell::xdg::XdgShellState,
        shm::ShmState,
        socket::ListeningSocketSource,
    },
};

use crate::{input::Modifier, CalloopData};

/// A tiled window on its way to its place: a critically damped spring.
pub struct Motion {
    pub pos: Point<f64, Logical>,
    pub vel: Point<f64, Logical>,
    pub target: Point<f64, Logical>,
}

impl Motion {
    fn at(target: Point<f64, Logical>) -> Self {
        Self { pos: target, vel: (0.0, 0.0).into(), target }
    }

    /// True while it still moves.
    fn step(&mut self, dt: f64) -> bool {
        // Settles in about a third of a second, without overshoot.
        const RESPONSE: f64 = 0.32;
        let omega = std::f64::consts::TAU / RESPONSE;
        let mut left = dt.min(0.1);
        while left > 0.0 {
            let h = left.min(0.004);
            let accel = (self.target - self.pos).upscale(omega * omega) - self.vel.upscale(2.0 * omega);
            self.vel += accel.upscale(h);
            self.pos += self.vel.upscale(h);
            left -= h;
        }
        let distance = self.target - self.pos;
        if distance.x.abs() < 0.5 && distance.y.abs() < 0.5 && self.vel.x.abs() < 5.0 && self.vel.y.abs() < 5.0 {
            self.pos = self.target;
            self.vel = (0.0, 0.0).into();
            return false;
        }
        true
    }
}

pub struct Scrollvil {
    pub start_time: Instant,
    pub socket_name: OsString,
    pub display_handle: DisplayHandle,

    pub space: Space<Window>,
    pub loop_signal: LoopSignal,
    pub loop_handle: LoopHandle<'static, CalloopData>,

    pub compositor_state: CompositorState,
    pub xdg_shell_state: XdgShellState,
    pub shm_state: ShmState,
    pub dmabuf_state: DmabufState,
    pub output_manager_state: OutputManagerState,
    pub seat_state: SeatState<Scrollvil>,
    pub data_device_state: DataDeviceState,
    pub popups: PopupManager,

    pub seat: Seat<Self>,

    pub backend: Option<WinitGraphicsBackend<GlesRenderer>>,
    pub output: Option<Output>,
    pub damage_tracker: Option<OutputDamageTracker>,
    /// Something changed since the last frame.
    pub dirty: bool,
    /// A frame is due on the frame clock.
    pub frame_due: bool,
    pub last_frame: Instant,

    /// Every toplevel, in the order they opened.
    pub windows: Vec<Window>,
    /// Opened but not committed yet: tiled or floating is decided then.
    pub unplaced: Vec<Window>,
    pub strip: Strip<Window>,
    pub floating: Vec<Window>,
    /// Floating windows to put in the middle once they have a size.
    pub unsized_floats: Vec<Window>,
    pub motion: HashMap<Window, Motion>,
    pub min_widths: HashMap<Window, i32>,
    pub focus: Option<Window>,
    pub last_tick: Instant,

    pub modifier: Modifier,
    /// Keys whose press was a shortcut: their release is kept from apps too.
    pub suppressed: HashSet<u32>,
    pub control_path: Option<PathBuf>,
    /// A screenshot to take after the next frame, and who asked for it.
    pub shot: Option<(PathBuf, UnixStream)>,
    pub frames: u64,
}

impl Scrollvil {
    pub fn new(event_loop: &mut EventLoop<'static, CalloopData>, display: Display<Self>, modifier: Modifier) -> Self {
        let dh = display.handle();

        let compositor_state = CompositorState::new::<Self>(&dh);
        let xdg_shell_state = XdgShellState::new::<Self>(&dh);
        let shm_state = ShmState::new::<Self>(&dh, vec![]);
        let dmabuf_state = DmabufState::new();
        let output_manager_state = OutputManagerState::new_with_xdg_output::<Self>(&dh);
        let mut seat_state = SeatState::new();
        let data_device_state = DataDeviceState::new::<Self>(&dh);

        let mut seat: Seat<Self> = seat_state.new_wl_seat(&dh, "winit");
        seat.add_keyboard(Default::default(), 200, 25).unwrap();
        seat.add_pointer();

        let socket_name = Self::init_wayland_listener(display, event_loop);

        Self {
            start_time: Instant::now(),
            socket_name,
            display_handle: dh,
            space: Space::default(),
            loop_signal: event_loop.get_signal(),
            loop_handle: event_loop.handle(),
            compositor_state,
            xdg_shell_state,
            shm_state,
            dmabuf_state,
            output_manager_state,
            seat_state,
            data_device_state,
            popups: PopupManager::default(),
            seat,
            backend: None,
            output: None,
            damage_tracker: None,
            dirty: false,
            frame_due: false,
            last_frame: Instant::now(),
            windows: Vec::new(),
            unplaced: Vec::new(),
            strip: Strip::new(Config::default(), Rect::new(0, 0, 1280, 800)),
            floating: Vec::new(),
            unsized_floats: Vec::new(),
            motion: HashMap::new(),
            min_widths: HashMap::new(),
            focus: None,
            last_tick: Instant::now(),
            modifier,
            suppressed: HashSet::new(),
            control_path: None,
            shot: None,
            frames: 0,
        }
    }

    fn init_wayland_listener(display: Display<Scrollvil>, event_loop: &mut EventLoop<CalloopData>) -> OsString {
        let listening_socket = ListeningSocketSource::new_auto().unwrap();
        let socket_name = listening_socket.socket_name().to_os_string();
        let loop_handle = event_loop.handle();

        loop_handle
            .insert_source(listening_socket, move |client_stream, _, state| {
                state
                    .display_handle
                    .insert_client(client_stream, Arc::new(ClientState::default()))
                    .unwrap();
            })
            .expect("Failed to init the wayland event source.");

        loop_handle
            .insert_source(Generic::new(display, Interest::READ, Mode::Level), |_, display, state| {
                // Safety: we don't drop the display
                unsafe {
                    display.get_mut().dispatch_clients(&mut state.state).unwrap();
                }
                Ok(PostAction::Continue)
            })
            .unwrap();

        socket_name
    }

    pub fn surface_under(&self, pos: Point<f64, Logical>) -> Option<(WlSurface, Point<f64, Logical>)> {
        self.space.element_under(pos).and_then(|(window, location)| {
            window
                .surface_under(pos - location.to_f64(), WindowSurfaceType::ALL)
                .map(|(s, p)| (s, (p + location).to_f64()))
        })
    }

    pub fn window_for(&self, surface: &WlSurface) -> Option<Window> {
        self.windows.iter().find(|w| w.toplevel().unwrap().wl_surface() == surface).cloned()
    }

    /// Draws a frame soon, at most sixty a second.
    pub fn request_redraw(&mut self) {
        self.dirty = true;
        if self.frame_due {
            return;
        }
        self.frame_due = true;
        let wait = Duration::from_millis(16).saturating_sub(self.last_frame.elapsed());
        let _ = self.loop_handle.insert_source(Timer::from_duration(wait), |_, _, data| {
            data.state.frame_due = false;
            if data.state.dirty {
                crate::winit::render_frame(&mut data.state, &mut data.display_handle, false);
            }
            TimeoutAction::Drop
        });
    }

    pub fn spawn(&self, command: &str) {
        let mut child = std::process::Command::new("sh");
        child.arg("-c").arg(command).env("WAYLAND_DISPLAY", &self.socket_name).env("GDK_BACKEND", "wayland");
        child.env("QT_QPA_PLATFORM", "wayland").env("MOZ_ENABLE_WAYLAND", "1").env_remove("DISPLAY");
        if let Some(path) = &self.control_path {
            child.env("SCROLLVIL_SOCKET", path);
        }
        if let Err(err) = child.spawn() {
            tracing::warn!("could not start {command}: {err}");
        }
    }

    /// Sizes the tiled windows and sets where they are going, after any
    /// change to the strip; then gives the focus to the strip's focused
    /// window.
    pub fn arrange(&mut self) {
        self.place_tiles();
        self.set_focus(self.strip.focused().cloned());
    }

    pub fn place_tiles(&mut self) {
        for p in self.strip.placements() {
            let toplevel = p.window.toplevel().unwrap();
            toplevel.with_pending_state(|state| {
                state.size = Some((p.rect.w, p.rect.h).into());
                state.states.set(xdg_toplevel::State::TiledLeft);
                state.states.set(xdg_toplevel::State::TiledRight);
                state.states.set(xdg_toplevel::State::TiledTop);
                state.states.set(xdg_toplevel::State::TiledBottom);
            });
            if toplevel.is_initial_configure_sent() {
                toplevel.send_pending_configure();
            }
            let target = Point::from((p.rect.x as f64, p.rect.y as f64));
            let motion = self.motion.entry(p.window.clone()).or_insert_with(|| Motion::at(target));
            motion.target = target;
            if p.shown {
                self.space.map_element(p.window.clone(), motion.pos.to_i32_round(), false);
            } else {
                self.space.unmap_elem(&p.window);
            }
        }
        for window in &self.floating {
            self.space.raise_element(window, false);
        }
        self.request_redraw();
    }

    pub fn set_focus(&mut self, window: Option<Window>) {
        self.focus = window.clone();
        for w in &self.windows {
            if w.set_activated(Some(w) == window.as_ref()) {
                w.toplevel().unwrap().send_pending_configure();
            }
        }
        if let Some(window) = &window {
            if self.floating.contains(window) {
                self.space.raise_element(window, false);
            }
        }
        let keyboard = self.seat.get_keyboard().unwrap();
        let surface = window.map(|w| w.toplevel().unwrap().wl_surface().clone());
        keyboard.set_focus(self, surface, SERIAL_COUNTER.next_serial());
        self.request_redraw();
    }

    /// Moves every tiled window a step towards its place. True while any
    /// still moves.
    pub fn tick(&mut self) -> bool {
        let now = Instant::now();
        let dt = (now - self.last_tick).as_secs_f64();
        self.last_tick = now;
        let (mut moving, mut moved) = (false, false);
        for (window, motion) in self.motion.iter_mut() {
            if motion.pos == motion.target {
                continue;
            }
            moving |= motion.step(dt);
            if self.space.element_location(window).is_some() {
                self.space.map_element(window.clone(), motion.pos.to_i32_round(), false);
                moved = true;
            }
        }
        if moved {
            // Moving a window raises it; floating windows stay on top.
            for window in &self.floating {
                self.space.raise_element(window, false);
            }
        }
        moving
    }

    /// The window is tiled now and floating no more, or the other way round.
    pub fn toggle_floating(&mut self, window: &Window) {
        if self.strip.remove(window) {
            self.motion.remove(window);
            self.floating.push(window.clone());
            let toplevel = window.toplevel().unwrap();
            toplevel.with_pending_state(|state| {
                state.states.unset(xdg_toplevel::State::TiledLeft);
                state.states.unset(xdg_toplevel::State::TiledRight);
                state.states.unset(xdg_toplevel::State::TiledTop);
                state.states.unset(xdg_toplevel::State::TiledBottom);
            });
            toplevel.send_pending_configure();
            let area = self.strip.area();
            let size = window.geometry().size;
            let at = (area.x + (area.w - size.w) / 2, area.y + (area.h - size.h) / 2);
            self.space.map_element(window.clone(), at, true);
            self.place_tiles();
            self.set_focus(Some(window.clone()));
        } else if let Some(i) = self.floating.iter().position(|w| w == window) {
            self.floating.remove(i);
            let start = self.space.element_location(window).map(|p| p.to_f64()).unwrap_or_default();
            self.strip.add(window.clone());
            self.motion.insert(window.clone(), Motion::at(start));
            self.arrange();
        }
    }

    /// The output's size changed.
    pub fn set_area(&mut self, w: i32, h: i32) {
        self.strip.set_area(Rect::new(0, 0, w, h));
        self.place_tiles();
    }

    pub fn is_tiled(&self, window: &Window) -> bool {
        self.strip.contains(window)
    }
}

#[derive(Default)]
pub struct ClientState {
    pub compositor_state: CompositorClientState,
}

impl ClientData for ClientState {
    fn initialized(&self, _client_id: ClientId) {}
    fn disconnected(&self, _client_id: ClientId, _reason: DisconnectReason) {}
}
