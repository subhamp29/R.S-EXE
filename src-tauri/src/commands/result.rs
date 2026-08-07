use serde::Serialize;

/// Generic typed result shape for all Tauri commands.
/// The frontend never has to guess the response shape.
#[derive(Debug, Serialize)]
pub struct CommandResult<T> {
    pub ok: bool,
    pub error: Option<String>,
    pub output: Option<T>,
}

impl<T> CommandResult<T> {
    pub fn success(output: T) -> Self {
        Self {
            ok: true,
            error: None,
            output: Some(output),
        }
    }

    pub fn fail(error: String) -> Self {
        Self {
            ok: false,
            error: Some(error),
            output: None,
        }
    }
}
