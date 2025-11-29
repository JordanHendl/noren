use crate::meta::{DeviceTexture, DeviceTextureList, GraphicsShader, HostTexture};

#[derive(Clone, Debug)]
pub struct HostMaterial {
    pub name: String,
    pub textures: Vec<HostTexture>,
    pub shader: Option<GraphicsShader>,
}

#[repr(C)]
#[derive(Clone, Debug, Default)]
pub struct DeviceMaterial {
    pub textures: DeviceTextureList,
    pub shader: Option<GraphicsShader>,
}

impl DeviceMaterial {
    /// Builds a material from the provided textures and optional graphics shader.
    pub fn new(textures: Vec<DeviceTexture>, shader: Option<GraphicsShader>) -> Self {
        let mut list = DeviceTextureList::new();
        for texture in textures
            .into_iter()
            .take(super::textures::DEVICE_TEXTURE_CAPACITY)
        {
            list.push(texture);
        }
        debug_assert!(list.len() <= super::textures::DEVICE_TEXTURE_CAPACITY);
        Self {
            textures: list,
            shader,
        }
    }
}
