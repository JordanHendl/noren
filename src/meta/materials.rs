use crate::meta::{DeviceTexture, DeviceTextureList, HostTexture};
use furikake::types::Material as FurikakeMaterial;
use std::fmt;

#[derive(Clone)]
pub struct HostMaterial {
    pub name: String,
    pub textures: Vec<HostTexture>,
    pub material: FurikakeMaterial,
}

#[repr(C)]
#[derive(Clone, Default)]
pub struct DeviceMaterial {
    pub textures: DeviceTextureList,
    pub material: FurikakeMaterial,
}

impl fmt::Debug for HostMaterial {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("HostMaterial")
            .field("name", &self.name)
            .field("textures", &self.textures)
            .field("material", &format_material(&self.material))
            .finish()
    }
}

impl fmt::Debug for DeviceMaterial {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DeviceMaterial")
            .field("textures", &self.textures)
            .field("material", &format_material(&self.material))
            .finish()
    }
}

fn format_material(material: &FurikakeMaterial) -> String {
    format!(
        "{{ base_color_texture_id: {}, normal_texture_id: {}, metallic_roughness_texture_id: {}, occlusion_texture_id: {}, emissive_texture_id: {}, render_mask: {} }}",
        material.base_color_texture_id,
        material.normal_texture_id,
        material.metallic_roughness_texture_id,
        material.occlusion_texture_id,
        material.emissive_texture_id,
        material.render_mask
    )
}

impl DeviceMaterial {
    /// Builds a material from the provided textures and furikake material definition.
    pub fn new(textures: Vec<DeviceTexture>, material: FurikakeMaterial) -> Self {
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
            material,
        }
    }
}
