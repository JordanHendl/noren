use crate::meta::{DeviceMaterial, DeviceTexture, DeviceTextureList, HostMaterial, HostTexture};
use crate::rdb::{DeviceGeometry, HostGeometry};
use dashi::{Buffer, Handle};

#[derive(Clone, Debug)]
pub struct HostMesh {
    pub name: String,
    pub geometry: HostGeometry,
    pub vertex_count: u32,
    pub index_count: Option<u32>,
    pub textures: Vec<HostTexture>,
    pub material: Option<HostMaterial>,
}

#[derive(Clone, Debug, Default)]
pub struct DeviceMesh {
    pub geometry: DeviceGeometry,
    pub vertex_count: u32,
    pub index_count: Option<u32>,
    pub textures: DeviceTextureList,
    pub material: Option<DeviceMaterial>,
}

impl DeviceMesh {
    /// Creates a GPU-ready mesh with geometry, textures, and an optional material.
    pub fn new(
        geometry: DeviceGeometry,
        textures: Vec<DeviceTexture>,
        material: Option<DeviceMaterial>,
    ) -> Self {
        let mut list = DeviceTextureList::new();
        for texture in textures
            .into_iter()
            .take(super::textures::DEVICE_TEXTURE_CAPACITY)
        {
            list.push(texture);
        }
        let vertex_count = geometry.vertex_count;
        let index_count = geometry.index_count;
        Self {
            geometry,
            vertex_count,
            index_count,
            textures: list,
            material,
        }
    }

    pub fn buffer_handles(&self) -> Vec<Handle<Buffer>> {
        self.geometry.buffer_handles()
    }
}
