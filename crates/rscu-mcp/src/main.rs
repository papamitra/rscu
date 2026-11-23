mod tools;

use serde_json::{Value, json};
use std::{
    io::{self, BufRead, Write},
    path::PathBuf,
    process::ExitCode,
};

const VERSIONS: &[&str] = &["2025-11-25", "2025-06-18", "2025-03-26", "2024-11-05"];

fn main() -> ExitCode {
    let mut args = std::env::args_os().skip(1);
    let socket = match (args.next(), args.next(), args.next()) {
        (Some(flag), Some(path), None) if flag == "--socket" => PathBuf::from(path),
        (Some(flag), None, None) if flag == "--help" || flag == "-h" => {
            println!("Usage: rscu-mcp --socket PATH");
            return ExitCode::SUCCESS;
        }
        _ => {
            eprintln!("Usage: rscu-mcp --socket PATH");
            return ExitCode::FAILURE;
        }
    };
    if let Err(error) = serve(socket) {
        eprintln!("rscu-mcp: {error}");
        return ExitCode::FAILURE;
    }
    ExitCode::SUCCESS
}

fn serve(socket: PathBuf) -> io::Result<()> {
    let tools = tools::Tools::new(socket);
    let mut output = io::stdout().lock();
    for line in io::stdin().lock().lines() {
        let response = match serde_json::from_str(&line?) {
            Ok(message) => dispatch(&tools, message),
            Err(_) => Some(error(Value::Null, -32700, "Parse error")),
        };
        if let Some(response) = response {
            serde_json::to_writer(&mut output, &response)?;
            writeln!(output)?;
            output.flush()?;
        }
    }
    Ok(())
}

fn dispatch(tools: &tools::Tools, message: Value) -> Option<Value> {
    let id = message.get("id");
    let method = message.get("method").and_then(Value::as_str);
    if message["jsonrpc"] != "2.0"
        || method.is_none()
        || id.is_some_and(|id| !(id.is_string() || id.is_i64() || id.is_u64()))
    {
        return Some(error(Value::Null, -32600, "Invalid request"));
    }
    let id = id?.clone(); // Notifications never receive a reply.
    let params = &message["params"];
    let result = match method? {
        "initialize" => {
            let Some(requested) = params["protocolVersion"].as_str() else {
                return Some(error(id, -32602, "protocolVersion is required"));
            };
            let version = if VERSIONS.contains(&requested) {
                requested
            } else {
                VERSIONS[0]
            };
            json!({
                "protocolVersion": version,
                "capabilities": {"tools": {}},
                "serverInfo": {"name": "rscu-mcp", "version": env!("CARGO_PKG_VERSION")},
                "instructions": "List windows, then take a screenshot. Mouse tools use its image ID and pixel coordinates. Take a fresh screenshot after UI changes."
            })
        }
        "ping" => json!({}),
        "tools/list" => json!({"tools": tools.definitions()}),
        "tools/call" => {
            let Some(name) = params["name"].as_str() else {
                return Some(error(id, -32602, "Tool name is required"));
            };
            if !tools.definitions().iter().any(|tool| tool["name"] == name) {
                return Some(error(id, -32602, "Unknown tool"));
            }
            match tools.call(name, params.get("arguments")) {
                Ok(content) => json!({"content": content}),
                Err(problem) => json!({
                    "content": [{"type": "text", "text": problem.to_string()}],
                    "isError": true
                }),
            }
        }
        _ => return Some(error(id, -32601, "Method not found")),
    };
    Some(json!({"jsonrpc": "2.0", "id": id, "result": result}))
}

fn error(id: Value, code: i32, message: &str) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": message}})
}
