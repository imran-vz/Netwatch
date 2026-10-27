//! The workbench's state across launches: the recipe being built, the input
//! (when it's small enough to keep), auto bake and favourite operations.
//! Stored as JSON wherever the app keeps its data.

use crate::Recipe;
use serde::{Deserialize, Serialize};
use std::path::Path;

/// Inputs longer than this aren't saved; reopening starts empty instead.
pub const MAX_SAVED_INPUT: usize = 256 * 1024;

/// CyberChef's default favourites, those that are ported.
const DEFAULT_FAVOURITES: &[&str] = &[
    "To Base64",
    "From Base64",
    "To Hex",
    "From Hex",
    "To Hexdump",
    "From Hexdump",
    "URL Decode",
    "Regular expression",
    "Entropy",
    "Fork",
    "Magic",
];

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Session {
    /// CyberChef recipe JSON.
    pub recipe: String,
    pub input: String,
    pub auto_bake: bool,
    pub favourites: Vec<String>,
}

impl Default for Session {
    fn default() -> Self {
        Session {
            recipe: "[]".into(),
            input: String::new(),
            auto_bake: true,
            favourites: DEFAULT_FAVOURITES.iter().map(|s| s.to_string()).collect(),
        }
    }
}

impl Session {
    /// The saved recipe, dropping it if it no longer loads.
    pub fn recipe(&self) -> Recipe {
        Recipe::parse(&self.recipe).unwrap_or_default()
    }

    /// Favourites that are ported, in their saved order.
    pub fn favourite_ops(&self) -> Vec<&'static crate::Operation> {
        self.favourites
            .iter()
            .filter_map(|n| crate::find(n))
            .collect()
    }

    pub fn is_favourite(&self, name: &str) -> bool {
        self.favourites.iter().any(|f| f == name)
    }

    pub fn toggle_favourite(&mut self, name: &str) {
        if let Some(i) = self.favourites.iter().position(|f| f == name) {
            self.favourites.remove(i);
        } else {
            self.favourites.push(name.to_string());
        }
    }

    /// Keep `input` only when it's small; large inputs aren't worth the disk.
    pub fn set_input(&mut self, input: &str) {
        self.input = if input.len() <= MAX_SAVED_INPUT {
            input.to_string()
        } else {
            String::new()
        };
    }
}

/// A missing or unreadable file gives the defaults.
pub fn load(path: &Path) -> Session {
    std::fs::read(path)
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default()
}

/// Each save writes its own temporary file and renames it into place, so
/// overlapping saves can't leave a corrupt file.
fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    let dir = match path.parent() {
        Some(d) if !d.as_os_str().is_empty() => d,
        _ => Path::new("."),
    };
    std::fs::create_dir_all(dir)?;
    let mut tmp = tempfile::NamedTempFile::new_in(dir)?;
    tmp.write_all(bytes)?;
    tmp.persist(path).map_err(|e| e.error)?;
    Ok(())
}

pub fn save(path: &Path, session: &Session) -> std::io::Result<()> {
    write_atomic(path, &serde_json::to_vec_pretty(session)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_and_defaults() {
        let dir = std::env::temp_dir().join(format!("utilify-session-{}", std::process::id()));
        let path = dir.join("workbench.json");
        assert_eq!(load(&path), Session::default());
        let mut s = Session {
            recipe: r#"[{"op":"To Hex","args":["Space",0]}]"#.into(),
            ..Session::default()
        };
        s.set_input("hello");
        s.toggle_favourite("To Hex");
        assert!(!s.is_favourite("To Hex"));
        save(&path, &s).unwrap();
        let back = load(&path);
        assert_eq!(back, s);
        assert_eq!(back.recipe().steps.len(), 1);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn large_input_is_not_kept() {
        let mut s = Session::default();
        s.set_input(&"x".repeat(MAX_SAVED_INPUT + 1));
        assert!(s.input.is_empty());
    }
}
