//! A socket for commands, one line each: `scrollvil msg focus-right` from a
//! terminal, a script or a test.

use std::{
    io::{BufRead, BufReader, Read, Write},
    os::unix::net::{UnixListener, UnixStream},
    path::PathBuf,
    time::Duration,
};

use scroll_layout::Action;
use smithay::{
    reexports::calloop::{generic::Generic, EventLoop, Interest, Mode, PostAction},
    wayland::{compositor::with_states, shell::xdg::XdgToplevelSurfaceData},
};

use crate::{input::Command, CalloopData, Scrollvil};

const HELP: &str = "\
focus-left focus-right focus-up focus-down focus-first focus-last focus-column N
move-left move-right move-up move-down
move-column-left move-column-right move-column-first move-column-last
next-width previous-width wider narrower full-width equalize stacked center
scroll-left scroll-right scroll-by PIXELS
toggle-floating close launch quit
spawn COMMAND…   layout   screenshot FILE.png   help
";

fn action(word: &str, arg: Option<&str>) -> Option<Action> {
    Some(match word {
        "focus-left" => Action::FocusLeft,
        "focus-right" => Action::FocusRight,
        "focus-up" => Action::FocusUp,
        "focus-down" => Action::FocusDown,
        "focus-first" => Action::FocusFirst,
        "focus-last" => Action::FocusLast,
        "focus-column" => Action::FocusColumn(arg?.parse::<usize>().ok()?.checked_sub(1)?),
        "move-left" => Action::MoveLeft,
        "move-right" => Action::MoveRight,
        "move-up" => Action::MoveUp,
        "move-down" => Action::MoveDown,
        "move-column-left" => Action::MoveColumnLeft,
        "move-column-right" => Action::MoveColumnRight,
        "move-column-first" => Action::MoveColumnFirst,
        "move-column-last" => Action::MoveColumnLast,
        "next-width" => Action::NextWidth,
        "previous-width" => Action::PreviousWidth,
        "wider" => Action::Wider,
        "narrower" => Action::Narrower,
        "full-width" => Action::FullWidth,
        "equalize" => Action::EqualizeVisible,
        "stacked" => Action::ToggleStacked,
        "center" => Action::CenterFocused,
        "scroll-left" => Action::ScrollLeft,
        "scroll-right" => Action::ScrollRight,
        _ => return None,
    })
}

fn socket_path(state: &Scrollvil) -> PathBuf {
    std::env::var_os("SCROLLVIL_SOCKET").map(PathBuf::from).unwrap_or_else(|| {
        runtime_dir().join(format!("scrollvil-{}.sock", state.socket_name.to_string_lossy()))
    })
}

fn runtime_dir() -> PathBuf {
    std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from).unwrap_or_else(std::env::temp_dir)
}

pub fn listen(event_loop: &mut EventLoop<CalloopData>, state: &mut Scrollvil) -> std::io::Result<()> {
    let path = socket_path(state);
    let _ = std::fs::remove_file(&path);
    let listener = UnixListener::bind(&path)?;
    listener.set_nonblocking(true)?;
    eprintln!(
        "scrollvil: WAYLAND_DISPLAY={} SCROLLVIL_SOCKET={}",
        state.socket_name.to_string_lossy(),
        path.display()
    );
    state.control_path = Some(path);
    event_loop
        .handle()
        .insert_source(Generic::new(listener, Interest::READ, Mode::Level), |_, listener, data| {
            while let Ok((stream, _)) = listener.accept() {
                data.state.answer(stream);
            }
            Ok(PostAction::Continue)
        })
        .map_err(|e| std::io::Error::other(e.to_string()))?;
    Ok(())
}

pub fn stop(state: &Scrollvil) {
    if let Some(path) = &state.control_path {
        let _ = std::fs::remove_file(path);
    }
}

impl Scrollvil {
    fn answer(&mut self, mut stream: UnixStream) {
        let _ = stream.set_nonblocking(false);
        let _ = stream.set_read_timeout(Some(Duration::from_millis(500)));
        let mut line = String::new();
        if BufReader::new(&stream).read_line(&mut line).is_err() {
            return;
        }
        let line = line.trim();
        let (word, rest) = line.split_once(' ').unwrap_or((line, ""));
        let rest = rest.trim();
        let reply = match word {
            "help" => HELP.to_string(),
            "layout" => self.describe(),
            "spawn" if !rest.is_empty() => {
                self.spawn(rest);
                "ok\n".into()
            }
            "screenshot" if !rest.is_empty() => {
                // Answered once the next frame is drawn.
                self.shot = Some((PathBuf::from(rest), stream));
                self.request_redraw();
                return;
            }
            "scroll-by" => match rest.parse::<f64>() {
                Ok(dx) => {
                    self.strip.scroll_by(dx);
                    self.place_tiles();
                    "ok\n".into()
                }
                Err(_) => "error scroll-by takes a number of pixels\n".into(),
            },
            "toggle-floating" => {
                self.run(Command::ToggleFloating);
                "ok\n".into()
            }
            "close" => {
                self.run(Command::Close);
                "ok\n".into()
            }
            "launch" => {
                self.run(Command::Launch);
                "ok\n".into()
            }
            "quit" => {
                self.run(Command::Quit);
                "ok\n".into()
            }
            _ => match action(word, (!rest.is_empty()).then_some(rest)) {
                Some(action) => {
                    self.run(Command::Layout(action));
                    "ok\n".into()
                }
                None => format!("error unknown command {line:?}; try help\n"),
            },
        };
        let _ = stream.write_all(reply.as_bytes());
    }

    /// One line per window: where it is going, the size it has, and its title.
    fn describe(&self) -> String {
        let title = |w: &smithay::desktop::Window| {
            with_states(w.toplevel().unwrap().wl_surface(), |states| {
                states
                    .data_map
                    .get::<XdgToplevelSurfaceData>()
                    .and_then(|d| d.lock().unwrap().title.clone())
                    .unwrap_or_default()
            })
        };
        let mut out = format!(
            "scroll {:.1} view {} scale {} frames {} windows {}\n",
            self.strip.scroll(),
            self.strip.view_width(),
            self.output.as_ref().map_or(1.0, |o| o.current_scale().fractional_scale()),
            self.frames,
            self.windows.len()
        );
        for p in self.strip.placements() {
            let size = p.window.geometry().size;
            out += &format!(
                "tiled x={} y={} w={} h={} has={}x{} shown={} focused={} {}\n",
                p.rect.x,
                p.rect.y,
                p.rect.w,
                p.rect.h,
                size.w,
                size.h,
                p.shown,
                p.focused,
                title(&p.window)
            );
        }
        for w in &self.floating {
            let at = self.space.element_location(w).unwrap_or_default();
            let size = w.geometry().size;
            out += &format!("floating x={} y={} has={}x{} {}\n", at.x, at.y, size.w, size.h, title(w));
        }
        out
    }
}

/// `scrollvil msg …`: sends one command and prints the answer.
pub fn client(args: &[String]) -> i32 {
    let path = match std::env::var_os("SCROLLVIL_SOCKET") {
        Some(path) => PathBuf::from(path),
        None => {
            let newest = std::fs::read_dir(runtime_dir()).ok().and_then(|entries| {
                entries
                    .flatten()
                    .filter(|e| {
                        let name = e.file_name();
                        let name = name.to_string_lossy();
                        name.starts_with("scrollvil-") && name.ends_with(".sock")
                    })
                    .max_by_key(|e| e.metadata().and_then(|m| m.modified()).ok())
                    .map(|e| e.path())
            });
            match newest {
                Some(path) => path,
                None => {
                    eprintln!("scrollvil: no running scrollvil found (set SCROLLVIL_SOCKET)");
                    return 1;
                }
            }
        }
    };
    let mut stream = match UnixStream::connect(&path) {
        Ok(stream) => stream,
        Err(err) => {
            eprintln!("scrollvil: {}: {err}", path.display());
            return 1;
        }
    };
    let words = if args.is_empty() { "help".to_string() } else { args.join(" ") };
    if stream.write_all(format!("{words}\n").as_bytes()).is_err() {
        return 1;
    }
    let mut reply = String::new();
    let _ = stream.read_to_string(&mut reply);
    print!("{reply}");
    if reply.starts_with("error") {
        1
    } else {
        0
    }
}
