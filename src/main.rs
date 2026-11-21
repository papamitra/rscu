mod control;
mod focus;
#[cfg(feature = "rdp")]
mod overview;
mod protocol;
#[cfg(feature = "rdp")]
mod rdp;
mod render;
mod session;
mod window;

use control::Request;
use std::{ffi::OsString, path::PathBuf};

const USAGE: &str = "Usage: rscu --socket PATH run [--renderer pixman|gles] [--rdp-port PORT] [--wayland APP [ARGS...] [--wayland APP ...]] [--x11 APP [ARGS...] ...]
       rscu --socket PATH windows | activate ID | maximize ID | restore ID | screenshot ID [SINCE] [--crop X Y W H] | click IMAGE X Y | key ID EVDEV [MODIFIER...] | stop";

fn main() {
    if let Err(error) = run() {
        eprintln!("rscu: {error}");
        std::process::exit(1);
    }
}

fn run() -> session::Result<()> {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    if args.len() == 1 && args[0] == "--help" {
        println!("{USAGE}");
        return Ok(());
    }
    let [flag, socket, action, rest @ ..] = args.as_slice() else {
        return Err(USAGE.into());
    };
    if flag != "--socket" {
        return Err(USAGE.into());
    }
    let socket: PathBuf = socket.into();
    if action == "run" {
        let mut wayland = Vec::new();
        let mut x11 = Vec::new();
        let (backend, mut args) = match rest {
            [flag, value, args @ ..] if flag == "--renderer" => {
                let backend = match value.to_str() {
                    Some("gles") => render::Backend::Gles,
                    Some("pixman") => render::Backend::Pixman,
                    _ => return Err(USAGE.into()),
                };
                (backend, args)
            }
            _ => (render::Backend::Pixman, rest),
        };
        let (rdp_port, remaining) = match args {
            [flag, value, remaining @ ..] if flag == "--rdp-port" => {
                (Some(number(value)?), remaining)
            }
            _ => (None, args),
        };
        args = remaining;
        while let [flag, command @ ..] = args {
            let next = command
                .iter()
                .position(|arg| arg == "--wayland" || arg == "--x11")
                .unwrap_or(command.len());
            let app = command[..next].to_vec();
            if app.is_empty() {
                return Err(USAGE.into());
            }
            match flag.to_str() {
                Some("--wayland") => wayland.push(app),
                Some("--x11") => x11.push(app),
                _ => return Err(USAGE.into()),
            }
            args = &command[next..];
        }
        return session::run(&socket, backend, rdp_port, wayland, x11);
    }
    let request = match (action.to_str(), rest) {
        (Some("windows"), []) => Request::Windows,
        (Some("maximize"), [window]) => Request::Maximize {
            window: number(window)?,
        },
        (Some("restore"), [window]) => Request::Restore {
            window: number(window)?,
        },
        (Some("activate"), [window]) => Request::Activate {
            window: number(window)?,
        },
        (Some("screenshot"), [window, options @ ..]) => {
            let (since, crop) = match options {
                [flag, ..] if flag == "--crop" => (None, options),
                [since, crop @ ..] => (Some(number(since)?), crop),
                [] => (None, options),
            };
            let crop = match crop {
                [] => None,
                [flag, x, y, w, h] if flag == "--crop" => {
                    Some([number(x)?, number(y)?, number(w)?, number(h)?])
                }
                _ => return Err(USAGE.into()),
            };
            Request::Screenshot {
                window: number(window)?,
                since,
                crop,
            }
        }
        (Some("click"), [image, x, y]) => Request::Click {
            image: number(image)?,
            x: number(x)?,
            y: number(y)?,
        },
        (Some("key"), [window, code, modifiers @ ..]) => Request::Key {
            window: number(window)?,
            code: number(code)?,
            modifiers: modifiers
                .iter()
                .map(number)
                .collect::<session::Result<_>>()?,
        },
        (Some("stop"), []) => Request::Stop,
        _ => return Err(USAGE.into()),
    };
    println!("{}", control::send(&socket, &request)?);
    Ok(())
}

fn number<T: std::str::FromStr>(value: &OsString) -> session::Result<T> {
    value
        .to_str()
        .and_then(|v| v.parse().ok())
        .ok_or_else(|| format!("invalid number: {value:?}").into())
}
