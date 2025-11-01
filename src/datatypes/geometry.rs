use crate::error::NorenError;

use super::DatabaseEntry;

pub struct HostGeometry;
pub struct DeviceGeometry;

pub struct GeometryDB {}

impl GeometryDB {
    fn new(module_path: &str) -> Self {
        todo!()
    }

    pub fn enter_gpu_geometry(
        entry: DatabaseEntry,
        geom: HostGeometry,
    ) -> Result<DeviceGeometry, NorenError> {
        todo!()
    }

    pub fn is_loaded(entry: &DatabaseEntry) -> bool {
        todo!()
    }

    pub fn fetch_raw_geometry(entry: DatabaseEntry) -> Result<HostGeometry, NorenError> {
        todo!()
    }

    pub fn fetch_gpu_geometry(entry: DatabaseEntry) -> Result<DeviceGeometry, NorenError> {
        todo!()
    }
}
