use std::ptr::NonNull;

use dashi::{Context, Handle, Image};
use serde::{Deserialize, Serialize};

use crate::{DataCache, RDBView, utils::NorenError};

use super::DatabaseEntry;

#[derive(Debug, Clone)]
pub struct GPUImageInfo {
    pub name: [u8; 64],
    pub dim: [u32; 3],
    pub layers: u32,
    pub format: dashi::Format,
    pub mip_levels: u32,
}

impl Default for GPUImageInfo {
    fn default() -> Self {
        Self {
            name: [0; 64],
            dim: Default::default(),
            layers: Default::default(),
            format: Default::default(),
            mip_levels: Default::default(),
        }
    }
}

#[derive(Default, Debug, Clone, Serialize, Deserialize)]
pub struct ImageInfo {
    pub name: String,
    pub dim: [u32; 3],
    pub layers: u32,
    pub format: dashi::Format,
    pub mip_levels: u32,
}

impl ImageInfo {
    pub fn dashi(&self) -> dashi::ImageInfo {
        dashi::ImageInfo {
            debug_name: &self.name,
            dim: self.dim,
            layers: self.layers,
            format: self.format,
            mip_levels: self.mip_levels,
            initial_data: None,
        }
    }

    pub fn gpu(&self) -> GPUImageInfo {
        let mut bytes: [u8; 64] = [0; 64];
        bytes[0..self.name.len()].copy_from_slice(self.name[0..self.name.len()].as_bytes());
        bytes[self.name.len().min(63)] = '\0' as u8;
        GPUImageInfo {
            name: bytes,
            dim: self.dim,
            layers: self.layers,
            format: self.format,
            mip_levels: self.mip_levels,
        }
    }
}

#[repr(C)]
#[derive(Serialize, Deserialize)]
pub struct HostImage {
    info: ImageInfo,
    data: Vec<u8>,
}

#[repr(C)]
#[derive(Clone, Debug, Default)]
pub struct DeviceImage {
    img: Handle<Image>,
    info: GPUImageInfo,
}

pub struct ImageDB {
    cache: DataCache<DeviceImage>,
    ctx: NonNull<Context>,
    data: Option<RDBView>,
}

impl ImageDB {
    pub fn new(ctx: *mut Context, module_path: &str) -> Self {
        let data = match RDBView::load(module_path) {
            Ok(d) => Some(d),
            Err(_) => None,
        };

        Self {
            data,
            ctx: NonNull::new(ctx).expect("Null GPU Context"),
            cache: Default::default(),
        }
    }

    pub fn enter_gpu_image(
        entry: DatabaseEntry,
        geom: HostImage,
    ) -> Result<DeviceImage, NorenError> {
        todo!()
    }

    pub fn is_loaded(&self, entry: &DatabaseEntry) -> bool {
        self.cache.get(*entry).is_some()
    }

    pub fn fetch_raw_image(&mut self, entry: DatabaseEntry) -> Result<HostImage, NorenError> {
        if let Some(rdb) = &mut self.data {
            return Ok(rdb.fetch::<HostImage>(entry)?);
        }

        return Err(NorenError::DataFailure());
    }

    pub fn fetch_gpu_image(&mut self, entry: DatabaseEntry) -> Result<DeviceImage, NorenError> {
        // pseudocode:
        // if not loaded in cache {
        //   fetch_raw_image, upload and cache
        //   refcount
        // }

        todo!()
    }

    // Checks whether any imagery needs to be unloaded, and does so.
    pub fn unload_pulse(&mut self) {
        todo!()
    }
}
