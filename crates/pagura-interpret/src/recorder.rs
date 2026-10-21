//! The scene recorder: a device that records a [`Scene`].

use crate::device::{Device, Needs};
use crate::scene::Scene;
use pagura_core::{Error, Limits, Result};

/// Records the marks a page's interpretation emits into a [`Scene`],
/// interning paints, states, pens and clips, and charging `display_commands`
/// and the scene's bytes.
#[derive(Debug)]
pub struct SceneRecorder {
    limits: Limits,
}

impl SceneRecorder {
    /// An empty recorder under `limits`.
    pub fn new(limits: Limits) -> Self {
        Self { limits }
    }

    /// The recorded scene.
    ///
    /// # Errors
    ///
    /// Returns `InvalidArgument` when no complete page was recorded.
    pub fn finish(self) -> Result<Scene> {
        let _ = self.limits;
        Err(Error::invalid_argument("scene recording is not implemented"))
    }
}

impl Device for SceneRecorder {
    fn needs(&self) -> Needs {
        Needs::SCENE
    }
}
