use crate::datatypes::{DeviceGeometry, DeviceImage, HostGeometry, HostImage, ShaderModule};

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
    pub meshes: Vec<HostMesh>,
}

#[derive(Clone, Debug)]
pub struct DeviceTexture {
    pub name: String,
    pub image: DeviceImage,
}

#[derive(Clone, Debug)]
pub struct DeviceMaterial {
    pub name: String,
    pub textures: Vec<DeviceTexture>,
    pub shader: Option<GraphicsShader>,
}

#[derive(Clone, Debug)]
pub struct DeviceMesh {
    pub name: String,
    pub geometry: DeviceGeometry,
    pub textures: Vec<DeviceTexture>,
    pub material: Option<DeviceMaterial>,
}

#[derive(Clone, Debug)]
pub struct DeviceModel {
    pub name: String,
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
}

impl GraphicsShader {
    pub fn new(name: String) -> Self {
        Self {
            name,
            ..Default::default()
        }
    }
}
