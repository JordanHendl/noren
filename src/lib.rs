pub mod datatypes;
pub mod error;
pub mod meta;
mod parsing;
mod rdbfile;

use datatypes::*;
use parsing::*;
use error::NorenError;
use meta::*;

pub struct DBInfo<'a> {
    pub ctx: *mut dashi::Context,
    pub base_dir: &'a str,
    pub layout_file: Option<&'a str>,
}


pub struct DB {

}

////////////////////////////////////////////////
///
///
///

impl DB {
    pub fn new(info: &DBInfo) -> Result<Self, NorenError> {
        todo!()
    }

    pub fn geometry(&self) -> &GeometryDB {
        todo!()
    }

    pub fn imagery(&self) -> &ImageDB {
        todo!()
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
