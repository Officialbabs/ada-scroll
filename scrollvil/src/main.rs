#![allow(irrefutable_let_patterns)]

mod control;
mod grabs;
mod handlers;
mod input;
mod state;
mod winit;

use smithay::reexports::{
    calloop::EventLoop,
    wayland_server::{Display, DisplayHandle},
};
pub use state::Scrollvil;

pub struct CalloopData {
    state: Scrollvil,
    display_handle: DisplayHandle,
}

const USAGE: &str = "usage: scrollvil [--mod super|alt|ctrl-alt] [-- COMMAND …]
       scrollvil msg COMMAND …   (scrollvil msg help lists them)";

/// The app picker's command: $SCROLLVIL_LAUNCHER, scrollvil-launch beside
/// this program, or the copy in a source checkout.
pub fn launcher() -> Option<String> {
    if let Ok(command) = std::env::var("SCROLLVIL_LAUNCHER") {
        return Some(command);
    }
    let exe = std::env::current_exe().ok()?;
    let dir = exe.parent()?;
    [dir.join("scrollvil-launch"), dir.join("../../tools/launch.py")]
        .into_iter()
        .find(|p| p.exists())
        .map(|p| format!("'{}'", p.display()))
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().map(String::as_str) == Some("msg") {
        std::process::exit(control::client(&args[1..]));
    }

    let mut modifier = std::env::var("SCROLLVIL_MOD").ok();
    let mut command = Vec::new();
    let mut rest = args.iter();
    while let Some(arg) = rest.next() {
        match arg.as_str() {
            "--mod" => modifier = rest.next().cloned(),
            "--" => command = rest.by_ref().cloned().collect(),
            "-h" | "--help" => {
                println!("{USAGE}");
                return Ok(());
            }
            _ => {
                eprintln!("{USAGE}");
                std::process::exit(2);
            }
        }
    }
    let modifier = match modifier.as_deref().unwrap_or("super") {
        "super" => input::Modifier::Super,
        "alt" => input::Modifier::Alt,
        "ctrl-alt" => input::Modifier::CtrlAlt,
        other => {
            eprintln!("scrollvil: --mod is super, alt or ctrl-alt, not {other}");
            std::process::exit(2);
        }
    };

    if let Ok(env_filter) = tracing_subscriber::EnvFilter::try_from_default_env() {
        tracing_subscriber::fmt().with_env_filter(env_filter).init();
    } else {
        tracing_subscriber::fmt().with_env_filter("warn").init();
    }

    let mut event_loop: EventLoop<'static, CalloopData> = EventLoop::try_new()?;

    let display: Display<Scrollvil> = Display::new()?;
    let display_handle = display.handle();
    let state = Scrollvil::new(&mut event_loop, display, modifier);

    let mut data = CalloopData { state, display_handle };

    crate::winit::init_winit(&mut event_loop, &mut data)?;
    control::listen(&mut event_loop, &mut data.state)?;

    if !command.is_empty() {
        data.state.spawn(&command.join(" "));
    } else if let Some(picker) = launcher() {
        data.state.spawn(&picker);
    }

    event_loop.run(None, &mut data, move |data| {
        // Replies reach clients even while no frame is being drawn.
        let _ = data.display_handle.flush_clients();
    })?;

    control::stop(&data.state);
    Ok(())
}
