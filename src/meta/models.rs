use crate::meta::{DeviceMesh, HostMesh};

#[derive(Clone, Debug)]
pub struct HostModel {
    pub name: String,
    pub meshes: Vec<HostMesh>,
    pub vertex_count: u32,
    pub index_count: Option<u32>,
}

#[derive(Clone, Debug)]
pub struct DeviceModel {
    pub name: String,
    pub meshes: Vec<DeviceMesh>,
    pub vertex_count: u32,
    pub index_count: Option<u32>,
}
