use std::collections::{HashMap, HashSet};

use crate::meta::{DeviceMesh, HostMesh};
use crate::rdb::{AnimationClip, Skeleton};
use dashi::{Buffer, Handle};
use furikake::types::{AnimationClip as FurikakeAnimationClip, SkeletonHeader};

#[derive(Clone, Debug)]
pub struct HostRig {
    pub skeleton: Skeleton,
    pub animations: HashMap<String, AnimationClip>,
}

#[derive(Clone, Debug)]
pub struct DeviceRig {
    pub skeleton: Handle<SkeletonHeader>,
    pub animations: HashMap<String, Handle<FurikakeAnimationClip>>,
}

#[derive(Clone, Debug)]
pub struct HostModel {
    pub name: String,
    pub meshes: Vec<HostMesh>,
    pub rig: Option<HostRig>,
}

#[derive(Clone, Debug)]
pub struct DeviceModel {
    pub name: String,
    pub meshes: Vec<DeviceMesh>,
    pub rig: Option<DeviceRig>,
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
