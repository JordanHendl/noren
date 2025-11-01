pub mod datatypes;
pub mod meta;
mod parsing;
mod utils;

use datatypes::*;
use error::NorenError;
use meta::*;
use parsing::*;
use utils::*;

pub struct DBInfo<'a> {
    pub ctx: *mut dashi::Context,
    pub base_dir: &'a str,
    pub layout_file: Option<&'a str>,
}

pub struct DB {
    geometry: GeometryDB,
    imagery: ImageDB,
    model_file: Option<ModelLayoutFile>,
}

////////////////////////////////////////////////
/// NorenDB (Noran Database)
/// * Provides readonly access to reading and loading data
///   from a Noren Generated Database.
///
/// * Handles access to Host(CPU) and Device(GPU) data.
/// ** CPU Data is read from the mapped memory when requested.
/// ** GPU Data is GPU-Ready (OK for usage/uploads), cached and refcounted when requested, and will unload on a timer when all refs
///    are released. The timer is so that if data is quickly unreffed/fetched, it will handle
///    gracefully. This timer is configurable.
////////////////////////////////////////////////

impl DB {
    pub fn new(info: &DBInfo) -> Result<Self, NorenError> {
        let layout: DatabaseLayoutFile = match info.layout_file {
            Some(f) => serde_json::from_str(&std::fs::read_to_string(f.to_string())?)?,
            None => Default::default(),
        };

        let geometry = GeometryDB::new(info.ctx, &format!("{}/{}", info.base_dir, layout.geometry));
        let imagery = ImageDB::new(info.ctx, &format!("{}/{}", info.base_dir, layout.imagery));
        let model_file = if let Ok(file) =
            std::fs::read_to_string(format!("{}/{}", info.base_dir, layout.models))
        {
            Some(serde_json::from_str(&file)?)
        } else {
            None
        };

        Ok(Self {
            geometry,
            imagery,
            model_file,
        })
    }

    pub fn geometry(&self) -> &GeometryDB {
        &self.geometry
    }

    pub fn geometry_mut(&mut self) -> &mut GeometryDB {
        &mut self.geometry
    }

    pub fn imagery(&self) -> &ImageDB {
        &self.imagery
    }

    pub fn imagery_mut(&mut self) -> &mut ImageDB {
        &mut self.imagery
    }

    pub fn font(&self) -> &FontDB {
        todo!()
    }

    pub fn fetch_model(&mut self, entry: DatabaseEntry) -> Result<HostModel, NorenError> {
        todo!()
    }

    pub fn fetch_gpu_model(&mut self, entry: DatabaseEntry) -> Result<DeviceModel, NorenError> {
        todo!()
    }
}

#[test]
fn db_init() {
    //    let a = DB::new(&DBInfo {});
    //    assert!(a.is_ok());
}
