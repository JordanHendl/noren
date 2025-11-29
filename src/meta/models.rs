use crate::meta::{DeviceMesh, HostMesh};

#[derive(Clone, Debug)]
pub struct HostModel {
    pub name: String,
    pub meshes: Vec<HostMesh>,
}

#[derive(Clone, Debug)]
pub struct DeviceModel {
    pub name: String,
    pub meshes: Vec<DeviceMesh>,
}
