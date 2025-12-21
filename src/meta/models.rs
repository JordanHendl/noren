use std::collections::HashSet;

use crate::meta::{DeviceMesh, HostMesh};
use dashi::{Buffer, Handle};

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

impl DeviceModel {
    pub fn buffer_handles(&self) -> Vec<Handle<Buffer>> {
        let mut handles = HashSet::new();
        for mesh in &self.meshes {
            for handle in mesh.buffer_handles() {
                handles.insert(handle);
            }
        }

        handles.into_iter().collect()
    }
}
