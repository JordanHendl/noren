use bytemuck::{Pod, Zeroable};
use memmap2::{Mmap, MmapMut};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use std::{
    fs::{File, OpenOptions},
    io::{Seek, SeekFrom, Write},
    path::Path,
};

const MAGIC: [u8; 4] = *b"RDB0";
const VERSION: u16 = 1;
use std::any::type_name;

use crate::error::RdbErr;

/// Simple, portable FNV-1a 64-bit hash of a string.
#[inline]
fn fnv1a64(s: &str) -> u64 {
    let mut h: u64 = 0xcbf29ce484222325; // offset basis
    for &b in s.as_bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(0x100000001b3); // FNV prime
    }
    h
}

/// A cross-device, cross-process “type hash”.
/// Stable as long as the type’s *name/path* doesn’t change.
fn portable_type_hash<T>() -> u64 {
    fnv1a64(type_name::<T>())
}

fn to_bytes<T: Serialize>(value: &T) -> Vec<u8> {
    bincode::serialize(value).unwrap()
}

fn from_bytes<T: DeserializeOwned>(bytes: &[u8]) -> T {
    bincode::deserialize(bytes).unwrap()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn same_everywhere_for_same_type() {
        let a = portable_type_hash::<Result<i32, ()>>();
        let b = portable_type_hash::<Result<i32, ()>>();
        assert_eq!(a, b);
    }
}

//////////////////////////////////////////////////////////////////////////////
//////////////////////////////////////////////////////////////////////////////
//////////////////////////////////////////////////////////////////////////////
//////////////////////////////////////////////////////////////////////////////

#[repr(C)]
#[derive(Clone, Copy, Zeroable, Pod)]
struct Header {
    magic: [u8; 4],   // "RDB0"
    version: u16,     // 1
    reserved: u16,    // alignment / future flags
    entry_count: u32, // number of entries
}

#[repr(C)]
#[derive(Clone, Copy, Zeroable)]
pub struct Entry {
    pub type_tag: u32,  // e.g. u32::from_le_bytes(*b"GEOM")
    pub offset: u64,    // absolute file offset to blob
    pub len: u64,       // blob length
    pub name: [u8; 64], // ASCII/UTF-8 (nul padded)
}

unsafe impl Pod for Entry {}

#[inline]
fn entry_size() -> usize {
    std::mem::size_of::<Entry>()
}

struct EntryIter<'a> {
    bytes: &'a [u8],
    idx: usize,
}
impl<'a> EntryIter<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, idx: 0 }
    }
}
impl<'a> Iterator for EntryIter<'a> {
    type Item = Entry;
    fn next(&mut self) -> Option<Self::Item> {
        let sz = entry_size();
        if self.idx + sz > self.bytes.len() {
            return None;
        }
        let e = bytemuck::pod_read_unaligned::<Entry>(&self.bytes[self.idx..self.idx + sz]);
        self.idx += sz;
        Some(e)
    }
}

fn name64(s: &str) -> [u8; 64] {
    let mut out = [0u8; 64];
    let bytes = s.as_bytes();
    let n = bytes.len().min(64);
    out[..n].copy_from_slice(&bytes[..n]);
    out
}

pub struct RDBFile {
    entries: Vec<Entry>,
    data: Vec<u8>,
    mmap: Option<Mmap>,
}

impl RDBFile {
    pub fn new() -> Self {
        Self {
            entries: Vec::new(),
            data: Vec::new(),
            mmap: None,
        }
    }

    pub fn add<T: Serialize>(&mut self, name: &str, obj: &T) -> Result<(), RdbErr> {
        let b = name.as_bytes();
        let mut nameb = [0u8; 64];
        nameb[..b.len()].copy_from_slice(&b[..b.len()]);

        let bytes = to_bytes(obj);
        self.entries.push(Entry {
            type_tag: portable_type_hash::<T>() as u32,
            offset: self.data.len() as u64,
            len: bytes.len() as u64,
            name: nameb,
        });

        self.data.extend_from_slice(&bytes);

        Ok(())
    }

    pub fn fetch<T: DeserializeOwned>(&mut self, name: &str) -> Result<T, RdbErr> {
        let b = name.as_bytes();
        if let Some(pos) = self
            .entries
            .iter()
            .position(|&x| x.name[0..name.len()] == name.as_bytes()[0..name.len()])
        {
            let entry = &self.entries[pos];

            // Types match?
            if entry.type_tag == portable_type_hash::<T>() as u32 {
                let data_start = entry.offset as usize;
                let data_end = data_start + entry.len as usize;

                let obj_bytes = &self.data[data_start..data_end];
                return Ok(from_bytes::<T>(obj_bytes));
            }
        }

        return Err(RdbErr::BadHeader);
    }

    /// Save using MmapMut for zero-copy struct writes.
    /// (This writes header + entries only; blobs should be appended separately
    /// and their offsets/lengths filled beforehand.)
    pub fn save(&self, path: impl AsRef<Path>) -> Result<(), RdbErr> {
        // Write header
        let hdr = Header {
            magic: MAGIC,
            version: VERSION,
            reserved: 0,
            entry_count: self.entries.len() as u32,
        };

        // Compute file size: header + entries
        let header_sz = std::mem::size_of::<Header>() as u64;
        let entries_sz = (self.entries.len() * std::mem::size_of::<Entry>()) as u64;
        let data_sz = self.data.len();
        let hdr_bytes = bytemuck::bytes_of(&hdr);

        let header_start = 0 as usize;
        let header_end = hdr_bytes.len();
        let entries_start = header_sz as usize;
        let entries_end = header_sz as usize + entries_sz as usize;
        let data_start = entries_end;
        let data_end = entries_end + self.data.len();
        let total = header_sz + entries_sz + data_sz as u64;

        let mut f = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .open(path)
            .unwrap();

        f.set_len(total).unwrap(); // extend file to final size
        f.seek(SeekFrom::Start(0))?;

        // Map for writing
        let mut map = unsafe { MmapMut::map_mut(&f).unwrap() };

        // Write entries
        let ent_bytes = bytemuck::cast_slice::<Entry, u8>(&self.entries);

        map[header_start..header_end].copy_from_slice(hdr_bytes);
        map[entries_start..entries_end].copy_from_slice(ent_bytes);
        map[data_start..data_end].copy_from_slice(&self.data);

        // Flush to disk
        map.flush().unwrap();

        Ok(())
    }

    /// Load by mmap, then cast header/entries directly from the mapped bytes.
    pub fn load(path: impl AsRef<Path>) -> Result<Self, RdbErr> {
        let f = File::open(path)?;
        let map = unsafe { Mmap::map(&f)? };

        // Sanity checks
        let header_sz = std::mem::size_of::<Header>();
        if map.len() < header_sz {
            return Err(RdbErr::TooSmall);
        }
        let hdr: &Header = bytemuck::from_bytes(&map[..header_sz]);
        if hdr.magic != MAGIC || hdr.version != VERSION {
            return Err(RdbErr::BadHeader);
        }

        let entries_sz = (hdr.entry_count as usize) * std::mem::size_of::<Entry>();
        let need = header_sz + entries_sz;
        if map.len() < need {
            return Err(RdbErr::TooSmall);
        }

        // Allocate aligned Vec<Entry> and memcpy the bytes into it
        let mut entries = vec![Entry::zeroed(); hdr.entry_count as usize];
        let dst_bytes: &mut [u8] = bytemuck::cast_slice_mut(&mut entries);
        dst_bytes.copy_from_slice(&map[header_sz..need]);

        let data = &map[need..map.len()];

        Ok(Self {
            entries: entries.to_vec(),
            data: data.to_vec(),
            mmap: Some(map),
        })
    }

    pub fn unmap(&mut self) {
        self.mmap = None;
    }
}

pub struct RDBView {
    header: Header,
    mmap: Mmap,
    entries_start: usize,
    data_start: usize,
}

impl RDBView {
    pub fn fetch<T: DeserializeOwned>(&mut self, name: &str) -> Result<T, RdbErr> {
        let data = &self.mmap[self.data_start..self.mmap.len()];

        for i in 0..self.header.entry_count {
            //self.mmap[self.entries_start..self.data_start]
            let offset = (i as isize * std::mem::size_of::<Entry>() as isize) as usize;
            let entry_end = self.entries_start + offset + std::mem::size_of::<Entry>();
            let ptr = self.mmap[self.entries_start + offset..entry_end].as_ptr() as *const Entry;
            let entry = unsafe { std::ptr::read_unaligned(ptr) };
            if entry.name[0..name.len()] == name.as_bytes()[0..name.len()] {
                // Types match?
                if entry.type_tag == portable_type_hash::<T>() as u32 {
                    let data_start = entry.offset as usize;
                    let data_end = data_start + entry.len as usize;

                    let obj_bytes = &data[data_start..data_end];
                    return Ok(from_bytes::<T>(obj_bytes));
                }
            }
        }

        return Err(RdbErr::BadHeader);
    }

    /// Load by mmap, then cast header/entries directly from the mapped bytes.
    pub fn load(path: impl AsRef<Path>) -> Result<Self, RdbErr> {
        let f = File::open(path)?;
        let map = unsafe { Mmap::map(&f)? };

        // Sanity checks
        let header_sz = std::mem::size_of::<Header>();
        if map.len() < header_sz {
            return Err(RdbErr::TooSmall);
        }
        let hdr: &Header = bytemuck::from_bytes(&map[..header_sz]);
        if hdr.magic != MAGIC || hdr.version != VERSION {
            return Err(RdbErr::BadHeader);
        }

        let entries_sz = (hdr.entry_count as usize) * std::mem::size_of::<Entry>();
        let need = header_sz + entries_sz;
        if map.len() < need {
            return Err(RdbErr::TooSmall);
        }

        Ok(Self {
            header: *hdr,
            mmap: map,
            entries_start: header_sz,
            data_start: need,
        })
    }
}
// ---------------------------
// Tiny example
// ---------------------------

mod test {
    use super::*;
    #[test]
    fn test_rdb_read_write() {
        let mut rdb = RDBFile::new();

        #[derive(Serialize, Deserialize)]
        struct TempObject {
            data: Vec<u32>,
            name: Vec<String>,
        }

        let tmp = TempObject {
            data: vec![12; 32],
            name: vec!["lmao".to_string(); 32],
        };
        let tmp_alt = TempObject {
            data: vec![34; 16],
            name: vec!["bruh".to_string(); 16],
        };

        rdb.add("obj/t.a.c.b", &tmp)
            .expect("Should be able to insert into an empty RDB");
        rdb.add("obj/t.a.c.c", &tmp_alt)
            .expect("Should be able to insert a second entry into the RDB");
        let tmp2 = rdb
            .fetch::<TempObject>("obj/t.a.c.b")
            .expect("Should be able to read object just inserted.");
        let tmp3 = rdb
            .fetch::<TempObject>("obj/t.a.c.c")
            .expect("Should be able to read second object just inserted.");

        rdb.save("target/read_write_multi.rdb")
            .expect("should be able to write multi entry file");

        let mut rdb_view = RDBView::load("target/read_write_multi.rdb")
            .expect("Should be able to load multi entry file");
        let tmp_view = rdb_view
            .fetch::<TempObject>("obj/t.a.c.c")
            .expect("Should be able to read the second object via view");

        // Verify read data is the same.
        for e in tmp2.data {
            assert_eq!(e, 12);
        }
        for s in tmp2.name {
            assert_eq!("lmao".to_string(), s);
        }
        for e in tmp3.data {
            assert_eq!(e, 34);
        }
        for s in tmp3.name {
            assert_eq!("bruh".to_string(), s);
        }
        for e in tmp_view.data {
            assert_eq!(e, 34);
        }
        for s in tmp_view.name {
            assert_eq!("bruh".to_string(), s);
        }
    }

    #[test]
    fn test_rdb_io() {
        ///////////////// Test creating new file...
        /////////////////

        let mut rdb = RDBFile::new();

        #[repr(C)]
        #[derive(Serialize, Deserialize)]
        struct TempObject {
            data: Vec<u32>,
            name: Vec<String>,
        }

        let tmp = TempObject {
            data: vec![12; 32],
            name: vec!["lmao".to_string(); 32],
        };

        rdb.add("obj/t.a.c.b", &tmp)
            .expect("Should be able to insert into an empty RDB");

        rdb.save("target/read_io_test.rdb")
            .expect("should be able to write file");

        //////////////////// Test loading file, and mutating.
        ////////////////////

        let mut rdb_in = RDBFile::load("target/read_io_test.rdb")
            .expect("Should be able to load file just saved");

        let tmp2 = rdb_in
            .fetch::<TempObject>("obj/t.a.c.b")
            .expect("Should be able to read object just inserted.");

        // Verify read data is the same.
        for e in tmp2.data {
            assert_eq!(e, 12);
        }
        for s in tmp2.name {
            assert_eq!("lmao".to_string(), s);
        }

        //////////////////// Test RDBView with the items we saved.
        //////////////////// Must be able to fetch everything correctly.

        let mut rdb_view = RDBView::load("target/read_io_test.rdb")
            .expect("Should be able to load file just saved");

        let tmp2 = rdb_view
            .fetch::<TempObject>("obj/t.a.c.b")
            .expect("Should be able to read object just inserted.");

        assert!(rdb_view.fetch::<TempObject>("obj/t.a.c.c").is_err());

        // Verify read data is the same.
        for e in tmp2.data {
            assert_eq!(e, 12);
        }
        for s in tmp2.name {
            assert_eq!("lmao".to_string(), s);
        }
    }

    #[test]
    fn test_rdb_failures() {
        let mut rdb = RDBFile::new();

        #[derive(Serialize, Deserialize)]
        struct TempObject {
            data: Vec<u32>,
            name: Vec<String>,
        }

        #[derive(Serialize, Deserialize)]
        struct TempObject2 {
            data: Vec<String>,
            name: Vec<u32>,
        }

        let tmp = TempObject {
            data: vec![12; 32],
            name: vec!["lmao".to_string(); 32],
        };

        assert!(rdb.fetch::<TempObject>("obj/t.a.c.b").is_err());

        rdb.add("obj/t.a.c.b", &tmp)
            .expect("Should be able to insert into an empty RDB");

        assert!(rdb.fetch::<TempObject>("obj/t.a.c.d").is_err());
        assert!(rdb.fetch::<TempObject>("t.a.c.d").is_err());
        assert!(rdb.fetch::<TempObject2>("obj/t.a.c.b").is_err());

        rdb.save("target/failure_test.rdb")
            .expect("should be able to write file");
    }
}
