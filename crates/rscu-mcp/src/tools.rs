use base64::{Engine, engine::general_purpose::STANDARD};
use rscu_control::{Request, send};
use serde_json::{Value, json};
use std::{fs, io, path::PathBuf};

pub struct Tools {
    socket: PathBuf,
    definitions: Vec<Value>,
}

impl Tools {
    pub fn new(socket: PathBuf) -> Self {
        let id = json!({"type": "integer", "minimum": 1});
        let integer = json!({"type": "integer"});
        let window = json!({"window": id});
        let pointer = json!({"image": id, "x": integer, "y": integer});
        let mut click = pointer.clone();
        click["button"] = json!({"enum": ["left", "right", "middle"], "default": "left"});
        click["double"] = json!({"type": "boolean", "default": false});
        let mut scroll = pointer.clone();
        scroll["horizontal"] = integer.clone();
        scroll["vertical"] = integer.clone();
        let mut drag = pointer.clone();
        drag["end_x"] = integer.clone();
        drag["end_y"] = integer;
        let xy = &["image", "x", "y"];
        let definitions = vec![
            tool(
                "windows",
                "List mapped windows and their IDs.",
                json!({}),
                &[],
            ),
            tool(
                "screenshot",
                "Capture a window as PNG and return its image ID. Supply since with the previous image ID to omit identical pixels.",
                json!({
                    "window": id,
                    "since": id,
                    "crop": {
                        "type": "array", "items": {"type": "integer"},
                        "minItems": 4, "maxItems": 4,
                        "description": "Window-relative [x, y, width, height] in pixels."
                    }
                }),
                &["window"],
            ),
            tool(
                "activate",
                "Focus a window by ID.",
                window.clone(),
                &["window"],
            ),
            tool(
                "maximize",
                "Maximize a window by ID.",
                window.clone(),
                &["window"],
            ),
            tool(
                "restore",
                "Restore a maximized window by ID.",
                window,
                &["window"],
            ),
            tool(
                "move",
                "Move the pointer to image-relative pixel coordinates.",
                pointer,
                xy,
            ),
            tool(
                "click",
                "Click at image-relative pixel coordinates.",
                click,
                xy,
            ),
            tool(
                "scroll",
                "Scroll in wheel steps at image-relative coordinates. Positive is right/down.",
                scroll,
                &["image", "x", "y", "horizontal", "vertical"],
            ),
            tool(
                "drag",
                "Drag with the left button between two points in the same image.",
                drag,
                &["image", "x", "y", "end_x", "end_y"],
            ),
            tool(
                "key",
                "Send a Linux evdev keycode to a window. Modifier keycodes are held for the keypress.",
                json!({
                    "window": id,
                    "code": {"type": "integer", "minimum": 0},
                    "modifiers": {"type": "array", "items": {"type": "integer", "minimum": 0}}
                }),
                &["window", "code"],
            ),
        ];
        Self {
            socket,
            definitions,
        }
    }

    pub fn definitions(&self) -> &[Value] {
        &self.definitions
    }

    pub fn call(&self, name: &str, arguments: Option<&Value>) -> io::Result<Vec<Value>> {
        let mut args = match arguments {
            None => serde_json::Map::new(),
            Some(Value::Object(args)) => args.clone(),
            Some(_) => return Err(io::Error::other("arguments must be an object")),
        };
        let definition = self
            .definitions
            .iter()
            .find(|tool| tool["name"] == name)
            .ok_or_else(|| io::Error::other("Unknown tool"))?;
        let properties = &definition["inputSchema"]["properties"];
        if let Some(key) = args
            .keys()
            .find(|key| properties.get(key.as_str()).is_none())
        {
            return Err(io::Error::other(format!("Unknown argument: {key}")));
        }

        args.insert("command".into(), json!(name));
        let request: Request =
            serde_json::from_value(Value::Object(args)).map_err(io::Error::other)?;
        let mut response = send(&self.socket, &request)
            .map_err(|error| io::Error::other(format!("{}: {error}", self.socket.display())))?;
        let image = if matches!(request, Request::Screenshot { .. })
            && response["unchanged"] != true
        {
            let path = response
                .as_object_mut()
                .and_then(|metadata| metadata.remove("path"))
                .ok_or_else(|| io::Error::other("Screenshot response is missing its PNG path"))?;
            let path = path
                .as_str()
                .ok_or_else(|| io::Error::other("Invalid PNG path"))?;
            let png = fs::read(path)?;
            Some(json!({"type": "image", "mimeType": "image/png", "data": STANDARD.encode(png)}))
        } else {
            None
        };
        let mut content = vec![json!({"type": "text", "text": response.to_string()})];
        content.extend(image);
        Ok(content)
    }
}

fn tool(name: &str, description: &str, properties: Value, required: &[&str]) -> Value {
    json!({
        "name": name,
        "description": description,
        "inputSchema": {
            "type": "object",
            "properties": properties,
            "required": required,
            "additionalProperties": false
        }
    })
}
