use std::fmt;

use crate::datatypes::{DeviceGeometry, DeviceImage, HostGeometry, HostImage, ShaderModule};
use dashi::{BindGroupLayout, BindTableLayout, GraphicsPipeline, GraphicsPipelineLayout, Handle};

pub const DEVICE_NAME_CAPACITY: usize = 64;
pub const DEVICE_TEXTURE_CAPACITY: usize = 8;

#[repr(C)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DeviceName {
    pub bytes: [u8; DEVICE_NAME_CAPACITY],
}

impl DeviceName {
    pub fn new() -> Self {
        Self {
            bytes: [0; DEVICE_NAME_CAPACITY],
        }
    }

    pub fn from_str(name: &str) -> Self {
        let mut bytes = [0u8; DEVICE_NAME_CAPACITY];
        let name_bytes = name.as_bytes();
        let copy_len = name_bytes.len().min(DEVICE_NAME_CAPACITY.saturating_sub(1));
        bytes[..copy_len].copy_from_slice(&name_bytes[..copy_len]);
        bytes[copy_len] = b'\0';
        Self { bytes }
    }

    pub fn to_string(&self) -> String {
        let nul_pos = self
            .bytes
            .iter()
            .position(|&b| b == 0)
            .unwrap_or(self.bytes.len());
        String::from_utf8_lossy(&self.bytes[..nul_pos]).into_owned()
    }
}

impl Default for DeviceName {
    fn default() -> Self {
        Self::new()
    }
}

impl From<&str> for DeviceName {
    fn from(value: &str) -> Self {
        Self::from_str(value)
    }
}

impl From<String> for DeviceName {
    fn from(value: String) -> Self {
        Self::from_str(&value)
    }
}

impl fmt::Display for DeviceName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.to_string())
    }
}

#[derive(Clone, Debug)]
pub struct HostTexture {
    pub name: String,
    pub image: HostImage,
}

#[derive(Clone, Debug)]
pub struct HostMaterial {
    pub name: String,
    pub textures: Vec<HostTexture>,
    pub shader: Option<GraphicsShader>,
}

#[derive(Clone, Debug)]
pub struct HostMesh {
    pub name: String,
    pub geometry: HostGeometry,
    pub textures: Vec<HostTexture>,
    pub material: Option<HostMaterial>,
}

#[derive(Clone, Debug)]
pub struct HostModel {
    pub name: String,
    pub render_passes: Vec<String>,
    pub meshes: Vec<HostMesh>,
}

#[repr(C)]
#[derive(Clone, Debug, Default)]
pub struct DeviceTexture {
    pub image: DeviceImage,
}

impl DeviceTexture {
    pub fn new(image: DeviceImage) -> Self {
        Self { image }
    }
}

#[repr(C)]
#[derive(Clone, Debug)]
pub struct DeviceTextureList {
    len: u8,
    textures: [DeviceTexture; DEVICE_TEXTURE_CAPACITY],
}

impl DeviceTextureList {
    pub fn new() -> Self {
        Self {
            len: 0,
            textures: std::array::from_fn(|_| DeviceTexture::default()),
        }
    }

    pub fn push(&mut self, texture: DeviceTexture) {
        if (self.len as usize) < DEVICE_TEXTURE_CAPACITY {
            self.textures[self.len as usize] = texture;
            self.len += 1;
        } else {
            debug_assert!(false, "DeviceTextureList capacity exceeded");
        }
    }

    pub fn len(&self) -> usize {
        self.len as usize
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    pub fn as_slice(&self) -> &[DeviceTexture] {
        &self.textures[..self.len()]
    }

    pub fn get(&self, index: usize) -> Option<&DeviceTexture> {
        if index < self.len() {
            Some(&self.textures[index])
        } else {
            None
        }
    }
}

impl Default for DeviceTextureList {
    fn default() -> Self {
        Self::new()
    }
}

impl<'a> IntoIterator for &'a DeviceTextureList {
    type Item = &'a DeviceTexture;
    type IntoIter = std::slice::Iter<'a, DeviceTexture>;

    fn into_iter(self) -> Self::IntoIter {
        self.as_slice().iter()
    }
}

#[repr(C)]
#[derive(Clone, Debug, Default)]
pub struct DeviceMaterial {
    pub textures: DeviceTextureList,
    pub shader: Option<GraphicsShader>,
}

impl DeviceMaterial {
    pub fn new(textures: Vec<DeviceTexture>, shader: Option<GraphicsShader>) -> Self {
        let mut list = DeviceTextureList::new();
        for texture in textures.into_iter().take(DEVICE_TEXTURE_CAPACITY) {
            list.push(texture);
        }
        debug_assert!(list.len() <= DEVICE_TEXTURE_CAPACITY);
        Self {
            textures: list,
            shader,
        }
    }
}

#[repr(C)]
#[derive(Clone, Debug, Default)]
pub struct DeviceMesh {
    pub geometry: DeviceGeometry,
    pub textures: DeviceTextureList,
    pub material: Option<DeviceMaterial>,
}

impl DeviceMesh {
    pub fn new(
        geometry: DeviceGeometry,
        textures: Vec<DeviceTexture>,
        material: Option<DeviceMaterial>,
    ) -> Self {
        let mut list = DeviceTextureList::new();
        for texture in textures.into_iter().take(DEVICE_TEXTURE_CAPACITY) {
            list.push(texture);
        }
        Self {
            geometry,
            textures: list,
            material,
        }
    }
}

#[derive(Clone, Debug)]
pub struct DeviceModel {
    pub name: String,
    pub render_passes: Vec<String>,
    pub meshes: Vec<DeviceMesh>,
}

#[derive(Clone, Debug)]
pub struct ShaderStage {
    pub entry: String,
    pub module: ShaderModule,
}

impl ShaderStage {
    pub fn new(entry: String, module: ShaderModule) -> Self {
        Self { entry, module }
    }
}

#[derive(Clone, Debug, Default)]
pub struct GraphicsShader {
    pub name: String,
    pub vertex: Option<ShaderStage>,
    pub fragment: Option<ShaderStage>,
    pub geometry: Option<ShaderStage>,
    pub tessellation_control: Option<ShaderStage>,
    pub tessellation_evaluation: Option<ShaderStage>,
    pub bind_group_layouts: [Option<Handle<BindGroupLayout>>; 4],
    pub bind_table_layouts: [Option<Handle<BindTableLayout>>; 4],
    pub pipeline_layout: Option<Handle<GraphicsPipelineLayout>>,
    pub pipeline: Option<Handle<GraphicsPipeline>>,
}

impl GraphicsShader {
    pub fn new(name: String) -> Self {
        Self {
            name,
            ..Default::default()
        }
    }
}
