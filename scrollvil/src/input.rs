use scroll_layout::Action;
use smithay::{
    backend::input::{
        AbsolutePositionEvent, Axis, AxisSource, ButtonState, Event, InputBackend, InputEvent, KeyState,
        KeyboardKeyEvent, PointerAxisEvent, PointerButtonEvent,
    },
    desktop::Window,
    input::{
        keyboard::{keysyms as k, FilterResult, ModifiersState},
        pointer::{AxisFrame, ButtonEvent, Focus, GrabStartData, MotionEvent},
    },
    reexports::wayland_protocols::xdg::shell::server::xdg_toplevel,
    utils::{Logical, Point, Serial, SERIAL_COUNTER},
};

use crate::{
    grabs::{Drag, WindowGrab},
    state::Scrollvil,
};

/// The key held for every shortcut. Nested in another desktop, that desktop
/// may keep Super's shortcuts for itself; Alt or Ctrl+Alt then reach here.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Modifier {
    Super,
    Alt,
    CtrlAlt,
}

impl Modifier {
    fn held(self, mods: &ModifiersState) -> bool {
        match self {
            Modifier::Super => mods.logo,
            Modifier::Alt => mods.alt && !mods.logo,
            Modifier::CtrlAlt => mods.ctrl && mods.alt,
        }
    }

    /// Whether Ctrl is free to mean something more, as in Mod+Ctrl+Left.
    fn ctrl_free(self) -> bool {
        self != Modifier::CtrlAlt
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Command {
    Layout(Action),
    ToggleFloating,
    Close,
    /// The app picker.
    Launch,
    Quit,
}

/// The shortcut for a key pressed with these modifiers, if it is one.
pub fn shortcut(modifier: Modifier, mods: &ModifiersState, sym: u32) -> Option<Command> {
    if mods.ctrl && mods.alt && sym == k::KEY_BackSpace {
        return Some(Command::Quit);
    }
    if !modifier.held(mods) {
        return None;
    }
    let shift = mods.shift;
    let ctrl = mods.ctrl && modifier.ctrl_free();
    let action = match sym {
        k::KEY_Left if ctrl => Action::MoveColumnLeft,
        k::KEY_Right if ctrl => Action::MoveColumnRight,
        k::KEY_Home if ctrl => Action::MoveColumnFirst,
        k::KEY_End if ctrl => Action::MoveColumnLast,
        k::KEY_Left if shift => Action::MoveLeft,
        k::KEY_Right if shift => Action::MoveRight,
        k::KEY_Up if shift => Action::MoveUp,
        k::KEY_Down if shift => Action::MoveDown,
        k::KEY_Left => Action::FocusLeft,
        k::KEY_Right => Action::FocusRight,
        k::KEY_Up => Action::FocusUp,
        k::KEY_Down => Action::FocusDown,
        k::KEY_Home => Action::FocusFirst,
        k::KEY_End => Action::FocusLast,
        k::KEY_r if shift => Action::PreviousWidth,
        k::KEY_r => Action::NextWidth,
        k::KEY_f => Action::FullWidth,
        k::KEY_minus => Action::Narrower,
        k::KEY_equal => Action::Wider,
        k::KEY_e => Action::EqualizeVisible,
        k::KEY_x => Action::ToggleStacked,
        k::KEY_c => Action::CenterFocused,
        k::KEY_Page_Up => Action::ScrollLeft,
        k::KEY_Page_Down => Action::ScrollRight,
        k::KEY_1..=k::KEY_9 => Action::FocusColumn((sym - k::KEY_1) as usize),
        k::KEY_t => return Some(Command::ToggleFloating),
        k::KEY_q => return Some(Command::Close),
        k::KEY_Return => return Some(Command::Launch),
        k::KEY_Escape if shift => return Some(Command::Quit),
        _ => return None,
    };
    Some(Command::Layout(action))
}

impl Scrollvil {
    pub fn run(&mut self, command: Command) {
        match command {
            Command::Layout(action) => {
                self.strip.apply(action);
                self.arrange();
            }
            Command::ToggleFloating => {
                if let Some(window) = self.focus.clone() {
                    self.toggle_floating(&window);
                }
            }
            Command::Close => {
                if let Some(window) = &self.focus {
                    window.toplevel().unwrap().send_close();
                }
            }
            Command::Launch => match crate::launcher() {
                Some(picker) => self.spawn(&picker),
                None => tracing::warn!("no app picker: set SCROLLVIL_LAUNCHER or install scrollvil-launch"),
            },
            Command::Quit => self.loop_signal.stop(),
        }
    }

    fn drag_window(&mut self, window: Window, button: u32, location: Point<f64, Logical>, serial: Serial) {
        const BTN_LEFT: u32 = 0x110;
        const BTN_RIGHT: u32 = 0x111;
        let start_data = GrabStartData { focus: None, button, location };
        let drag = match button {
            BTN_LEFT => {
                if self.is_tiled(&window) {
                    self.toggle_floating(&window);
                }
                Drag::Move { initial: self.space.element_location(&window).unwrap_or_default() }
            }
            BTN_RIGHT => {
                let mut initial_size = window.geometry().size;
                if let Some(width) = self.strip.width_of(&window) {
                    initial_size.w = width;
                }
                Drag::Resize {
                    edges: xdg_toplevel::ResizeEdge::BottomRight,
                    initial_loc: self.space.element_location(&window).unwrap_or_default(),
                    initial_size,
                }
            }
            _ => return,
        };
        self.set_focus(Some(window.clone()));
        let grab = WindowGrab { start_data, window, drag };
        let pointer = self.seat.get_pointer().unwrap();
        pointer.set_grab(self, grab, serial, Focus::Clear);
    }

    pub fn process_input_event<I: InputBackend>(&mut self, event: InputEvent<I>) {
        match event {
            InputEvent::Keyboard { event, .. } => {
                let serial = SERIAL_COUNTER.next_serial();
                let time = Event::time_msec(&event);
                let state = event.state();
                let command = self.seat.get_keyboard().unwrap().input::<Option<Command>, _>(
                    self,
                    event.key_code(),
                    state,
                    serial,
                    time,
                    |data, mods, handle| {
                        let code = handle.raw_code().raw();
                        if state == KeyState::Released {
                            return if data.suppressed.remove(&code) {
                                FilterResult::Intercept(None)
                            } else {
                                FilterResult::Forward
                            };
                        }
                        let sym = handle.raw_latin_sym_or_raw_current_sym().map(|s| s.raw()).unwrap_or(0);
                        match shortcut(data.modifier, mods, sym) {
                            Some(command) => {
                                data.suppressed.insert(code);
                                FilterResult::Intercept(Some(command))
                            }
                            None => FilterResult::Forward,
                        }
                    },
                );
                if let Some(Some(command)) = command {
                    self.run(command);
                }
            }
            InputEvent::PointerMotion { .. } => {}
            InputEvent::PointerMotionAbsolute { event, .. } => {
                let output = self.space.outputs().next().unwrap();
                let output_geo = self.space.output_geometry(output).unwrap();
                let pos = event.position_transformed(output_geo.size) + output_geo.loc.to_f64();
                let serial = SERIAL_COUNTER.next_serial();
                let pointer = self.seat.get_pointer().unwrap();
                let under = self.surface_under(pos);
                pointer.motion(self, under, &MotionEvent { location: pos, serial, time: event.time_msec() });
                pointer.frame(self);
            }
            InputEvent::PointerButton { event, .. } => {
                let pointer = self.seat.get_pointer().unwrap();
                let serial = SERIAL_COUNTER.next_serial();
                let mods = self.seat.get_keyboard().unwrap().modifier_state();
                if event.state() == ButtonState::Pressed && !pointer.is_grabbed() && self.modifier.held(&mods) {
                    // The modifier and a drag: left moves (and floats) the
                    // window, right changes its width, whatever the app's own
                    // edges allow.
                    let location = pointer.current_location();
                    if let Some(window) = self.space.element_under(location).map(|(w, _)| w.clone()) {
                        self.drag_window(window, event.button_code(), location, serial);
                        return;
                    }
                }
                if event.state() == ButtonState::Pressed && !pointer.is_grabbed() {
                    let clicked = self.space.element_under(pointer.current_location()).map(|(w, _)| w.clone());
                    if let Some(window) = clicked {
                        if self.strip.focus(&window) {
                            // A click on a window at the screen's edge brings
                            // it in.
                            self.arrange();
                        } else {
                            self.set_focus(Some(window));
                        }
                    }
                }
                pointer.button(
                    self,
                    &ButtonEvent { button: event.button_code(), state: event.state(), serial, time: event.time_msec() },
                );
                pointer.frame(self);
            }
            InputEvent::PointerAxis { event, .. } => {
                let source = event.source();
                let amount = |axis| {
                    event.amount(axis).unwrap_or_else(|| event.amount_v120(axis).unwrap_or(0.0) * 15.0 / 120.)
                };
                let (horizontal, vertical) = (amount(Axis::Horizontal), amount(Axis::Vertical));

                // With the modifier held, the wheel or a two-finger swipe
                // scrolls the strip.
                let mods = self.seat.get_keyboard().unwrap().modifier_state();
                if self.modifier.held(&mods) {
                    let speed = if source == AxisSource::Wheel { 6.0 } else { 1.5 };
                    self.strip.scroll_by((horizontal + vertical) * speed);
                    self.place_tiles();
                    return;
                }

                let mut frame = AxisFrame::new(event.time_msec()).source(source);
                if horizontal != 0.0 {
                    frame = frame.value(Axis::Horizontal, horizontal);
                    if let Some(discrete) = event.amount_v120(Axis::Horizontal) {
                        frame = frame.v120(Axis::Horizontal, discrete as i32);
                    }
                }
                if vertical != 0.0 {
                    frame = frame.value(Axis::Vertical, vertical);
                    if let Some(discrete) = event.amount_v120(Axis::Vertical) {
                        frame = frame.v120(Axis::Vertical, discrete as i32);
                    }
                }
                if source == AxisSource::Finger {
                    if event.amount(Axis::Horizontal) == Some(0.0) {
                        frame = frame.stop(Axis::Horizontal);
                    }
                    if event.amount(Axis::Vertical) == Some(0.0) {
                        frame = frame.stop(Axis::Vertical);
                    }
                }
                let pointer = self.seat.get_pointer().unwrap();
                pointer.axis(self, frame);
                pointer.frame(self);
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mods(logo: bool, alt: bool, ctrl: bool, shift: bool) -> ModifiersState {
        ModifiersState { logo, alt, ctrl, shift, ..Default::default() }
    }

    #[test]
    fn super_arrows_focus_and_move() {
        let m = Modifier::Super;
        let plain = mods(true, false, false, false);
        assert_eq!(shortcut(m, &plain, k::KEY_Left), Some(Command::Layout(Action::FocusLeft)));
        let shift = mods(true, false, false, true);
        assert_eq!(shortcut(m, &shift, k::KEY_Right), Some(Command::Layout(Action::MoveRight)));
        let ctrl = mods(true, false, true, false);
        assert_eq!(shortcut(m, &ctrl, k::KEY_Left), Some(Command::Layout(Action::MoveColumnLeft)));
        assert_eq!(shortcut(m, &plain, k::KEY_3), Some(Command::Layout(Action::FocusColumn(2))));
        assert_eq!(shortcut(m, &plain, k::KEY_t), Some(Command::ToggleFloating));
    }

    #[test]
    fn keys_without_the_modifier_go_to_the_app() {
        assert_eq!(shortcut(Modifier::Super, &mods(false, false, false, false), k::KEY_Left), None);
        assert_eq!(shortcut(Modifier::Super, &mods(false, true, false, false), k::KEY_Left), None);
        assert_eq!(shortcut(Modifier::Alt, &mods(true, true, false, false), k::KEY_Left), None);
        assert_eq!(shortcut(Modifier::Super, &mods(true, false, false, false), k::KEY_a), None);
    }

    #[test]
    fn ctrl_alt_backspace_always_quits() {
        assert_eq!(shortcut(Modifier::Super, &mods(false, true, true, false), k::KEY_BackSpace), Some(Command::Quit));
    }

    #[test]
    fn with_ctrl_alt_as_the_modifier_ctrl_means_nothing_more() {
        let both = mods(false, true, true, false);
        assert_eq!(shortcut(Modifier::CtrlAlt, &both, k::KEY_Left), Some(Command::Layout(Action::FocusLeft)));
    }
}
