use bytemuck::{Pod, Zeroable};
use serde::{Deserialize, Serialize};

/// Vertex type for PBR rendering (static meshes)
#[repr(C)]
#[derive(Copy, Clone, Debug, Zeroable, Pod, Serialize, Deserialize)]
pub struct Vertex {
    pub position: [f32; 3],
    pub normal: [f32; 3],
    pub tangent: [f32; 4],
    pub uv: [f32; 2],
    pub color: [f32; 4],
}

/// Vertex type for skeletal meshes (adds skinning info)
#[repr(C)]
#[derive(Copy, Clone, Debug, Zeroable, Pod, Serialize, Deserialize)]
pub struct SkeletalVertex {
    pub position: [f32; 3],
    pub normal: [f32; 3],
    pub tangent: [f32; 4],
    pub uv: [f32; 2],
    pub color: [f32; 4],
    pub joint_indices: [u32; 4],
    pub joint_weights: [f32; 4],
}


