use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::io::{self, BufRead, BufReader, Read, Write};
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::time::Duration;

#[derive(Debug, Deserialize, Serialize)]
#[serde(tag = "command", rename_all = "snake_case")]
pub enum Request {
    Windows,
    Maximize {
        window: u64,
    },
    Restore {
        window: u64,
    },
    Activate {
        window: u64,
    },
    Screenshot {
        window: u64,
        since: Option<u64>,
        crop: Option<[i32; 4]>,
    },
    Click {
        image: u64,
        x: i32,
        y: i32,
    },
    Key {
        window: u64,
        code: u32,
        #[serde(default)]
        modifiers: Vec<u32>,
    },
    Stop,
}

pub fn configure(stream: &UnixStream) -> io::Result<()> {
    let timeout = Some(Duration::from_secs(2));
    stream.set_read_timeout(timeout)?;
    stream.set_write_timeout(timeout)
}

pub fn read_request(stream: &UnixStream) -> io::Result<Request> {
    let mut line = String::new();
    BufReader::new(stream).take(4097).read_line(&mut line)?;
    if line.len() > 4096 {
        return Err(io::Error::other("request exceeds 4096 bytes"));
    }
    Ok(serde_json::from_str(&line)?)
}

pub fn send(path: &Path, request: &Request) -> io::Result<Value> {
    let mut stream = UnixStream::connect(path)?;
    configure(&stream)?;
    serde_json::to_writer(&mut stream, request)?;
    stream.write_all(b"\n")?;
    let response: Value = serde_json::from_reader(stream.take(1024 * 1024))?;
    if let Some(error) = response.get("error").and_then(Value::as_str) {
        return Err(io::Error::other(error));
    }
    Ok(response)
}
