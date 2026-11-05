use smithay::{
    desktop::Window,
    input::pointer::{
        AxisFrame, ButtonEvent, GestureHoldBeginEvent, GestureHoldEndEvent, GesturePinchBeginEvent,
        GesturePinchEndEvent, GesturePinchUpdateEvent, GestureSwipeBeginEvent, GestureSwipeEndEvent,
        GestureSwipeUpdateEvent, GrabStartData as PointerGrabStartData, MotionEvent, PointerGrab, PointerInnerHandle,
        RelativeMotionEvent,
    },
    reexports::{wayland_protocols::xdg::shell::server::xdg_toplevel, wayland_server::protocol::wl_surface::WlSurface},
    utils::{Logical, Point, Size},
};

use crate::Scrollvil;

/// What a grab does as the pointer moves.
pub enum Drag {
    /// A floating window follows the pointer.
    Move { initial: Point<i32, Logical> },
    /// A tiled window's column, or a floating window, changes size.
    Resize { edges: xdg_toplevel::ResizeEdge, initial_loc: Point<i32, Logical>, initial_size: Size<i32, Logical> },
}

pub struct WindowGrab {
    pub start_data: PointerGrabStartData<Scrollvil>,
    pub window: Window,
    pub drag: Drag,
}

impl WindowGrab {
    fn drag_to(&mut self, data: &mut Scrollvil, location: Point<f64, Logical>) {
        let delta = (location - self.start_data.location).to_i32_round::<i32>();
        match &self.drag {
            Drag::Move { initial } => {
                data.space.map_element(self.window.clone(), *initial + delta, true);
            }
            Drag::Resize { edges, initial_loc, initial_size } => {
                use xdg_toplevel::ResizeEdge as E;
                let left = matches!(edges, E::Left | E::TopLeft | E::BottomLeft);
                let right = matches!(edges, E::Right | E::TopRight | E::BottomRight);
                let top = matches!(edges, E::Top | E::TopLeft | E::TopRight);
                let bottom = matches!(edges, E::Bottom | E::BottomLeft | E::BottomRight);
                let mut size = *initial_size;
                if left {
                    size.w -= delta.x;
                } else if right {
                    size.w += delta.x;
                }
                if data.is_tiled(&self.window) {
                    // Only the column's width is the person's to change.
                    data.strip.set_width(&self.window, size.w);
                    data.place_tiles();
                    return;
                }
                if top {
                    size.h -= delta.y;
                } else if bottom {
                    size.h += delta.y;
                }
                size.w = size.w.max(80);
                size.h = size.h.max(60);
                let mut loc = *initial_loc;
                if left {
                    loc.x = initial_loc.x + initial_size.w - size.w;
                }
                if top {
                    loc.y = initial_loc.y + initial_size.h - size.h;
                }
                let toplevel = self.window.toplevel().unwrap();
                toplevel.with_pending_state(|state| state.size = Some(size));
                toplevel.send_pending_configure();
                data.space.map_element(self.window.clone(), loc, true);
            }
        }
    }
}

impl PointerGrab<Scrollvil> for WindowGrab {
    fn motion(
        &mut self,
        data: &mut Scrollvil,
        handle: &mut PointerInnerHandle<'_, Scrollvil>,
        _focus: Option<(WlSurface, Point<f64, Logical>)>,
        event: &MotionEvent,
    ) {
        // No client has the pointer while it drags.
        handle.motion(data, None, event);
        self.drag_to(data, event.location);
        data.request_redraw();
    }

    fn relative_motion(
        &mut self,
        data: &mut Scrollvil,
        handle: &mut PointerInnerHandle<'_, Scrollvil>,
        focus: Option<(WlSurface, Point<f64, Logical>)>,
        event: &RelativeMotionEvent,
    ) {
        handle.relative_motion(data, focus, event);
    }

    fn button(&mut self, data: &mut Scrollvil, handle: &mut PointerInnerHandle<'_, Scrollvil>, event: &ButtonEvent) {
        handle.button(data, event);
        if !handle.current_pressed().contains(&self.start_data.button) {
            if let Drag::Resize { .. } = self.drag {
                let toplevel = self.window.toplevel().unwrap();
                toplevel.with_pending_state(|state| {
                    state.states.unset(xdg_toplevel::State::Resizing);
                });
                toplevel.send_pending_configure();
            }
            handle.unset_grab(self, data, event.serial, event.time, true);
        }
    }

    fn axis(&mut self, data: &mut Scrollvil, handle: &mut PointerInnerHandle<'_, Scrollvil>, details: AxisFrame) {
        handle.axis(data, details)
    }

    fn frame(&mut self, data: &mut Scrollvil, handle: &mut PointerInnerHandle<'_, Scrollvil>) {
        handle.frame(data);
    }

    fn gesture_swipe_begin(
        &mut self,
        data: &mut Scrollvil,
        handle: &mut PointerInnerHandle<'_, Scrollvil>,
        event: &GestureSwipeBeginEvent,
    ) {
        handle.gesture_swipe_begin(data, event)
    }

    fn gesture_swipe_update(
        &mut self,
        data: &mut Scrollvil,
        handle: &mut PointerInnerHandle<'_, Scrollvil>,
        event: &GestureSwipeUpdateEvent,
    ) {
        handle.gesture_swipe_update(data, event)
    }

    fn gesture_swipe_end(
        &mut self,
        data: &mut Scrollvil,
        handle: &mut PointerInnerHandle<'_, Scrollvil>,
        event: &GestureSwipeEndEvent,
    ) {
        handle.gesture_swipe_end(data, event)
    }

    fn gesture_pinch_begin(
        &mut self,
        data: &mut Scrollvil,
        handle: &mut PointerInnerHandle<'_, Scrollvil>,
        event: &GesturePinchBeginEvent,
    ) {
        handle.gesture_pinch_begin(data, event)
    }

    fn gesture_pinch_update(
        &mut self,
        data: &mut Scrollvil,
        handle: &mut PointerInnerHandle<'_, Scrollvil>,
        event: &GesturePinchUpdateEvent,
    ) {
        handle.gesture_pinch_update(data, event)
    }

    fn gesture_pinch_end(
        &mut self,
        data: &mut Scrollvil,
        handle: &mut PointerInnerHandle<'_, Scrollvil>,
        event: &GesturePinchEndEvent,
    ) {
        handle.gesture_pinch_end(data, event)
    }

    fn gesture_hold_begin(
        &mut self,
        data: &mut Scrollvil,
        handle: &mut PointerInnerHandle<'_, Scrollvil>,
        event: &GestureHoldBeginEvent,
    ) {
        handle.gesture_hold_begin(data, event)
    }

    fn gesture_hold_end(
        &mut self,
        data: &mut Scrollvil,
        handle: &mut PointerInnerHandle<'_, Scrollvil>,
        event: &GestureHoldEndEvent,
    ) {
        handle.gesture_hold_end(data, event)
    }

    fn start_data(&self) -> &PointerGrabStartData<Scrollvil> {
        &self.start_data
    }

    fn unset(&mut self, _data: &mut Scrollvil) {}
}
