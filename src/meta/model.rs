use crate::datatypes::{DeviceGeometry, DeviceImage, HostGeometry, HostImage};

#[derive(Clone, Debug)]
pub struct HostTexture {
    pub name: String,
    pub image: HostImage,
}

#[derive(Clone, Debug)]
pub struct HostMaterial {
    pub name: String,
    pub textures: Vec<HostTexture>,
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
