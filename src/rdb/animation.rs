use serde::{Deserialize, Serialize};

use super::DatabaseEntry;
use crate::{RDBView, utils::NorenError};

#[repr(C)]
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct AnimationClip {
    pub name: String,
    #[serde(default)]
    pub duration_seconds: f32,
    #[serde(default)]
    pub samplers: Vec<AnimationSampler>,
    #[serde(default)]
    pub channels: Vec<AnimationChannel>,
    /// Reserved for future binary payloads.
    #[serde(default)]
    pub data: Vec<u8>,
}

impl AnimationClip {
    pub fn new(
        name: impl Into<String>,
        duration_seconds: f32,
        samplers: Vec<AnimationSampler>,
        channels: Vec<AnimationChannel>,
    ) -> Self {
        Self {
            name: name.into(),
            duration_seconds,
            samplers,
            channels,
            data: Vec::new(),
        }
    }
}

#[repr(C)]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum AnimationInterpolation {
    Linear,
    Step,
    CubicSpline,
}

impl Default for AnimationInterpolation {
    fn default() -> Self {
        Self::Linear
    }
}

#[repr(C)]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum AnimationTargetPath {
    Translation,
    Rotation,
    Scale,
    Weights,
}

impl Default for AnimationTargetPath {
    fn default() -> Self {
        Self::Translation
    }
}

#[repr(C)]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum AnimationOutput {
    Translations(Vec<[f32; 3]>),
    Rotations(Vec<[f32; 4]>),
    Scales(Vec<[f32; 3]>),
    Weights(Vec<f32>),
}

impl Default for AnimationOutput {
    fn default() -> Self {
        Self::Translations(Vec::new())
    }
}

#[repr(C)]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct AnimationSampler {
    #[serde(default)]
    pub interpolation: AnimationInterpolation,
    #[serde(default)]
    pub input: Vec<f32>,
    #[serde(default)]
    pub output: AnimationOutput,
}

#[repr(C)]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct AnimationChannel {
    #[serde(default)]
    pub sampler_index: usize,
    #[serde(default)]
    pub target_node: usize,
    #[serde(default)]
    pub target_path: AnimationTargetPath,
}

#[derive(Default)]
pub struct AnimationDB {
    data: Option<RDBView>,
}

impl AnimationDB {
    /// Loads animation clips from the given `.rdb` file path, if present.
    pub fn new(module_path: &str) -> Self {
        let data = match RDBView::load(module_path) {
            Ok(d) => Some(d),
            Err(_) => None,
        };

        Self { data }
    }

    /// Fetches an animation clip by entry name.
    pub fn fetch_animation(
        &mut self,
        entry: DatabaseEntry<'_>,
    ) -> Result<AnimationClip, NorenError> {
        if let Some(rdb) = &mut self.data {
            return Ok(rdb.fetch::<AnimationClip>(entry)?);
        }

        Err(NorenError::DataFailure())
    }

    /// Lists animation entries available in the backing database.
    pub fn enumerate_entries(&self) -> Vec<String> {
        self.data
            .as_ref()
            .map(|rdb| rdb.entries().into_iter().map(|meta| meta.name).collect())
            .unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::utils::rdbfile::RDBFile;

    const ENTRY: &str = "animations/test";

    #[test]
    fn fetch_animation_clip() {
        let clip = AnimationClip::new("wave", 1.5, Vec::new(), Vec::new());
        let mut file = RDBFile::new();
        file.add(ENTRY, &clip).expect("add animation");

        let tmp = std::env::temp_dir().join("animations.rdb");
        file.save(&tmp).expect("save animation file");

        let mut db = AnimationDB::new(tmp.to_str().unwrap());
        let loaded = db.fetch_animation(ENTRY).expect("load animation");

        assert_eq!(loaded, clip);
    }
}
