use smithay::{
    backend::renderer::{utils::on_commit_buffer_handler, ImportDma},
    delegate_compositor, delegate_data_device, delegate_dmabuf, delegate_output, delegate_seat, delegate_shm,
    delegate_xdg_shell,
    desktop::{find_popup_root_surface, get_popup_toplevel_coords, PopupKind, Window},
    input::{
        pointer::{CursorImageStatus, Focus, GrabStartData as PointerGrabStartData},
        Seat, SeatHandler, SeatState,
    },
    reexports::{
        wayland_protocols::xdg::shell::server::xdg_toplevel,
        wayland_server::{
            protocol::{wl_buffer, wl_seat, wl_surface::WlSurface},
            Client, Resource,
        },
    },
    utils::Serial,
    wayland::{
        buffer::BufferHandler,
        compositor::{
            get_parent, is_sync_subsurface, with_states, CompositorClientState, CompositorHandler, CompositorState,
        },
        dmabuf::{DmabufGlobal, DmabufHandler, DmabufState, ImportNotifier},
        output::OutputHandler,
        selection::{
            data_device::{
                set_data_device_focus, ClientDndGrabHandler, DataDeviceHandler, DataDeviceState, ServerDndGrabHandler,
            },
            SelectionHandler,
        },
        shell::xdg::{
            PopupSurface, PositionerState, SurfaceCachedState, ToplevelSurface, XdgShellHandler, XdgShellState,
        },
        shm::{ShmHandler, ShmState},
    },
    backend::allocator::dmabuf::Dmabuf,
};

use crate::{
    grabs::{Drag, WindowGrab},
    state::ClientState,
    Scrollvil,
};

impl CompositorHandler for Scrollvil {
    fn compositor_state(&mut self) -> &mut CompositorState {
        &mut self.compositor_state
    }

    fn client_compositor_state<'a>(&self, client: &'a Client) -> &'a CompositorClientState {
        &client.get_data::<ClientState>().unwrap().compositor_state
    }

    fn commit(&mut self, surface: &WlSurface) {
        on_commit_buffer_handler::<Self>(surface);
        if !is_sync_subsurface(surface) {
            let mut root = surface.clone();
            while let Some(parent) = get_parent(&root) {
                root = parent;
            }
            if let Some(window) = self.window_for(&root) {
                window.on_commit();
                if &root == surface {
                    self.toplevel_committed(&window);
                }
            }
        }
        self.popup_committed(surface);
        self.request_redraw();
    }
}

impl Scrollvil {
    fn toplevel_committed(&mut self, window: &Window) {
        let toplevel = window.toplevel().unwrap();
        let (min, max) = with_states(toplevel.wl_surface(), |states| {
            let mut cached = states.cached_state.get::<SurfaceCachedState>();
            let current = cached.current();
            (current.min_size, current.max_size)
        });

        if let Some(i) = self.unplaced.iter().position(|w| w == window) {
            // Its first commit: dialogs and fixed-size windows float; the
            // rest join the strip.
            self.unplaced.remove(i);
            let fixed = min.w > 0 && min == max;
            if toplevel.parent().is_some() || fixed {
                self.floating.push(window.clone());
                self.unsized_floats.push(window.clone());
                toplevel.send_configure();
            } else {
                self.strip.add(window.clone());
                if min.w > 0 {
                    self.strip.set_min_width(window, min.w);
                    self.min_widths.insert(window.clone(), min.w);
                }
                self.place_tiles();
                toplevel.send_configure();
                self.set_focus(Some(window.clone()));
            }
            return;
        }

        if self.strip.contains(window) && min.w > 0 && min == max {
            // A fixed size that GTK only sets after its first frame: it
            // floats after all.
            self.toggle_floating(window);
            return;
        }

        if self.strip.contains(window) && self.min_widths.get(window).copied().unwrap_or(0) != min.w {
            self.min_widths.insert(window.clone(), min.w);
            self.strip.set_min_width(window, min.w);
            self.place_tiles();
        }

        if let Some(i) = self.unsized_floats.iter().position(|w| w == window) {
            let size = window.geometry().size;
            if size.w > 0 && size.h > 0 {
                self.unsized_floats.remove(i);
                let area = self.strip.area();
                let at = (area.x + (area.w - size.w) / 2, area.y + (area.h - size.h) / 2);
                self.space.map_element(window.clone(), at, true);
                self.set_focus(Some(window.clone()));
            }
        }
    }

    /// A tiled window's maximize is its column taking the screen's width.
    fn set_full_width(&mut self, surface: &ToplevelSurface, full: bool) {
        if let Some(window) = self.window_for(surface.wl_surface()) {
            if self.strip.focus(&window) {
                if (self.strip.width_of(&window) == Some(self.strip.view_width())) != full {
                    self.strip.apply(scroll_layout::Action::FullWidth);
                }
                self.arrange();
                return;
            }
        }
        surface.send_configure();
    }

    fn popup_committed(&mut self, surface: &WlSurface) {
        self.popups.commit(surface);
        if let Some(PopupKind::Xdg(ref xdg)) = self.popups.find_popup(surface) {
            if !xdg.is_initial_configure_sent() {
                // The initial configure is always allowed.
                xdg.send_configure().expect("initial configure failed");
            }
        }
    }

    fn unconstrain_popup(&self, popup: &PopupSurface) {
        let Ok(root) = find_popup_root_surface(&PopupKind::Xdg(popup.clone())) else {
            return;
        };
        let Some(window) = self.window_for(&root) else {
            return;
        };
        let Some(window_geo) = self.space.element_geometry(&window) else {
            return;
        };
        let output = self.space.outputs().next().unwrap();
        let output_geo = self.space.output_geometry(output).unwrap();

        // Relative to the parent's geometry.
        let mut target = output_geo;
        target.loc -= get_popup_toplevel_coords(&PopupKind::Xdg(popup.clone()));
        target.loc -= window_geo.loc;

        popup.with_pending_state(|state| {
            state.geometry = state.positioner.get_unconstrained_geometry(target);
        });
    }
}

impl BufferHandler for Scrollvil {
    fn buffer_destroyed(&mut self, _buffer: &wl_buffer::WlBuffer) {}
}

impl ShmHandler for Scrollvil {
    fn shm_state(&self) -> &ShmState {
        &self.shm_state
    }
}

impl DmabufHandler for Scrollvil {
    fn dmabuf_state(&mut self) -> &mut DmabufState {
        &mut self.dmabuf_state
    }

    fn dmabuf_imported(&mut self, _global: &DmabufGlobal, dmabuf: Dmabuf, notifier: ImportNotifier) {
        let imported = self.backend.as_mut().is_some_and(|b| b.renderer().import_dmabuf(&dmabuf, None).is_ok());
        if imported {
            let _ = notifier.successful::<Scrollvil>();
        } else {
            notifier.failed();
        }
    }
}

delegate_compositor!(Scrollvil);
delegate_shm!(Scrollvil);
delegate_dmabuf!(Scrollvil);

impl XdgShellHandler for Scrollvil {
    fn xdg_shell_state(&mut self) -> &mut XdgShellState {
        &mut self.xdg_shell_state
    }

    fn new_toplevel(&mut self, surface: ToplevelSurface) {
        let window = Window::new_wayland_window(surface);
        self.windows.push(window.clone());
        self.unplaced.push(window);
    }

    fn toplevel_destroyed(&mut self, surface: ToplevelSurface) {
        let Some(window) = self.window_for(surface.wl_surface()) else {
            return;
        };
        self.windows.retain(|w| w != &window);
        self.unplaced.retain(|w| w != &window);
        self.floating.retain(|w| w != &window);
        self.unsized_floats.retain(|w| w != &window);
        self.motion.remove(&window);
        self.min_widths.remove(&window);
        self.space.unmap_elem(&window);
        let was_focused = self.focus.as_ref() == Some(&window);
        self.strip.remove(&window);
        if was_focused || self.focus.is_none() {
            self.arrange();
        } else {
            self.place_tiles();
        }
    }

    fn new_popup(&mut self, surface: PopupSurface, _positioner: PositionerState) {
        self.unconstrain_popup(&surface);
        let _ = self.popups.track_popup(PopupKind::Xdg(surface));
    }

    fn reposition_request(&mut self, surface: PopupSurface, positioner: PositionerState, token: u32) {
        surface.with_pending_state(|state| {
            state.geometry = positioner.get_geometry();
            state.positioner = positioner;
        });
        self.unconstrain_popup(&surface);
        surface.send_repositioned(token);
    }

    fn move_request(&mut self, surface: ToplevelSurface, seat: wl_seat::WlSeat, serial: Serial) {
        let seat = Seat::from_resource(&seat).unwrap();
        let Some(start_data) = check_grab(&seat, surface.wl_surface(), serial) else {
            return;
        };
        let Some(window) = self.window_for(surface.wl_surface()) else {
            return;
        };
        // Dragging a tiled window by its title takes it off the strip.
        if self.is_tiled(&window) {
            self.toggle_floating(&window);
            let pointer = start_data.location.to_i32_round::<i32>();
            let size = window.geometry().size;
            self.space.map_element(window.clone(), (pointer.x - size.w / 2, pointer.y - 20), true);
        }
        let initial = self.space.element_location(&window).unwrap_or_default();
        let grab = WindowGrab { start_data, window, drag: Drag::Move { initial } };
        seat.get_pointer().unwrap().set_grab(self, grab, serial, Focus::Clear);
    }

    fn resize_request(
        &mut self,
        surface: ToplevelSurface,
        seat: wl_seat::WlSeat,
        serial: Serial,
        edges: xdg_toplevel::ResizeEdge,
    ) {
        let seat = Seat::from_resource(&seat).unwrap();
        let Some(start_data) = check_grab(&seat, surface.wl_surface(), serial) else {
            return;
        };
        let Some(window) = self.window_for(surface.wl_surface()) else {
            return;
        };
        let initial_loc = self.space.element_location(&window).unwrap_or_default();
        let mut initial_size = window.geometry().size;
        if let Some(width) = self.strip.width_of(&window) {
            initial_size.w = width;
        }
        surface.with_pending_state(|state| {
            state.states.set(xdg_toplevel::State::Resizing);
        });
        surface.send_pending_configure();
        let grab = WindowGrab { start_data, window, drag: Drag::Resize { edges, initial_loc, initial_size } };
        seat.get_pointer().unwrap().set_grab(self, grab, serial, Focus::Clear);
    }

    fn maximize_request(&mut self, surface: ToplevelSurface) {
        self.set_full_width(&surface, true);
    }

    fn unmaximize_request(&mut self, surface: ToplevelSurface) {
        self.set_full_width(&surface, false);
    }

    fn grab(&mut self, _surface: PopupSurface, _seat: wl_seat::WlSeat, _serial: Serial) {}
}

delegate_xdg_shell!(Scrollvil);

fn check_grab(seat: &Seat<Scrollvil>, surface: &WlSurface, serial: Serial) -> Option<PointerGrabStartData<Scrollvil>> {
    let pointer = seat.get_pointer()?;
    if !pointer.has_grab(serial) {
        return None;
    }
    let start_data = pointer.grab_start_data()?;
    let (focus, _) = start_data.focus.as_ref()?;
    if !focus.id().same_client_as(&surface.id()) {
        return None;
    }
    Some(start_data)
}

impl SeatHandler for Scrollvil {
    type KeyboardFocus = WlSurface;
    type PointerFocus = WlSurface;
    type TouchFocus = WlSurface;

    fn seat_state(&mut self) -> &mut SeatState<Scrollvil> {
        &mut self.seat_state
    }

    fn cursor_image(&mut self, _seat: &Seat<Self>, _image: CursorImageStatus) {}

    fn focus_changed(&mut self, seat: &Seat<Self>, focused: Option<&WlSurface>) {
        let dh = &self.display_handle;
        let client = focused.and_then(|s| dh.get_client(s.id()).ok());
        set_data_device_focus(dh, seat, client);
    }
}

delegate_seat!(Scrollvil);

impl SelectionHandler for Scrollvil {
    type SelectionUserData = ();
}

impl DataDeviceHandler for Scrollvil {
    fn data_device_state(&self) -> &DataDeviceState {
        &self.data_device_state
    }
}

impl ClientDndGrabHandler for Scrollvil {}
impl ServerDndGrabHandler for Scrollvil {}

delegate_data_device!(Scrollvil);

impl OutputHandler for Scrollvil {}
delegate_output!(Scrollvil);
