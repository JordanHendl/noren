use std::ptr::NonNull;

use bytemuck::{Pod, Zeroable};
use dashi::{Buffer, Context, Handle};
use serde::{Deserialize, Serialize};

use super::{DatabaseEntry, primitives::Vertex};
use crate::{DataCache, RDBView, error::NorenError};

#[repr(C)]
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct HostGeometry {
    pub vertices: Vec<Vertex>,
    pub indices: Option<Vec<u32>>,
}

#[repr(C)]
#[derive(Clone, Debug, Default)]
pub struct DeviceGeometry {
    pub vertices: Handle<Buffer>,
    pub indices: Handle<Buffer>,
}

pub struct GeometryDB {
    cache: DataCache<DeviceGeometry>,
    ctx: NonNull<Context>,
    data: Option<RDBView>,
}

impl GeometryDB {
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

    pub fn enter_gpu_geometry(
        entry: DatabaseEntry,
        geom: HostGeometry,
    ) -> Result<DeviceGeometry, NorenError> {
        todo!()
    }

    pub fn is_loaded(&self, entry: &DatabaseEntry) -> bool {
        self.cache.get(*entry).is_some()
    }

    pub fn fetch_raw_geometry(&mut self, entry: DatabaseEntry) -> Result<HostGeometry, NorenError> {
        if let Some(rdb) = &mut self.data {
            return Ok(rdb.fetch::<HostGeometry>(entry)?);
        }

        return Err(NorenError::DataFailure());
    }

    pub fn fetch_gpu_geometry(
        &mut self,
        entry: DatabaseEntry,
    ) -> Result<DeviceGeometry, NorenError> {
        // pseudocode:
        // if not loaded in cache {
        //   fetch_raw_geometry, upload and cache
        //   refcount
        // }
        todo!()
    }

    pub fn unref_entry(&mut self, entry: DatabaseEntry) -> Result<(), NorenError> {
        // pseudocode:
        // if loaded in cache {
        //   dec refcount
        //   if refcount is 0, start 'free countdown' timer.
        // }
        todo!()
    }

    // Checks whether any geometry needs to be unloaded, and does so.
    pub fn unload_pulse(&mut self) {
        todo!()
    }
}
